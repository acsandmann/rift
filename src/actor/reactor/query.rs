use objc2_core_foundation::CGRect;

use crate::actor::app::WindowId;
use crate::actor::menu_bar;
use crate::actor::reactor::{Event, Reactor};
use crate::common::collections::HashSet;
use crate::model::server::{
    ApplicationData, DisplayData, LayoutStateData, WindowData, WorkspaceData,
};
use crate::model::virtual_workspace::VirtualWorkspaceId;
use crate::sys::screen::{SpaceId, get_active_space_number};

impl Reactor {
    pub(super) fn handle_query(&mut self, event: Event) {
        match event {
            Event::QueryWorkspaces { space_id, response } => {
                let workspaces = self.handle_workspace_query(space_id);
                response.send(workspaces);
            }
            Event::QueryWindows { space_id, response } => {
                let windows = self.handle_windows_query(space_id);
                response.send(windows);
            }
            Event::QueryActiveWorkspace { space_id, response } => {
                let active = self.handle_active_workspace_query(space_id);
                let _ = response.send(active);
            }
            Event::QueryWindowInfo { window_id, response } => {
                let window_info = self.handle_window_info_query(window_id);
                response.send(window_info);
            }
            Event::QueryApplications(response) => {
                let apps = self.handle_applications_query();
                response.send(apps);
            }
            Event::QueryLayoutState { space_id, response } => {
                let layout_state = self.handle_layout_state_query(space_id);
                response.send(layout_state);
            }
            Event::QueryMetrics(response) => {
                let metrics = self.handle_metrics_query();
                response.send(metrics);
            }
            Event::QueryDisplays(response) => {
                let displays = self.handle_displays_query();
                response.send(displays);
            }
            _ => {}
        }
    }

    pub(super) fn maybe_send_menu_update(&mut self) {
        let menu_tx = match self.menu_manager.menu_tx.as_ref() {
            Some(tx) => tx.clone(),
            None => return,
        };

        let active_space = match self
            .main_window_space()
            .or_else(|| self.space_manager.screens.first().and_then(|s| s.space))
        {
            Some(space) => space,
            None => return,
        };

        let workspaces = self.handle_workspace_query(Some(active_space));
        let active_workspace = self.layout_manager.layout_engine.active_workspace(active_space);
        let active_workspace_idx =
            self.layout_manager.layout_engine.active_workspace_idx(active_space);
        let windows = self.handle_windows_query(Some(active_space));

        menu_tx.send(menu_bar::Event::Update(menu_bar::Update {
            active_space,
            workspaces,
            active_workspace_idx,
            active_workspace,
            windows,
        }));
    }

    fn handle_workspace_query(&mut self, space_id_param: Option<SpaceId>) -> Vec<WorkspaceData> {
        let mut workspaces = Vec::new();

        let space_id = space_id_param
            .or_else(|| get_active_space_number())
            .or_else(|| self.space_manager.screens.first().and_then(|s| s.space));
        let workspace_list: Vec<(crate::model::VirtualWorkspaceId, String)> =
            if let Some(space) = space_id {
                self.layout_manager
                    .layout_engine
                    .virtual_workspace_manager_mut()
                    .list_workspaces(space)
            } else {
                Vec::new()
            };

        for (index, (workspace_id, workspace_name)) in workspace_list.iter().enumerate() {
            let is_active = if let Some(space) = space_id {
                self.layout_manager.layout_engine.active_workspace(space) == Some(*workspace_id)
            } else {
                false
            };

            let workspace_windows_ids: Vec<crate::actor::app::WindowId> =
                if let Some(space) = space_id {
                    if is_active {
                        self.layout_manager.layout_engine.windows_in_active_workspace(space)
                    } else {
                        self.layout_manager
                            .layout_engine
                            .virtual_workspace_manager()
                            .workspace_info(space, *workspace_id)
                            .map(|ws| ws.windows().collect())
                            .unwrap_or_default()
                    }
                } else {
                    Vec::new()
                };

            let predicted_positions = if !is_active {
                if let Some(space) = space_id {
                    let screen_info = self
                        .space_manager
                        .screens
                        .iter()
                        .find(|s| s.space == Some(space))
                        .cloned()
                        .or_else(|| self.space_manager.screens.first().cloned());

                    if let Some(screen) = screen_info {
                        let display_uuid = if screen.display_uuid.is_empty() {
                            None
                        } else {
                            Some(screen.display_uuid.as_str())
                        };
                        let gaps =
                            self.config.settings.layout.gaps.effective_for_display(display_uuid);
                        self.layout_manager.layout_engine.calculate_layout_for_workspace(
                            space,
                            *workspace_id,
                            screen.frame,
                            &gaps,
                            self.config.settings.ui.stack_line.thickness(),
                            self.config.settings.ui.stack_line.horiz_placement,
                            self.config.settings.ui.stack_line.vert_placement,
                        )
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

            let predicted_map: std::collections::HashMap<WindowId, CGRect> =
                predicted_positions.into_iter().collect();

            let mut windows: Vec<WindowData> = Vec::new();
            for wid in workspace_windows_ids.into_iter() {
                if let Some(mut wd) = self.create_window_data(wid) {
                    if !is_active {
                        if let Some(pred) = predicted_map.get(&wid).copied() {
                            wd.frame = pred;
                        }
                    }
                    windows.push(wd);
                }
            }

            workspaces.push(WorkspaceData {
                id: format!("{:?}", workspace_id),
                name: workspace_name.to_string(),
                is_active,
                window_count: windows.len(),
                windows,
                index,
            });
        }

        workspaces
    }

    fn handle_active_workspace_query(
        &self,
        space_id_param: Option<SpaceId>,
    ) -> Option<VirtualWorkspaceId> {
        let space_id = space_id_param
            .or_else(|| get_active_space_number())
            .or_else(|| self.space_manager.screens.first().and_then(|s| s.space))?;
        self.layout_manager.layout_engine.active_workspace(space_id)
    }

    fn handle_displays_query(&self) -> Vec<DisplayData> {
        let active_context_space = self.workspace_command_space();
        self.space_manager
            .screens
            .iter()
            .map(|screen| {
                let space_for_screen = screen.space;
                DisplayData {
                    uuid: screen.display_uuid.clone(),
                    name: screen.name.clone(),
                    screen_id: screen.screen_id.as_u32(),
                    frame: screen.frame,
                    space: space_for_screen.map(|s: SpaceId| s.get()),
                    is_active_context: match (space_for_screen, active_context_space) {
                        (Some(s1), Some(s2)) => s1 == s2,
                        _ => false,
                    },
                }
            })
            .collect()
    }

    fn handle_windows_query(&self, space_id: Option<SpaceId>) -> Vec<WindowData> {
        let target_space = space_id.or_else(|| self.space_manager.first_known_space());

        if let Some(space) = target_space {
            let active_windows =
                self.layout_manager.layout_engine.windows_in_active_workspace(space);

            active_windows
                .into_iter()
                .filter_map(|wid| self.create_window_data(wid))
                .collect()
        } else {
            self.window_manager
                .windows
                .keys()
                .filter_map(|&wid| self.create_window_data(wid))
                .collect()
        }
    }

    fn handle_window_info_query(&self, window_id: WindowId) -> Option<WindowData> {
        self.create_window_data(window_id)
    }

    fn handle_applications_query(&self) -> Vec<ApplicationData> {
        self.app_manager
            .apps
            .iter()
            .map(|(&pid, app)| {
                let window_count =
                    self.window_manager.windows.keys().filter(|wid| wid.pid == pid).count();

                let is_frontmost = self
                    .main_window_tracker
                    .main_window()
                    .map(|wid| wid.pid == pid)
                    .unwrap_or(false);

                ApplicationData {
                    pid,
                    bundle_id: app.info.bundle_id.clone(),
                    name: app.info.localized_name.clone().unwrap_or_else(|| "Unknown".to_string()),
                    is_frontmost,
                    window_count,
                }
            })
            .collect()
    }

    fn handle_layout_state_query(&self, space_id_u64: u64) -> Option<LayoutStateData> {
        if space_id_u64 == 0 {
            return None;
        }
        let space_id = SpaceId::new(space_id_u64);
        if !self.space_manager.iter_known_spaces().any(|space| space == space_id) {
            return None;
        }

        let _active_workspace = self.layout_manager.layout_engine.active_workspace(space_id)?;

        let active_windows =
            self.layout_manager.layout_engine.windows_in_active_workspace(space_id);
        let floating_windows: Vec<WindowId> = active_windows
            .iter()
            .filter(|&&wid| self.layout_manager.layout_engine.is_window_floating(wid))
            .copied()
            .collect();

        let tiled_windows: Vec<WindowId> = active_windows
            .iter()
            .filter(|&&wid| !self.layout_manager.layout_engine.is_window_floating(wid))
            .copied()
            .collect();

        let focused_window = self.main_window();

        Some(LayoutStateData {
            space_id: space_id_u64,
            mode: self.layout_manager.layout_engine.layout_mode().to_string(),
            floating_windows,
            tiled_windows,
            focused_window,
        })
    }

    fn handle_metrics_query(&self) -> serde_json::Value {
        let stats = self.layout_manager.layout_engine.virtual_workspace_manager().get_stats();

        let workspace_stats: crate::common::collections::HashMap<String, usize> = stats
            .workspace_window_counts
            .iter()
            .map(|(id, count)| (format!("{:?}", id), *count))
            .collect();

        serde_json::json!({
               "windows_managed": self.window_manager.windows.len(),
            "workspaces": stats.total_workspaces,
            "applications": self.app_manager.apps.len(),
            "screens": self.space_manager.screens.len(),
            "workspace_stats": workspace_stats,
        })
    }

    pub(crate) fn serialize_state(&mut self) -> Result<String, serde_json::Error> {
        let layout_engine_ron = self.layout_manager.layout_engine.serialize_to_string();
        let vwm = self.layout_manager.layout_engine.virtual_workspace_manager_mut();

        let stats = vwm.get_stats();
        let mut workspace_window_counts = serde_json::Map::new();
        for (ws_id, count) in &stats.workspace_window_counts {
            workspace_window_counts.insert(format!("{:?}", ws_id), serde_json::json!(*count));
        }

        let mut spaces_intermediate: Vec<(
            u64,
            Vec<(
                crate::model::VirtualWorkspaceId,
                String,
                bool,
                Vec<crate::actor::app::WindowId>,
                Option<crate::actor::app::WindowId>,
                Vec<(crate::actor::app::WindowId, objc2_core_foundation::CGRect)>,
            )>,
        )> = Vec::new();

        for screen in &self.space_manager.screens {
            if let Some(space) = screen.space {
                let workspaces = vwm.list_workspaces(space);
                let active_ws = vwm.active_workspace(space);

                let mut ws_entries = Vec::new();
                for (workspace_id, workspace_name) in workspaces {
                    let window_ids: Vec<crate::actor::app::WindowId> =
                        if let Some(ws) = vwm.workspace_info(space, workspace_id) {
                            ws.windows().collect()
                        } else {
                            Vec::new()
                        };

                    let last_focused = vwm.last_focused_window(space, workspace_id);

                    let floating_positions =
                        vwm.get_workspace_floating_positions(space, workspace_id);

                    ws_entries.push((
                        workspace_id,
                        workspace_name,
                        active_ws == Some(workspace_id),
                        window_ids,
                        last_focused,
                        floating_positions,
                    ));
                }

                spaces_intermediate.push((space.get(), ws_entries));
            }
        }

        let mut mapping_intermediate: Vec<(
            u64,
            crate::actor::app::WindowId,
            crate::model::VirtualWorkspaceId,
        )> = Vec::new();
        for ((space, window_id), workspace_id) in &vwm.window_to_workspace {
            mapping_intermediate.push((space.get(), *window_id, *workspace_id));
        }

        let _ = vwm;

        let mut included_windows: HashSet<crate::actor::app::WindowId> = HashSet::default();

        let mut spaces_json = Vec::new();
        for (space_num, ws_entries) in spaces_intermediate {
            let mut ws_json = Vec::new();
            for (
                workspace_id,
                workspace_name,
                is_active,
                window_ids,
                last_focused,
                floating_positions,
            ) in ws_entries
            {
                let mut windows_json = Vec::new();
                for wid in window_ids {
                    if let Some(window_data) = self.create_window_data(wid) {
                        let v = serde_json::to_value(&window_data)
                            .unwrap_or_else(|_| serde_json::json!({ "id": wid.to_debug_string() }));
                        windows_json.push(v);
                    } else {
                        windows_json.push(serde_json::json!({ "id": wid.to_debug_string() }));
                    }

                    let _ = included_windows.insert(wid);
                }

                let last_focused_json = last_focused.map(|w| w.to_debug_string());

                let floating_json: Vec<serde_json::Value> = floating_positions
                    .into_iter()
                    .map(|(wid, rect)| {
                        serde_json::json!({
                            "window": wid.to_debug_string(),
                            "rect": {
                                "x": rect.origin.x,
                                "y": rect.origin.y,
                                "w": rect.size.width,
                                "h": rect.size.height
                            }
                        })
                    })
                    .collect();

                let id_str = workspace_id.to_string();
                let digits: String = id_str.chars().filter(|c| c.is_ascii_digit()).collect();
                let id_num = digits.parse::<u64>().unwrap_or(0);

                ws_json.push(serde_json::json!({
                    "id": id_str,
                    "id_num": id_num,
                    "name": workspace_name,
                    "is_active": is_active,
                    "windows": windows_json,
                    "last_focused": last_focused_json,
                    "floating_positions": floating_json,
                }));
            }

            spaces_json.push(serde_json::json!({
                "space": space_num,
                "workspaces": ws_json,
            }));
        }

        let mut mapping = Vec::new();
        for (space_num, window_id, workspace_id) in mapping_intermediate {
            let window_json = if let Some(window_data) = self.create_window_data(window_id) {
                serde_json::to_value(&window_data)
                    .unwrap_or_else(|_| serde_json::json!({ "id": window_id.to_debug_string() }))
            } else {
                serde_json::json!({ "id": window_id.to_debug_string() })
            };

            let _ = included_windows.insert(window_id);

            mapping.push(serde_json::json!({
                "space": space_num,
                "window": window_json,
                "workspace": workspace_id.to_string()
            }));
        }

        let known_managed_windows: Vec<serde_json::Value> = self
            .window_manager
            .windows
            .keys()
            .filter(|w| !included_windows.contains(*w))
            .map(|w| {
                if let Some(window_data) = self.create_window_data(*w) {
                    serde_json::to_value(&window_data)
                        .unwrap_or_else(|_| serde_json::json!({ "id": w.to_debug_string() }))
                } else {
                    serde_json::json!({ "id": w.to_debug_string() })
                }
            })
            .collect();

        let reactor_summary = serde_json::json!({
            "apps": self.app_manager.apps.len(),
            "managed_windows": self.window_manager.windows.len(),
            "window_server_info": self.window_server_info_manager.window_server_info.len(),
            "visible_window_server_ids": self.window_manager.visible_windows.len(),
            "screens": self.space_manager.screens.len(),
            "known_managed_windows": known_managed_windows,
        });

        let out = serde_json::json!({
            "layout_engine_ron": layout_engine_ron,
            "virtual_workspace_manager": {
                "total_workspaces": stats.total_workspaces,
                "total_windows": stats.total_windows,
                "active_spaces": stats.active_spaces,
                "workspace_window_counts": workspace_window_counts,
            },
            "spaces": spaces_json,
            "window_to_workspace": mapping,
            "reactor": reactor_summary,
        });

        serde_json::to_string_pretty(&out)
    }
}

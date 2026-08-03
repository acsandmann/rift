//! Dims every window in each active Rift workspace except the focused window.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::ffi::c_float;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

use rift_client::{RiftMachClient, RiftRequest, RiftResponse};
use serde_json::Value;

type ConnID = i32;
type WinID = u32;
type CGError = i32;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

// -1.0 = fully dimmed, 0.0 = normal, 1.0 = full brightness.
const DIMMED: f32 = -0.45;
const NORMAL: f32 = 0.0;

type DimmedBySpace = HashMap<u64, HashSet<WinID>>;

unsafe extern "C" {
    fn SLSMainConnectionID() -> ConnID;

    fn SLSSetWindowListBrightness(
        cid: ConnID,
        window_list: *const WinID,
        brightness_levels: *const c_float,
        count: isize,
    ) -> CGError;
}

struct Dimmer {
    client: RiftMachClient,
    cid: ConnID,
    dimmed: Arc<Mutex<DimmedBySpace>>,
    space_by_display: HashMap<String, u64>,
}

impl Dimmer {
    fn new(client: RiftMachClient) -> Self {
        Self {
            client,
            cid: unsafe { SLSMainConnectionID() },
            dimmed: Arc::default(),
            space_by_display: HashMap::new(),
        }
    }

    fn initialize(&mut self) -> Result<()> {
        let displays = self.query(RiftRequest::GetDisplays)?;

        for display in displays.as_array().into_iter().flatten() {
            let Some(space_id) = display.get("space").and_then(Value::as_u64) else {
                continue;
            };

            if let Some(uuid) = display.get("uuid").and_then(Value::as_str) {
                self.space_by_display.insert(uuid.into(), space_id);
            }

            if display.get("is_active_space").and_then(Value::as_bool) == Some(true) {
                self.refresh(space_id)?;
            }
        }

        Ok(())
    }

    fn handle_event(&mut self, event: &Value) -> Result<()> {
        let Some(kind) = event.get("type").and_then(Value::as_str) else {
            return Ok(());
        };

        let Some(space_id) = event.get("space_id").and_then(Value::as_u64) else {
            return Ok(());
        };

        match kind {
            "workspace_changed" => {
                if let Some(display) = event.get("display_uuid").and_then(Value::as_str)
                    && let Some(old_space) = self.space_by_display.insert(display.into(), space_id)
                    && old_space != space_id
                {
                    self.reset(old_space)?;
                }

                self.refresh(space_id)?;
            }

            "focused_window_changed" | "windows_changed" => {
                self.refresh(space_id)?;
            }

            _ => {}
        }

        Ok(())
    }

    fn refresh(&self, space_id: u64) -> Result<()> {
        let workspaces = self.query(RiftRequest::GetWorkspaces { space_id: Some(space_id) })?;

        let desired: HashSet<WinID> = workspaces
            .as_array()
            .into_iter()
            .flatten()
            .find(|workspace| workspace.get("is_active").and_then(Value::as_bool) == Some(true))
            .and_then(|workspace| workspace.get("windows").and_then(Value::as_array))
            .into_iter()
            .flatten()
            .filter(|window| window.get("is_focused").and_then(Value::as_bool) != Some(true))
            .filter_map(|window| window.get("window_server_id")?.as_u64()?.try_into().ok())
            .collect();

        let mut dimmed = lock(&self.dimmed);
        let empty = HashSet::new();
        let current = dimmed.get(&space_id).unwrap_or(&empty);

        let changes = current
            .difference(&desired)
            .copied()
            .map(|id| (id, NORMAL))
            .chain(desired.difference(current).copied().map(|id| (id, DIMMED)));

        set_brightness(self.cid, changes)?;

        if desired.is_empty() {
            dimmed.remove(&space_id);
        } else {
            dimmed.insert(space_id, desired);
        }

        Ok(())
    }

    fn reset(&self, space_id: u64) -> Result<()> {
        let mut dimmed = lock(&self.dimmed);

        let Some(windows) = dimmed.remove(&space_id) else {
            return Ok(());
        };

        set_brightness(self.cid, windows.into_iter().map(|id| (id, NORMAL)))
    }

    fn query(&self, request: RiftRequest) -> Result<Value> {
        match self.client.send_request(&request)? {
            RiftResponse::Success { data } => Ok(data),
            RiftResponse::Error { error } => {
                Err(io::Error::other(format!("Rift query failed: {error}")).into())
            }
            _ => Err(io::Error::other("unexpected Rift response").into()),
        }
    }
}

impl Drop for Dimmer {
    fn drop(&mut self) { restore_all(self.cid, &self.dimmed); }
}

fn lock(state: &Mutex<DimmedBySpace>) -> MutexGuard<'_, DimmedBySpace> {
    state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn restore_all(cid: ConnID, state: &Mutex<DimmedBySpace>) {
    let mut state = lock(state);

    let windows: HashSet<_> = state.drain().flat_map(|(_, windows)| windows).collect();

    let _ = set_brightness(cid, windows.into_iter().map(|id| (id, NORMAL)));
}

fn set_brightness(cid: ConnID, changes: impl IntoIterator<Item = (WinID, f32)>) -> Result<()> {
    let (windows, levels): (Vec<_>, Vec<_>) = changes.into_iter().unzip();

    if windows.is_empty() {
        return Ok(());
    }

    let result = unsafe {
        SLSSetWindowListBrightness(cid, windows.as_ptr(), levels.as_ptr(), windows.len() as isize)
    };

    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!("SLSSetWindowListBrightness failed: {result}")).into())
    }
}

fn main() -> Result<()> {
    let client = RiftMachClient::connect()?;
    let events = client.subscribe("*")?;
    let mut dimmer = Dimmer::new(client);

    let cid = dimmer.cid;
    let dimmed = Arc::clone(&dimmer.dimmed);

    ctrlc::set_handler(move || {
        restore_all(cid, &dimmed);
        std::process::exit(130);
    })?;

    dimmer.initialize()?;

    loop {
        dimmer.handle_event(&events.recv_event()?)?;
    }
}

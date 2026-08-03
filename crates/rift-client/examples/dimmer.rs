//! Dims every window in each active Rift workspace except the focused window.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::ffi::c_float;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

use rift_client::{EventKind, RiftEvent, RiftMachClient};

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
        let displays = self.client.get_displays()?;

        for display in displays {
            let Some(space_id) = display.space else {
                continue;
            };

            self.space_by_display.insert(display.uuid, space_id);

            if display.is_active_space {
                self.refresh(space_id)?;
            }
        }

        Ok(())
    }

    fn handle_event(&mut self, event: &RiftEvent) -> Result<()> {
        let space_id = event.space_id();

        match event {
            RiftEvent::WorkspaceChanged { display_uuid, .. } => {
                if let Some(display) = display_uuid.as_deref()
                    && let Some(old_space) = self.space_by_display.insert(display.into(), space_id)
                    && old_space != space_id
                {
                    self.reset(old_space)?;
                }

                self.refresh(space_id)?;
            }

            RiftEvent::FocusedWindowChanged { .. } | RiftEvent::WindowsChanged { .. } => {
                self.refresh(space_id)?;
            }

            _ => {}
        }

        Ok(())
    }

    fn refresh(&self, space_id: u64) -> Result<()> {
        let workspaces = self.client.get_workspaces(Some(space_id))?;

        let desired: HashSet<WinID> = workspaces
            .into_iter()
            .find(|workspace| workspace.is_active)
            .into_iter()
            .flat_map(|workspace| workspace.windows)
            .filter(|window| !window.is_focused)
            .filter_map(|window| window.window_server_id.and_then(|id| id.try_into().ok()))
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
    let events = client.subscribe(EventKind::All)?;
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

//! The app actor manages messaging to an application using the system
//! accessibility APIs.
//!
//! These APIs support reading and writing window states like position and size.

use std::cell::RefCell;
use std::fmt::Debug;
use std::num::NonZeroU32;
use std::sync::LazyLock;
use std::thread;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2_app_kit::NSRunningApplication;
use objc2_application_services::AXError;
use objc2_core_foundation::{CFRunLoop, CGPoint, CGRect};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio::{join, select};
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Span, debug, info, instrument, trace, warn};

use crate::actor;
use crate::actor::reactor::transaction_manager::TransactionId;
use crate::actor::reactor::{self, Event, Requested};
use crate::common::collections::HashMap;
use crate::model::tx_store::WindowTxStore;
use crate::sys::app::NSRunningApplicationExt;
pub use crate::sys::app::{AppInfo, WindowInfo, pid_t};
use crate::sys::axuielement::{
    AX_STANDARD_WINDOW_SUBROLE, AX_WINDOW_ROLE, AXUIElement, Error as AxError,
};
use crate::sys::enhanced_ui::with_enhanced_ui_disabled;
use crate::sys::event;
use crate::sys::executor::Executor;
use crate::sys::observer::Observer;
use crate::sys::process::ProcessInfo;
use crate::sys::skylight::{G_CONNECTION, SLSDisableUpdate, SLSReenableUpdate};
use crate::sys::timer::Timer;
use crate::sys::window_server::{self, WindowServerId, WindowServerInfo};

const kAXApplicationActivatedNotification: &str = "AXApplicationActivated";
const kAXApplicationDeactivatedNotification: &str = "AXApplicationDeactivated";
const kAXApplicationHiddenNotification: &str = "AXApplicationHidden";
const kAXApplicationShownNotification: &str = "AXApplicationShown";
const kAXMainWindowChangedNotification: &str = "AXMainWindowChanged";
const kAXWindowCreatedNotification: &str = "AXWindowCreated";
const kAXMenuOpenedNotification: &str = "AXMenuOpened";
const kAXMenuClosedNotification: &str = "AXMenuClosed";
const kAXUIElementDestroyedNotification: &str = "AXUIElementDestroyed";
const kAXWindowMovedNotification: &str = "AXWindowMoved";
const kAXWindowResizedNotification: &str = "AXWindowResized";
const kAXWindowMiniaturizedNotification: &str = "AXWindowMiniaturized";
const kAXWindowDeminiaturizedNotification: &str = "AXWindowDeminiaturized";
const kAXTitleChangedNotification: &str = "AXTitleChanged";

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum AxNotificationKind {
    ApplicationActivated = 1,
    ApplicationDeactivated,
    ApplicationHidden,
    ApplicationShown,
    MainWindowChanged,
    WindowCreated,
    MenuOpened,
    MenuClosed,
    WindowDestroyed,
    WindowMoved,
    WindowResized,
    WindowMiniaturized,
    WindowDeminiaturized,
    TitleChanged,
}

const APP_NOTIFICATIONS: &[(AxNotificationKind, &str)] = &[
    (
        AxNotificationKind::ApplicationActivated,
        kAXApplicationActivatedNotification,
    ),
    (
        AxNotificationKind::ApplicationDeactivated,
        kAXApplicationDeactivatedNotification,
    ),
    (
        AxNotificationKind::ApplicationHidden,
        kAXApplicationHiddenNotification,
    ),
    (
        AxNotificationKind::ApplicationShown,
        kAXApplicationShownNotification,
    ),
    (
        AxNotificationKind::MainWindowChanged,
        kAXMainWindowChangedNotification,
    ),
    (AxNotificationKind::WindowCreated, kAXWindowCreatedNotification),
    (AxNotificationKind::MenuOpened, kAXMenuOpenedNotification),
    (AxNotificationKind::MenuClosed, kAXMenuClosedNotification),
    (AxNotificationKind::TitleChanged, kAXTitleChangedNotification),
];

const WINDOW_NOTIFICATIONS: &[(AxNotificationKind, &str)] = &[
    (
        AxNotificationKind::WindowDestroyed,
        kAXUIElementDestroyedNotification,
    ),
    (AxNotificationKind::WindowMoved, kAXWindowMovedNotification),
    (AxNotificationKind::WindowResized, kAXWindowResizedNotification),
    (
        AxNotificationKind::WindowMiniaturized,
        kAXWindowMiniaturizedNotification,
    ),
    (
        AxNotificationKind::WindowDeminiaturized,
        kAXWindowDeminiaturizedNotification,
    ),
];

const WINDOW_ANIMATION_NOTIFICATIONS: &[AxNotificationKind] = &[
    AxNotificationKind::WindowMoved,
    AxNotificationKind::WindowResized,
];

/// An identifier representing a window.
///
/// This identifier is only valid for the lifetime of the process that owns it.
/// It is not stable across restarts of the window manager.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct WindowId {
    pub pid: pid_t,
    pub idx: NonZeroU32,
}

impl serde::ser::Serialize for WindowId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::ser::Serializer {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("WindowId", 2)?;
        s.serialize_field("pid", &self.pid)?;
        s.serialize_field("idx", &self.idx.get())?;
        s.end()
    }
}

impl<'de> serde::de::Deserialize<'de> for WindowId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::de::Deserializer<'de> {
        struct WindowIdVisitor;
        impl<'de> serde::de::Visitor<'de> for WindowIdVisitor {
            type Value = WindowId;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "a WindowId struct (with fields `pid` and `idx`), a tuple/seq (pid, idx), or a debug string like `WindowId { pid: 123, idx: 456 }`",
                )
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where E: serde::de::Error {
                WindowId::from_debug_string(v)
                    .ok_or_else(|| E::custom("invalid WindowId debug string"))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<WindowId, A::Error>
            where A: serde::de::SeqAccess<'de> {
                let pid: pid_t = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;

                let idx_u32: u32 = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;

                let idx = std::num::NonZeroU32::new(idx_u32)
                    .ok_or_else(|| serde::de::Error::custom("idx must be non-zero"))?;
                Ok(WindowId { pid, idx })
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where M: serde::de::MapAccess<'de> {
                let mut pid: Option<pid_t> = None;
                let mut idx: Option<u32> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "pid" => {
                            pid = Some(map.next_value()?);
                        }
                        "idx" => {
                            idx = Some(map.next_value()?);
                        }
                        // ignore unknown fields to be forward compatible
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let pid = pid.ok_or_else(|| serde::de::Error::missing_field("pid"))?;
                let idx_val = idx.ok_or_else(|| serde::de::Error::missing_field("idx"))?;
                let nz = std::num::NonZeroU32::new(idx_val)
                    .ok_or_else(|| serde::de::Error::custom("idx must be non-zero"))?;

                Ok(WindowId { pid, idx: nz })
            }
        }

        deserializer.deserialize_any(WindowIdVisitor)
    }
}

impl WindowId {
    pub fn new(pid: pid_t, idx: u32) -> WindowId {
        WindowId {
            pid,
            idx: NonZeroU32::new(idx).unwrap(),
        }
    }

    /// Parse a WindowId from its string representation (format: "WindowId { pid: 123, idx: 456 }")
    pub fn from_debug_string(s: &str) -> Option<WindowId> {
        if !s.starts_with("WindowId { pid: ") {
            return None;
        }

        let s = s.strip_prefix("WindowId { pid: ")?;
        let (pid_str, rest) = s.split_once(", idx: ")?;
        let idx_str = rest.strip_suffix(" }")?;

        let pid: pid_t = pid_str.parse().ok()?;
        let idx: u32 = idx_str.parse().ok()?;

        Some(WindowId {
            pid,
            idx: std::num::NonZeroU32::new(idx)?,
        })
    }

    pub fn to_debug_string(&self) -> String { format!("{:?}", self) }
}

impl AxNotificationKind {
    fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            1 => Self::ApplicationActivated,
            2 => Self::ApplicationDeactivated,
            3 => Self::ApplicationHidden,
            4 => Self::ApplicationShown,
            5 => Self::MainWindowChanged,
            6 => Self::WindowCreated,
            7 => Self::MenuOpened,
            8 => Self::MenuClosed,
            9 => Self::WindowDestroyed,
            10 => Self::WindowMoved,
            11 => Self::WindowResized,
            12 => Self::WindowMiniaturized,
            13 => Self::WindowDeminiaturized,
            14 => Self::TitleChanged,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::ApplicationActivated => kAXApplicationActivatedNotification,
            Self::ApplicationDeactivated => kAXApplicationDeactivatedNotification,
            Self::ApplicationHidden => kAXApplicationHiddenNotification,
            Self::ApplicationShown => kAXApplicationShownNotification,
            Self::MainWindowChanged => kAXMainWindowChangedNotification,
            Self::WindowCreated => kAXWindowCreatedNotification,
            Self::MenuOpened => kAXMenuOpenedNotification,
            Self::MenuClosed => kAXMenuClosedNotification,
            Self::WindowDestroyed => kAXUIElementDestroyedNotification,
            Self::WindowMoved => kAXWindowMovedNotification,
            Self::WindowResized => kAXWindowResizedNotification,
            Self::WindowMiniaturized => kAXWindowMiniaturizedNotification,
            Self::WindowDeminiaturized => kAXWindowDeminiaturizedNotification,
            Self::TitleChanged => kAXTitleChangedNotification,
        }
    }
}

fn encode_notification_data(kind: AxNotificationKind, wid: Option<WindowId>) -> usize {
    const KIND_BITS: usize = 8;
    let idx = wid.map_or(0, |wid| wid.idx.get()) as usize;
    (idx << KIND_BITS) | kind as usize
}

fn decode_notification_data(
    pid: pid_t,
    data: usize,
) -> Option<(AxNotificationKind, Option<WindowId>)> {
    const KIND_MASK: usize = (1 << 8) - 1;
    let kind = AxNotificationKind::from_tag((data & KIND_MASK) as u8)?;
    let idx = NonZeroU32::new((data >> 8) as u32);
    let wid = idx.map(|idx| WindowId { pid, idx });
    Some((kind, wid))
}

#[derive(Clone)]
pub struct AppThreadHandle {
    requests_tx: actor::Sender<Request>,
}

impl AppThreadHandle {
    pub(crate) fn new_for_test(requests_tx: actor::Sender<Request>) -> Self {
        let this = AppThreadHandle { requests_tx };
        this
    }

    pub fn send(&self, req: Request) -> anyhow::Result<()> { Ok(self.requests_tx.send(req)) }
}

impl Debug for AppThreadHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadHandle").finish()
    }
}

#[derive(Debug)]
pub enum Request {
    Terminate,
    GetVisibleWindows,
    WindowMaybeDestroyed(WindowId),
    CloseWindow(Option<WindowServerId>),

    SetWindowFrame(WindowId, CGRect, TransactionId, bool),
    SetBatchWindowFrame(Vec<(WindowId, CGRect)>, TransactionId, bool),
    SetWindowPos(WindowId, CGPoint, TransactionId, bool),
    AnimationFrame {
        wid: WindowId,
        frame: CGRect,
        set_size: bool,
        txid: TransactionId,
    },

    BeginWindowAnimation(WindowId),
    EndWindowAnimation(WindowId),

    /// Raise the windows within a single space, in the given order. All windows must be
    /// in the same space, or they will not be raised correctly.
    ///
    /// Events attributed to this request will use the provided [`Quiet`]
    /// parameter for the last window only. Events for other windows will be
    /// marked `Quiet::Yes` automatically.
    Raise(Vec<WindowId>, CancellationToken, u64, Quiet),
}

struct RaiseRequest(Vec<WindowId>, CancellationToken, u64, Quiet);

#[derive(Debug, Copy, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum Quiet {
    Yes,
    #[default]
    No,
}

pub fn spawn_app_thread(
    pid: pid_t,
    info: AppInfo,
    events_tx: reactor::Sender,
    tx_store: Option<WindowTxStore>,
) {
    thread::Builder::new()
        .name(format!("{}({pid})", info.bundle_id.as_deref().unwrap_or("")))
        .spawn(move || app_thread_main(pid, info, events_tx, tx_store))
        .unwrap();
}

struct State {
    pid: pid_t,
    bundle_id: Option<String>,
    running_app: Retained<NSRunningApplication>,
    app: AXUIElement,
    observer: Observer,
    events_tx: reactor::Sender,
    windows: HashMap<WindowId, AppWindowState>,
    elem_to_wid: HashMap<AXUIElement, WindowId>,
    last_window_idx: u32,
    main_window: Option<WindowId>,
    last_activated: Option<(Instant, Quiet, Option<WindowId>, oneshot::Sender<()>)>,
    is_hidden: bool,
    is_frontmost: bool,
    active_animation_count: usize,
    raises_tx: actor::Sender<RaiseRequest>,
    tx_store: Option<WindowTxStore>,
    pending_frames: HashMap<WindowId, PendingFrame>,
}

struct AppWindowState {
    pub elem: AXUIElement,
    last_seen_txid: TransactionId,
    hidden_by_app: bool,
    window_server_id: Option<WindowServerId>,
    title: String,
    is_animating: bool,
    last_animation_frame: Option<CGRect>,
}

struct PendingFrame {
    span: Span,
    frame: CGRect,
    set_size: bool,
    txid: TransactionId,
}

impl State {
    fn refresh_visible_windows(&mut self) -> Result<(), AxError> {
        let window_elems = match self.app.windows() {
            Ok(elems) => elems,
            Err(e) => {
                self.send_event(Event::WindowsDiscovered {
                    pid: self.pid,
                    new: Default::default(),
                    known_visible: Default::default(),
                });
                return Err(e);
            }
        };
        let server_info_by_id = self.visible_window_server_info_map(&window_elems);
        let mut new = Vec::with_capacity(window_elems.len());
        let mut known_visible = Vec::with_capacity(window_elems.len());

        for elem in window_elems {
            let wsid = WindowServerId::try_from(&elem).ok();
            let hint = wsid.and_then(|id| server_info_by_id.get(&id).copied());
            let info = match WindowInfo::from_ax_element(&elem, hint) {
                Ok((info, _)) => info,
                Err(err) => {
                    let id = self.id(&elem).ok();
                    trace!(?id, ?err, "Failed to refresh window info; will retry later");
                    continue;
                }
            };
            if !Self::has_visible_cg_peer(wsid, hint) && !info.is_minimized {
                trace!(pid = ?self.pid, ?wsid, "Ignoring AX window without a visible CG window");
                continue;
            }

            let Some(wid) = self.id(&elem).ok().or_else(|| {
                self.register_window(elem.clone(), hint).map(|(info, wid, _)| {
                    if !info.is_minimized {
                        known_visible.push(wid);
                    }
                    new.push((wid, info));
                    wid
                })
            }) else {
                continue;
            };

            // The WindowServer id is stable across sleep/display transitions, but
            // the corresponding AXUIElement is not. `id` intentionally resolves the
            // fresh element to the existing wid by that stable id; refresh the actor's
            // handle as well or subsequent frame writes keep targeting the pre-wake
            // element and can never recover.
            self.rebind_window_element(wid, elem, &info);

            if !info.is_minimized {
                known_visible.push(wid);
            }
            new.push((wid, info));
        }

        self.send_event(Event::WindowsDiscovered {
            pid: self.pid,
            new,
            known_visible,
        });
        self.on_main_window_changed(None, true);
        Ok(())
    }

    fn txid_from_store(&self, wsid: Option<WindowServerId>) -> Option<TransactionId> {
        let store = self.tx_store.as_ref()?;
        let wsid = wsid?;
        let record = store.get(&wsid)?;
        record.target.map(|_| record.txid)
    }

    fn txid_for_window_state(&self, window: &AppWindowState) -> Option<TransactionId> {
        self.txid_from_store(window.window_server_id)
            .or_else(|| Self::some_txid(window.last_seen_txid))
    }

    fn some_txid(txid: TransactionId) -> Option<TransactionId> {
        if txid == TransactionId::default() {
            None
        } else {
            Some(txid)
        }
    }

    async fn run(
        mut self,
        info: AppInfo,
        requests_tx: actor::Sender<Request>,
        requests_rx: actor::Receiver<Request>,
        notifications_rx: actor::Receiver<(AXUIElement, AxNotificationKind, Option<WindowId>)>,
        raises_rx: actor::Receiver<RaiseRequest>,
    ) {
        let handle = AppThreadHandle { requests_tx };
        if !self.init(handle, info) {
            return;
        }

        let this = RefCell::new(self);
        join!(
            Self::handle_incoming(&this, requests_rx, notifications_rx),
            Self::handle_raises(&this, raises_rx),
        );
    }

    async fn handle_incoming(
        this: &RefCell<Self>,
        mut requests_rx: actor::Receiver<Request>,
        mut notifications_rx: actor::Receiver<(AXUIElement, AxNotificationKind, Option<WindowId>)>,
    ) {
        loop {
            let batch = select! {
                biased;
                req = requests_rx.recv() => {
                    let Some(req) = req else { break };
                    let mut batch = vec![req];
                    while let Ok(req) = requests_rx.try_recv() {
                        batch.push(req);
                    }
                    batch
                }
                notif = notifications_rx.recv() => {
                    let Some((_, (elem, notif, hinted_wid))) = notif else { break };
                    this.borrow_mut().handle_notification(elem, notif, hinted_wid);
                    continue;
                }
            };
            if Self::handle_request_batch(this, batch) {
                break;
            }
        }
    }

    fn handle_request_batch(this: &RefCell<Self>, batch: Vec<(Span, Request)>) -> bool {
        for (span, request) in batch {
            let mut this = this.borrow_mut();
            let _guard = span.enter();
            debug!(?this.bundle_id, ?this.pid, ?request, "Got request");
            let request_dbg = format!("{request:?}");
            match this.handle_request(request) {
                Ok(should_terminate) if should_terminate => return true,
                Ok(_) => (),
                #[allow(non_upper_case_globals)]
                Err(AxError::Ax(AXError::CannotComplete)) if this.running_app.isTerminated() => {
                    warn!(?this.bundle_id, ?this.pid, "Application terminated without notification");
                    this.send_event(Event::ApplicationThreadTerminated(this.pid));
                    return true;
                }
                Err(err) => {
                    warn!(?this.bundle_id, ?this.pid, request = %request_dbg, "Error handling request: {:?}", err);
                }
            }
        }
        this.borrow_mut().flush_all_frames();
        false
    }

    fn flush_frames(&mut self, wid: WindowId) -> Result<(), AxError> {
        let Some(PendingFrame { span, frame, set_size, txid }) = self.pending_frames.remove(&wid)
        else {
            return Ok(());
        };
        let _guard = span.enter();
        let window = self.window_mut(wid)?;
        window.last_seen_txid = txid;
        if set_size {
            window.last_animation_frame = Some(frame);
            let _ = window.elem.set_size(frame.size);
            let _ = window.elem.set_position(frame.origin);
            let _ = window.elem.set_size(frame.size);
        } else {
            let _ = window.elem.set_position(frame.origin);
        }
        Ok(())
    }

    fn flush_all_frames(&mut self) {
        let wids: Vec<WindowId> = self.pending_frames.keys().copied().collect();
        for wid in wids {
            if let Err(err) = self.flush_frames(wid) {
                warn!(?wid, ?err, "Failed to apply animation frame");
            }
        }
    }

    async fn handle_raises(this: &RefCell<Self>, mut rx: actor::Receiver<RaiseRequest>) {
        while let Some((span, raise)) = rx.recv().await {
            let RaiseRequest(wids, token, sequence_id, quiet) = raise;
            if let Err(e) = Self::handle_raise_request(this, wids, &token, sequence_id, quiet)
                .instrument(span)
                .await
            {
                debug!("Raise request failed: {e:?}");
            }
        }
    }

    #[instrument(skip_all, fields(?info))]
    #[must_use]
    fn init(&mut self, handle: AppThreadHandle, info: AppInfo) -> bool {
        let extended_timeout_prefixes = ["com.jetbrains.", "org.gnu.Emacs"];
        let timeout = Instant::now()
            + match info.bundle_id.as_deref() {
                Some(id)
                    if extended_timeout_prefixes.iter().any(|prefix| id.starts_with(prefix)) =>
                {
                    Duration::from_secs(60)
                }

                _ => Duration::ZERO,
            };
        let mut sleep_dur = Duration::from_millis(20);
        let mut sleep = || {
            let now = Instant::now();
            let Some(remaining) = timeout.checked_duration_since(now) else {
                return false;
            };
            thread::sleep(Duration::min(sleep_dur, remaining));
            sleep_dur = Duration::min(sleep_dur * 2, Duration::from_secs(1));
            true
        };
        for &(kind, notif) in APP_NOTIFICATIONS {
            // App-level notifications are not tied to a specific window, but the
            // observer callback still recovers the notification kind by decoding
            // the refcon hint (see `decode_notification_data`). Registering with the
            // plain `add_notification` would attach a zero hint, which decodes to an
            // invalid tag and causes the notification to be silently dropped - so
            // encode the kind here just like the per-window registrations do.
            let data = encode_notification_data(kind, None);
            loop {
                match self.observer.add_notification_with_data(&self.app, notif, data) {
                    Ok(()) => break,
                    #[allow(non_upper_case_globals)]
                    Err(AxError::Ax(AXError::NotificationAlreadyRegistered)) => {
                        debug!(
                            pid = ?self.pid,
                            "Watching app for {notif} was already registered; continuing"
                        );
                        break;
                    }
                    Err(err) => {
                        debug!(pid = ?self.pid, ?err, "Watching app for {notif} failed");
                        if !sleep() {
                            return false;
                        }
                    }
                }
            }
        }

        let initial_window_elements = self.app.windows().unwrap_or_default();
        let server_info_by_id = self.visible_window_server_info_map(&initial_window_elements);

        let window_count = initial_window_elements.len();
        self.windows.reserve(window_count);
        self.elem_to_wid.reserve(window_count);
        let mut windows = Vec::with_capacity(window_count);
        let mut window_server_info = Vec::with_capacity(window_count);

        for elem in initial_window_elements {
            let wsid = WindowServerId::try_from(&elem).ok();
            let hint = wsid.and_then(|id| server_info_by_id.get(&id).copied());
            if let Some(info) = hint {
                window_server_info.push(info);
            }
            if !Self::has_visible_cg_peer(wsid, hint) {
                trace!(pid = ?self.pid, ?wsid, "Ignoring AX window without a visible CG window");
                continue;
            }
            let Some((info, wid, _)) = self.register_window(elem, hint) else {
                continue;
            };
            windows.push((wid, info));
        }

        self.main_window = self.app.main_window().ok().and_then(|w| self.id(&w).ok());
        self.is_frontmost = self.app.frontmost().unwrap_or(false);

        self.events_tx.send(Event::ApplicationLaunched {
            pid: self.pid,
            handle,
            info,
            is_frontmost: self.is_frontmost,
            main_window: self.main_window,
            visible_windows: windows,
            window_server_info,
        });

        true
    }

    #[instrument(skip_all, fields(app = ?self.app, ?request))]
    fn handle_request(&mut self, request: Request) -> Result<bool, AxError> {
        match request {
            Request::Terminate => {
                CFRunLoop::current().unwrap().stop();
                self.send_event(Event::ApplicationThreadTerminated(self.pid));
                return Ok(true);
            }
            Request::WindowMaybeDestroyed(wid) => {
                if wid.pid != self.pid {
                    return Ok(false);
                }

                // If we don't know this window, nothing to verify.
                if !self.windows.contains_key(&wid) {
                    return Ok(false);
                }

                // Trigger a visible windows refresh. If the window is gone, the reactor
                // will detect it via missing membership and tear down state.
                self.refresh_visible_windows()?;
                return Ok(false);
            }
            Request::CloseWindow(window_server_id) => {
                if let Some(wsid) = window_server_id
                    && let Err(err) = window_server::make_key_window(self.pid, wsid)
                {
                    warn!(pid = self.pid, ?wsid, ?err, "Failed to focus close target");
                    return Ok(false);
                }
                if !event::post_command_w(self.pid) {
                    warn!(pid = self.pid, ?window_server_id, "Failed to post Command-W");
                }
            }
            Request::GetVisibleWindows => {
                self.refresh_visible_windows()?;
            }
            Request::SetWindowPos(wid, pos, txid, eui) => {
                let (elem, is_animating) = match self.window_mut(wid) {
                    Ok(window) => {
                        window.last_seen_txid = txid;
                        (window.elem.clone(), window.is_animating)
                    }
                    Err(err) => match err {
                        AxError::Ax(code) => {
                            if self.handle_ax_error(wid, &code) {
                                return Ok(false);
                            }
                            return Err(AxError::Ax(code));
                        }
                        AxError::NotFound => {
                            return Ok(false);
                        }
                    },
                };

                if eui && !is_animating {
                    let _ = with_enhanced_ui_disabled(&self.app, || elem.set_position(pos));
                } else {
                    let _ = elem.set_position(pos);
                };

                let frame =
                    match self.handle_ax_result(wid, trace("frame", &elem, || elem.frame()))? {
                        Some(frame) => frame,
                        None => return Ok(false),
                    };

                self.send_event(Event::WindowFrameChanged(
                    wid,
                    frame,
                    Some(txid),
                    Requested(true),
                    None,
                ));
            }
            Request::AnimationFrame { wid, frame, set_size, txid } => {
                self.pending_frames.insert(wid, PendingFrame {
                    span: Span::current(),
                    frame,
                    set_size,
                    txid,
                });
            }
            Request::SetWindowFrame(wid, desired, txid, eui) => {
                let (elem, is_animating) = match self.window_mut(wid) {
                    Ok(window) => {
                        window.last_seen_txid = txid;
                        (window.elem.clone(), window.is_animating)
                    }
                    Err(err) => match err {
                        AxError::Ax(code) => {
                            if self.handle_ax_error(wid, &code) {
                                return Ok(false);
                            }
                            return Err(AxError::Ax(code));
                        }
                        AxError::NotFound => return Ok(false),
                    },
                };

                if eui && !is_animating {
                    with_enhanced_ui_disabled(&self.app, || {
                        let _ = elem.set_size(desired.size);
                        let _ = elem.set_position(desired.origin);
                        let _ = elem.set_size(desired.size);
                    });
                } else {
                    let _ = elem.set_size(desired.size);
                    let _ = elem.set_position(desired.origin);
                    let _ = elem.set_size(desired.size);
                }

                let frame =
                    match self.handle_ax_result(wid, trace("frame", &elem, || elem.frame()))? {
                        Some(frame) => frame,
                        None => return Ok(false),
                    };

                self.send_event(Event::WindowFrameChanged(
                    wid,
                    frame,
                    Some(txid),
                    Requested(true),
                    None,
                ));
            }
            Request::SetBatchWindowFrame(frames, txid, eui) => {
                let disable_eui_for_batch = eui
                    && frames.iter().any(|(wid, _)| {
                        self.windows.get(wid).is_some_and(|window| !window.is_animating)
                    });

                if disable_eui_for_batch {
                    let _ = self.app.set_bool_attribute("AXEnhancedUserInterface", false);
                }

                for (wid, desired) in frames.iter() {
                    let (elem, is_animating) = match self.window_mut(*wid) {
                        Ok(window) => {
                            window.last_seen_txid = txid;
                            (window.elem.clone(), window.is_animating)
                        }
                        Err(err) => match err {
                            AxError::Ax(code) => {
                                if self.handle_ax_error(*wid, &code) {
                                    continue;
                                }
                                return Err(AxError::Ax(code));
                            }
                            AxError::NotFound => continue,
                        },
                    };

                    if disable_eui_for_batch || (eui && !is_animating) {
                        if disable_eui_for_batch {
                            let _ = elem.set_size(desired.size);
                            let _ = elem.set_position(desired.origin);
                            let _ = elem.set_size(desired.size);
                        } else {
                            with_enhanced_ui_disabled(&self.app, || {
                                let _ = elem.set_size(desired.size);
                                let _ = elem.set_position(desired.origin);
                                let _ = elem.set_size(desired.size);
                            });
                        }
                    } else {
                        let _ = elem.set_size(desired.size);
                        let _ = elem.set_position(desired.origin);
                        let _ = elem.set_size(desired.size);
                    }

                    let frame = match self
                        .handle_ax_result(*wid, trace("frame", &elem, || elem.frame()))?
                    {
                        Some(frame) => frame,
                        None => continue,
                    };

                    self.send_event(Event::WindowFrameChanged(
                        *wid,
                        frame,
                        Some(txid),
                        Requested(true),
                        None,
                    ));
                }
                if disable_eui_for_batch {
                    let _ = self.app.set_bool_attribute("AXEnhancedUserInterface", true);
                }
            }
            Request::BeginWindowAnimation(wid) => {
                let (elem, started_animation) = {
                    let window = self.window_mut(wid)?;
                    let started_animation = !std::mem::replace(&mut window.is_animating, true);
                    window.last_animation_frame = None;
                    (window.elem.clone(), started_animation)
                };
                if started_animation {
                    self.active_animation_count += 1;
                }
                if started_animation && self.active_animation_count == 1 {
                    let _ = self.app.set_bool_attribute("AXEnhancedUserInterface", false);
                }
                self.stop_notifications_for_animation(&elem);

                SLSDisableUpdate(*G_CONNECTION);
            }
            Request::EndWindowAnimation(wid) => {
                if let Err(err) = self.flush_frames(wid) {
                    warn!(?wid, ?err, "Failed to flush animation frame on end");
                }
                let (elem, window_server_id, last_seen_txid, last_animation_frame, ended_animation) =
                    match self.window_mut(wid) {
                        Ok(window) => {
                            let ended_animation =
                                std::mem::replace(&mut window.is_animating, false);
                            (
                                window.elem.clone(),
                                window.window_server_id,
                                window.last_seen_txid,
                                window.last_animation_frame.take(),
                                ended_animation,
                            )
                        }
                        Err(err) => match err {
                            AxError::Ax(code) => {
                                if self.handle_ax_error(wid, &code) {
                                    return Ok(false);
                                }
                                return Err(AxError::Ax(code));
                            }
                            AxError::NotFound => return Ok(false),
                        },
                    };
                let txid = self
                    .txid_from_store(window_server_id)
                    .or_else(|| Self::some_txid(last_seen_txid));
                if let Some(frame) = last_animation_frame {
                    let _ = elem.set_size(frame.size);
                    let _ = elem.set_position(frame.origin);
                    let _ = elem.set_size(frame.size);
                }
                if ended_animation {
                    self.active_animation_count = self.active_animation_count.saturating_sub(1);
                }
                if ended_animation && self.active_animation_count == 0 {
                    let _ = self.app.set_bool_attribute("AXEnhancedUserInterface", true);
                }
                self.restart_notifications_after_animation(&elem);
                let frame =
                    match self.handle_ax_result(wid, trace("frame", &elem, || elem.frame()))? {
                        Some(frame) => frame,
                        None => return Ok(false),
                    };
                self.send_event(Event::WindowFrameChanged(
                    wid,
                    frame,
                    txid,
                    Requested(true),
                    None,
                ));
                SLSReenableUpdate(*G_CONNECTION);
            }
            Request::Raise(wids, token, sequence_id, quiet) => {
                self.raises_tx.send(RaiseRequest(wids, token, sequence_id, quiet));
            }
        }
        Ok(false)
    }

    #[instrument(skip_all, fields(app = ?self.app, ?notif))]
    fn handle_notification(
        &mut self,
        elem: AXUIElement,
        notif: AxNotificationKind,
        hinted_wid: Option<WindowId>,
    ) {
        trace!(?notif, ?elem, "Got notification");
        match notif {
            AxNotificationKind::ApplicationHidden => self.on_application_hidden(),
            AxNotificationKind::ApplicationShown => self.on_application_shown(),
            AxNotificationKind::ApplicationActivated
            | AxNotificationKind::ApplicationDeactivated => {
                _ = self.on_activation_changed();
            }
            AxNotificationKind::MainWindowChanged => {
                // `AXWindows` is filtered to the current macOS space, so using it as
                // a membership list here will incorrectly "destroy" windows that
                // merely live on another space. This fallback therefore only prunes
                // windows whose AX element has actually gone invalid.
                self.remove_stale_windows();
                self.on_main_window_changed(None, false);
            }
            AxNotificationKind::WindowCreated => {
                if self.id(&elem).is_ok() {
                    return;
                }
                let Some((window, wid, window_server_info)) = self.register_window(elem, None)
                else {
                    return;
                };
                let window_server_info = window_server_info
                    .or_else(|| window.sys_id.and_then(window_server::get_window));
                self.send_event(Event::WindowCreated(
                    wid,
                    window,
                    window_server_info,
                    event::get_mouse_state(),
                ));
            }
            AxNotificationKind::MenuOpened => self.send_event(Event::MenuOpened(self.pid)),
            AxNotificationKind::MenuClosed => self.send_event(Event::MenuClosed(self.pid)),
            AxNotificationKind::WindowDestroyed => {
                let Ok(wid) = self.wid_for_notification(&elem, hinted_wid) else {
                    return;
                };
                if self.remove_window(wid).is_none() {
                    return;
                }
                self.send_event(Event::WindowDestroyed(wid));

                self.on_main_window_changed(Some(wid), false);
            }
            AxNotificationKind::WindowMoved | AxNotificationKind::WindowResized => {
                let Ok(wid) = self.wid_for_notification(&elem, hinted_wid) else {
                    return;
                };

                let txid = match self.window(wid) {
                    Ok(window) => {
                        if window.is_animating {
                            trace!(?wid, ?notif, "Ignoring notification during animation");
                            return;
                        }
                        self.txid_for_window_state(window)
                    }
                    Err(err) => {
                        match err {
                            AxError::Ax(code) => {
                                if self.handle_ax_error(wid, &code) {
                                    return;
                                }
                            }
                            AxError::NotFound => {}
                        }
                        return;
                    }
                };
                let frame = match elem.frame() {
                    Ok(frame) => frame,
                    // During display teardown, macOS can send AXWindowMoved after
                    // the old AX element has been invalidated. This is not a
                    // destruction notification. Only AXUIElementDestroyed is
                    // authoritative for removing the app's window record; treating
                    // this transient read failure as a destroy drops manual
                    // workspace ownership before the window is rediscovered.
                    Err(AxError::Ax(AXError::InvalidUIElement)) => {
                        trace!(
                            ?wid,
                            ?notif,
                            "Ignoring invalid AX element from move/resize notification"
                        );
                        return;
                    }
                    Err(AxError::Ax(AXError::CannotComplete)) => return,
                    Err(err) => {
                        debug!(?wid, ?err, "Failed to read frame for window");
                        return;
                    }
                };
                self.send_event(Event::WindowFrameChanged(
                    wid,
                    frame,
                    txid,
                    Requested(false),
                    event::get_mouse_state(),
                ));
            }
            AxNotificationKind::WindowMiniaturized => {
                let Ok(wid) = self.wid_for_notification(&elem, hinted_wid) else {
                    return;
                };
                if let Some(window) = self.windows.get_mut(&wid) {
                    window.hidden_by_app = false;
                }
                self.send_event(Event::WindowMinimized(wid));
            }
            AxNotificationKind::WindowDeminiaturized => {
                let Ok(wid) = self.wid_for_notification(&elem, hinted_wid) else {
                    return;
                };
                if let Some(window) = self.windows.get_mut(&wid) {
                    window.hidden_by_app = false;
                }
                self.send_event(Event::WindowDeminiaturized(wid));
            }
            AxNotificationKind::TitleChanged => {
                let Ok(wid) = self.wid_for_notification(&elem, hinted_wid) else {
                    return;
                };
                match elem.title() {
                    Ok(title) => {
                        let Ok(window) = self.window_mut(wid) else {
                            return;
                        };
                        if window.title == title {
                            return;
                        }
                        window.title = title.clone();
                        self.send_event(Event::WindowTitleChanged(wid, title));
                    }
                    Err(err) => debug!(
                        ?wid,
                        ?err,
                        "Failed to read title for WindowTitleChanged notification"
                    ),
                }
            }
        }
    }
}

#[derive(Debug)]
#[allow(dead_code, reason = "uesed by Debug impls")]
enum RaiseError {
    RaiseCancelled,
    AXError(AxError),
}

impl From<AxError> for RaiseError {
    fn from(value: AxError) -> Self { Self::AXError(value) }
}

impl State {
    async fn handle_raise_request(
        this_ref: &RefCell<Self>,
        wids: Vec<WindowId>,
        token: &CancellationToken,
        sequence_id: u64,
        quiet: Quiet,
    ) -> Result<(), RaiseError> {
        let check_cancel = || {
            if token.is_cancelled() {
                return Err(RaiseError::RaiseCancelled);
            }
            Ok(())
        };
        check_cancel()?;

        let Some(&first) = wids.first() else {
            warn!("Got empty list of wids to raise; this might misbehave");
            return Ok(());
        };
        let is_standard = {
            let this = this_ref.borrow();
            let window = this.window(first)?;
            window.elem.subrole().map(|s| s == AX_STANDARD_WINDOW_SUBROLE).unwrap_or(false)
        };

        check_cancel()?;

        static MUTEX: LazyLock<parking_lot::Mutex<()>> =
            LazyLock::new(|| parking_lot::Mutex::new(()));
        let mut mutex_guard = Some(MUTEX.lock());
        check_cancel()?;
        let mut this = this_ref.borrow_mut();

        let is_frontmost = trace("is_frontmost", &this.app, || this.app.frontmost())?;

        let window_server_id = match WindowServerId::try_from(&this.window(first)?.elem) {
            Ok(wsid) => Some(wsid),
            Err(AxError::NotFound) => {
                debug!(
                    ?first,
                    "Skipping make-key request because window has no server id yet"
                );
                None
            }
            Err(err) => return Err(err.into()),
        };
        let make_key_result =
            window_server_id.map(|wsid| window_server::make_key_window(this.pid, wsid));
        if let Some(Err(err)) = &make_key_result {
            warn!(?this.pid, ?err, "Failed to activate app");
        }

        let waits_for_activation =
            !is_frontmost && make_key_result.as_ref().is_some_and(Result::is_ok) && is_standard;
        if waits_for_activation {
            // Keep the WindowServer make-key request and AX raise adjacent, matching
            // yabai's focus ordering. If we wait for process activation first, macOS
            // can temporarily make the application's previous key window authoritative.
            // For apps with windows on multiple displays that produces a spurious
            // active-display hop before the requested window is finally raised.
            //
            // This deliberately starts the focus operation before the cancellable
            // activation wait. Cancellation can stop follow-up batch raises, but it
            // must not leave process activation detached from its target window.
            let window = this.window(first)?;
            trace("raise before activation wait", &window.elem, || {
                window.elem.raise()
            })?;

            if wids.len() == 1 {
                // `quiet` only applies if the first window is also the last.
                let quiet_window_change = (quiet == Quiet::Yes).then_some(first);
                Self::wait_for_activation(this, quiet, quiet_window_change, &token).await?;
            } else {
                // Windows before the last are always quiet.
                Self::wait_for_activation(this, Quiet::Yes, Some(first), &token).await?;
            }
            this = this_ref.borrow_mut();
        } else {
            trace!(
                "Not awaiting activation event. is_frontmost={is_frontmost:?} \
                make_key_result={make_key_result:?} is_standard={is_standard:?}"
            )
        }

        for (i, &wid) in wids.iter().enumerate() {
            debug_assert_eq!(wid.pid, this.pid);
            if waits_for_activation && i == 0 {
                trace!(?wid, "Skipping duplicate raise after activation wait");
            } else {
                let window = this.window(wid)?;
                trace("raise", &window.elem, || window.elem.raise())?;
            }

            // TODO: Check the frontmost (layer 0) window of the window server and retry if necessary.

            trace!("Sending completion");
            this.send_event(Event::RaiseCompleted { window_id: wid, sequence_id });

            let is_last = i + 1 == wids.len();
            let quiet_if = if is_last {
                mutex_guard.take();
                (quiet == Quiet::Yes).then_some(wid)
            } else {
                None
            };

            if is_last {
                let main_window = this.on_main_window_changed(quiet_if, true);
                if main_window != Some(wid) {
                    warn!(
                        "Raise request failed to raise {desired:?}; instead got main_window={main_window:?}",
                        desired = this.window(wid).map(|w| &w.elem).ok(),
                    );
                }
            }
        }

        Ok(())
    }

    fn on_main_window_changed(
        &mut self,
        quiet_if: Option<WindowId>,
        allow_register: bool,
    ) -> Option<WindowId> {
        let elem = match trace("main_window", &self.app, || self.app.main_window()) {
            Ok(elem) => elem,
            Err(e) => {
                if self.windows.is_empty() {
                    trace!("Failed to read main window (no windows): {e:?}");
                } else {
                    warn!("Failed to read main window: {e:?}");
                }
                return None;
            }
        };

        let wid = match self.id(&elem).ok() {
            Some(wid) => wid,
            None => {
                if !allow_register {
                    info!(?self.pid, "Got MainWindowChanged on unknown window; clearing main window");
                    if self.main_window.take().is_some() {
                        self.send_event(Event::ApplicationMainWindowChanged(
                            self.pid,
                            None,
                            Quiet::No,
                        ));
                    }
                    return None;
                }
                let Some((info, wid, window_server_info)) = self.register_window(elem, None) else {
                    debug!(?self.pid, "Got MainWindowChanged on unknown window");
                    return None;
                };
                let window_server_info =
                    window_server_info.or_else(|| info.sys_id.and_then(window_server::get_window));
                self.send_event(Event::WindowCreated(
                    wid,
                    info,
                    window_server_info,
                    event::get_mouse_state(),
                ));
                wid
            }
        };

        if self.main_window == Some(wid) {
            return Some(wid);
        }
        self.main_window = Some(wid);
        let quiet = match quiet_if {
            Some(id) if id == wid => Quiet::Yes,
            _ => Quiet::No,
        };
        self.send_event(Event::ApplicationMainWindowChanged(self.pid, Some(wid), quiet));
        Some(wid)
    }

    fn on_activation_changed(&mut self) -> Result<(), AxError> {
        // TODO: this prolly isnt needed
        let is_frontmost = trace("is_frontmost", &self.app, || self.app.frontmost())?;
        let old_frontmost = std::mem::replace(&mut self.is_frontmost, is_frontmost);
        debug!(
            "on_activation_changed, pid={:?}, is_frontmost={:?}, old_frontmost={:?}",
            self.pid, is_frontmost, old_frontmost
        );

        let event = if !is_frontmost {
            Event::ApplicationDeactivated(self.pid)
        } else {
            let (quiet_activation, quiet_window_change) = match self.last_activated.take() {
                Some((ts, quiet_activation, quiet_window_change, tx)) => {
                    _ = tx.send(());
                    if ts.elapsed() < Duration::from_millis(1000) {
                        trace!("by us");
                        (quiet_activation, quiet_window_change)
                    } else {
                        trace!("by user");
                        (Quiet::No, None)
                    }
                }
                None => {
                    trace!("by user");
                    (Quiet::No, None)
                }
            };

            self.on_main_window_changed(quiet_window_change, true);

            Event::ApplicationActivated(self.pid, quiet_activation)
        };

        if old_frontmost != is_frontmost {
            self.send_event(event);
        }
        Ok(())
    }

    async fn wait_for_activation(
        mut this: std::cell::RefMut<'_, Self>,
        quiet_activation: Quiet,
        quiet_window_change: Option<WindowId>,
        token: &CancellationToken,
    ) -> Result<(), RaiseError> {
        let app = this.app.clone();
        let (tx, rx) = oneshot::channel();
        if let Some((_, _, _, prev_tx)) =
            this.last_activated
                .replace((Instant::now(), quiet_activation, quiet_window_change, tx))
        {
            let _ = prev_tx.send(());
        }
        drop(this);
        trace!("Awaiting activation");
        tokio::pin!(rx);
        loop {
            select! {
                _ = &mut rx => break,
                _ = token.cancelled() => {
                    debug!("Raise cancelled while awaiting activation event");
                    return Err(RaiseError::RaiseCancelled);
                }
                _ = Timer::sleep(Duration::from_millis(10)) => {
                    if app.frontmost().unwrap_or(false) {
                        trace!("Activation observed via frontmost polling");
                        break;
                    }
                }
            }
        }
        trace!("Activation complete");
        Ok(())
    }

    fn on_application_hidden(&mut self) {
        if self.is_hidden {
            return;
        }

        self.is_hidden = true;
        let mut to_minimize = Vec::new();
        for (wid, window) in self.windows.iter_mut() {
            if window.hidden_by_app {
                continue;
            }
            window.hidden_by_app = true;
            to_minimize.push(*wid);
        }

        for wid in to_minimize {
            self.send_event(Event::WindowMinimized(wid));
        }
    }

    fn on_application_shown(&mut self) {
        if !self.is_hidden {
            return;
        }

        self.is_hidden = false;
        let mut to_restore = Vec::new();
        for (wid, window) in self.windows.iter_mut() {
            if !window.hidden_by_app {
                continue;
            }
            window.hidden_by_app = false;
            let minimized = match trace("minimized", &window.elem, || window.elem.minimized()) {
                Ok(minimized) => minimized,
                Err(err) => {
                    debug!(?wid, ?err, "Failed to read minimized state after app shown");
                    false
                }
            };
            if minimized {
                continue;
            }
            let wid = *wid;
            to_restore.push(wid);
        }

        for wid in to_restore {
            self.send_event(Event::WindowDeminiaturized(wid));
        }
    }

    #[must_use]
    fn register_window(
        &mut self,
        elem: AXUIElement,
        server_info_hint: Option<WindowServerInfo>,
    ) -> Option<(WindowInfo, WindowId, Option<WindowServerInfo>)> {
        let Ok((mut info, server_info)) = WindowInfo::from_ax_element(&elem, server_info_hint)
        else {
            return None;
        };
        if !Self::has_visible_cg_peer(info.sys_id, server_info) && !info.is_minimized {
            trace!(pid = ?self.pid, sys_id = ?info.sys_id, "Ignoring AX window without a visible CG window");
            return None;
        }

        let bundle_is_widget = info.bundle_id.as_deref().map_or(false, |id| {
            let id_lower = id.to_ascii_lowercase();
            id_lower.ends_with(".widget") || id_lower.contains(".widget.")
        });

        let path_is_extension = info.path.as_ref().and_then(|p| p.to_str()).map_or(false, |path| {
            let lower = path.to_ascii_lowercase();
            lower.contains(".appex/") || lower.ends_with(".appex")
        });

        if bundle_is_widget || path_is_extension {
            trace!(bundle_id = ?info.bundle_id, path = ?info.path, "Ignoring widget/app-extension window");
            return None;
        }

        if info.ax_role.as_deref() == Some("AXPopover") || info.ax_role.as_deref() == Some("AXMenu")
        //|| info.ax_subrole.as_deref() == Some("AXUnknown")
        {
            trace!(
                role = ?info.ax_role,
                subrole = ?info.ax_subrole,
                "Ignoring non-standard AX window"
            );
            return None;
        }

        // TODO: improve this heuristic using ideas from AeroSpace(maybe implement a similar testing architecture based on ax dumps)
        if (self.bundle_id.as_deref() == Some("com.googlecode.iterm2")
            || self.bundle_id.as_deref() == Some("com.apple.TextInputUI.xpc.CursorUIViewService"))
            && elem.attribute("AXTitleUIElement").is_err()
        {
            info.is_standard = false;
        }

        if let Some(wsid) = info.sys_id {
            info.is_root = window_server::window_parent(wsid).is_none();
        } else {
            info.is_root = true;
        }

        let window_server_id = info.sys_id.filter(|sid| sid.as_nonzero().is_some()).or_else(|| {
            WindowServerId::try_from(&elem)
                .or_else(|e| {
                    info!("Could not get window server id for {elem:?}: {e}");
                    Err(e)
                })
                .ok()
        });

        let idx = window_server_id.and_then(WindowServerId::as_nonzero).unwrap_or_else(|| {
            self.last_window_idx += 1;
            NonZeroU32::new(self.last_window_idx).unwrap()
        });
        let wid = WindowId { pid: self.pid, idx };
        if self.windows.contains_key(&wid) {
            trace!(?wid, "Window already registered; skipping duplicate");
            return None;
        }

        if !self.register_window_notifications(&elem, wid) {
            return None;
        }
        let hidden_by_app = self.is_hidden;
        let last_seen_txid = self.txid_from_store(window_server_id).unwrap_or_default();

        let old = self.windows.insert(wid, AppWindowState {
            elem: elem.clone(),
            last_seen_txid,
            hidden_by_app,
            window_server_id,
            title: info.title.clone(),
            is_animating: false,
            last_animation_frame: None,
        });
        debug_assert!(old.is_none(), "Duplicate window id {wid:?}");
        self.elem_to_wid.insert(elem, wid);
        if hidden_by_app {
            self.send_event(Event::WindowMinimized(wid));
        }
        Some((info, wid, server_info))
    }

    fn register_window_notifications(&self, elem: &AXUIElement, wid: WindowId) -> bool {
        match elem.role() {
            Ok(role) if role == AX_WINDOW_ROLE => (),
            _ => return false,
        }
        for &(kind, notif) in WINDOW_NOTIFICATIONS {
            let res = self.observer.add_notification_with_data(
                elem,
                notif,
                encode_notification_data(kind, Some(wid)),
            );
            if let Err(err) = res {
                let is_already_registered = matches!(
                    err,
                    AxError::Ax(code) if code == AXError::NotificationAlreadyRegistered
                );
                if !is_already_registered {
                    trace!("Watching failed with error {err:?} on window {elem:#?}");
                    return false;
                }
            }
        }
        true
    }

    fn rebind_window_element(&mut self, wid: WindowId, elem: AXUIElement, info: &WindowInfo) {
        let Some((old_elem, was_animating)) =
            self.windows.get(&wid).map(|window| (window.elem.clone(), window.is_animating))
        else {
            return;
        };
        if old_elem == elem {
            return;
        }

        // Move observer ownership before replacing the handle. Removing from an
        // invalid old element can legitimately fail; Observer retains its callback
        // context in that case, so a late notification remains memory-safe and its
        // encoded wid still resolves to this logical window.
        self.remove_window_notifications(&old_elem);
        if !self.register_window_notifications(&elem, wid) {
            // Keep the last usable binding and restore its notifications when the
            // replacement cannot yet be observed. A later AXWindows refresh retries.
            self.remove_window_notifications(&elem);
            let _ = self.register_window_notifications(&old_elem, wid);
            return;
        }
        if was_animating {
            self.stop_notifications_for_animation(&elem);
        }

        self.elem_to_wid.remove(&old_elem);
        self.elem_to_wid.insert(elem.clone(), wid);
        if let Some(window) = self.windows.get_mut(&wid) {
            window.elem = elem;
            window.window_server_id = info.sys_id.or(window.window_server_id);
            window.title = info.title.clone();
        }
        debug!(?wid, "Rebound window to refreshed AX element");
    }

    fn remove_window_notifications(&self, elem: &AXUIElement) {
        for &(_, notif) in WINDOW_NOTIFICATIONS {
            if let Err(err) = self.observer.remove_notification(elem, notif) {
                trace!(
                    ?elem,
                    notif,
                    ?err,
                    "Could not remove notification from superseded AX element"
                );
            }
        }
    }

    fn visible_window_server_info_map(
        &self,
        window_elements: &[AXUIElement],
    ) -> HashMap<WindowServerId, WindowServerInfo> {
        let wsids: Vec<WindowServerId> = window_elements
            .iter()
            .filter_map(|elem| WindowServerId::try_from(elem).ok())
            .collect();
        let mut info_by_id = HashMap::with_capacity_and_hasher(wsids.len(), Default::default());
        for info in window_server::get_windows(&wsids) {
            info_by_id.insert(info.id, info);
        }
        info_by_id
    }

    #[inline]
    fn has_visible_cg_peer(wsid: Option<WindowServerId>, hint: Option<WindowServerInfo>) -> bool {
        wsid.is_none() || hint.is_some()
    }

    fn handle_ax_error(&mut self, wid: WindowId, err: &AXError) -> bool {
        if matches!(*err, AXError::InvalidUIElement) {
            if self.remove_window(wid).is_some() {
                self.send_event(Event::WindowDestroyed(wid));
                self.on_main_window_changed(Some(wid), false);
            }
            return true;
        }

        false
    }

    fn handle_ax_result<T>(
        &mut self,
        wid: WindowId,
        result: Result<T, AxError>,
    ) -> Result<Option<T>, AxError> {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(AxError::Ax(code)) if code == AXError::CannotComplete => {
                trace!(
                    ?wid,
                    "AX request returned CannotComplete; leaving window registered"
                );
                Ok(None)
            }
            Err(AxError::Ax(code)) => {
                if self.handle_ax_error(wid, &code) {
                    Ok(None)
                } else {
                    Err(AxError::Ax(code))
                }
            }
            Err(AxError::NotFound) => Ok(None),
        }
    }

    fn remove_stale_windows(&mut self) {
        let mut to_remove = Vec::new();
        for (&wid, window) in self.windows.iter() {
            // `kAXWindowsAttribute` is space-filtered and cannot be used to decide
            // whether a tracked window still exists globally. Only drop state when
            // the element itself has become invalid.
            if matches!(window.elem.role(), Err(AxError::Ax(AXError::InvalidUIElement))) {
                to_remove.push(wid);
            }
        }

        for wid in to_remove {
            self.remove_tracked_window(wid, "Removed stale window (invalid AX element)");
        }
    }

    fn remove_tracked_window(&mut self, wid: WindowId, reason: &'static str) {
        if self.remove_window(wid).is_some() {
            debug!(?wid, reason);
            self.send_event(Event::WindowDestroyed(wid));
        }
    }

    fn send_event(&self, event: Event) { self.events_tx.send(event); }

    fn window(&self, wid: WindowId) -> Result<&AppWindowState, AxError> {
        assert_eq!(wid.pid, self.pid);
        self.windows.get(&wid).ok_or(AxError::NotFound)
    }

    fn window_mut(&mut self, wid: WindowId) -> Result<&mut AppWindowState, AxError> {
        assert_eq!(wid.pid, self.pid);
        self.windows.get_mut(&wid).ok_or(AxError::NotFound)
    }

    fn id(&self, elem: &AXUIElement) -> Result<WindowId, AxError> {
        if let Ok(id) = WindowServerId::try_from(elem) {
            if let Some(idx) = id.as_nonzero() {
                let wid = WindowId { pid: self.pid, idx };
                if self.windows.contains_key(&wid) {
                    return Ok(wid);
                }
            }
        }
        if let Some(&wid) = self.elem_to_wid.get(elem) {
            return Ok(wid);
        }
        Err(AxError::NotFound)
    }

    fn wid_for_notification(
        &self,
        elem: &AXUIElement,
        hinted_wid: Option<WindowId>,
    ) -> Result<WindowId, AxError> {
        hinted_wid
            .filter(|wid| wid.pid == self.pid)
            .or_else(|| self.id(elem).ok())
            .ok_or(AxError::NotFound)
    }

    fn stop_notifications_for_animation(&self, elem: &AXUIElement) {
        for &kind in WINDOW_ANIMATION_NOTIFICATIONS {
            let res = self.observer.remove_notification(elem, kind.name());
            if let Err(err) = res {
                debug!(
                    notif = kind.name(),
                    ?elem,
                    "Removing notification failed with error {err}"
                );
            }
        }
    }

    fn restart_notifications_after_animation(&self, elem: &AXUIElement) {
        let hinted_wid = self.id(elem).ok();
        for &kind in WINDOW_ANIMATION_NOTIFICATIONS {
            let res = match hinted_wid {
                Some(wid) => self.observer.add_notification_with_data(
                    elem,
                    kind.name(),
                    encode_notification_data(kind, Some(wid)),
                ),
                None => self.observer.add_notification_with_data(
                    elem,
                    kind.name(),
                    encode_notification_data(kind, None),
                ),
            };
            if let Err(err) = res {
                debug!(
                    notif = kind.name(),
                    ?elem,
                    "Adding notification failed with error {err}"
                );
            }
        }
    }

    fn remove_window(&mut self, wid: WindowId) -> Option<AppWindowState> {
        let window = self.windows.remove(&wid)?;
        self.elem_to_wid.remove(&window.elem);
        if window.is_animating {
            self.active_animation_count = self.active_animation_count.saturating_sub(1);
        }
        if window.is_animating && self.active_animation_count == 0 {
            let _ = self.app.set_bool_attribute("AXEnhancedUserInterface", true);
        }
        Some(window)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some((_, _, _, tx)) = self.last_activated.take() {
            let _ = tx.send(());
        }
    }
}

fn app_thread_main(
    pid: pid_t,
    info: AppInfo,
    events_tx: reactor::Sender,
    tx_store: Option<WindowTxStore>,
) {
    let app = AXUIElement::application(pid);
    let Some(running_app) = NSRunningApplication::with_process_id(pid) else {
        info!(?pid, "Making NSRunningApplication failed; exiting app thread");
        return;
    };

    let bundle_id = running_app.bundleIdentifier();

    let Ok(process_info) = ProcessInfo::for_pid(pid) else {
        info!(?pid, ?bundle_id, "Could not get ProcessInfo; exiting app thread");
        return;
    };
    if process_info.is_xpc {
        // XPC processes are not supposed to have windows so at best they are
        // extra work and noise. Worse, Apple's QuickLookUIService reports
        // having standard windows (these seem to be for Finder previews), but
        // they are non-standard and unmanageable.
        debug!(?pid, ?bundle_id, "Filtering out XPC process");
        return;
    }

    let Ok(observer) = Observer::new(pid) else {
        info!(?pid, ?bundle_id, "Making observer failed; exiting app thread");
        return;
    };
    let (notifications_tx, notifications_rx) = actor::channel();
    let observer = observer.install(move |elem, data| {
        if let Some((notif, wid)) = decode_notification_data(pid, data) {
            _ = notifications_tx.send((elem, notif, wid));
        }
    });

    let (raises_tx, raises_rx) = actor::channel();
    let state = State {
        pid,
        running_app,
        bundle_id: info.bundle_id.clone(),
        app: app.clone(),
        observer,
        events_tx,
        windows: HashMap::default(),
        elem_to_wid: HashMap::default(),
        last_window_idx: 0,
        main_window: None,
        last_activated: None,
        is_hidden: false,
        is_frontmost: false,
        active_animation_count: 0,
        raises_tx,
        tx_store,
        pending_frames: HashMap::default(),
    };

    let (requests_tx, requests_rx) = actor::channel();
    Executor::run(state.run(info, requests_tx, requests_rx, notifications_rx, raises_rx));
}

fn trace<T>(
    desc: &str,
    elem: &AXUIElement,
    f: impl FnOnce() -> Result<T, AxError>,
) -> Result<T, AxError> {
    let start = Instant::now();
    let out = f();
    let end = Instant::now();
    // FIXME: ?elem here can change system behavior because it sends requests
    // to the app.
    trace!(time = ?(end - start), /*?elem,*/ "{desc:12}");
    if let Err(err) = &out {
        let app = elem.parent().ok().flatten();
        match err {
            AxError::Ax(ax_err)
                if matches!(
                    *ax_err,
                    AXError::CannotComplete | AXError::InvalidUIElement | AXError::Failure
                ) =>
            {
                debug!("{desc} failed with {err} - app may have quit or become unresponsive");
            }
            _ => {
                debug!("{desc} failed with {err} for element {elem:#?} with parent {app:#?}");
            }
        }
    }
    out
}

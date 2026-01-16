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

use r#continue::continuation;
use objc2::rc::Retained;
use objc2_app_kit::NSRunningApplication;
use objc2_application_services::AXError;
use objc2_core_foundation::{CFRunLoop, CGPoint, CGRect};
use serde::{Deserialize, Serialize};
use tokio::{join, select};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Span, debug, error, info, instrument, trace, warn};

use crate::actor;
use crate::actor::reactor::transaction_manager::TransactionId;
use crate::actor::reactor::{self, Event, Requested};
use crate::common::collections::{HashMap, HashSet};
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
    CloseWindow(WindowId),

    SetWindowFrame(WindowId, CGRect, TransactionId, bool),
    SetBatchWindowFrame(Vec<(WindowId, CGRect)>, TransactionId),
    SetWindowPos(WindowId, CGPoint, TransactionId, bool),

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
    windows: HashMap<WindowId, WindowState>,
    last_window_idx: u32,
    main_window: Option<WindowId>,
    last_activated: Option<(Instant, Quiet, Option<WindowId>, r#continue::Sender<()>)>,
    is_hidden: bool,
    is_frontmost: bool,
    raises_tx: actor::Sender<RaiseRequest>,
    tx_store: Option<WindowTxStore>,
}

struct WindowState {
    pub elem: AXUIElement,
    last_seen_txid: TransactionId,
    hidden_by_app: bool,
    window_server_id: Option<WindowServerId>,
    is_animating: bool,
}

const APP_NOTIFICATIONS: &[&str] = &[
    kAXApplicationActivatedNotification,
    kAXApplicationDeactivatedNotification,
    kAXApplicationHiddenNotification,
    kAXApplicationShownNotification,
    kAXMainWindowChangedNotification,
    kAXWindowCreatedNotification,
    kAXMenuOpenedNotification,
    kAXMenuClosedNotification,
    kAXTitleChangedNotification,
];

const WINDOW_NOTIFICATIONS: &[&str] = &[
    kAXUIElementDestroyedNotification,
    kAXWindowMovedNotification,
    kAXWindowResizedNotification,
    kAXWindowMiniaturizedNotification,
    kAXWindowDeminiaturizedNotification,
];

const WINDOW_ANIMATION_NOTIFICATIONS: &[&str] =
    &[kAXWindowMovedNotification, kAXWindowResizedNotification];

impl State {
    fn txid_from_store(&self, wsid: Option<WindowServerId>) -> Option<TransactionId> {
        let store = self.tx_store.as_ref()?;
        let wsid = wsid?;
        store.get(&wsid).map(|record| record.txid)
    }

    fn txid_for_window_state(&self, window: &WindowState) -> Option<TransactionId> {
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
        notifications_rx: actor::Receiver<(AXUIElement, String)>,
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
        requests_rx: actor::Receiver<Request>,
        notifications_rx: actor::Receiver<(AXUIElement, String)>,
    ) {
        pub enum Incoming {
            Notification((Span, (AXUIElement, String))),
            Request((Span, Request)),
        }

        let mut merged = StreamExt::merge(
            UnboundedReceiverStream::new(requests_rx).map(Incoming::Request),
            UnboundedReceiverStream::new(notifications_rx).map(Incoming::Notification),
        );

        while let Some(incoming) = merged.next().await {
            let mut this = this.borrow_mut();
            match incoming {
                Incoming::Request((span, mut request)) => {
                    let _guard = span.enter();
                    debug!(?this.bundle_id, ?this.pid, ?request, "Got request");
                    match this.handle_request(&mut request) {
                        Ok(should_terminate) if should_terminate => break,
                        Ok(_) => (),
                        #[allow(non_upper_case_globals)]
                        Err(AxError::Ax(AXError::CannotComplete))
                        // SAFETY: NSRunningApplication is thread-safe.
                        if this.running_app.isTerminated() =>
                        {
                            // The app does not appear to be running anymore.
                            // Normally this would be noticed by notification_center,
                            // but the notification doesn't always happen.
                            warn!(?this.bundle_id, ?this.pid, "Application terminated without notification");
                            this.send_event(Event::ApplicationThreadTerminated(this.pid));
                            break;
                        }
                        Err(err) => {
                            warn!(?this.bundle_id, ?this.pid, ?request, "Error handling request: {:?}", err);
                        }
                    }
                }
                Incoming::Notification((_, (elem, notif))) => {
                    this.handle_notification(elem, &notif);
                }
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
        for notif in APP_NOTIFICATIONS {
            let res = self.observer.add_notification(&self.app, notif);
            if let Err(err) = res {
                debug!(pid = ?self.pid, ?err, "Watching app failed");
                return false;
            }
        }

        let initial_window_elements = self.app.windows().unwrap_or_default();

        let window_count = initial_window_elements.len() as usize;
        self.windows.reserve(window_count);
        let mut windows = Vec::with_capacity(window_count);

        let mut elements_with_ids = Vec::with_capacity(window_count);
        let mut wsids = Vec::with_capacity(window_count);
        for elem in initial_window_elements.into_iter() {
            let wsid = WindowServerId::try_from(&elem).ok();
            if let Some(id) = wsid {
                wsids.push(id);
            }
            elements_with_ids.push((elem, wsid));
        }

        let window_server_info = window_server::get_windows(&wsids);
        let mut server_info_by_id: HashMap<WindowServerId, WindowServerInfo> = HashMap::default();
        for info in &window_server_info {
            server_info_by_id.insert(info.id, *info);
        }

        for (elem, wsid) in elements_with_ids {
            let hint = wsid.and_then(|id| server_info_by_id.get(&id).copied());
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
    fn handle_request(&mut self, request: &mut Request) -> Result<bool, AxError> {
        match request {
            Request::Terminate => {
                CFRunLoop::current().unwrap().stop();
                self.send_event(Event::ApplicationThreadTerminated(self.pid));
                return Ok(true);
            }
            Request::WindowMaybeDestroyed(wid) => {
                let wid = *wid;
                if wid.pid != self.pid {
                    return Ok(false);
                }

                // If we don't know this window, nothing to verify.
                if !self.windows.contains_key(&wid) {
                    return Ok(false);
                }

                // Trigger a visible windows refresh. If the window is gone, the reactor
                // will detect it via missing membership and tear down state.
                *request = Request::GetVisibleWindows;
                return self.handle_request(request);
            }
            Request::CloseWindow(wid) => {
                if let Some(window) = self.windows.get(wid)
                    && let Err(err) = window.elem.close()
                {
                    warn!(?wid, error = ?err, "Failed to close window");
                }
            }
            Request::GetVisibleWindows => {
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
                let mut new = Vec::with_capacity(window_elems.len() as usize);
                let mut known_visible = Vec::with_capacity(window_elems.len() as usize);
                for elem in window_elems.iter() {
                    let elem = elem.clone();
                    if let Ok(id) = self.id(&elem) {
                        known_visible.push(id);
                        match WindowInfo::from_ax_element(&elem, None) {
                            Ok((info, _)) => {
                                new.push((id, info));
                            }
                            Err(err) => {
                                trace!(
                                    ?id,
                                    ?err,
                                    "Failed to refresh window info; will retry later"
                                );
                            }
                        }
                        continue;
                    }
                    let Some((info, wid, _)) = self.register_window(elem, None) else {
                        continue;
                    };
                    new.push((wid, info));
                }
                self.send_event(Event::WindowsDiscovered {
                    pid: self.pid,
                    new,
                    known_visible,
                });
            }
            &mut Request::SetWindowPos(wid, pos, txid, eui) => {
                let elem = match self.window_mut(wid) {
                    Ok(window) => {
                        window.last_seen_txid = txid;
                        window.elem.clone()
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

                if eui {
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
            &mut Request::SetWindowFrame(wid, desired, txid, eui) => {
                let elem = match self.window_mut(wid) {
                    Ok(window) => {
                        window.last_seen_txid = txid;
                        window.elem.clone()
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

                if eui {
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
            &mut Request::SetBatchWindowFrame(ref mut frames, txid) => {
                unsafe { SLSDisableUpdate(*G_CONNECTION) };
                let app = self.app.clone();
                let result = with_enhanced_ui_disabled(&app, || -> Result<(), AxError> {
                    for (wid, desired) in frames.iter() {
                        let elem = match self.window_mut(*wid) {
                            Ok(window) => {
                                window.last_seen_txid = txid;
                                window.elem.clone()
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

                        let _ = elem.set_size(desired.size);
                        let _ = elem.set_position(desired.origin);
                        let _ = elem.set_size(desired.size);

                        let frame = match self.handle_ax_result(*wid, elem.frame())? {
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
                    Ok(())
                });
                unsafe { SLSReenableUpdate(*G_CONNECTION) };
                if let Err(err) = result {
                    return Err(err);
                }
            }
            &mut Request::BeginWindowAnimation(wid) => {
                let window = self.window_mut(wid)?;
                window.is_animating = true;
                self.stop_notifications_for_animation(&self.window(wid)?.elem);
            }
            &mut Request::EndWindowAnimation(wid) => {
                let (elem, txid) = match self.window(wid) {
                    Ok(window) => (window.elem.clone(), self.txid_for_window_state(window)),
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
                if let Ok(window) = self.window_mut(wid) {
                    window.is_animating = false;
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
            }
            &mut Request::Raise(ref wids, ref token, sequence_id, quiet) => {
                self.raises_tx
                    .send(RaiseRequest(wids.clone(), token.clone(), sequence_id, quiet));
            }
        }
        Ok(false)
    }

    #[instrument(skip_all, fields(app = ?self.app, ?notif))]
    fn handle_notification(&mut self, elem: AXUIElement, notif: &str) {
        trace!(?notif, ?elem, "Got notification");
        #[allow(non_upper_case_globals)]
        match notif {
            kAXApplicationHiddenNotification => self.on_application_hidden(),
            kAXApplicationShownNotification => self.on_application_shown(),
            kAXApplicationActivatedNotification | kAXApplicationDeactivatedNotification => {
                _ = self.on_activation_changed();
            }
            kAXMainWindowChangedNotification => {
                // NOTE(acsandmann):
                // because of apps like firefox that send delayed(or dont send at all) axuielementdestroyed/windowserverdisappeared
                // this is a fallback to ensure we handle windows being closed
                self.remove_stale_windows();
                self.on_main_window_changed(None);
            }
            kAXWindowCreatedNotification => {
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
            kAXMenuOpenedNotification => self.send_event(Event::MenuOpened),
            kAXMenuClosedNotification => self.send_event(Event::MenuClosed),
            kAXUIElementDestroyedNotification => {
                let Ok(wid) = self.id(&elem) else {
                    return;
                };
                self.windows.remove(&wid);
                self.send_event(Event::WindowDestroyed(wid));

                self.on_main_window_changed(Some(wid));
            }
            kAXWindowMovedNotification | kAXWindowResizedNotification => {
                let Ok(wid) = self.id(&elem) else {
                    return;
                };

                if let Ok(window) = self.window(wid) {
                    if window.is_animating {
                        trace!(?wid, ?notif, "Ignoring notification during animation");
                        return;
                    }
                }
                let txid = match self.window(wid) {
                    Ok(window) => self.txid_for_window_state(window),
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
                let frame = match self.handle_ax_result(wid, elem.frame()) {
                    Ok(Some(frame)) => frame,
                    Ok(None) => return,
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
            kAXWindowMiniaturizedNotification => {
                let Ok(wid) = self.id(&elem) else {
                    return;
                };
                if let Some(window) = self.windows.get_mut(&wid) {
                    window.hidden_by_app = false;
                }
                self.send_event(Event::WindowMinimized(wid));
            }
            kAXWindowDeminiaturizedNotification => {
                let Ok(wid) = self.id(&elem) else {
                    return;
                };
                if let Some(window) = self.windows.get_mut(&wid) {
                    window.hidden_by_app = false;
                }
                self.send_event(Event::WindowDeminiaturized(wid));
            }
            kAXTitleChangedNotification => {
                let Ok(wid) = self.id(&elem) else {
                    return;
                };
                match WindowInfo::from_ax_element(&elem, None) {
                    Ok((info, _)) => {
                        self.send_event(Event::WindowTitleChanged(wid, info.title));
                    }
                    Err(err) => {
                        trace!(
                            ?wid,
                            ?err,
                            "Failed to refresh window info for WindowTitleChanged notification"
                        );
                    }
                }
            }
            _ => error!("Unhandled notification {notif:?} on {elem:#?}"),
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

        let make_key_result = window_server::make_key_window(
            this.pid,
            WindowServerId::try_from(&this.window(first)?.elem)?,
        );
        if make_key_result.is_err() {
            warn!(?this.pid, "Failed to activate app");
        }

        if !is_frontmost && make_key_result.is_ok() && is_standard {
            let (tx, rx) = continuation();
            let (quiet_activation, quiet_window_change);
            if wids.len() == 1 {
                // `quiet` only applies if the first window is also the last.
                quiet_activation = quiet;
                quiet_window_change = (quiet == Quiet::Yes).then_some(first);
            } else {
                // Windows before the last are always quiet.
                quiet_activation = Quiet::Yes;
                quiet_window_change = Some(first);
            }
            // this.last_activated = Some((Instant::now(), quiet_activation, quiet_window_change, tx));

            if let Some((_, _, _, prev_tx)) = this.last_activated.replace((
                Instant::now(),
                quiet_activation,
                quiet_window_change,
                tx,
            )) {
                let _ = prev_tx.send(());
            }

            drop(this);
            trace!("Awaiting activation");
            select! {
                _ = rx => {}
                _ = token.cancelled() => {
                    debug!("Raise cancelled while awaiting activation event");
                    return Err(RaiseError::RaiseCancelled);
                }
            }
            trace!("Activation complete");
            this = this_ref.borrow_mut();
        } else {
            trace!(
                "Not awaiting activation event. is_frontmost={is_frontmost:?} \
                make_key_result={make_key_result:?} is_standard={is_standard:?}"
            )
        }

        for (i, &wid) in wids.iter().enumerate() {
            debug_assert_eq!(wid.pid, this.pid);
            let window = this.window(wid)?;
            let _ = trace("raise", &window.elem, || window.elem.raise());

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
                let main_window = this.on_main_window_changed(quiet_if);
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

    fn on_main_window_changed(&mut self, quiet_if: Option<WindowId>) -> Option<WindowId> {
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
                let Some((info, wid, window_server_info)) = self.register_window(elem, None) else {
                    warn!(?self.pid, "Got MainWindowChanged on unknown window");
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

            self.on_main_window_changed(quiet_window_change);

            Event::ApplicationActivated(self.pid, quiet_activation)
        };

        if old_frontmost != is_frontmost {
            self.send_event(event);
        }
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

        let window_server_id = info.sys_id.or_else(|| {
            WindowServerId::try_from(&elem)
                .or_else(|e| {
                    info!("Could not get window server id for {elem:?}: {e}");
                    Err(e)
                })
                .ok()
        });

        let idx = window_server_id
            .map(|sid| NonZeroU32::new(sid.as_u32()).expect("Window server id was 0"))
            .unwrap_or_else(|| {
                self.last_window_idx += 1;
                NonZeroU32::new(self.last_window_idx).unwrap()
            });
        let wid = WindowId { pid: self.pid, idx };
        if self.windows.contains_key(&wid) {
            trace!(?wid, "Window already registered; skipping duplicate");
            return None;
        }

        if !register_notifs(&elem, self) {
            return None;
        }
        let hidden_by_app = self.is_hidden;
        let last_seen_txid = self.txid_from_store(window_server_id).unwrap_or_default();

        let old = self.windows.insert(wid, WindowState {
            elem,
            last_seen_txid,
            hidden_by_app,
            window_server_id,
            is_animating: false,
        });
        debug_assert!(old.is_none(), "Duplicate window id {wid:?}");
        if hidden_by_app {
            self.send_event(Event::WindowMinimized(wid));
        }
        return Some((info, wid, server_info));

        fn register_notifs(win: &AXUIElement, state: &State) -> bool {
            match win.role() {
                Ok(role) if role == AX_WINDOW_ROLE => (),
                _ => return false,
            }
            for notif in WINDOW_NOTIFICATIONS {
                let res = state.observer.add_notification(win, notif);
                if let Err(err) = res {
                    let is_already_registered = matches!(
                        err,
                        AxError::Ax(code) if code == AXError::NotificationAlreadyRegistered
                    );
                    if !is_already_registered {
                        trace!("Watching failed with error {err:?} on window {win:#?}");
                        return false;
                    }
                }
            }
            true
        }
    }

    fn handle_ax_error(&mut self, wid: WindowId, err: &AXError) -> bool {
        if matches!(*err, AXError::InvalidUIElement) {
            if self.windows.remove(&wid).is_some() {
                self.send_event(Event::WindowDestroyed(wid));
                self.on_main_window_changed(Some(wid));
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
        let current_window_ids: HashSet<WindowId> = match self.app.windows() {
            Ok(elems) => elems.iter().filter_map(|elem| self.id(&elem).ok()).collect(),
            Err(e) => {
                trace!(?e, "Failed to get windows; checking each tracked window");
                let mut to_remove = Vec::new();
                for (&wid, window) in self.windows.iter() {
                    // InvalidUIElement means the window is gone
                    if matches!(window.elem.role(), Err(AxError::Ax(AXError::InvalidUIElement))) {
                        to_remove.push(wid);
                    }
                }
                for wid in to_remove {
                    self.remove_tracked_window(wid, "Removed stale window (individual check)");
                }
                return;
            }
        };

        let tracked_wids: Vec<WindowId> = self.windows.keys().copied().collect();
        for wid in tracked_wids {
            if !current_window_ids.contains(&wid) {
                self.remove_tracked_window(wid, "Removed stale window (not in current list)");
            }
        }
    }

    fn remove_tracked_window(&mut self, wid: WindowId, reason: &'static str) {
        if self.windows.remove(&wid).is_some() {
            debug!(?wid, reason);
            self.send_event(Event::WindowDestroyed(wid));
        }
    }

    fn send_event(&self, event: Event) { self.events_tx.send(event); }

    fn window(&self, wid: WindowId) -> Result<&WindowState, AxError> {
        assert_eq!(wid.pid, self.pid);
        self.windows.get(&wid).ok_or(AxError::NotFound)
    }

    fn window_mut(&mut self, wid: WindowId) -> Result<&mut WindowState, AxError> {
        assert_eq!(wid.pid, self.pid);
        self.windows.get_mut(&wid).ok_or(AxError::NotFound)
    }

    fn id(&self, elem: &AXUIElement) -> Result<WindowId, AxError> {
        if let Ok(id) = WindowServerId::try_from(elem) {
            let wid = WindowId {
                pid: self.pid,
                idx: NonZeroU32::new(id.as_u32()).expect("Window server id was 0"),
            };
            if self.windows.contains_key(&wid) {
                return Ok(wid);
            }
        } else if let Some((&wid, _)) = self.windows.iter().find(|(_, w)| &w.elem == elem) {
            return Ok(wid);
        }
        Err(AxError::NotFound)
    }

    fn stop_notifications_for_animation(&self, elem: &AXUIElement) {
        for notif in WINDOW_ANIMATION_NOTIFICATIONS {
            let res = self.observer.remove_notification(elem, notif);
            if let Err(err) = res {
                debug!(?notif, ?elem, "Removing notification failed with error {err}");
            }
        }
    }

    fn restart_notifications_after_animation(&self, elem: &AXUIElement) {
        for notif in WINDOW_ANIMATION_NOTIFICATIONS {
            let res = self.observer.add_notification(elem, notif);
            if let Err(err) = res {
                debug!(?notif, ?elem, "Adding notification failed with error {err}");
            }
        }
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
    let observer =
        observer.install(move |elem, notif| _ = notifications_tx.send((elem, notif.to_owned())));

    let (raises_tx, raises_rx) = actor::channel();
    let state = State {
        pid,
        running_app,
        bundle_id: info.bundle_id.clone(),
        app: app.clone(),
        observer,
        events_tx,
        windows: HashMap::default(),
        last_window_idx: 0,
        main_window: None,
        last_activated: None,
        is_hidden: false,
        is_frontmost: false,
        raises_tx,
        tx_store,
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

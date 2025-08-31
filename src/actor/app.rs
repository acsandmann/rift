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

use accessibility::{AXAttribute, AXUIElement, AXUIElementActions, AXUIElementAttributes};
use accessibility_sys::{
    kAXApplicationActivatedNotification, kAXApplicationDeactivatedNotification,
    kAXErrorCannotComplete, kAXErrorFailure, kAXErrorInvalidUIElement,
    kAXMainWindowChangedNotification, kAXStandardWindowSubrole, kAXTitleChangedNotification,
    kAXUIElementDestroyedNotification, kAXWindowCreatedNotification,
    kAXWindowDeminiaturizedNotification, kAXWindowMiniaturizedNotification,
    kAXWindowMovedNotification, kAXWindowResizedNotification, kAXWindowRole,
};
use r#continue::continuation;
use core_foundation::runloop::CFRunLoop;
use core_foundation::string::CFString;
use objc2::rc::Retained;
use objc2_app_kit::NSRunningApplication;
use objc2_core_foundation::{CGPoint, CGRect};
use serde::{Deserialize, Serialize};
use tokio::{join, select};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Span, debug, error, info, instrument, trace, warn};

use crate::actor;
use crate::actor::reactor::{self, Event, Requested, TransactionId};
use crate::common::collections::HashMap;
use crate::sys::app::NSRunningApplicationExt;
pub use crate::sys::app::{AppInfo, WindowInfo, pid_t};
use crate::sys::axuielement::AXUIElementExt;
use crate::sys::enhanced_ui::{with_enhanced_ui_disabled, with_system_enhanced_ui_disabled};
use crate::sys::event;
use crate::sys::executor::Executor;
use crate::sys::geometry::{ToCGType, ToICrate};
use crate::sys::observer::Observer;
use crate::sys::process::ProcessInfo;
use crate::sys::skylight::{G_CONNECTION, SLSDisableUpdate, SLSReenableUpdate};
use crate::sys::window_server::{self, WindowServerId};

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
    #[cfg(test)]
    pub(crate) fn new(pid: pid_t, idx: u32) -> WindowId {
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

    SetWindowFrame(WindowId, CGRect, TransactionId, bool),
    SetBatchWindowFrame(Vec<(WindowId, CGRect)>, TransactionId),
    SetWindowPos(WindowId, CGPoint, TransactionId, bool),

    BeginWindowAnimation(WindowId),
    EndWindowAnimation(WindowId),

    /// Raise the windows on the screen, in the given order. All windows must be
    /// on the same screen, or they will not be raised correctly.
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

pub fn spawn_app_thread(pid: pid_t, info: AppInfo, events_tx: reactor::Sender) {
    thread::Builder::new()
        .name(format!("{}({pid})", info.bundle_id.as_deref().unwrap_or("")))
        .spawn(move || app_thread_main(pid, info, events_tx))
        .unwrap();
}

struct State {
    pid: pid_t,
    bundle_id: Option<String>,
    #[expect(dead_code, reason = "unused for now")]
    running_app: Retained<NSRunningApplication>,
    app: AXUIElement,
    observer: Observer,
    events_tx: reactor::Sender,
    windows: HashMap<WindowId, WindowState>,
    last_window_idx: u32,
    main_window: Option<WindowId>,
    last_activated: Option<(Instant, Quiet, r#continue::Sender<()>)>,
    is_frontmost: bool,
    raises_tx: actor::Sender<RaiseRequest>,
}

struct WindowState {
    pub elem: AXUIElement,
    last_seen_txid: TransactionId,
}

const APP_NOTIFICATIONS: &[&str] = &[
    kAXApplicationActivatedNotification,
    kAXApplicationDeactivatedNotification,
    kAXMainWindowChangedNotification,
    kAXWindowCreatedNotification,
];

const WINDOW_NOTIFICATIONS: &[&str] = &[
    kAXUIElementDestroyedNotification,
    kAXWindowMovedNotification,
    kAXWindowResizedNotification,
    kAXWindowMiniaturizedNotification,
    kAXWindowDeminiaturizedNotification,
    kAXTitleChangedNotification,
];

const WINDOW_ANIMATION_NOTIFICATIONS: &[&str] =
    &[kAXWindowMovedNotification, kAXWindowResizedNotification];

impl State {
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
                        Err(err) => match err {
                            accessibility::Error::Ax(ax_err)
                                if matches!(
                                    ax_err,
                                    kAXErrorCannotComplete
                                        | kAXErrorInvalidUIElement
                                        | kAXErrorFailure
                                ) =>
                            {
                                debug!(?this.bundle_id, ?this.pid, ?request,
                                            "AX operation failed: {ax_err}");

                                match &request {
                                    Request::SetWindowFrame(wid, ..)
                                    | Request::SetWindowPos(wid, ..)
                                    | Request::BeginWindowAnimation(wid)
                                    | Request::EndWindowAnimation(wid) => {
                                        this.send_event(Event::WindowDestroyed(*wid));
                                        continue;
                                    }
                                    _ => {}
                                }
                            }
                            _ => {
                                error!(?this.bundle_id, ?this.pid, ?request, "Error handling request: {err}")
                            }
                        },
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

        let Ok(initial_window_elements) = self.app.windows() else {
            return false;
        };

        self.windows.reserve(initial_window_elements.len() as usize);
        let mut windows = Vec::with_capacity(initial_window_elements.len() as usize);
        let mut wsids = Vec::with_capacity(initial_window_elements.len() as usize);
        for elem in initial_window_elements.iter() {
            let elem = elem.clone();
            let wsid = WindowServerId::try_from(&elem).ok();
            let Some((info, wid)) = self.register_window(elem) else {
                continue;
            };
            if let Some(wsid) = wsid {
                wsids.push(wsid);
            }
            windows.push((wid, info));
        }
        let window_server_info = window_server::get_windows(&wsids);

        self.main_window = self.app.main_window().ok().and_then(|w| self.id(&w).ok());
        self.is_frontmost = self.app.frontmost().map(|b| b.into()).unwrap_or(false);

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
    fn handle_request(&mut self, request: &mut Request) -> Result<bool, accessibility::Error> {
        match request {
            Request::Terminate => {
                CFRunLoop::get_current().stop();
                self.send_event(Event::ApplicationThreadTerminated(self.pid));
                return Ok(true);
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
                        continue;
                    }
                    let Some((info, wid)) = self.register_window(elem) else {
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
                let window = self.window_mut(wid)?;
                window.last_seen_txid = txid;
                if eui {
                    with_enhanced_ui_disabled(&window.elem, || {
                        trace("set_position", &window.elem, || {
                            window.elem.set_position(pos.to_cgtype())
                        })?;
                        Ok::<(), accessibility::Error>(())
                    })?;
                } else {
                    trace("set_position", &window.elem, || {
                        window.elem.set_position(pos.to_cgtype())
                    })?;
                }
                let frame = trace("frame", &window.elem, || window.elem.fframe(wid))?;
                self.send_event(Event::WindowFrameChanged(
                    wid,
                    frame,
                    txid,
                    Requested(true),
                    None,
                ));
            }
            &mut Request::SetWindowFrame(wid, frame, txid, eui) => {
                let window = self.window_mut(wid)?;
                window.last_seen_txid = txid;
                // https://github.com/koekeishiya/yabai/blob/17c023039eae6a50d18285fce4d8d12f4a8c3626/src/window_manager.c#L729
                // reason for size->position->size
                // optional disabling of enhanced ui has to be cleaner this is doing too much (as in the if stmt)
                if eui {
                    with_enhanced_ui_disabled(&window.elem, || {
                        trace("set_size_1", &window.elem, || {
                            window.elem.set_size(frame.size.to_cgtype())
                        })?;
                        trace("set_position", &window.elem, || {
                            window.elem.set_position(frame.origin.to_cgtype())
                        })?;
                        trace("set_size_2", &window.elem, || {
                            window.elem.set_size(frame.size.to_cgtype())
                        })?;
                        Ok::<(), accessibility::Error>(())
                    })?;
                } else {
                    trace("set_size_1", &window.elem, || {
                        window.elem.set_size(frame.size.to_cgtype())
                    })?;
                    trace("set_position", &window.elem, || {
                        window.elem.set_position(frame.origin.to_cgtype())
                    })?;
                    trace("set_size_2", &window.elem, || {
                        window.elem.set_size(frame.size.to_cgtype())
                    })?;
                }

                let frame = trace("frame", &window.elem, || window.elem.fframe(wid))?;
                self.send_event(Event::WindowFrameChanged(
                    wid,
                    frame,
                    txid,
                    Requested(true),
                    None,
                ));
            }
            &mut Request::SetBatchWindowFrame(ref mut frames, txid) => {
                unsafe { SLSDisableUpdate(*G_CONNECTION) };
                let _ = with_system_enhanced_ui_disabled(|| {
                    for (wid, frame) in frames.iter() {
                        let window = self.window_mut(*wid)?;
                        window.last_seen_txid = txid;
                        window.elem.set_size(frame.size.to_cgtype())?;
                        window.elem.set_position(frame.origin.to_cgtype())?;
                        window.elem.set_size(frame.size.to_cgtype())?;

                        let frame = window.elem.frame()?;
                        self.send_event(Event::WindowFrameChanged(
                            *wid,
                            frame.to_icrate(),
                            txid,
                            Requested(true),
                            None,
                        ));
                    }
                    Ok::<(), accessibility::Error>(())
                });
                unsafe { SLSReenableUpdate(*G_CONNECTION) };
            }
            &mut Request::BeginWindowAnimation(wid) => {
                let window = self.window(wid)?;
                self.stop_notifications_for_animation(&window.elem);
            }
            &mut Request::EndWindowAnimation(wid) => {
                let &WindowState { ref elem, last_seen_txid } = self.window(wid)?;
                self.restart_notifications_after_animation(elem);
                let frame = trace("frame", elem, || elem.fframe(wid))?;
                self.send_event(Event::WindowFrameChanged(
                    wid,
                    frame,
                    last_seen_txid,
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
            kAXApplicationActivatedNotification | kAXApplicationDeactivatedNotification => {
                _ = self.on_activation_changed();
            }
            kAXMainWindowChangedNotification => {
                self.on_main_window_changed(None);
            }
            kAXWindowCreatedNotification => {
                if self.id(&elem).is_ok() {
                    return;
                }
                let Some((window, wid)) = self.register_window(elem) else {
                    return;
                };
                let window_server_info = window_server::get_window(WindowServerId(wid.idx.into()));
                self.send_event(Event::WindowCreated(
                    wid,
                    window,
                    window_server_info,
                    event::get_mouse_state(),
                ));
            }
            kAXUIElementDestroyedNotification => {
                let Some((&wid, _)) = self.windows.iter().find(|(_, w)| w.elem == elem) else {
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
                let last_seen = self.window(wid).unwrap().last_seen_txid;
                let Ok(frame) = elem.fframe(wid) else {
                    return;
                };
                self.send_event(Event::WindowFrameChanged(
                    wid,
                    frame,
                    last_seen,
                    Requested(false),
                    Some(event::get_mouse_state()),
                ));
            }
            // do we care about miniturized/deminiaturized?
            kAXWindowMiniaturizedNotification => {}
            kAXWindowDeminiaturizedNotification => {}
            // TODO: track titles and send them to sketchybar since people seem to care about that
            kAXTitleChangedNotification => {}
            _ => {
                error!("Unhandled notification {notif:?} on {elem:#?}");
            }
        }
    }
}

#[derive(Debug)]
#[allow(dead_code, reason = "used by Debug impls")]
enum RaiseError {
    RaiseCancelled,
    AXError(accessibility::Error),
}

impl From<accessibility::Error> for RaiseError {
    fn from(value: accessibility::Error) -> Self { Self::AXError(value) }
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
            window.elem.subrole().map(|s| s == kAXStandardWindowSubrole).unwrap_or(false)
        };

        check_cancel()?;

        static MUTEX: LazyLock<parking_lot::Mutex<()>> =
            LazyLock::new(|| parking_lot::Mutex::new(()));
        let mut mutex_guard = Some(MUTEX.lock());
        check_cancel()?;
        let mut this = this_ref.borrow_mut();

        let is_frontmost: bool = trace("is_frontmost", &this.app, || this.app.frontmost())?.into();

        let make_key_result = window_server::make_key_window(
            this.pid,
            WindowServerId::try_from(&this.window(first)?.elem)?,
        );
        if make_key_result.is_err() {
            warn!(?this.pid, "Failed to activate app");
        }

        if !is_frontmost && make_key_result.is_ok() && is_standard {
            let (tx, rx) = continuation();
            let quiet_activation = if wids.len() == 1 { quiet } else { Quiet::Yes };

            if let Some((_, _, prev_tx)) =
                this.last_activated.replace((Instant::now(), quiet_activation, tx))
            {
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
                let Some((info, wid)) = self.register_window(elem) else {
                    warn!(?self.pid, "Got MainWindowChanged on unknown window");
                    return None;
                };
                let window_server_info = window_server::get_window(WindowServerId(wid.idx.into()));
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

    fn on_activation_changed(&mut self) -> Result<(), accessibility::Error> {
        // TODO: this prolly isnt needed
        let is_frontmost: bool = trace("is_frontmost", &self.app, || self.app.frontmost())?.into();
        let old_frontmost = std::mem::replace(&mut self.is_frontmost, is_frontmost);
        debug!(
            "on_activation_changed, pid={:?}, is_frontmost={:?}, old_frontmost={:?}",
            self.pid, is_frontmost, old_frontmost
        );

        let event = if is_frontmost {
            let quiet = match self.last_activated.take() {
                Some((ts, quiet, tx)) if ts.elapsed() < Duration::from_millis(1000) => {
                    trace!("by us");
                    _ = tx.send(());
                    quiet
                }
                _ => {
                    trace!("by user");
                    self.on_main_window_changed(None);
                    Quiet::No
                }
            };
            Event::ApplicationActivated(self.pid, quiet)
        } else {
            Event::ApplicationDeactivated(self.pid)
        };

        if old_frontmost != is_frontmost {
            self.send_event(event);
        }
        Ok(())
    }

    #[must_use]
    fn register_window(&mut self, elem: AXUIElement) -> Option<(WindowInfo, WindowId)> {
        let Ok(mut info) = WindowInfo::try_from(&elem) else {
            return None;
        };

        // TODO: improve this heuristic using ideas from AeroSpace(maybe implement a similar testing architecture based on ax dumps)
        if (self.bundle_id.as_deref() == Some("com.googlecode.iterm2")
            || self.bundle_id.as_deref() == Some("com.apple.TextInputUI.xpc.CursorUIViewService"))
            && elem
                .attribute(&AXAttribute::new(&CFString::from_static_string(
                    "AXTitleUIElement",
                )))
                .is_err()
        {
            info.is_standard = false;
        }

        if !register_notifs(&elem, self) {
            return None;
        }
        let idx = WindowServerId::try_from(&elem)
            .or_else(|e| {
                info!("Could not get window server id for {elem:?}: {e}");
                Err(e)
            })
            .ok()
            .map(|id| NonZeroU32::new(id.as_u32()).expect("Window server id was 0"))
            .unwrap_or_else(|| {
                self.last_window_idx += 1;
                NonZeroU32::new(self.last_window_idx).unwrap()
            });
        let wid = WindowId { pid: self.pid, idx };
        let old = self.windows.insert(wid, WindowState {
            elem,
            last_seen_txid: TransactionId::default(),
        });
        assert!(old.is_none(), "Duplicate window id {wid:?}");
        return Some((info, wid));

        fn register_notifs(win: &AXUIElement, state: &State) -> bool {
            match win.role() {
                Ok(role) if role == kAXWindowRole => (),
                _ => return false,
            }
            for notif in WINDOW_NOTIFICATIONS {
                let res = state.observer.add_notification(win, notif);
                if let Err(err) = res {
                    trace!("Watching failed with error {err:?} on window {win:#?}");
                    return false;
                }
            }
            true
        }
    }

    fn send_event(&self, event: Event) { self.events_tx.send(event); }

    fn window(&self, wid: WindowId) -> Result<&WindowState, accessibility::Error> {
        assert_eq!(wid.pid, self.pid);
        self.windows.get(&wid).ok_or(accessibility::Error::NotFound)
    }

    fn window_mut(&mut self, wid: WindowId) -> Result<&mut WindowState, accessibility::Error> {
        assert_eq!(wid.pid, self.pid);
        self.windows.get_mut(&wid).ok_or(accessibility::Error::NotFound)
    }

    fn id(&self, elem: &AXUIElement) -> Result<WindowId, accessibility::Error> {
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
        Err(accessibility::Error::NotFound)
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
        if let Some((_, _, tx)) = self.last_activated.take() {
            let _ = tx.send(());
        }
    }
}

fn app_thread_main(pid: pid_t, info: AppInfo, events_tx: reactor::Sender) {
    let app = AXUIElement::application(pid);
    let Some(running_app) = NSRunningApplication::with_process_id(pid) else {
        info!(?pid, "Making NSRunningApplication failed; exiting app thread");
        return;
    };

    let bundle_id = unsafe { running_app.bundleIdentifier() };

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
        is_frontmost: false,
        raises_tx,
    };

    let (requests_tx, requests_rx) = actor::channel();
    Executor::run(state.run(info, requests_tx, requests_rx, notifications_rx, raises_rx));
}

fn trace<T>(
    desc: &str,
    elem: &AXUIElement,
    f: impl FnOnce() -> Result<T, accessibility::Error>,
) -> Result<T, accessibility::Error> {
    let start = Instant::now();
    let out = f();
    let end = Instant::now();
    // FIXME: ?elem here can change system behavior because it sends requests
    // to the app.
    trace!(time = ?(end - start), /*?elem,*/ "{desc:12}");
    if let Err(err) = &out {
        let app = elem.parent();
        match err {
            accessibility::Error::Ax(ax_err)
                if matches!(
                    *ax_err,
                    kAXErrorCannotComplete | kAXErrorInvalidUIElement | kAXErrorFailure
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

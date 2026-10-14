//! `rift --settings`: the Settings window in a short-lived child process.
//!
//! The daemon owns window management and configuration; this process owns only the native UI
//! and reaches the daemon over its Mach service. Closing the window exits the process, which
//! returns every Settings allocation to the system.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::{Arc, mpsc};

use block2::RcBlock;
use cgs::{Alert, Ui};
use dispatchr::time::Time;
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDidBecomeActiveNotification,
    NSApplicationDidChangeScreenParametersNotification,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSProcessInfo, NSString};
use rift_client::{
    ApplicationData, ClientError, DisplayData, RiftEvent, RiftMachClient, RiftRequest,
};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::{Action, Finish, Request, Settings, updates};
use crate::actor::config::{SourceEdit, SourceSnapshot};
use crate::common::config::ConfigSource;
use crate::sys::dispatch::DispatchExt;
use crate::sys::executor::Executor;
use crate::sys::screen::{ScreenId, ScreenInfo, SpaceId};

type Runtime = (Vec<ScreenInfo>, Vec<ApplicationData>);

/// Daemon requests, answered in order by the IPC worker.
enum Job {
    Source,
    Reload,
    Apply(u64, Arc<ConfigSource>, SourceEdit),
    Runtime,
}

/// Results delivered to the main thread.
enum Message {
    Source(Result<Snapshot, String>),
    /// Candidate preparation failed locally; the acknowledged source is still valid.
    InvalidEdit(String),
    /// `None` when the daemon could not answer this time.
    Runtime(Option<Runtime>),
    ConfigChanged(u64),
    RefreshSettings,
    RefreshRuntime,
    Installed(Vec<(String, String)>),
    Updated(Result<String, String>),
    Closed,
    /// The daemon is gone.
    Lost(String),
}

/// Shared only within Settings; the wire format remains SourceSnapshot.
struct Snapshot {
    revision: u64,
    source: Arc<ConfigSource>,
}
impl From<SourceSnapshot> for Snapshot {
    fn from(snapshot: SourceSnapshot) -> Self {
        Self {
            revision: snapshot.revision,
            source: Arc::new(snapshot.source),
        }
    }
}

struct PendingEdit {
    control: Option<Rc<()>>,
    edit: Option<SourceEdit>,
    finishes: Vec<Finish>,
}
impl PendingEdit {
    fn into_finish(self) -> Finish {
        Box::new(move |result| {
            for finish in self.finishes {
                finish(result.clone());
            }
        })
    }
}

/// Only adjacent absolute assignments from the same native control supersede each other.
/// A different control, reload, or transaction is an ordering barrier.
fn enqueue(
    edits: &mut VecDeque<PendingEdit>,
    control: Option<Rc<()>>,
    edit: Option<SourceEdit>,
    finish: Finish,
) {
    if let Some(control) = &control
        && let Some(last) = edits.back_mut()
        && last.control.as_ref().is_some_and(|last| Rc::ptr_eq(last, control))
    {
        last.edit = edit;
        last.finishes.push(finish);
        return;
    }
    edits.push_back(PendingEdit {
        control,
        edit,
        finishes: vec![finish],
    });
}

pub fn run(config_path: PathBuf) -> ! {
    let startup = super::timing::start();
    let (messages, inbox) = unbounded_channel();
    // The daemon launches Settings; once it exits, nothing can save the configuration.
    let lost = messages.clone();
    let daemon = std::os::unix::process::parent_id();
    crate::sys::dispatch::on_proc_exit(daemon as i32, move || {
        let _ = lost.send(Message::Lost("Rift quit.".into()));
    });
    if std::os::unix::process::parent_id() != daemon {
        let _ = messages.send(Message::Lost("Rift quit.".into()));
    }
    let (jobs, queue) = mpsc::channel();
    let worker = messages.clone();
    std::thread::Builder::new()
        .name("settings-ipc".into())
        .spawn(move || serve(queue, worker))
        .expect("spawn Settings IPC worker");

    // Keep the same executable and signing identity, but distinguish the Settings child
    // in OS process listings. Foundation's processName alone does not rename it in the kernel.
    NSProcessInfo::processInfo().setProcessName(&NSString::from_str("rift-settings"));
    let name = c"rift-settings";
    // SAFETY: both strings are valid for this call; only the current process is renamed.
    let result = unsafe {
        nix::libc::sysctlbyname(
            c"kern.procname".as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_ptr().cast_mut().cast(),
            name.to_bytes().len(),
        )
    };
    if result != 0 {
        tracing::warn!(error = %std::io::Error::last_os_error(), "Could not name the Settings process");
    }
    let mtm = MainThreadMarker::new().expect("Settings runs on the main thread");
    let appkit = super::timing::start();
    let app = NSApplication::sharedApplication(mtm);
    super::timing::finish(appkit, "NSApplication::sharedApplication");
    let policy = super::timing::start();
    // Settings uses a normal application menu and Dock presence while open.
    // Avoid registering as Accessory only to switch at presentation.
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    super::timing::finish(policy, "initial activation policy");
    let launch = super::timing::start();
    app.finishLaunching();
    super::timing::finish(launch, "finishLaunching");
    Executor::run_main(mtm, open(mtm, config_path, startup, jobs, messages, inbox));
    std::process::exit(0)
}

async fn open(
    mtm: MainThreadMarker,
    config_path: PathBuf,
    startup: Option<super::timing::Start>,
    jobs: mpsc::Sender<Job>,
    messages: UnboundedSender<Message>,
    mut inbox: tokio::sync::mpsc::UnboundedReceiver<Message>,
) {
    let snapshot_wait = super::timing::start();
    let (mut snapshot, mut announced) = (None, 0);
    while snapshot.is_none() {
        match inbox.recv().await {
            Some(Message::Source(Ok(source))) => snapshot = Some(source),
            Some(Message::Source(Err(error)) | Message::Lost(error)) => {
                fail(&error);
                return std::future::pending().await;
            }
            Some(Message::ConfigChanged(revision)) => announced = announced.max(revision),
            _ => {}
        }
    }
    super::timing::finish(snapshot_wait, "snapshot wait after AppKit");
    let Snapshot { revision, source } = snapshot.unwrap();
    let (requests, mut pending) = unbounded_channel();
    let closed = messages.clone();
    let construction = super::timing::start();
    let settings = Settings::new(
        Ui::new(mtm),
        source.clone(),
        config_path,
        Vec::new(),
        Vec::new(),
        requests,
        move || {
            // Keep the previous close behavior while submitted edits drain:
            // the window and Dock presence disappear before the process exits.
            NSApplication::sharedApplication(mtm)
                .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            let _ = closed.send(Message::Closed);
        },
    );
    super::timing::finish(construction, "Settings::new");
    if let Some(startup) = startup {
        let updated = std::cell::Cell::new(false);
        let update = RcBlock::new(move |note: NonNull<NSNotification>| {
            // Activation can update an invisible window. Only count an update after ordering.
            let visible = unsafe { note.as_ref() }
                .object()
                .and_then(|object| object.downcast::<objc2_app_kit::NSWindow>().ok())
                .is_some_and(|window| window.isVisible());
            if visible && !updated.replace(true) {
                super::timing::finish_from_parent(
                    "first visible native window update (render proxy)",
                );
                super::timing::finish(
                    Some(startup),
                    "first visible native window update (render proxy)",
                );
            }
        });
        let _ = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(objc2_app_kit::NSWindowDidUpdateNotification),
                Some(settings.window.ns_window()),
                None,
                &update,
            )
        };
    }
    for (name, activation) in unsafe {
        [
            (NSApplicationDidBecomeActiveNotification, true),
            (NSApplicationDidChangeScreenParametersNotification, false),
        ]
    } {
        let messages = messages.clone();
        let initial_activation =
            std::cell::Cell::new(activation && !NSApplication::sharedApplication(mtm).isActive());
        let refresh = RcBlock::new(move |_: NonNull<NSNotification>| {
            // The first activation only presents the snapshot just obtained. Later activations
            // recover missed broadcasts; screen changes only invalidate runtime data.
            if initial_activation.replace(false) {
                return;
            }
            let _ = messages.send(if activation {
                Message::RefreshSettings
            } else {
                Message::RefreshRuntime
            });
        });
        let _ = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                None,
                &refresh,
            )
        };
    }
    settings.show();
    super::timing::finish(startup, "child entry to presented");
    let mut session = Session {
        settings: Some(settings),
        jobs,
        messages,
        source,
        revision,
        announced,
        edits: VecDeque::new(),
        in_flight: None,
        refresh: false,
        stalled: None,
        runtime: (false, false),
        scan: None,
        update: None,
    };
    session.pump();
    session.refresh_runtime();
    loop {
        tokio::select! {
            // Queue edits sent before the window closed ahead of the close itself.
            biased;
            Some(request) = pending.recv() => session.request(request),
            Some(message) = inbox.recv() => {
                if let Message::Lost(error) = message {
                    if session.settings.is_none() {
                        std::process::exit(1);
                    }
                    fail(&error);
                    return std::future::pending().await;
                }
                session.handle(message);
            }
        }
        // Closing leaves submitted edits to finish; then nothing remains to wait for.
        if session.settings.is_none() && session.in_flight.is_none() && session.edits.is_empty() {
            std::process::exit(0);
        }
    }
}

/// Reports `error`, then exits. The alert runs from the main queue rather than inside the
/// executor's poll, whose run-loop source would otherwise re-enter during the modal loop.
/// Callers stop handling messages until then.
fn fail(error: &str) {
    tracing::error!(%error, "Settings cannot reach Rift");
    dispatchr::queue::main().after_f_s(Time::NOW, error.to_owned(), |error| {
        let ui = Ui::new(MainThreadMarker::new().expect("main queue"));
        Alert::new(&ui, "Rift Settings can’t reach Rift", &error)
            .button("OK")
            .run_modal();
        std::process::exit(1)
    });
}

struct Session {
    settings: Option<Settings>,
    jobs: mpsc::Sender<Job>,
    messages: UnboundedSender<Message>,
    /// The daemon's last acknowledged source; queued edits apply to it in order.
    source: Arc<ConfigSource>,
    revision: u64,
    /// The newest revision the daemon announced.
    announced: u64,
    /// Edits waiting for the one in flight; `None` reloads the file.
    edits: VecDeque<PendingEdit>,
    /// The configuration request awaiting a reply; `Some(None)` is a snapshot refresh.
    in_flight: Option<Option<Finish>>,
    /// Re-read the source before the next edit.
    refresh: bool,
    /// The announced revision when a refresh last failed. Until a newer announcement, refreshes
    /// are retried only for user edits, so a persistent failure cannot loop.
    stalled: Option<u64>,
    /// Runtime query (in flight, outdated by a later notification).
    runtime: (bool, bool),
    /// Completions awaiting installed-application discovery; `Some` while a scan runs.
    scan: Option<Vec<Finish>>,
    update: Option<updates::Completion>,
}

impl Session {
    fn request(&mut self, Request { action, finish, control }: Request) {
        match action {
            Action::Edit(edit) => enqueue(&mut self.edits, control, Some(edit), finish),
            Action::Reload => enqueue(&mut self.edits, None, None, finish),
            Action::RefreshRuntime => return self.discover_applications(finish),
            Action::CheckUpdates(done) => {
                self.update = Some(done);
                let messages = self.messages.clone();
                updates::start(move |result| {
                    let _ = messages.send(Message::Updated(result));
                });
                return finish(Ok(()));
            }
        }
        self.pump();
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Source(result) => {
                let finish = self.in_flight.take().flatten();
                match result {
                    Ok(snapshot) => {
                        self.stalled = None;
                        self.adopt(snapshot);
                        if let Some(finish) = finish {
                            finish(Ok(()));
                        }
                    }
                    // A failed change may have been rejected as stale; resynchronize first.
                    Err(error) => match finish {
                        Some(finish) => {
                            self.refresh = true;
                            finish(Err(error));
                        }
                        None => self.stall(error),
                    },
                }
                self.pump();
            }
            Message::InvalidEdit(error) => {
                if let Some(finish) = self.in_flight.take().flatten() {
                    finish(Err(error));
                }
                self.pump();
            }
            Message::ConfigChanged(revision) => {
                self.announced = self.announced.max(revision);
                self.pump();
            }
            Message::RefreshSettings => {
                // Broadcasts are best-effort. Refocusing also recovers a missed revision or
                // retries a previous failed refresh without creating a retry loop.
                self.refresh = true;
                self.stalled = None;
                self.pump();
                self.refresh_runtime();
            }
            Message::RefreshRuntime => self.refresh_runtime(),
            Message::Runtime(runtime) => {
                if let (Some((displays, applications)), Some(settings)) = (runtime, &self.settings)
                {
                    settings.refresh_applications(applications);
                    settings.refresh_displays(displays);
                }
                let (_, outdated) = std::mem::take(&mut self.runtime);
                if outdated {
                    self.refresh_runtime();
                }
            }
            Message::Installed(apps) => {
                if let Some(settings) = &self.settings {
                    settings.set_installed_applications(apps);
                }
                self.scan.take().into_iter().flatten().for_each(|finish| finish(Ok(())));
            }
            Message::Updated(result) => {
                if let Some(done) = self.update.take() {
                    done(result);
                }
            }
            // Runs after windowWillClose returns, rather than dropping AppKit's active delegate.
            Message::Closed => autoreleasepool(|_| drop(self.settings.take())),
            Message::Lost(_) => unreachable!("handled by the session loop"),
        }
    }

    /// Starts the next configuration request unless one is in flight.
    fn pump(&mut self) {
        if self.in_flight.is_some() {
            return;
        }
        if self.stalled.is_some_and(|announced| self.announced > announced) {
            self.stalled = None;
        }
        let outdated = self.refresh || self.announced > self.revision;
        if (outdated && self.stalled.is_none())
            || (self.stalled.is_some() && !self.edits.is_empty())
        {
            self.refresh = false;
            return self.send(Job::Source, None);
        }
        let Some(mut pending) = self.edits.pop_front() else {
            return;
        };
        let edit = pending.edit.take();
        let finish = pending.into_finish();
        let Some(edit) = edit else {
            return self.send(Job::Reload, Some(finish));
        };
        self.send(
            Job::Apply(self.revision, self.source.clone(), edit),
            Some(finish),
        );
    }

    /// A refresh failed: edits queued behind it would apply to an unknown source, so reject
    /// them, and report the first failure of a stall.
    fn stall(&mut self, error: String) {
        tracing::warn!(%error, "Could not refresh Settings");
        let first = self.stalled.replace(self.announced).is_none();
        for pending in std::mem::take(&mut self.edits) {
            pending.into_finish()(Err(error.clone()));
        }
        if let (true, Some(settings)) = (first, &self.settings) {
            let ui = settings.router.ui;
            Alert::new(&ui, "Settings can’t load the latest configuration", &error)
                .button("OK")
                .show_sheet(settings.window.ns_window(), |_| {});
        }
    }

    fn send(&mut self, job: Job, finish: Option<Finish>) {
        self.in_flight = Some(finish);
        let _ = self.jobs.send(job);
    }

    fn adopt(&mut self, snapshot: Snapshot) {
        self.revision = snapshot.revision;
        self.source = snapshot.source;
        if let Some(settings) = &self.settings {
            settings.synchronize(self.source.clone());
        }
    }

    fn refresh_runtime(&mut self) {
        if self.runtime.0 {
            self.runtime.1 = true;
        } else {
            self.runtime = (true, false);
            let _ = self.jobs.send(Job::Runtime);
        }
    }

    /// Scan installed applications off the main thread, once per process.
    fn discover_applications(&mut self, finish: Finish) {
        if self.settings.as_ref().is_none_or(Settings::has_installed_applications) {
            return finish(Ok(()));
        }
        let messages = &self.messages;
        let waiting = self.scan.get_or_insert_with(|| {
            let messages = messages.clone();
            std::thread::spawn(move || {
                let _ = messages.send(Message::Installed(crate::sys::installed_apps::installed()));
            });
            Vec::new()
        });
        waiting.push(finish);
    }
}

/// The IPC worker: one blocking client, used sequentially, plus one event receiver.
fn serve(jobs: mpsc::Receiver<Job>, messages: UnboundedSender<Message>) {
    let client = RiftMachClient;
    let initialization = super::timing::start();
    let (snapshot, subscription) = match client.initialize_settings::<SourceSnapshot>() {
        Ok(session) => session,
        Err(error) => return drop(messages.send(Message::Lost(error.to_string()))),
    };
    super::timing::finish(initialization, "Settings session IPC (subscribe + snapshot)");
    if messages.send(Message::Source(Ok(snapshot.into()))).is_err() {
        return;
    }
    let events = messages.clone();
    std::thread::Builder::new()
        .name("settings-events".into())
        .spawn(move || {
            loop {
                match subscription.recv_event() {
                    Ok(RiftEvent::ConfigChanged { revision }) => {
                        if events.send(Message::ConfigChanged(revision)).is_err() {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        // Do not leave a window silently disconnected from future updates.
                        // Reopening Settings creates a fresh subscription.
                        let _ = events.send(Message::Lost(error.to_string()));
                        return;
                    }
                }
            }
        })
        .expect("spawn Settings event receiver");
    for job in jobs {
        let timing = super::timing::start();
        let stage = if matches!(job, Job::Runtime) {
            "runtime IPC"
        } else {
            "configuration IPC"
        };
        let message = match job {
            Job::Runtime => match runtime(&client) {
                Ok(runtime) => Message::Runtime(Some(runtime)),
                Err(error) => recoverable(error).map_or_else(Message::Lost, |error| {
                    tracing::warn!(%error, "Could not refresh displays and applications");
                    Message::Runtime(None)
                }),
            },
            Job::Source => source(&client, RiftRequest::GetSettingsSource),
            Job::Reload => source(&client, RiftRequest::ReloadSettingsSource),
            Job::Apply(expected_revision, base, edit) => {
                apply(&client, expected_revision, base, edit)
            }
        };
        super::timing::finish(timing, stage);
        let lost = matches!(message, Message::Lost(_));
        if messages.send(message).is_err() || lost {
            return;
        }
    }
}

fn call<T: serde::de::DeserializeOwned>(
    client: &RiftMachClient,
    request: &RiftRequest,
) -> Result<T, ClientError> {
    client.send_typed_request(request)?.into_result().map_err(ClientError::Server)
}

fn source(client: &RiftMachClient, request: RiftRequest) -> Message {
    match call::<SourceSnapshot>(client, &request) {
        Ok(snapshot) => Message::Source(Ok(snapshot.into())),
        Err(error) => recoverable(error).map_or_else(Message::Lost, |e| Message::Source(Err(e))),
    }
}

/// Clone, compare and serialize off the main thread. A no-op returns the same shared
/// snapshot, so it needs neither serialization nor a Mach request.
fn apply(
    client: &RiftMachClient,
    revision: u64,
    base: Arc<ConfigSource>,
    edit: SourceEdit,
) -> Message {
    let mut candidate = base.as_ref().clone();
    if let Err(error) = edit(&mut candidate) {
        return Message::InvalidEdit(error);
    }
    if candidate == *base {
        return Message::Source(Ok(Snapshot { revision, source: base }));
    }
    match serde_json::to_value(&candidate) {
        Ok(candidate) => source(client, RiftRequest::ApplySettingsSource {
            expected_revision: revision,
            source: candidate,
        }),
        Err(error) => Message::InvalidEdit(error.to_string()),
    }
}

fn runtime(client: &RiftMachClient) -> Result<Runtime, ClientError> {
    let displays: Vec<DisplayData> = call(client, &RiftRequest::GetDisplays)?;
    let applications = call(client, &RiftRequest::GetApplications)?;
    Ok((displays.into_iter().map(screen).collect(), applications))
}

/// A user-facing message, or `Err` when the daemon itself is unreachable.
fn recoverable(error: ClientError) -> Result<String, String> {
    match error {
        ClientError::ServiceUnavailable | ClientError::Mach { .. } => Err(error.to_string()),
        ClientError::MessageTooLarge => Ok("The configuration is too large for Settings to \
                                            send. Edit the configuration file directly."
            .into()),
        ClientError::Server(error) => Ok(match error.get("message").and_then(|m| m.as_str()) {
            Some(message) => message.to_owned(),
            None => error.to_string(),
        }),
        error => Ok(error.to_string()),
    }
}

fn screen(display: DisplayData) -> ScreenInfo {
    let rift_client::Rect { origin, size } = display.frame;
    ScreenInfo {
        id: ScreenId::new(display.screen_id),
        frame: CGRect::new(
            CGPoint::new(origin.x, origin.y),
            CGSize::new(size.width, size.height),
        ),
        backing_scale: display.backing_scale,
        display_uuid: display.uuid,
        name: display.name,
        space: display.space.map(SpaceId::new),
    }
}

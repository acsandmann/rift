//! `rift --settings`: the Settings window in a short-lived child process.
//!
//! The daemon owns window management and configuration; this process owns only the native UI
//! and reaches the daemon over its Mach service. Closing the window exits the process, which
//! returns every Settings allocation to the system.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::mpsc;

use block2::RcBlock;
use cgs::{Alert, Application, Ui};
use dispatchr::time::Time;
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDidBecomeActiveNotification,
    NSApplicationDidChangeScreenParametersNotification,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNotification, NSNotificationCenter};
use rift_client::{
    ApplicationData, ClientError, DisplayData, EventKind, RiftEvent, RiftMachClient, RiftRequest,
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
    Apply(u64, serde_json::Value),
    Runtime,
}

/// Results delivered to the main thread.
enum Message {
    Source(Result<Box<SourceSnapshot>, String>),
    /// `None` when the daemon could not answer this time.
    Runtime(Option<Runtime>),
    ConfigChanged(u64),
    RefreshRuntime,
    Installed(Vec<(String, String)>),
    Updated(Result<String, String>),
    Closed,
    /// The daemon is gone.
    Lost(String),
}

pub fn run(config_path: PathBuf) -> ! {
    let mtm = MainThreadMarker::new().expect("Settings runs on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();
    Application::shared(&Ui::new(mtm)).install_close_window_command();
    Executor::run_main(mtm, open(mtm, config_path));
    std::process::exit(0)
}

async fn open(mtm: MainThreadMarker, config_path: PathBuf) {
    let (messages, mut inbox) = unbounded_channel();
    // The daemon launches Settings; once it exits, nothing can save the configuration.
    let lost = messages.clone();
    let daemon = std::os::unix::process::parent_id();
    crate::sys::dispatch::on_proc_exit(daemon as i32, move || {
        let _ = lost.send(Message::Lost("Rift quit.".into()));
    });
    if std::os::unix::process::parent_id() != daemon {
        fail("Rift quit.");
        return std::future::pending().await;
    }
    let (jobs, queue) = mpsc::channel();
    let worker = messages.clone();
    std::thread::Builder::new()
        .name("settings-ipc".into())
        .spawn(move || serve(queue, worker))
        .expect("spawn Settings IPC worker");
    let _ = jobs.send(Job::Source);
    let _ = jobs.send(Job::Runtime);
    let (mut snapshot, mut runtime, mut announced) = (None, None, 0);
    while snapshot.is_none() || runtime.is_none() {
        match inbox.recv().await {
            Some(Message::Source(Ok(source))) => snapshot = Some(source),
            Some(Message::Source(Err(error)) | Message::Lost(error)) => {
                fail(&error);
                return std::future::pending().await;
            }
            Some(Message::Runtime(result)) => runtime = Some(result.unwrap_or_default()),
            Some(Message::ConfigChanged(revision)) => announced = announced.max(revision),
            _ => {}
        }
    }
    let SourceSnapshot { revision, source } = *snapshot.unwrap();
    let (displays, applications) = runtime.unwrap();
    let (requests, mut pending) = unbounded_channel();
    let closed = messages.clone();
    let settings = Settings::new(
        Ui::new(mtm),
        source.clone(),
        config_path,
        displays,
        applications,
        requests,
        move || {
            let _ = closed.send(Message::Closed);
        },
    );
    settings.show();
    for name in unsafe {
        [
            NSApplicationDidBecomeActiveNotification,
            NSApplicationDidChangeScreenParametersNotification,
        ]
    } {
        let messages = messages.clone();
        let refresh = RcBlock::new(move |_: NonNull<NSNotification>| {
            let _ = messages.send(Message::RefreshRuntime);
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
    source: ConfigSource,
    revision: u64,
    /// The newest revision the daemon announced.
    announced: u64,
    /// Edits waiting for the one in flight; `None` reloads the file.
    edits: VecDeque<(Option<SourceEdit>, Finish)>,
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
    fn request(&mut self, Request { action, finish }: Request) {
        match action {
            Action::Edit(edit) => self.edits.push_back((Some(edit), finish)),
            Action::Reload => self.edits.push_back((None, finish)),
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
                        self.adopt(*snapshot);
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
            Message::ConfigChanged(revision) => {
                self.announced = self.announced.max(revision);
                self.pump();
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
        while self.in_flight.is_none() {
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
            let Some((edit, finish)) = self.edits.pop_front() else {
                return;
            };
            let Some(edit) = edit else {
                return self.send(Job::Reload, Some(finish));
            };
            let mut source = self.source.clone();
            if let Err(error) = edit(&mut source) {
                finish(Err(error));
            } else if source == self.source {
                finish(Ok(()));
            } else {
                match serde_json::to_value(&source) {
                    Ok(source) => {
                        return self.send(Job::Apply(self.revision, source), Some(finish));
                    }
                    Err(error) => finish(Err(error.to_string())),
                }
            }
        }
    }

    /// A refresh failed: edits queued behind it would apply to an unknown source, so reject
    /// them, and report the first failure of a stall.
    fn stall(&mut self, error: String) {
        tracing::warn!(%error, "Could not refresh Settings");
        let first = self.stalled.replace(self.announced).is_none();
        for (_, finish) in std::mem::take(&mut self.edits) {
            finish(Err(error.clone()));
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

    fn adopt(&mut self, snapshot: SourceSnapshot) {
        self.revision = snapshot.revision;
        if self.source != snapshot.source {
            self.source = snapshot.source;
            if let Some(settings) = &self.settings {
                settings.synchronize(self.source.clone());
            }
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
    // Subscribe before the first snapshot so no change can fall between them.
    let subscription = match client.subscribe(EventKind::ConfigChanged) {
        Ok(subscription) => subscription,
        Err(error) => return drop(messages.send(Message::Lost(error.to_string()))),
    };
    let events = messages.clone();
    std::thread::Builder::new()
        .name("settings-events".into())
        .spawn(move || {
            while let Ok(event) = subscription.recv_event() {
                if let RiftEvent::ConfigChanged { revision } = event
                    && events.send(Message::ConfigChanged(revision)).is_err()
                {
                    return;
                }
            }
        })
        .expect("spawn Settings event receiver");
    for job in jobs {
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
            Job::Apply(expected_revision, source_json) => {
                source(&client, RiftRequest::ApplySettingsSource {
                    expected_revision,
                    source: source_json,
                })
            }
        };
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
    match call(client, &request) {
        Ok(snapshot) => Message::Source(Ok(snapshot)),
        Err(error) => recoverable(error).map_or_else(Message::Lost, |e| Message::Source(Err(e))),
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

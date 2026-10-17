//! The launcher window: the home screen with the update bar and PLAY, the settings menu, the
//! install/update pipeline and process orchestration.

mod home;

use crate::assets;
use crate::config_gen;
use crate::events::{AppEvent, LogLevel};
use crate::jobs::{JobKind, JobRunner, kill_pid};
use crate::log_files::LogFiles;
use crate::settings::{
    BOT_COUNT_RANGE, MOB_DENSITY_RANGE, PACKETVER_RANGE, PORT_RANGE, RATE_RANGE, Settings,
};

use eframe::egui;
use egui::{Color32, RichText};
use std::collections::VecDeque;
use std::fmt::Display;
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const OK: Color32 = Color32::from_rgb(0x66, 0xBB, 0x6A);
const WARN: Color32 = Color32::from_rgb(0xE5, 0xC0, 0x7B);
const BAD: Color32 = Color32::from_rgb(0xEF, 0x53, 0x50);
const MUTED: Color32 = Color32::from_rgb(0x9E, 0x9E, 0x9E);

/// How long a freshly started server may take to accept connections before the launcher gives up.
const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(120);
const SERVER_POLL_INTERVAL: Duration = Duration::from_millis(250);
const PORT_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// How long the bots may take to finish their step, save and log out before they are killed.
const BOT_LOGOUT_TIMEOUT: Duration = Duration::from_secs(60);

/// Pages of the settings menu.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Server,
    Components,
    Processes,
    Logs,
}

impl Page {
    const ALL: [Page; 5] = [
        Page::General,
        Page::Server,
        Page::Components,
        Page::Processes,
        Page::Logs,
    ];

    fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Server => "Server & bots",
            Page::Components => "Components",
            Page::Processes => "Processes",
            Page::Logs => "Logs",
        }
    }
}

/// The automatic update: `git pull` in every clone, then a build of every component and whatever
/// else the install is missing.
#[derive(Clone, PartialEq, Eq)]
enum UpdateState {
    /// Not run yet this session.
    Idle,
    Pulling,
    Building,
    /// `changed`: a pull brought new commits.
    Done {
        changed: bool,
    },
    Failed(String),
    /// "Play without updating" was ticked while the update ran.
    Skipped,
}

impl UpdateState {
    fn running(&self) -> bool {
        matches!(self, UpdateState::Pulling | UpdateState::Building)
    }
}

struct LogLine {
    time: String,
    source: String,
    text: String,
    level: LogLevel,
}

pub struct App {
    settings: Settings,
    settings_snapshot: String,
    /// What the menu changes in the generated configuration files; `skip_update` is not part of it.
    config_snapshot: String,
    install_dir_input: String,
    assets_dir_input: String,
    /// The settings menu, when it is open.
    menu: Option<Page>,
    background: Option<egui::TextureHandle>,

    update: UpdateState,
    /// Commits checked out before the update started, to tell whether it changed anything.
    heads_before: String,
    /// Commits checked out now, shown under PLAY.
    heads: String,
    /// Jobs finished since the queue was last empty, for the overall progress.
    batch_done: usize,
    job_started: Option<Instant>,
    /// The last line the running job printed.
    job_line: String,
    /// For a cargo job: the packages in `Cargo.lock` and the `Compiling` lines seen so far, which
    /// estimate its progress.
    cargo_units: Option<usize>,
    cargo_compiled: usize,

    logs: VecDeque<LogLine>,
    log_files: Arc<LogFiles>,
    log_file_error_shown: bool,
    log_filter: String,
    autoscroll: bool,

    runner: JobRunner,
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
    ctx: egui::Context,
    queue: VecDeque<(String, JobKind)>,

    /// Kept until the process exits, also while it is stopping.
    server_pid: Option<u32>,
    server_started: Option<Instant>,
    /// Set by the server's wait thread once it exited.
    server_exited: Arc<AtomicBool>,
    server_stopping: bool,
    client_pid: Option<u32>,
    client_started: Option<Instant>,
    /// Kept until the process exits, also while the bots log out.
    bot_pid: Option<u32>,
    bot_started: Option<Instant>,
    bot_exited: Arc<AtomicBool>,
    /// A line `stop` here, or closing it, logs the bots out (`--stop-on-stdin`).
    bot_stdin: Option<ChildStdin>,
    bots_stopping: bool,
    /// Counts server starts and stops, so a readiness check for an earlier server is ignored.
    server_gen: u64,
    /// The server's game and client data ports have accepted connections.
    server_ready: bool,
    /// The client was asked to start while the server was still initializing.
    client_waiting: bool,
    /// The bots were asked to start while the server was still initializing.
    bots_waiting: bool,
    /// Threads that wait for processes to exit and kill them after a grace period. Joined on exit,
    /// so the launcher does not leave a server running.
    stoppers: Vec<JoinHandle<()>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load();
        let settings_snapshot = serde_json::to_string(&settings).unwrap_or_default();
        let config_snapshot = config_snapshot(&settings);
        home::apply_style(&cc.egui_ctx);
        let background = home::load_background(&cc.egui_ctx);
        let heads = heads_text(&settings);
        let install_dir_input = settings.install_dir.display().to_string();
        let assets_dir_input = settings.assets_dir.display().to_string();

        let (tx, rx) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();

        let mut app = Self {
            settings,
            settings_snapshot,
            config_snapshot,
            install_dir_input,
            assets_dir_input,
            menu: None,
            background,
            update: UpdateState::Idle,
            heads_before: String::new(),
            heads,
            batch_done: 0,
            job_started: None,
            job_line: String::new(),
            cargo_units: None,
            cargo_compiled: 0,
            logs: VecDeque::new(),
            log_files: LogFiles::new(),
            log_file_error_shown: false,
            log_filter: String::new(),
            autoscroll: true,
            runner: JobRunner::new(tx.clone(), ctx.clone()),
            tx,
            rx,
            ctx,
            queue: VecDeque::new(),
            server_pid: None,
            server_started: None,
            server_exited: Arc::new(AtomicBool::new(false)),
            server_stopping: false,
            client_pid: None,
            client_started: None,
            bot_pid: None,
            bot_started: None,
            bot_exited: Arc::new(AtomicBool::new(false)),
            bot_stdin: None,
            bots_stopping: false,
            server_gen: 0,
            server_ready: false,
            client_waiting: false,
            bots_waiting: false,
            stoppers: Vec::new(),
        };

        app.push_log(
            "launcher",
            "Ready. The install folder and your kRO assets folder are set in Settings → General."
                .into(),
            LogLevel::Info,
        );
        app
    }

    // -- event plumbing ----------------------------------------------------------

    fn push_log(&mut self, source: &str, text: String, level: LogLevel) {
        if let Err(error) = self
            .log_files
            .write(&self.settings.install_dir, source, &text)
            && !self.log_file_error_shown
        {
            self.log_file_error_shown = true;
            self.push_log_line(
                "launcher",
                format!("could not write log files: {error}"),
                LogLevel::Warn,
            );
        }
        self.push_log_line(source, text, level);
    }

    fn push_log_line(&mut self, source: &str, text: String, level: LogLevel) {
        if self.logs.len() >= 5_000 {
            self.logs.pop_front();
        }
        self.logs.push_back(LogLine {
            time: now_hhmmss(),
            source: source.to_string(),
            text,
            level,
        });
    }

    fn handle_events(&mut self) {
        let current_id = self.runner.current.as_ref().map(|job| job.id);
        while let Ok(event) = self.rx.try_recv() {
            match event {
                AppEvent::JobLog { id, line } => {
                    if Some(id) == current_id {
                        self.note_job_line(&line);
                        self.push_log("job", line, LogLevel::Info);
                    }
                }
                AppEvent::JobProgress { id, fraction, note } => {
                    if Some(id) == current_id {
                        self.runner.progress = fraction;
                        self.runner.progress_note = note;
                    }
                }
                AppEvent::JobDone { id, label, result } => {
                    if Some(id) != current_id {
                        continue;
                    }
                    self.runner.current = None;
                    self.runner.progress = 0.0;
                    self.runner.progress_note.clear();
                    self.heads = heads_text(&self.settings);
                    match result {
                        Ok(message) => {
                            self.batch_done += 1;
                            self.push_log("job", format!("{label}: {message}"), LogLevel::Success)
                        }
                        Err(error) => {
                            self.push_log(
                                "job",
                                format!("{label} failed: {error}"),
                                LogLevel::Error,
                            );
                            self.queue.clear();
                            if self.update.running() {
                                self.update = UpdateState::Failed(format!("{label}: {error}"));
                            }
                        }
                    }
                }
                AppEvent::ProcLog { name, line } => self.push_log_line(&name, line, LogLevel::Info),
                AppEvent::ServerReady { generation, result } => {
                    if generation != self.server_gen || self.server_pid.is_none() {
                        continue;
                    }
                    match result {
                        Ok(()) => {
                            self.server_ready = true;
                            self.push_log(
                                "server",
                                "accepting connections".into(),
                                LogLevel::Success,
                            );
                            if std::mem::take(&mut self.client_waiting) {
                                self.start_client();
                            }
                            if std::mem::take(&mut self.bots_waiting) || self.settings.launch_bots {
                                self.start_bots();
                            }
                        }
                        Err(error) => {
                            self.client_waiting = false;
                            self.bots_waiting = false;
                            self.push_log(
                                "server",
                                format!("{error}; client and bots not started"),
                                LogLevel::Error,
                            );
                        }
                    }
                }
                AppEvent::ProcExited { name, code } => {
                    if name == "server" {
                        self.forget_server();
                    }
                    if name == "client" {
                        self.client_pid = None;
                        self.client_started = None;
                    }
                    if name == "bots" {
                        self.bot_pid = None;
                        self.bot_started = None;
                        self.bot_stdin = None;
                        self.bots_stopping = false;
                    }
                    let level = if code == Some(0) {
                        LogLevel::Info
                    } else {
                        LogLevel::Warn
                    };
                    self.push_log(&name, format!("process exited (code {code:?})"), level);
                }
            }
        }
    }

    fn maybe_save_settings(&mut self) {
        let snapshot = serde_json::to_string(&self.settings).unwrap_or_default();
        if snapshot != self.settings_snapshot {
            self.settings_snapshot = snapshot;
            if let Err(error) = self.settings.save() {
                self.push_log(
                    "launcher",
                    format!("could not save settings: {error}"),
                    LogLevel::Warn,
                );
            }
            let config = config_snapshot(&self.settings);
            if config != self.config_snapshot {
                self.config_snapshot = config;
                self.apply_configs();
            }
        }
    }

    /// Writes the server and client configuration as soon as a menu value changes, so the files
    /// match the menu without waiting for the next start. A running server reads its configuration
    /// only at startup, so it is told to restart.
    fn apply_configs(&mut self) {
        let settings = self.settings.clone();
        if let Err(error) = settings.validate() {
            self.push_log(
                "launcher",
                format!("configuration not written: {error}"),
                LogLevel::Warn,
            );
            return;
        }
        if settings.server_cloned() {
            match config_gen::generate_server(&settings) {
                Ok(()) if self.server_pid.is_some() => self.push_log(
                    "server",
                    "configuration updated; restart the server to apply it".into(),
                    LogLevel::Info,
                ),
                Ok(()) => {}
                Err(error) => self.push_log(
                    "server",
                    format!("configuration failed: {error}"),
                    LogLevel::Error,
                ),
            }
        }
        if settings.client_cloned()
            && let Err(error) = config_gen::generate_client(&settings)
        {
            self.push_log(
                "client",
                format!("configuration failed: {error}"),
                LogLevel::Error,
            );
        }
    }

    fn drain_queue(&mut self) {
        if self.runner.is_running() {
            return;
        }
        if self.queue.is_empty() {
            self.advance_update();
        }
        if let Some((label, kind)) = self.queue.pop_front() {
            self.push_log("job", format!("starting: {label}"), LogLevel::Info);
            self.job_started = Some(Instant::now());
            self.job_line.clear();
            self.cargo_compiled = 0;
            self.cargo_units = match &kind {
                JobKind::CargoBuild { dir, .. } => cargo_lock_packages(dir),
                JobKind::BuildClientData { server_dir, .. } => cargo_lock_packages(server_dir),
                _ => None,
            };
            self.runner.start(label, kind);
        } else {
            self.batch_done = 0;
        }
    }

    /// The running job printed `line`: shown under the progress bar, and for cargo counted
    /// against the packages it builds.
    fn note_job_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if line.starts_with("Compiling ") {
            self.cargo_compiled += 1;
        }
        self.job_line = line.chars().take(120).collect();
    }

    /// Progress of the running job, from its own reports or estimated from cargo's output.
    fn job_fraction(&self) -> f32 {
        if self.runner.progress > 0.0 {
            return self.runner.progress;
        }
        match self.cargo_units {
            Some(units) if units > 0 => (self.cargo_compiled as f32 / units as f32).min(0.95),
            _ => 0.0,
        }
    }

    /// Starts the update: pulls first, then [`App::advance_update`] queues the builds.
    fn start_update(&mut self) {
        self.heads_before = heads_text(&self.settings);
        self.update = UpdateState::Pulling;
        self.batch_done = 0;
        if self.settings.assets_dir.as_os_str().is_empty() {
            self.push_log(
                "setup",
                "No assets folder selected: the client data will not be built. Choose it in Settings → General."
                    .into(),
                LogLevel::Warn,
            );
        }
        self.push_log("launcher", "checking for updates…".into(), LogLevel::Info);
        self.queue
            .extend(crate::jobs::update_pull_plan(&self.settings));
    }

    /// The queue ran empty during an update: after the pulls come the builds, after the builds
    /// the update is done.
    fn advance_update(&mut self) {
        match self.update {
            UpdateState::Pulling => {
                self.update = UpdateState::Building;
                self.queue
                    .extend(crate::jobs::update_build_plan(&self.settings));
            }
            UpdateState::Building => {
                let changed = heads_text(&self.settings) != self.heads_before;
                self.update = UpdateState::Done { changed };
                self.push_log(
                    "launcher",
                    if changed {
                        "update installed".into()
                    } else {
                        "everything is up to date".into()
                    },
                    LogLevel::Success,
                );
            }
            _ => {}
        }
    }

    /// Runs the update once per session, before anything was started, unless "Play without
    /// updating" is ticked. An install that is not complete is always finished.
    fn maybe_start_update(&mut self) {
        if !matches!(self.update, UpdateState::Idle | UpdateState::Skipped)
            || (self.settings.skip_update && self.setup_complete())
            || self.runner.is_running()
            || !self.queue.is_empty()
            || self.any_process()
        {
            return;
        }
        self.start_update();
    }

    /// "Play without updating" was ticked during the update: the rest of it is dropped.
    fn skip_update_now(&mut self) {
        if !self.update.running() {
            return;
        }
        self.update = UpdateState::Skipped;
        self.queue.clear();
        self.runner.cancel();
        self.push_log("launcher", "update skipped".into(), LogLevel::Warn);
    }

    fn any_process(&self) -> bool {
        self.server_pid.is_some() || self.client_pid.is_some() || self.bot_pid.is_some()
    }

    fn enqueue(&mut self, label: impl Into<String>, kind: JobKind) {
        self.queue.push_back((label.into(), kind));
    }

    // -- install planning --------------------------------------------------------

    fn queue_install(&mut self, force: bool) {
        if self.settings.assets_dir.as_os_str().is_empty() {
            self.push_log(
                "setup",
                "No assets folder selected: the client data will not be built.".into(),
                LogLevel::Warn,
            );
        }
        for (label, kind) in crate::jobs::install_plan(&self.settings, force) {
            self.queue.push_back((label, kind));
        }
    }

    // -- process management ------------------------------------------------------

    fn start_server(&mut self) {
        if self.server_pid.is_some() {
            return;
        }
        let settings = self.settings.clone();
        if !settings.server_built() {
            self.push_log(
                "server",
                format!(
                    "binary not found: {} (build it in the Setup tab)",
                    settings.server_bin().display()
                ),
                LogLevel::Warn,
            );
            return;
        }
        if let Err(error) = config_gen::generate_server(&settings) {
            self.push_log(
                "server",
                format!("configuration failed: {error}"),
                LogLevel::Error,
            );
            return;
        }
        if let Some(spawned) = self.spawn_managed(
            "server",
            settings.server_bin(),
            settings.server_dir(),
            settings.server_args(),
            false,
        ) {
            self.server_pid = Some(spawned.pid);
            self.server_exited = spawned.exited;
            self.server_started = Some(Instant::now());
            self.server_gen += 1;
            self.server_ready = false;
            self.push_log(
                "server",
                format!(
                    "game server starting on {}:{}",
                    settings.server_host, settings.game_port
                ),
                LogLevel::Success,
            );
            spawn_readiness_check(
                self.tx.clone(),
                self.ctx.clone(),
                self.server_gen,
                settings.server_probe_host(),
                settings.server_ports(),
            );
        }
    }

    fn start_client(&mut self) {
        if self.client_pid.is_some() {
            return;
        }
        let settings = self.settings.clone();
        if !settings.client_built() {
            self.push_log(
                "client",
                format!(
                    "binary not found: {} (build it in the Setup tab)",
                    settings.client_bin().display()
                ),
                LogLevel::Warn,
            );
            return;
        }
        if let Err(error) = config_gen::generate_client(&settings) {
            self.push_log(
                "client",
                format!("configuration failed: {error}"),
                LogLevel::Error,
            );
            return;
        }
        // The client logs in to the game server and reads /data from it, so it waits until both ports
        // accept connections. ServerReady starts it then.
        if self.server_pid.is_some() && !self.server_ready {
            if !self.client_waiting {
                self.client_waiting = true;
                self.push_log(
                    "client",
                    "waiting for the game server to finish starting…".into(),
                    LogLevel::Info,
                );
            }
            return;
        }
        // The client reads config.json and data/ relative to its working dir.
        if let Some(spawned) = self.spawn_managed(
            "client",
            settings.client_bin(),
            settings.client_dir(),
            Vec::new(),
            false,
        ) {
            self.client_pid = Some(spawned.pid);
            self.client_started = Some(Instant::now());
            self.push_log(
                "client",
                format!(
                    "nostalro client connecting to {}:{}",
                    settings.connect_host(),
                    settings.game_port
                ),
                LogLevel::Success,
            );
        }
    }

    /// Starts rust-ro-bot once the server accepts connections: it needs the Bot API, the reference
    /// and the API key the server writes on its first start.
    fn start_bots(&mut self) {
        if self.bot_pid.is_some() {
            return;
        }
        let settings = self.settings.clone();
        if !settings.bot_built() {
            self.push_log(
                "bots",
                format!(
                    "binary not found: {} (build it in the Setup tab)",
                    settings.bot_bin().display()
                ),
                LogLevel::Warn,
            );
            return;
        }
        if !settings.bot_api_enabled {
            self.push_log(
                "bots",
                "not started: the Bot API is disabled in the server settings".into(),
                LogLevel::Warn,
            );
            return;
        }
        if self.server_pid.is_none() || self.server_stopping {
            self.push_log(
                "bots",
                "not started: start the game server first".into(),
                LogLevel::Warn,
            );
            return;
        }
        if !self.server_ready {
            if !self.bots_waiting {
                self.bots_waiting = true;
                self.push_log(
                    "bots",
                    "waiting for the game server to finish starting…".into(),
                    LogLevel::Info,
                );
            }
            return;
        }
        if !settings.bot_api_key_file().is_file() {
            self.push_log(
                "bots",
                format!(
                    "not started: the server wrote no API key at {}",
                    settings.bot_api_key_file().display()
                ),
                LogLevel::Error,
            );
            return;
        }
        // The bots read names.txt from their working directory and keep their memory (state/) and
        // the reference cache (cache/) there.
        if let Some(spawned) = self.spawn_managed(
            "bots",
            settings.bot_bin(),
            settings.bot_dir(),
            settings.bot_args(),
            true,
        ) {
            self.bot_pid = Some(spawned.pid);
            self.bot_exited = spawned.exited;
            self.bot_stdin = spawned.stdin;
            self.bots_stopping = false;
            self.bot_started = Some(Instant::now());
            self.push_log(
                "bots",
                format!(
                    "{} bots joining the game server in {}",
                    settings.bot_count,
                    settings.server_dir().display()
                ),
                LogLevel::Success,
            );
        }
    }

    /// Starts a process whose output goes to the Logs tab. With `stdin`, the launcher keeps a pipe
    /// to its standard input.
    fn spawn_managed(
        &mut self,
        name: &'static str,
        program: std::path::PathBuf,
        cwd: std::path::PathBuf,
        args: Vec<String>,
        stdin: bool,
    ) -> Option<Spawned> {
        let mut command = Command::new(&program);
        command
            .args(&args)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if stdin { Stdio::piped() } else { Stdio::null() });
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }

        match command.spawn() {
            Ok(mut child) => {
                let pid = child.id();
                let tx = self.tx.clone();
                let ctx = self.ctx.clone();
                let sink = ProcessLogSink {
                    files: self.log_files.clone(),
                    install_dir: self.settings.install_dir.clone(),
                };
                if let Some(stdout) = child.stdout.take() {
                    spawn_process_reader(
                        stdout,
                        name.to_string(),
                        tx.clone(),
                        ctx.clone(),
                        sink.clone(),
                    );
                }
                if let Some(stderr) = child.stderr.take() {
                    spawn_process_reader(stderr, name.to_string(), tx.clone(), ctx.clone(), sink);
                }
                let stdin = child.stdin.take();
                let exited = Arc::new(AtomicBool::new(false));
                let exited_flag = Arc::clone(&exited);
                std::thread::spawn(move || {
                    let status = child.wait();
                    // Set while `child` still holds the process handle, so the pid is not reused yet.
                    exited_flag.store(true, Ordering::SeqCst);
                    let _ = tx.send(AppEvent::ProcExited {
                        name: name.to_string(),
                        code: status.ok().and_then(|status| status.code()),
                    });
                    ctx.request_repaint();
                });
                Some(Spawned { pid, exited, stdin })
            }
            Err(error) => {
                self.push_log(
                    name,
                    format!("could not start {}: {error}", program.display()),
                    LogLevel::Error,
                );
                None
            }
        }
    }

    /// Logs the bots out first, while the server still answers them, then kills the server. The
    /// server keeps its pid until it exited, so no new server starts on its ports meanwhile.
    fn stop_server(&mut self) {
        let Some(pid) = self.server_pid else {
            return;
        };
        if self.server_stopping {
            return;
        }
        self.server_stopping = true;
        self.push_log("server", "stopping…".into(), LogLevel::Warn);
        self.abandon_server();
        let bots = self.request_bot_logout();
        let exited = Arc::clone(&self.server_exited);
        self.stoppers.push(std::thread::spawn(move || {
            if let Some((bot_pid, bot_exited)) = bots {
                wait_or_kill(bot_pid, &bot_exited, BOT_LOGOUT_TIMEOUT);
            }
            wait_or_kill(pid, &exited, Duration::ZERO);
        }));
    }

    /// The server exited: clears its state, and running bots log out.
    fn forget_server(&mut self) {
        self.server_pid = None;
        self.server_started = None;
        self.server_stopping = false;
        self.abandon_server();
        self.stop_bots();
    }

    /// A server on its way out: a readiness check still running for it is ignored, and a client or
    /// bots waiting for it are not started.
    fn abandon_server(&mut self) {
        self.server_ready = false;
        self.server_gen += 1;
        if std::mem::take(&mut self.client_waiting) {
            self.push_log(
                "client",
                "not started: the game server stopped".into(),
                LogLevel::Warn,
            );
        }
        if std::mem::take(&mut self.bots_waiting) {
            self.push_log(
                "bots",
                "not started: the game server stopped".into(),
                LogLevel::Warn,
            );
        }
    }

    fn stop_client(&mut self) {
        if let Some(pid) = self.client_pid.take() {
            kill_pid(pid);
            self.client_started = None;
            self.push_log("client", "stopping…".into(), LogLevel::Warn);
        }
    }

    /// Logs the bots out: each finishes its step, saves its memory and leaves the game. They are
    /// killed if they still run after `BOT_LOGOUT_TIMEOUT`.
    fn stop_bots(&mut self) {
        let first = !self.bots_stopping;
        if let Some((pid, exited)) = self.request_bot_logout()
            && first
        {
            self.stoppers.push(std::thread::spawn(move || {
                wait_or_kill(pid, &exited, BOT_LOGOUT_TIMEOUT);
            }));
        }
    }

    /// Sends the running bots `stop` and closes their stdin. Returns their pid and exit flag for
    /// whoever waits for them.
    fn request_bot_logout(&mut self) -> Option<(u32, Arc<AtomicBool>)> {
        self.bots_waiting = false;
        let pid = self.bot_pid?;
        if !self.bots_stopping {
            self.bots_stopping = true;
            if let Some(mut stdin) = self.bot_stdin.take() {
                let _ = stdin.write_all(b"stop\n");
            }
            self.push_log("bots", "logging out…".into(), LogLevel::Warn);
        }
        Some((pid, Arc::clone(&self.bot_exited)))
    }

    /// Stops everything; the bots log out before the server goes down.
    fn stop_all(&mut self) {
        self.stop_server();
        self.stop_client();
        self.stop_bots();
    }

    fn launch_game(&mut self) {
        if !self.setup_complete() {
            self.push_log(
                "launcher",
                "The package is not fully installed yet — see Settings → Components.".into(),
                LogLevel::Warn,
            );
            self.menu = Some(Page::Components);
            return;
        }
        if self.update.running() {
            return;
        }

        // start_server / start_client each regenerate their own configuration. start_client waits
        // for the server to accept connections, so the client is not started if the server failed.
        self.start_server();
        if self.server_pid.is_some() {
            self.start_client();
        }
    }

    /// True when every component is built and the game assets are prepared.
    fn setup_complete(&self) -> bool {
        let settings = &self.settings;
        settings.server_built()
            && settings.client_built()
            && (!settings.launch_bots || settings.bot_built())
            && assets::client_data_ready(settings)
            && (!settings.install_translation || settings.translation_applied())
    }

    // -- UI ----------------------------------------------------------------------

    /// Component and process states, shown under the menu's pages.
    fn status_overview(&self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Components").strong());
        let settings = &self.settings;
        status_line(
            ui,
            "rust-ro server",
            settings.server_built(),
            settings.server_cloned(),
        );
        status_line(
            ui,
            "nostalro client",
            settings.client_built(),
            settings.client_cloned(),
        );
        status_line(
            ui,
            "rust-ro-bot",
            settings.bot_built(),
            settings.bot_cloned(),
        );
        status_line(
            ui,
            "client data",
            assets::client_data_ready(settings),
            !settings.assets_dir.as_os_str().is_empty(),
        );
        if settings.install_translation {
            status_line(
                ui,
                "translation",
                settings.translation_applied(),
                settings.translation_cloned(),
            );
        }
        ui.add_space(10.0);
        ui.label(RichText::new("Processes").strong());
        process_line(
            ui,
            "server",
            self.server_pid,
            self.server_started,
            self.server_stopping,
        );
        process_line(ui, "client", self.client_pid, self.client_started, false);
        process_line(
            ui,
            "bots",
            self.bot_pid,
            self.bot_started,
            self.bots_stopping,
        );
    }

    fn general_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Folders").strong());
        ui.group(|ui| {
            let mut browse_install = false;
            let mut browse_assets = false;

            path_field_row(ui, "Install folder", &mut self.install_dir_input, &mut browse_install);
            path_field_row(ui, "Assets folder", &mut self.assets_dir_input, &mut browse_assets);

            if browse_install {
                let start = std::path::PathBuf::from(self.install_dir_input.trim());
                if let Some(dir) = rfd::FileDialog::new().set_directory(&start).pick_folder() {
                    self.install_dir_input = dir.display().to_string();
                }
            }
            if browse_assets {
                let start = std::path::PathBuf::from(self.assets_dir_input.trim());
                if let Some(dir) = rfd::FileDialog::new().set_directory(&start).pick_folder() {
                    self.assets_dir_input = dir.display().to_string();
                }
            }

            ui.label(
                RichText::new(
                    "The assets folder is your client root: the folder that contains DATA.INI and the .grf archives.",
                )
                .small()
                .color(MUTED),
            );
            ui.add_space(4.0);
            let install = self.install_dir_input.trim().to_string();
            if !install.is_empty() {
                self.settings.install_dir = std::path::PathBuf::from(install);
            }
            let assets = self.assets_dir_input.trim().to_string();
            self.settings.assets_dir = if assets.is_empty() {
                std::path::PathBuf::new()
            } else {
                std::path::PathBuf::from(assets)
            };
        });

        ui.add_space(8.0);
        ui.label(RichText::new("Client data").strong());
        ui.group(|ui| {
            ui.label(
                RichText::new(
                    "The server builds the game data from your kRO folder (rust-ro tools/client-data) and serves it on the client data port. The client downloads what it needs from there, so no archives are copied into nostalro-client.",
                )
                .small()
                .color(MUTED),
            );
            ui.add_space(4.0);
            ui.checkbox(
                &mut self.settings.install_translation,
                "Install the English translation (ROenglishRE · Pre-Renewal)",
            );
            ui.label(
                RichText::new(
                    "Cloned during setup and layered over nostalro-client/data/extracted/, which the client reads before the served data: Renewal base, then the Pre-Renewal files, then the 2012-era compatibility tables (item/skill/message text). Enabled by default on English systems.",
                )
                .small()
                .color(MUTED),
            );
            ui.add_space(4.0);
        });

        ui.add_space(8.0);
        ui.label(RichText::new("Updates & builds").strong());
        ui.group(|ui| {
            ui.checkbox(&mut self.settings.skip_update, "Play without updating")
                .on_hover_text(
                    "Unticked, the launcher pulls every repository and rebuilds what changed each time it starts. Ticked, it only finishes an install that is not complete.",
                );
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.settings.build_release, "Release build");
                ui.checkbox(&mut self.settings.debug_log, "Server debug log")
                    .on_hover_text(
                        "Starts the server with --debug-log: logs failed NPC conversations and other script failures. Applies on the next server start.",
                    );
            });
        });
    }

    fn server_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Server & network").strong());
        ui.group(|ui| {
            egui::Grid::new("network")
                .num_columns(2)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Bind host");
                    ui.add(egui::TextEdit::singleline(&mut self.settings.server_host).desired_width(180.0));
                    ui.end_row();

                    ui.label("Advertised host");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.public_host)
                            .desired_width(180.0)
                            .hint_text("empty = auto (127.0.0.1)"),
                    );
                    ui.end_row();

                    ui.label("Game port");
                    ui.horizontal(|ui| {
                        checked_field(ui, "game_port", &mut self.settings.game_port, PORT_RANGE);
                    });
                    ui.end_row();

                    ui.label("Client data port");
                    ui.horizontal(|ui| {
                        checked_field(ui, "client_data_port", &mut self.settings.client_data_port, PORT_RANGE);
                    })
                    .response
                    .on_hover_text("Serves the game data the client downloads (the client's data_url).");
                    ui.end_row();

                    ui.label("Bot API");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.settings.bot_api_enabled, "enabled")
                            .on_hover_text("The /bots API lets an external program play characters. Unticked, the server does not open its port.");
                        checked_field(ui, "bot_api_port", &mut self.settings.bot_api_port, PORT_RANGE);
                    });
                    ui.end_row();

                    ui.label("Reference port");
                    ui.horizontal(|ui| {
                        checked_field(ui, "reference_port", &mut self.settings.reference_port, PORT_RANGE);
                    })
                    .response
                    .on_hover_text("Serves the game data as JSON (/db) and the wiki (/wiki). The bots plan with it.");
                    ui.end_row();

                    ui.label("Login");
                    ui.checkbox(&mut self.settings.ip_login, "by IP: no password from this computer")
                        .on_hover_text("Logins from this computer skip the account system: the client asks only for a name and a sex, and a new name gets an account on first use. Other computers still log in with an account.");
                    ui.end_row();

                    ui.label("Bots");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.settings.launch_bots, "start with the server")
                            .on_hover_text("Starts rust-ro-bot once the server accepts connections. The bots talk and pick some goals with the decision model at 127.0.0.1:9931; without it they play by the rules alone.");
                        checked_field(ui, "bot_count", &mut self.settings.bot_count, BOT_COUNT_RANGE);
                        ui.label(RichText::new("playing at once").small().color(MUTED));
                    });
                    ui.end_row();

                    ui.label("Packet version");
                    ui.horizontal(|ui| {
                        checked_field(ui, "packetver", &mut self.settings.packetver, PACKETVER_RANGE);
                    });
                    ui.end_row();

                    ui.label("Base EXP rate");
                    ui.horizontal(|ui| {
                        checked_field(ui, "base_exp_rate", &mut self.settings.base_exp_rate, RATE_RANGE);
                    });
                    ui.end_row();

                    ui.label("Job EXP rate");
                    ui.horizontal(|ui| {
                        checked_field(ui, "job_exp_rate", &mut self.settings.job_exp_rate, RATE_RANGE);
                    });
                    ui.end_row();

                    ui.label("Drop rate");
                    ui.horizontal(|ui| {
                        checked_field(ui, "drop_rate", &mut self.settings.drop_rate, RATE_RANGE);
                    });
                    ui.end_row();

                    ui.label("Card drop rate");
                    ui.horizontal(|ui| {
                        checked_field(ui, "drop_rate_card", &mut self.settings.drop_rate_card, RATE_RANGE);
                    });
                    ui.end_row();

                    ui.label("MVP drop rate");
                    ui.horizontal(|ui| {
                        checked_field(ui, "drop_rate_mvp", &mut self.settings.drop_rate_mvp, RATE_RANGE);
                    });
                    ui.end_row();

                    ui.label("Mob density");
                    ui.horizontal(|ui| {
                        checked_field(ui, "mob_density", &mut self.settings.mob_density, MOB_DENSITY_RANGE);
                    });
                    ui.end_row();
                });
            ui.label(
                RichText::new(
                    "Rates are multipliers from 0.0 to 10000.0. Changes are written to the configuration files at once; a running server applies them after a restart.",
                )
                .small()
                .color(MUTED),
            );
            ui.label(
                RichText::new(if self.settings.ip_login {
                    "In-game login: any name, no password. From other computers: admin / qwertz (account 2000000)."
                } else {
                    "In-game login: admin / qwertz (account 2000000)."
                })
                .small()
                .color(MUTED),
            );
        });
    }

    fn components_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Components").strong());
        ui.group(|ui| {
            self.components_ui(ui);
        });

        ui.add_space(8.0);
        ui.label(RichText::new("Progress").strong());
        ui.group(|ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.runner.is_running(),
                        egui::Button::new("Rebuild Everything"),
                    )
                    .clicked()
                {
                    self.queue_install(true);
                }
                if ui.button("Clear queue").clicked() {
                    self.queue.clear();
                }
            });

            let current_label = self.runner.current.as_ref().map(|job| job.label.clone());
            if let Some(label) = current_label {
                ui.add_space(4.0);
                ui.label(format!("Running: {label}"));
                ui.add(
                    egui::ProgressBar::new(self.job_fraction())
                        .show_percentage()
                        .animate(true),
                );
                let note = if self.runner.progress_note.is_empty() {
                    &self.job_line
                } else {
                    &self.runner.progress_note
                };
                if !note.is_empty() {
                    ui.label(RichText::new(note).small().color(MUTED));
                }
                if ui.button("Cancel current job").clicked() {
                    self.runner.cancel();
                }
            } else if self.queue.is_empty() {
                ui.label(RichText::new("Nothing queued.").small().color(MUTED));
            } else {
                ui.label(format!("{} job(s) queued.", self.queue.len()));
            }
            for (label, _) in self.queue.iter().take(15) {
                ui.label(RichText::new(format!("• {label}")).small().color(MUTED));
            }
        });
    }

    fn components_ui(&mut self, ui: &mut egui::Ui) {
        let settings = self.settings.clone();

        let everything_ready = self.setup_complete();
        if !everything_ready {
            let install_button =
                egui::Button::new(RichText::new("Install Everything").strong().size(16.0));
            if ui
                .add_sized([ui.available_width().min(560.0), 40.0], install_button)
                .on_hover_text("Clone, build and prepare everything that is still missing")
                .clicked()
            {
                self.queue_install(false);
            }
            ui.label(
                RichText::new(
                    "Installs whatever is missing: repositories, builds, client data and translation.",
                )
                .small()
                .color(MUTED),
            );
            ui.add_space(8.0);
        }

        // Server -----------------------------------------------------------------
        ui.horizontal(|ui| {
            ui.label(RichText::new("Server · rust-ro").strong());
            state_chip(ui, settings.server_built(), settings.server_cloned());
        });
        ui.label(
            RichText::new(settings.server_dir().display().to_string())
                .small()
                .color(MUTED),
        );
        if settings.server_cloned()
            && let Some(channel) = crate::jobs::read_toolchain_channel(&settings.server_dir())
        {
            ui.label(
                RichText::new(format!("pinned toolchain: {channel}"))
                    .small()
                    .color(MUTED),
            );
        }
        if settings.server_cloned() {
            ui.horizontal(|ui| {
                if ui.button("Build").clicked() {
                    self.enqueue(
                        "Build rust-ro server",
                        JobKind::CargoBuild {
                            dir: settings.server_dir(),
                            release: settings.build_release,
                            package: None,
                        },
                    );
                }
                if ui.button("Update (git pull)").clicked() {
                    self.enqueue(
                        "Update rust-ro server",
                        JobKind::GitPull {
                            dir: settings.server_dir(),
                            stale_marker: None,
                        },
                    );
                }
                if ui
                    .add_enabled(settings.server_built(), egui::Button::new("Run"))
                    .clicked()
                {
                    self.start_server();
                }
            });
        }
        ui.separator();

        // Client -----------------------------------------------------------------
        ui.horizontal(|ui| {
            ui.label(RichText::new("Client · nostalro-client").strong());
            state_chip(ui, settings.client_built(), settings.client_cloned());
        });
        ui.label(
            RichText::new(settings.client_dir().display().to_string())
                .small()
                .color(MUTED),
        );
        if settings.client_cloned() {
            ui.horizontal(|ui| {
                if ui.button("Build").clicked() {
                    self.enqueue(
                        "Build nostalro client",
                        JobKind::CargoBuild {
                            dir: settings.client_dir(),
                            release: settings.build_release,
                            package: Some("ragnarok-client".into()),
                        },
                    );
                }
                if ui.button("Update (git pull)").clicked() {
                    self.enqueue(
                        "Update nostalro client",
                        JobKind::GitPull {
                            dir: settings.client_dir(),
                            stale_marker: None,
                        },
                    );
                }
                if ui
                    .add_enabled(settings.client_built(), egui::Button::new("Run"))
                    .clicked()
                {
                    self.start_client();
                }
            });
        }
        ui.separator();

        // Bots -------------------------------------------------------------------
        ui.horizontal(|ui| {
            ui.label(RichText::new("Bots · rust-ro-bot").strong());
            state_chip(ui, settings.bot_built(), settings.bot_cloned());
        });
        ui.label(
            RichText::new(settings.bot_dir().display().to_string())
                .small()
                .color(MUTED),
        );
        if settings.bot_cloned() {
            ui.horizontal(|ui| {
                if ui.button("Build").clicked() {
                    self.enqueue(
                        "Build rust-ro-bot",
                        JobKind::CargoBuild {
                            dir: settings.bot_dir(),
                            release: settings.build_release,
                            package: None,
                        },
                    );
                }
                if ui.button("Update (git pull)").clicked() {
                    self.enqueue(
                        "Update rust-ro-bot",
                        JobKind::GitPull {
                            dir: settings.bot_dir(),
                            stale_marker: None,
                        },
                    );
                }
                if ui
                    .add_enabled(
                        settings.bot_built()
                            && self.server_pid.is_some()
                            && !self.server_stopping
                            && self.bot_pid.is_none(),
                        egui::Button::new("Run"),
                    )
                    .on_disabled_hover_text("Build the bots and start the game server first")
                    .clicked()
                {
                    self.start_bots();
                }
            });
        }
        ui.separator();

        // Client data -----------------------------------------------------------
        ui.horizontal(|ui| {
            ui.label(RichText::new("Client data · rust-ro tools").strong());
            let ready = assets::client_data_ready(&settings);
            ui.colored_label(
                if ready { OK } else { WARN },
                if ready { "• built" } else { "• not built" },
            );
        });
        ui.label(
            RichText::new(settings.server_client_data_dir().display().to_string())
                .small()
                .color(MUTED),
        );
        ui.horizontal(|ui| {
            if ui.button("Build client data").clicked() {
                if settings.assets_dir.as_os_str().is_empty() {
                    self.push_log(
                        "assets",
                        "Choose an assets folder first.".into(),
                        LogLevel::Warn,
                    );
                } else {
                    self.enqueue(
                        "Build client data (from the kRO assets)",
                        JobKind::BuildClientData {
                            source: settings.assets_dir.clone(),
                            server_dir: settings.server_dir(),
                        },
                    );
                }
            }
            if ui.button("Generate configuration").clicked() {
                self.enqueue(
                    "Generate configuration",
                    JobKind::GenerateConfigs {
                        settings: settings.clone(),
                    },
                );
            }
        });

        // Translation -------------------------------------------------------------
        if settings.install_translation {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Translation · ROenglishRE (Pre-Renewal)").strong());
                state_chip_with(
                    ui,
                    settings.translation_applied(),
                    settings.translation_cloned(),
                    "applied",
                    "cloned, not applied",
                );
            });
            ui.label(
                RichText::new(settings.translation_dir().display().to_string())
                    .small()
                    .color(MUTED),
            );
            if settings.translation_cloned() {
                ui.horizontal(|ui| {
                    if ui.button("Apply into data").clicked() {
                        self.enqueue(
                            "Apply English translation (ROenglishRE, Pre-Renewal)",
                            JobKind::CopyTranslation {
                                sources: settings.translation_sources(),
                                extracted: settings.client_extracted_dir(),
                                marker: settings.translation_marker(),
                            },
                        );
                    }
                    if ui.button("Update (git pull)").clicked() {
                        self.enqueue(
                            "Update ROenglishRE",
                            JobKind::GitPull {
                                dir: settings.translation_dir(),
                                stale_marker: Some(settings.translation_marker()),
                            },
                        );
                    }
                });
            }
        }
    }

    fn dashboard_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Processes").strong());
        let settings = self.settings.clone();

        ui.horizontal_wrapped(|ui| {
            ui.group(|ui| {
                ui.set_min_width(250.0);
                ui.label(RichText::new("Game server").strong());
                process_card_status(
                    ui,
                    self.server_pid,
                    self.server_started,
                    self.server_stopping.then_some("Stopping…"),
                );
                ui.label(
                    RichText::new(format!("{}:{}", settings.server_host, settings.game_port))
                        .small()
                        .color(MUTED),
                );
                ui.horizontal(|ui| {
                    let running = self.server_pid.is_some();
                    if ui
                        .add_enabled(!running, egui::Button::new("Start"))
                        .clicked()
                    {
                        self.start_server();
                    }
                    if ui
                        .add_enabled(running && !self.server_stopping, egui::Button::new("Stop"))
                        .clicked()
                    {
                        self.stop_server();
                    }
                });
            });

            ui.group(|ui| {
                ui.set_min_width(250.0);
                ui.label(RichText::new("Game client").strong());
                process_card_status(ui, self.client_pid, self.client_started, None);
                ui.label(
                    RichText::new(format!(
                        "{}:{}",
                        settings.connect_host(),
                        settings.game_port
                    ))
                    .small()
                    .color(MUTED),
                );
                ui.horizontal(|ui| {
                    let running = self.client_pid.is_some();
                    if ui
                        .add_enabled(!running, egui::Button::new("Start"))
                        .clicked()
                    {
                        self.start_client();
                    }
                    if ui.add_enabled(running, egui::Button::new("Stop")).clicked() {
                        self.stop_client();
                    }
                });
            });

            ui.group(|ui| {
                ui.set_min_width(250.0);
                ui.label(RichText::new("Bots").strong());
                process_card_status(
                    ui,
                    self.bot_pid,
                    self.bot_started,
                    self.bots_stopping.then_some("Logging out…"),
                );
                ui.label(
                    RichText::new(format!(
                        "{} bots · Bot API port {}",
                        settings.bot_count, settings.bot_api_port
                    ))
                    .small()
                    .color(MUTED),
                );
                ui.horizontal(|ui| {
                    let running = self.bot_pid.is_some();
                    if ui
                        .add_enabled(
                            !running && self.server_pid.is_some() && !self.server_stopping,
                            egui::Button::new("Start"),
                        )
                        .on_disabled_hover_text("The bots join a running game server")
                        .clicked()
                    {
                        self.start_bots();
                    }
                    if ui
                        .add_enabled(running && !self.bots_stopping, egui::Button::new("Stop"))
                        .clicked()
                    {
                        self.stop_bots();
                    }
                });
            });
        });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.any_process(), egui::Button::new("Stop All"))
                .clicked()
            {
                self.stop_all();
            }
            if ui.button("Open install folder").clicked() {
                open_path(&settings.install_dir);
            }
            if ui.button("Open client folder").clicked() {
                open_path(&settings.client_dir());
            }
        });

        ui.add_space(10.0);
        ui.group(|ui| {
            ui.label(RichText::new("Quick facts").strong());
            ui.label(if settings.ip_login {
                "In-game login: any name, no password (from this computer)"
            } else {
                "In-game account: admin / qwertz (id 2000000)"
            });
            ui.label(format!(
                "Client config: {}",
                settings.client_config_json().display()
            ));
            ui.label(format!("Server config: {}", settings.server_config_json().display()));
            ui.label(format!("The client downloads its data from {}", settings.client_data_url()));
            ui.label("nostalro-client/data/extracted holds the translation, which overrides the served data.");
        });

        if !assets::client_data_ready(&settings) {
            ui.add_space(8.0);
            ui.colored_label(
                WARN,
                "Client data is not built yet. Choose your kRO folder in General, then press Build client data in Components.",
            );
        }
    }

    fn logs_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut self.log_filter).desired_width(320.0));
            ui.checkbox(&mut self.autoscroll, "Auto-scroll");
            if ui.button("Clear").clicked() {
                self.logs.clear();
            }
            ui.label(
                RichText::new(format!("{} lines", self.logs.len()))
                    .small()
                    .color(MUTED),
            );
        });
        ui.separator();

        let filter = self.log_filter.to_lowercase();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(self.autoscroll)
            .show(ui, |ui| {
                for line in self.logs.iter() {
                    if !filter.is_empty()
                        && !line.text.to_lowercase().contains(&filter)
                        && !line.source.to_lowercase().contains(&filter)
                    {
                        continue;
                    }
                    let color = match line.level {
                        LogLevel::Info => Color32::from_gray(0xD0),
                        LogLevel::Success => OK,
                        LogLevel::Warn => WARN,
                        LogLevel::Error => BAD,
                    };
                    let text = format!("[{}] [{}] {}", line.time, line.source, line.text);
                    ui.add(egui::Label::new(RichText::new(text).monospace().color(color)).wrap());
                }
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
        self.stoppers.retain(|stopper| !stopper.is_finished());
        self.maybe_save_settings();
        self.maybe_start_update();
        self.drain_queue();
        self.window(ctx);
        ctx.request_repaint_after(Duration::from_millis(300));
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop_all();
        self.runner.cancel();
        // The bots log out and the server is killed after them: wait, or the server outlives us.
        for stopper in self.stoppers.drain(..) {
            let _ = stopper.join();
        }
        self.log_files.flush();
    }
}

// -- small UI helpers ------------------------------------------------------------

/// A number typed into the menu. The value is only replaced once the text parses and lies in
/// `range`, so a half-typed or out-of-range number never reaches the settings. The text itself is
/// kept between frames, which lets the user keep typing.
fn number_input<T>(
    ui: &mut egui::Ui,
    id: egui::Id,
    value: &mut T,
    range: RangeInclusive<f64>,
) -> Option<String>
where
    T: Copy + Display + FromStr + Into<f64> + Send + Sync + 'static,
{
    let mut text = ui
        .data_mut(|data| data.get_temp::<String>(id))
        .unwrap_or_else(|| value.to_string());
    ui.add(egui::TextEdit::singleline(&mut text).desired_width(120.0));
    let out_of_range = || {
        Some(format!(
            "must be between {} and {}",
            range.start(),
            range.end()
        ))
    };
    let problem = match text.trim().parse::<T>() {
        Ok(parsed) => {
            let number: f64 = parsed.into();
            if range.contains(&number) {
                *value = parsed;
                None
            } else {
                out_of_range()
            }
        }
        // `70000` is a number too large for a port, not a typo.
        Err(_) if text.trim().parse::<f64>().is_ok() => out_of_range(),
        Err(_) => Some("not a number".to_string()),
    };
    ui.data_mut(|data| data.insert_temp(id, text));
    problem
}

/// [`number_input`] for a settings field, showing why the text is not applied.
fn checked_field<T>(
    ui: &mut egui::Ui,
    name: &'static str,
    value: &mut T,
    range: RangeInclusive<f64>,
) where
    T: Copy + Display + FromStr + Into<f64> + Send + Sync + 'static,
{
    if let Some(problem) = number_input(ui, egui::Id::new(name), value, range) {
        ui.colored_label(BAD, problem);
    }
}

fn status_line(ui: &mut egui::Ui, label: &str, ready: bool, started: bool) {
    let (glyph, color) = if ready {
        ("•", OK)
    } else if started {
        ("•", WARN)
    } else {
        ("•", BAD)
    };
    ui.horizontal(|ui| {
        ui.colored_label(color, glyph);
        ui.label(label);
    });
}

fn state_chip(ui: &mut egui::Ui, built: bool, cloned: bool) {
    state_chip_with(ui, built, cloned, "built", "cloned, not built");
}

fn state_chip_with(
    ui: &mut egui::Ui,
    ready: bool,
    cloned: bool,
    ready_text: &str,
    cloned_text: &str,
) {
    let (text, color) = if ready {
        (ready_text, OK)
    } else if cloned {
        (cloned_text, WARN)
    } else {
        ("missing", BAD)
    };
    ui.colored_label(color, format!("• {text}"));
}

fn process_line(
    ui: &mut egui::Ui,
    name: &str,
    pid: Option<u32>,
    started: Option<Instant>,
    stopping: bool,
) {
    match pid {
        Some(pid) if stopping => {
            ui.label(RichText::new(format!("• {name}: pid {pid} (stopping)")).color(WARN));
        }
        Some(pid) => {
            let elapsed = started
                .map(|instant| instant.elapsed().as_secs())
                .unwrap_or(0);
            ui.label(RichText::new(format!("• {name}: pid {pid} ({elapsed}s)")).color(OK));
        }
        None => {
            ui.label(RichText::new(format!("• {name}: stopped")).color(MUTED));
        }
    }
}

/// `stopping` names what a process on its way out is doing.
fn process_card_status(
    ui: &mut egui::Ui,
    pid: Option<u32>,
    started: Option<Instant>,
    stopping: Option<&str>,
) {
    match pid {
        Some(pid) if stopping.is_some() => {
            ui.colored_label(
                WARN,
                format!("{} · pid {pid}", stopping.unwrap_or_default()),
            );
        }
        Some(pid) => {
            let elapsed = started
                .map(|instant| instant.elapsed().as_secs())
                .unwrap_or(0);
            ui.colored_label(OK, format!("Running · pid {pid} · {elapsed}s"));
        }
        None => {
            ui.colored_label(MUTED, "Stopped");
        }
    }
}

/// One labelled path field with a Browse button, sized to fill the row.
fn path_field_row(ui: &mut egui::Ui, label: &str, value: &mut String, browse: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(label);
        let button_width = 84.0;
        let spacing = ui.spacing().item_spacing.x;
        let field_width = (ui.available_width() - button_width - spacing).max(240.0);
        ui.add_sized(
            [field_width, 30.0],
            egui::TextEdit::singleline(value)
                .font(egui::TextStyle::Monospace)
                .margin(egui::Margin::symmetric(8.0, 6.0))
                .desired_width(field_width),
        );
        if ui
            .add_sized([button_width, 30.0], egui::Button::new("Browse"))
            .clicked()
        {
            *browse = true;
        }
    });
}

/// The settings as they reach the generated configuration files: "Play without updating" changes
/// nothing there.
fn config_snapshot(settings: &Settings) -> String {
    let mut settings = settings.clone();
    settings.skip_update = false;
    serde_json::to_string(&settings).unwrap_or_default()
}

/// The commits the server, client and bots have checked out, e.g. `server 1a2b3c4 · client …`.
fn heads_text(settings: &Settings) -> String {
    [
        ("server", settings.server_dir()),
        ("client", settings.client_dir()),
        ("bots", settings.bot_dir()),
    ]
    .into_iter()
    .filter_map(|(name, dir)| Some(format!("{name} {}", crate::jobs::git_head(&dir)?)))
    .collect::<Vec<_>>()
    .join(" · ")
}

/// Packages in a workspace's `Cargo.lock`: about what a build from scratch compiles.
fn cargo_lock_packages(dir: &Path) -> Option<usize> {
    let lock = std::fs::read_to_string(dir.join("Cargo.lock")).ok()?;
    Some(
        lock.lines()
            .filter(|line| line.trim() == "[[package]]")
            .count(),
    )
}

/// A process started by [`App::spawn_managed`].
struct Spawned {
    pid: u32,
    /// Set by the process's wait thread once it exited.
    exited: Arc<AtomicBool>,
    stdin: Option<ChildStdin>,
}

/// Gives a process up to `grace` to exit, then kills it. `exited` comes from the process's wait
/// thread, so a pid the system already gave to another process is not killed.
fn wait_or_kill(pid: u32, exited: &AtomicBool, grace: Duration) {
    let deadline = Instant::now() + grace;
    while !exited.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(SERVER_POLL_INTERVAL);
    }
    if !exited.load(Ordering::SeqCst) {
        kill_pid(pid);
    }
}

/// Process output is written to disk by the reader threads, so it is kept even while the window is not repainting.
#[derive(Clone)]
struct ProcessLogSink {
    files: Arc<LogFiles>,
    install_dir: PathBuf,
}

fn spawn_process_reader<R: std::io::Read + Send + 'static>(
    reader: R,
    name: String,
    tx: Sender<AppEvent>,
    ctx: egui::Context,
    sink: ProcessLogSink,
) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        let buffered = std::io::BufReader::new(reader);
        for line in buffered.lines() {
            match line {
                Ok(line) => {
                    let _ = sink.files.write(&sink.install_dir, &name, &line);
                    let _ = tx.send(AppEvent::ProcLog {
                        name: name.clone(),
                        line,
                    });
                    ctx.request_repaint();
                }
                Err(_) => break,
            }
        }
    });
}

/// Polls the server's ports from a background thread until all of them accept connections, then
/// reports the result for server instance `generation`.
fn spawn_readiness_check(
    tx: Sender<AppEvent>,
    ctx: egui::Context,
    generation: u64,
    host: String,
    ports: Vec<u16>,
) {
    std::thread::spawn(move || {
        let deadline = Instant::now() + SERVER_READY_TIMEOUT;
        let result = loop {
            if ports.iter().all(|&port| port_accepts(&host, port)) {
                break Ok(());
            }
            if Instant::now() >= deadline {
                let ports: Vec<String> = ports.iter().map(u16::to_string).collect();
                break Err(format!(
                    "game server did not accept connections on {host} ports {} within {}s",
                    ports.join(", "),
                    SERVER_READY_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(SERVER_POLL_INTERVAL);
        };
        let _ = tx.send(AppEvent::ServerReady { generation, result });
        ctx.request_repaint();
    });
}

fn port_accepts(host: &str, port: u16) -> bool {
    match (host, port).to_socket_addrs() {
        Ok(mut addrs) => {
            addrs.any(|addr| TcpStream::connect_timeout(&addr, PORT_PROBE_TIMEOUT).is_ok())
        }
        Err(_) => false,
    }
}

fn now_hhmmss() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

fn open_path(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("explorer").arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn a_process_still_running_after_its_grace_is_killed() {
        #[cfg(windows)]
        let mut command = Command::new("ping");
        #[cfg(windows)]
        command.args(["-n", "30", "127.0.0.1"]);
        #[cfg(not(windows))]
        let mut command = Command::new("sleep");
        #[cfg(not(windows))]
        command.arg("30");
        let mut child = command.stdout(Stdio::null()).spawn().unwrap();

        let started = Instant::now();
        wait_or_kill(
            child.id(),
            &AtomicBool::new(false),
            Duration::from_millis(300),
        );
        child.wait().unwrap();
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn an_exited_process_is_not_waited_for() {
        let started = Instant::now();
        // The pid is never looked at: the exit flag says it is gone.
        wait_or_kill(u32::MAX, &AtomicBool::new(true), Duration::from_secs(30));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn port_probe_sees_only_ports_that_accept_connections() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let open = listener.local_addr().unwrap().port();
        assert!(port_accepts("127.0.0.1", open));

        let closed = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert!(!port_accepts("127.0.0.1", closed));
    }

    #[test]
    fn readiness_check_reports_once_every_port_accepts_connections() {
        let game = TcpListener::bind("127.0.0.1:0").unwrap();
        let data = TcpListener::bind("127.0.0.1:0").unwrap();
        let ports = vec![
            game.local_addr().unwrap().port(),
            data.local_addr().unwrap().port(),
        ];
        let (tx, rx) = mpsc::channel();

        spawn_readiness_check(tx, egui::Context::default(), 7, "127.0.0.1".into(), ports);

        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            AppEvent::ServerReady { generation, result } => {
                assert_eq!(generation, 7);
                assert!(result.is_ok(), "{result:?}");
            }
            _ => panic!("expected ServerReady"),
        }
    }

    #[test]
    fn readiness_check_waits_for_a_port_that_is_not_open_yet() {
        let game = TcpListener::bind("127.0.0.1:0").unwrap();
        let game_port = game.local_addr().unwrap().port();
        // Reserve a port for the client data endpoint, but only start listening on it later.
        let data_port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let (tx, rx) = mpsc::channel();

        spawn_readiness_check(
            tx,
            egui::Context::default(),
            1,
            "127.0.0.1".into(),
            vec![game_port, data_port],
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(600)).is_err(),
            "reported before the data port opened"
        );

        let _data = TcpListener::bind(("127.0.0.1", data_port)).unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)),
            Ok(AppEvent::ServerReady {
                generation: 1,
                result: Ok(())
            })
        ));
    }
}

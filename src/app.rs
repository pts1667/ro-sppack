//! egui dashboard: setup checklist, install pipeline and process orchestration.

use crate::assets;
use crate::config_gen;
use crate::events::{AppEvent, LogLevel};
use crate::jobs::{JobKind, JobRunner, kill_pid};
use crate::log_files::LogFiles;
use crate::settings::{AssetMode, Settings};

use eframe::egui;
use egui::{Color32, RichText};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const OK: Color32 = Color32::from_rgb(0x66, 0xBB, 0x6A);
const WARN: Color32 = Color32::from_rgb(0xE5, 0xC0, 0x7B);
const BAD: Color32 = Color32::from_rgb(0xEF, 0x53, 0x50);
const MUTED: Color32 = Color32::from_rgb(0x9E, 0x9E, 0x9E);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Setup,
    Dashboard,
    Logs,
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
    install_dir_input: String,
    assets_dir_input: String,
    tab: Tab,

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

    server_pid: Option<u32>,
    server_started: Option<Instant>,
    client_pid: Option<u32>,
    client_started: Option<Instant>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load();
        let settings_snapshot = serde_json::to_string(&settings).unwrap_or_default();
        let install_dir_input = settings.install_dir.display().to_string();
        let assets_dir_input = settings.assets_dir.display().to_string();

        let (tx, rx) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();

        let mut app = Self {
            settings,
            settings_snapshot,
            install_dir_input,
            assets_dir_input,
            tab: Tab::Setup,
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
            client_pid: None,
            client_started: None,
        };

        app.push_log(
            "launcher",
            "Ready. Pick an install folder and your kRO assets folder, then press Install Everything.".into(),
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
                    match result {
                        Ok(message) => {
                            self.push_log("job", format!("{label}: {message}"), LogLevel::Success)
                        }
                        Err(error) => {
                            self.push_log(
                                "job",
                                format!("{label} failed: {error}"),
                                LogLevel::Error,
                            );
                            self.queue.clear();
                        }
                    }
                }
                AppEvent::ProcLog { name, line } => self.push_log_line(&name, line, LogLevel::Info),
                AppEvent::ProcExited { name, code } => {
                    if name == "server" {
                        self.server_pid = None;
                        self.server_started = None;
                    }
                    if name == "client" {
                        self.client_pid = None;
                        self.client_started = None;
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
        }
    }

    fn drain_queue(&mut self) {
        if self.runner.is_running() {
            return;
        }
        if let Some((label, kind)) = self.queue.pop_front() {
            self.push_log("job", format!("starting: {label}"), LogLevel::Info);
            self.runner.start(label, kind);
        }
    }

    fn enqueue(&mut self, label: impl Into<String>, kind: JobKind) {
        self.queue.push_back((label.into(), kind));
    }

    // -- install planning --------------------------------------------------------

    fn queue_install(&mut self, force: bool) {
        if self.settings.assets_dir.as_os_str().is_empty() {
            self.push_log(
                "setup",
                "No assets folder selected: game archives will not be prepared.".into(),
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
        if let Some(pid) = self.spawn_managed(
            "server",
            settings.server_bin(),
            settings.server_dir(),
            settings.server_args(),
            Vec::new(),
        ) {
            self.server_pid = Some(pid);
            self.server_started = Some(Instant::now());
            self.push_log(
                "server",
                format!(
                    "game server listening on {}:{}",
                    settings.server_host, settings.game_port
                ),
                LogLevel::Success,
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
        // The client reads config.json and data/ relative to its working dir.
        if let Some(pid) = self.spawn_managed(
            "client",
            settings.client_bin(),
            settings.client_dir(),
            Vec::new(),
            Vec::new(),
        ) {
            self.client_pid = Some(pid);
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

    fn spawn_managed(
        &mut self,
        name: &'static str,
        program: std::path::PathBuf,
        cwd: std::path::PathBuf,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Option<u32> {
        let mut command = Command::new(&program);
        command
            .args(&args)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        for (key, value) in env {
            command.env(key, value);
        }
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
                std::thread::spawn(move || {
                    let status = child.wait();
                    let _ = tx.send(AppEvent::ProcExited {
                        name: name.to_string(),
                        code: status.ok().and_then(|status| status.code()),
                    });
                    ctx.request_repaint();
                });
                Some(pid)
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

    fn stop_server(&mut self) {
        if let Some(pid) = self.server_pid.take() {
            kill_pid(pid);
            self.server_started = None;
            self.push_log("server", "stopping…".into(), LogLevel::Warn);
        }
    }

    fn stop_client(&mut self) {
        if let Some(pid) = self.client_pid.take() {
            kill_pid(pid);
            self.client_started = None;
            self.push_log("client", "stopping…".into(), LogLevel::Warn);
        }
    }

    fn stop_all(&mut self) {
        self.stop_server();
        self.stop_client();
    }

    fn launch_game(&mut self) {
        if !self.setup_complete() {
            self.push_log(
                "launcher",
                "The package is not fully installed yet — open the Setup tab.".into(),
                LogLevel::Warn,
            );
            self.tab = Tab::Setup;
            return;
        }

        // start_server / start_client each regenerate their own configuration.
        self.start_server();
        self.start_client();
    }

    /// True when every component is built and the game assets are prepared.
    fn setup_complete(&self) -> bool {
        let settings = &self.settings;
        settings.server_built()
            && settings.client_built()
            && assets::is_ready(settings)
            && (!settings.install_translation || settings.translation_applied())
    }

    // -- UI ----------------------------------------------------------------------

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("ro-sppack");
                ui.label(RichText::new("Ragnarok Online · rust-ro + nostalro-client").color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ready = self.setup_complete();
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(RichText::new("Launch Game").strong()),
                        )
                        .on_hover_text(
                            "Generate configuration, start the game server and the native client",
                        )
                        .clicked()
                    {
                        self.launch_game();
                    }
                    if ui.button("Stop All").clicked() {
                        self.stop_all();
                    }
                });
            });
            ui.add_space(4.0);
        });
    }

    fn side_bar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(190.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.selectable_value(&mut self.tab, Tab::Setup, "Setup");
                ui.selectable_value(&mut self.tab, Tab::Dashboard, "Dashboard");
                ui.selectable_value(&mut self.tab, Tab::Logs, "Logs");
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new("Components").strong());
                let settings = self.settings.clone();
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
                    "game assets",
                    assets::is_ready(&settings),
                    !settings.assets_dir.as_os_str().is_empty(),
                );
                if settings.install_translation {
                    status_line(
                        ui,
                        "translation",
                        settings.translation_applied(),
                        settings.translation_re_cloned() && settings.translation_pre_cloned(),
                    );
                }
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new("Processes").strong());
                process_line(ui, "server", self.server_pid, self.server_started);
                process_line(ui, "client", self.client_pid, self.client_started);
            });
    }

    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Setup => self.setup_tab(ui),
            Tab::Dashboard => self.dashboard_tab(ui),
            Tab::Logs => self.logs_tab(ui),
        });
    }

    fn setup_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(4.0);
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

            // Primary actions, right below the folders they operate on.
            ui.add_space(10.0);
            let ready = self.setup_complete();
            let busy = self.runner.is_running();
            ui.horizontal(|ui| {
                let size = egui::vec2(160.0, 44.0);
                let install = ui
                    .add_enabled_ui(!busy, |ui| {
                        ui.add_sized(
                            size,
                            egui::Button::new(RichText::new("INSTALL").strong().size(18.0)),
                        )
                    })
                    .inner;
                let install = if busy {
                    install.on_disabled_hover_text("An install is already running")
                } else if ready {
                    install.on_hover_text("Everything is already installed")
                } else {
                    install.on_hover_text("Clone, build and prepare whatever is missing")
                };
                if install.clicked() {
                    self.queue_install(false);
                }
                let play = ui
                    .add_enabled_ui(ready, |ui| {
                        ui.add_sized(
                            size,
                            egui::Button::new(RichText::new("PLAY").strong().size(18.0)),
                        )
                    })
                    .inner;
                let play = if ready {
                    play.on_hover_text("Start the game server and the native client")
                } else {
                    play.on_disabled_hover_text("Install everything first")
                };
                if play.clicked() {
                    self.launch_game();
                }
                if !ready {
                    ui.label(RichText::new("Setup incomplete").small().color(WARN));
                }
            });

            ui.add_space(8.0);
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
                        ui.add(egui::DragValue::new(&mut self.settings.game_port).range(1..=65535));
                        ui.end_row();

                        ui.label("Packet version");
                        ui.add(egui::DragValue::new(&mut self.settings.packetver).speed(1.0));
                        ui.end_row();

                        ui.label("Base EXP rate");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.base_exp_rate)
                                .speed(0.1)
                                .range(0.0..=1000.0),
                        );
                        ui.end_row();

                        ui.label("Job EXP rate");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.job_exp_rate)
                                .speed(0.1)
                                .range(0.0..=1000.0),
                        );
                        ui.end_row();

                        ui.label("Drop rate");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.drop_rate)
                                .speed(0.1)
                                .range(0.0..=1000.0),
                        );
                        ui.end_row();

                        ui.label("Card drop rate");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.drop_rate_card)
                                .speed(0.1)
                                .range(0.0..=1000.0),
                        );
                        ui.end_row();

                        ui.label("Mob density");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.mob_density)
                                .speed(0.1)
                                .range(0.0..=10.0),
                        );
                        ui.end_row();

                        ui.label("Options");
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut self.settings.build_release, "Release build");
                            ui.checkbox(&mut self.settings.debug_log, "Server debug log")
                                .on_hover_text(
                                    "Starts the server with --debug-log: logs failed NPC conversations and other script failures. Applies on the next server start.",
                                );
                        });
                        ui.end_row();
                    });
                ui.label(
                    RichText::new("In-game login: admin / qwertz (account 2000000).")
                        .small()
                        .color(MUTED),
                );
            });

            ui.add_space(8.0);
            ui.label(RichText::new("Game assets").strong());
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut self.settings.asset_mode,
                        AssetMode::Copy,
                        "Copy archives into the package",
                    );
                    ui.selectable_value(
                        &mut self.settings.asset_mode,
                        AssetMode::Reference,
                        "Reference archives in place",
                    );
                });
                ui.label(
                    RichText::new(
                        "Archives land in nostalro-client/data/ (or stay where they are and are referenced by absolute path).",
                    )
                    .small()
                    .color(MUTED),
                );
                ui.checkbox(
                    &mut self.settings.copy_loose_data,
                    "Copy loose data/ overrides into data/extracted/",
                );
                ui.checkbox(
                    &mut self.settings.copy_bgm_ai,
                    "Copy BGM/ into the package (otherwise referenced in place)",
                );
                ui.add_space(4.0);
                ui.checkbox(
                    &mut self.settings.install_translation,
                    "Install the English translation (ROenglishRE + ROenglishPRE)",
                );
                ui.label(
                    RichText::new(
                        "Cloned during setup and copied over data/extracted/ (item, skill and message tables, interface textures) — no GRF repacking needed. ROenglishPRE is layered on top of ROenglishRE. Enabled by default on English systems.",
                    )
                    .small()
                    .color(MUTED),
                );
                ui.add_space(4.0);
            });

            ui.add_space(8.0);
            ui.label(RichText::new("Components").strong());
            ui.group(|ui| {
                self.components_ui(ui);
            });

            ui.add_space(8.0);
            ui.label(RichText::new("Progress").strong());
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.runner.is_running(), egui::Button::new("Rebuild Everything"))
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
                        egui::ProgressBar::new(self.runner.progress)
                            .show_percentage()
                            .animate(true),
                    );
                    if !self.runner.progress_note.is_empty() {
                        ui.label(RichText::new(&self.runner.progress_note).small().color(MUTED));
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

            ui.add_space(12.0);
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
                    "Installs whatever is missing: repositories, builds and game assets.",
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

        // Assets -----------------------------------------------------------------
        ui.horizontal(|ui| {
            ui.label(RichText::new("Game assets").strong());
            let ready = assets::is_ready(&settings);
            ui.colored_label(
                if ready { OK } else { WARN },
                if ready {
                    "• ready"
                } else {
                    "• not prepared"
                },
            );
        });
        ui.horizontal(|ui| {
            if ui.button("Prepare assets").clicked() {
                if settings.assets_dir.as_os_str().is_empty() {
                    self.push_log(
                        "assets",
                        "Choose an assets folder first.".into(),
                        LogLevel::Warn,
                    );
                } else {
                    self.enqueue(
                        "Prepare game assets",
                        JobKind::CopyAssets {
                            source: settings.assets_dir.clone(),
                            client: settings.client_dir(),
                            reference: settings.asset_mode == AssetMode::Reference,
                            copy_bgm_ai: settings.copy_bgm_ai,
                            copy_loose_data: settings.copy_loose_data,
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

        // Translations ------------------------------------------------------------
        if settings.install_translation {
            let layers = [
                (
                    "ROenglishRE (base)",
                    settings.translation_re_dir(),
                    settings.translation_re_source(),
                    settings.translation_re_cloned(),
                    settings.translation_re_applied(),
                    settings.translation_re_marker(),
                    "Apply English translation (ROenglishRE)",
                ),
                (
                    "ROenglishPRE (overlay)",
                    settings.translation_pre_dir(),
                    settings.translation_pre_source(),
                    settings.translation_pre_cloned(),
                    settings.translation_pre_applied(),
                    settings.translation_pre_marker(),
                    "Apply English translation (ROenglishPRE)",
                ),
            ];
            for (name, dir, source, cloned, applied, marker, label) in layers {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Translation · {name}")).strong());
                    state_chip_with(ui, applied, cloned, "applied", "cloned, not applied");
                });
                ui.label(
                    RichText::new(dir.display().to_string())
                        .small()
                        .color(MUTED),
                );
                if cloned {
                    ui.horizontal(|ui| {
                        if ui.button("Apply into data").clicked() {
                            self.enqueue(
                                label,
                                JobKind::CopyTranslation {
                                    source: source.clone(),
                                    extracted: settings.client_extracted_dir(),
                                    marker: marker.clone(),
                                },
                            );
                        }
                        if ui.button("Update (git pull)").clicked() {
                            self.enqueue(
                                format!("Update {name}"),
                                JobKind::GitPull { dir: dir.clone() },
                            );
                        }
                    });
                }
            }
        }
    }

    fn dashboard_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("Processes").strong());
        let settings = self.settings.clone();

        ui.horizontal(|ui| {
            ui.group(|ui| {
                ui.set_min_width(300.0);
                ui.label(RichText::new("Game server").strong());
                process_card_status(ui, self.server_pid, self.server_started);
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
                    if ui.add_enabled(running, egui::Button::new("Stop")).clicked() {
                        self.stop_server();
                    }
                });
            });

            ui.group(|ui| {
                ui.set_min_width(300.0);
                ui.label(RichText::new("Game client").strong());
                process_card_status(ui, self.client_pid, self.client_started);
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
        });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let ready = self.setup_complete();
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(RichText::new("Launch Game").strong()),
                )
                .clicked()
            {
                self.launch_game();
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
            ui.label("In-game account: admin / qwertz (id 2000000)");
            ui.label(format!(
                "Client config: {}",
                settings.client_config_json().display()
            ));
            ui.label(format!("Server config: {}", settings.server_config_json().display()));
            ui.label("The client reads GRFs from nostalro-client/data; loose files in data/extracted override them.");
        });

        if !assets::is_ready(&settings) {
            ui.add_space(8.0);
            ui.colored_label(
                WARN,
                "Assets are not prepared yet. Open Setup, choose your kRO folder and press Prepare assets.",
            );
        }
    }

    fn logs_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
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
        self.maybe_save_settings();
        self.drain_queue();
        self.top_bar(ctx);
        self.side_bar(ctx);
        self.central(ctx);
        ctx.request_repaint_after(Duration::from_millis(300));
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop_all();
        self.runner.cancel();
        self.log_files.flush();
    }
}

// -- small UI helpers ------------------------------------------------------------

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

fn process_line(ui: &mut egui::Ui, name: &str, pid: Option<u32>, started: Option<Instant>) {
    match pid {
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

fn process_card_status(ui: &mut egui::Ui, pid: Option<u32>, started: Option<Instant>) {
    match pid {
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

//! Sequential background job runner: git clones, cargo builds, asset copy.

use crate::assets;
use crate::config_gen;
use crate::events::AppEvent;
use crate::settings::Settings;

use eframe::egui;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

#[derive(Clone, Debug)]
pub enum JobKind {
    GitClone {
        url: String,
        dest: PathBuf,
    },
    /// Clone `url` into `dest`, or repoint an existing clone that came from a
    /// different repository. Used for the translation, whose source changed.
    GitSync {
        url: String,
        dest: PathBuf,
    },
    GitPull {
        dir: PathBuf,
    },
    CargoBuild {
        dir: PathBuf,
        release: bool,
        /// Optional `-p <package>` for workspace repositories.
        package: Option<String>,
    },
    /// Run rust-ro's `client-data-setup` over a kRO client folder, writing the tree the server
    /// serves under `/data`.
    BuildClientData {
        source: PathBuf,
        server_dir: PathBuf,
    },
    CopyTranslation {
        sources: Vec<PathBuf>,
        extracted: PathBuf,
        marker: PathBuf,
    },
    GenerateConfigs {
        settings: Settings,
    },
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    pub label: String,
    pub kind: JobKind,
}

pub struct JobRunner {
    pub current: Option<Job>,
    pub progress: f32,
    pub progress_note: String,
    cancel: Arc<AtomicBool>,
    pid: Arc<AtomicU32>,
    tx: Sender<AppEvent>,
    ctx: egui::Context,
    next_id: u64,
}

impl JobRunner {
    pub fn new(tx: Sender<AppEvent>, ctx: egui::Context) -> Self {
        Self {
            current: None,
            progress: 0.0,
            progress_note: String::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            pid: Arc::new(AtomicU32::new(0)),
            tx,
            ctx,
            next_id: 1,
        }
    }

    pub fn is_running(&self) -> bool {
        self.current.is_some()
    }

    /// Spawn a job on a worker thread. Only one job may run at a time; the UI
    /// queues the rest.
    pub fn start(&mut self, label: String, kind: JobKind) {
        let id = self.next_id;
        self.next_id += 1;
        let job = Job { id, label, kind };
        self.current = Some(job.clone());
        self.progress = 0.0;
        self.progress_note.clear();
        self.cancel.store(false, Ordering::SeqCst);
        self.pid.store(0, Ordering::SeqCst);

        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let cancel = Arc::clone(&self.cancel);
        let pid = Arc::clone(&self.pid);
        std::thread::spawn(move || {
            let result = run_job(&job, &tx, &ctx, &cancel, &pid);
            pid.store(0, Ordering::SeqCst);
            let _ = tx.send(AppEvent::JobDone {
                id: job.id,
                label: job.label.clone(),
                result,
            });
            ctx.request_repaint();
        });
    }

    /// Ask the running job to stop and kill its process tree.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid != 0 {
            kill_pid(pid);
        }
    }
}

fn run_job(
    job: &Job,
    tx: &Sender<AppEvent>,
    ctx: &egui::Context,
    cancel: &AtomicBool,
    pid_slot: &AtomicU32,
) -> Result<String, String> {
    match &job.kind {
        JobKind::GitClone { url, dest } => {
            if dest.join(".git").is_dir() {
                return Ok("repository already present".into());
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create {}: {e}", parent.display()))?;
            }
            run_program(
                job.id,
                "git",
                &[
                    "clone".into(),
                    "--depth".into(),
                    "1".into(),
                    url.clone(),
                    dest.display().to_string(),
                ],
                Path::new("."),
                &[],
                tx,
                ctx,
                cancel,
                pid_slot,
            )
        }

        JobKind::GitSync { url, dest } => {
            if !dest.join(".git").is_dir() {
                if dest.exists() {
                    return Err(format!(
                        "{} already exists but is not a git clone; move it away so the repository can be cloned",
                        dest.display()
                    ));
                }
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("create {}: {e}", parent.display()))?;
                }
                return run_program(
                    job.id,
                    "git",
                    &[
                        "clone".into(),
                        "--depth".into(),
                        "1".into(),
                        url.clone(),
                        dest.display().to_string(),
                    ],
                    Path::new("."),
                    &[],
                    tx,
                    ctx,
                    cancel,
                    pid_slot,
                );
            }
            // An existing clone of a different repository (e.g. the old two
            // zackdreaver translations): point it at the requested one and
            // check the fetched revision out, local changes included.
            let _ = tx.send(AppEvent::JobLog {
                id: job.id,
                line: format!(
                    "existing clone at {}; repointing it at {url}",
                    dest.display()
                ),
            });
            ctx.request_repaint();
            if run_program(
                job.id,
                "git",
                &[
                    "remote".into(),
                    "set-url".into(),
                    "origin".into(),
                    url.clone(),
                ],
                dest,
                &[],
                tx,
                ctx,
                cancel,
                pid_slot,
            )
            .is_err()
            {
                run_program(
                    job.id,
                    "git",
                    &["remote".into(), "add".into(), "origin".into(), url.clone()],
                    dest,
                    &[],
                    tx,
                    ctx,
                    cancel,
                    pid_slot,
                )?;
            }
            // Fetch into the remote-tracking branch so `git pull` keeps working
            // after the repository was repointed.
            let fetched = run_program(
                job.id,
                "git",
                &[
                    "fetch".into(),
                    "--depth".into(),
                    "1".into(),
                    "origin".into(),
                    "+master:refs/remotes/origin/master".into(),
                ],
                dest,
                &[],
                tx,
                ctx,
                cancel,
                pid_slot,
            );
            if fetched.is_err() {
                run_program(
                    job.id,
                    "git",
                    &[
                        "fetch".into(),
                        "--depth".into(),
                        "1".into(),
                        "origin".into(),
                        "+main:refs/remotes/origin/main".into(),
                    ],
                    dest,
                    &[],
                    tx,
                    ctx,
                    cancel,
                    pid_slot,
                )?;
            }
            run_program(
                job.id,
                "git",
                &[
                    "checkout".into(),
                    "-f".into(),
                    "-B".into(),
                    "master".into(),
                    "FETCH_HEAD".into(),
                ],
                dest,
                &[],
                tx,
                ctx,
                cancel,
                pid_slot,
            )?;
            // Best effort: make sure the branch tracks the new remote.
            let _ = run_program(
                job.id,
                "git",
                &[
                    "branch".into(),
                    "--set-upstream-to=origin/master".into(),
                    "master".into(),
                ],
                dest,
                &[],
                tx,
                ctx,
                cancel,
                pid_slot,
            );
            Ok(format!("repointed {} at {url}", dest.display()))
        }

        JobKind::GitPull { dir } => run_program(
            job.id,
            "git",
            &["pull".into(), "--ff-only".into()],
            dir,
            &[],
            tx,
            ctx,
            cancel,
            pid_slot,
        ),

        JobKind::CargoBuild {
            dir,
            release,
            package,
        } => {
            let mut cargo_args = pinned_toolchain(job.id, dir, tx, ctx, cancel, pid_slot);
            cargo_args.push("build".into());
            if *release {
                cargo_args.push("--release".into());
            }
            if let Some(package) = package {
                cargo_args.push("-p".into());
                cargo_args.push(package.clone());
            }
            run_program(
                job.id,
                "cargo",
                &cargo_args,
                dir,
                &[],
                tx,
                ctx,
                cancel,
                pid_slot,
            )
        }

        JobKind::BuildClientData { source, server_dir } => {
            if !server_dir.join("Cargo.toml").is_file() {
                return Err(format!(
                    "rust-ro is not cloned at {}; install it first",
                    server_dir.display()
                ));
            }
            let archives = assets::collect_grfs(source)?;
            if archives.is_empty() {
                return Err(format!("no .grf archives in {}", source.display()));
            }
            // The tool runs from the server folder, so `--out` lands in the root the
            // server publishes (`client_data.root` in its config).
            let mut cargo_args = pinned_toolchain(job.id, server_dir, tx, ctx, cancel, pid_slot);
            cargo_args.extend(
                [
                    "run",
                    "--release",
                    "--package",
                    "tools",
                    "--bin",
                    "client-data-setup",
                    "--",
                    "--client",
                ]
                .map(String::from),
            );
            cargo_args.push(source.display().to_string());
            cargo_args.extend(["--out".into(), "config/client-data".into()]);
            for archive in archives {
                cargo_args.push("--archive".into());
                cargo_args.push(archive.display().to_string());
            }
            let message = run_program(
                job.id,
                "cargo",
                &cargo_args,
                server_dir,
                &[],
                tx,
                ctx,
                cancel,
                pid_slot,
            )?;
            Ok(format!("client data built: {message}"))
        }

        JobKind::CopyTranslation {
            sources,
            extracted,
            marker,
        } => {
            let message =
                assets::copy_translation(job.id, sources, extracted, marker, tx, ctx, cancel)?;
            // Best effort: drop markers left by the old two-repository layout.
            for old in [
                marker.with_file_name("translation-re.json"),
                marker.with_file_name("translation-pre.json"),
            ] {
                let _ = std::fs::remove_file(old);
            }
            let _ = tx.send(AppEvent::JobLog {
                id: job.id,
                line: message.clone(),
            });
            ctx.request_repaint();
            Ok(message)
        }

        JobKind::GenerateConfigs { settings } => {
            let message = config_gen::generate_all(settings)?;
            let _ = tx.send(AppEvent::JobLog {
                id: job.id,
                line: message.clone(),
            });
            ctx.request_repaint();
            Ok(message)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_program(
    id: u64,
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    tx: &Sender<AppEvent>,
    ctx: &egui::Context,
    cancel: &AtomicBool,
    pid_slot: &AtomicU32,
) -> Result<String, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW keeps helper consoles from flashing up.
        command.creation_flags(0x0800_0000);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("could not start `{program}`: {e}"))?;
    pid_slot.store(child.id(), Ordering::SeqCst);

    let mut readers: Vec<JoinHandle<()>> = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_reader(stdout, id, tx.clone(), ctx.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_reader(stderr, id, tx.clone(), ctx.clone()));
    }

    let status = child
        .wait()
        .map_err(|e| format!("could not wait for `{program}`: {e}"))?;
    for reader in readers {
        let _ = reader.join();
    }

    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    if status.success() {
        Ok(format!("`{program}` finished successfully"))
    } else {
        Err(format!("`{program}` exited with {status}"))
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    id: u64,
    tx: Sender<AppEvent>,
    ctx: egui::Context,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let buffered = BufReader::new(reader);
        for line in buffered.lines() {
            match line {
                Ok(line) => {
                    let _ = tx.send(AppEvent::JobLog { id, line });
                    ctx.request_repaint();
                }
                Err(_) => break,
            }
        }
    })
}

/// Read the pinned toolchain channel from `rust-toolchain.toml` (or the legacy
/// `rust-toolchain` file) in `dir`. Returns `None` when there is no pin.
pub fn read_toolchain_channel(dir: &Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(dir.join("rust-toolchain.toml")) {
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("channel") {
                let value = rest.trim_start().strip_prefix('=')?.trim();
                let channel = value.trim_matches('"').trim_matches('\'').trim();
                if !channel.is_empty() {
                    return Some(channel.to_string());
                }
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("rust-toolchain")) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('[') || line.starts_with('#') {
                continue;
            }
            return Some(line.trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

/// Whether the rustup proxy is available to install or activate pinned toolchains.
fn rustup_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("rustup")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

/// The `+<channel>` argument for the toolchain pinned by `dir`, or nothing when there is no pin or
/// rustup is missing. rust-ro and the client pin their toolchain in rust-toolchain.toml, so it is
/// installed explicitly (a no-op when present) and used instead of the default toolchain.
fn pinned_toolchain(
    id: u64,
    dir: &Path,
    tx: &Sender<AppEvent>,
    ctx: &egui::Context,
    cancel: &AtomicBool,
    pid_slot: &AtomicU32,
) -> Vec<String> {
    let Some(channel) = read_toolchain_channel(dir) else {
        return Vec::new();
    };
    if !rustup_available() {
        return Vec::new();
    }
    let _ = tx.send(AppEvent::JobLog {
        id,
        line: format!("ensuring pinned toolchain `{channel}` is installed"),
    });
    ctx.request_repaint();
    let installed = run_program(
        id,
        "rustup",
        &[
            "toolchain".into(),
            "install".into(),
            channel.clone(),
            "--profile".into(),
            "minimal".into(),
        ],
        dir,
        &[],
        tx,
        ctx,
        cancel,
        pid_slot,
    );
    if let Err(error) = installed {
        let _ = tx.send(AppEvent::JobLog {
            id,
            line: format!("rustup could not prepare `{channel}` ({error}); cargo will try again"),
        });
        ctx.request_repaint();
    }
    vec![format!("+{channel}")]
}

/// Compute the sequential setup plan. `force` rebuilds even when an artifact
/// already exists; otherwise only missing pieces are queued.
pub fn install_plan(settings: &Settings, force: bool) -> Vec<(String, JobKind)> {
    let mut plan: Vec<(String, JobKind)> = Vec::new();

    let mut have_server = settings.server_cloned();
    if !have_server {
        plan.push((
            "Clone rust-ro server".to_string(),
            JobKind::GitClone {
                url: settings.server_repo.clone(),
                dest: settings.server_dir(),
            },
        ));
        have_server = true;
    }
    let mut have_client = settings.client_cloned();
    if !have_client {
        plan.push((
            "Clone nostalro client".to_string(),
            JobKind::GitClone {
                url: settings.client_repo.clone(),
                dest: settings.client_dir(),
            },
        ));
        have_client = true;
    }
    let mut have_bot = settings.bot_cloned();
    if !have_bot {
        plan.push((
            "Clone rust-ro-bot".to_string(),
            JobKind::GitClone {
                url: settings.bot_repo.clone(),
                dest: settings.bot_dir(),
            },
        ));
        have_bot = true;
    }

    let mut have_translation = settings.translation_cloned();
    if settings.install_translation && !have_translation {
        plan.push((
            "Clone English translation (ROenglishRE)".to_string(),
            JobKind::GitSync {
                url: settings.translation_repo.clone(),
                dest: settings.translation_dir(),
            },
        ));
        have_translation = true;
    }

    if have_server && (force || !settings.server_built()) {
        plan.push((
            "Build rust-ro server".to_string(),
            JobKind::CargoBuild {
                dir: settings.server_dir(),
                release: settings.build_release,
                package: None,
            },
        ));
    }
    if have_client && (force || !settings.client_built()) {
        plan.push((
            "Build nostalro client".to_string(),
            JobKind::CargoBuild {
                dir: settings.client_dir(),
                release: settings.build_release,
                package: Some("ragnarok-client".into()),
            },
        ));
    }
    if have_bot && (force || !settings.bot_built()) {
        plan.push((
            "Build rust-ro-bot".to_string(),
            JobKind::CargoBuild {
                dir: settings.bot_dir(),
                release: settings.build_release,
                package: None,
            },
        ));
    }
    if have_server
        && !settings.assets_dir.as_os_str().is_empty()
        && (force || !assets::client_data_ready(settings))
    {
        plan.push((
            "Build client data (from the kRO assets)".to_string(),
            JobKind::BuildClientData {
                source: settings.assets_dir.clone(),
                server_dir: settings.server_dir(),
            },
        ));
    }
    // The translation is layered inside the client's data folder: renewal
    // base, then pre-renewal, then the 2012-era compatibility tables. The
    // order matters, so it is one job.
    if settings.install_translation
        && have_translation
        && (force || !settings.translation_applied())
    {
        plan.push((
            "Apply English translation (ROenglishRE, Pre-Renewal)".to_string(),
            JobKind::CopyTranslation {
                sources: settings.translation_sources(),
                extracted: settings.client_extracted_dir(),
                marker: settings.translation_marker(),
            },
        ));
    }
    plan.push((
        "Generate configuration".to_string(),
        JobKind::GenerateConfigs {
            settings: settings.clone(),
        },
    ));
    plan
}

/// Kill a process and its children. Used for both jobs and managed processes.
pub fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("taskkill");
        command
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000);
        let _ = command.status();
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("kill");
        command
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = command.status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32};
    use std::sync::mpsc;

    #[test]
    fn reads_pinned_toolchain_channel() {
        let dir = std::env::temp_dir().join(format!(
            "ro-sppack-toolchain-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"nightly-2025-12-22\"\ncomponents = [\"miri\"]\n",
        )
        .unwrap();
        assert_eq!(
            read_toolchain_channel(&dir).as_deref(),
            Some("nightly-2025-12-22")
        );

        let legacy = std::env::temp_dir().join("ro-sppack-toolchain-legacy");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("rust-toolchain"), "nightly-2026-01-01\n").unwrap();
        assert_eq!(
            read_toolchain_channel(&legacy).as_deref(),
            Some("nightly-2026-01-01")
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&legacy);
    }

    #[test]
    fn install_plan_queues_one_translation_clone_and_apply() {
        let mut settings = Settings::default();
        assert_eq!(
            settings.translation_repo,
            "https://github.com/llchrisll/ROenglishRE.git"
        );

        settings.install_translation = true;
        let plan = install_plan(&settings, false);
        let labels: Vec<String> = plan.iter().map(|(label, _)| label.clone()).collect();
        assert_eq!(
            labels
                .iter()
                .filter(|label| label.starts_with("Clone English"))
                .count(),
            1
        );
        let clone = labels
            .iter()
            .position(|label| label == "Clone English translation (ROenglishRE)")
            .expect("the translation should be cloned");
        assert!(
            plan.iter().any(
                |(label, kind)| label == "Clone English translation (ROenglishRE)"
                    && matches!(kind, JobKind::GitSync { .. })
            ),
            "legacy clones must be repointed rather than skipped"
        );
        let apply = labels
            .iter()
            .position(|label| label == "Apply English translation (ROenglishRE, Pre-Renewal)")
            .expect("the translation should be applied");
        assert!(clone < apply, "the clone must come before the apply");
        // All three layers are copied by that single job.
        let (_, kind) = plan
            .into_iter()
            .find(|(label, _)| label.starts_with("Apply English"))
            .unwrap();
        match kind {
            JobKind::CopyTranslation { sources, .. } => {
                assert_eq!(sources.len(), 3);
                assert!(sources[0].ends_with("Translation/Renewal"));
                assert!(sources[1].ends_with("Translation/Pre-Renewal"));
                assert!(sources[2].ends_with("Translation/Compatibility/2012-04-10/Pre-Renewal"));
            }
            other => panic!("unexpected job: {other:?}"),
        }

        settings.install_translation = false;
        let labels: Vec<String> = install_plan(&settings, false)
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        assert!(
            !labels
                .iter()
                .any(|label| label.starts_with("Clone English"))
        );
        assert!(
            !labels
                .iter()
                .any(|label| label.starts_with("Apply English"))
        );
    }

    #[test]
    fn install_plan_builds_client_data_only_while_it_is_missing() {
        let install = std::env::temp_dir().join(format!(
            "ro-sppack-plan-client-data-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let server = install.join("rust-ro");
        std::fs::create_dir_all(&server).unwrap();
        std::fs::write(server.join("Cargo.toml"), "[package]\n").unwrap();
        let assets = install.join("kro");
        std::fs::create_dir_all(&assets).unwrap();
        let settings = Settings {
            install_dir: install.clone(),
            assets_dir: assets.clone(),
            install_translation: false,
            ..Settings::default()
        };
        let builds = |settings: &Settings| {
            install_plan(settings, false)
                .into_iter()
                .filter_map(|(_, kind)| match kind {
                    JobKind::BuildClientData { source, server_dir } => Some((source, server_dir)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(builds(&settings), vec![(assets.clone(), server.clone())]);

        let tree = settings.server_client_data_dir();
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("manifest.json"), "{}").unwrap();
        std::fs::write(tree.join("check.jsonl"), "").unwrap();
        assert!(builds(&settings).is_empty());

        std::fs::remove_dir_all(&install).ok();
    }

    /// Requires git and cargo to be installed. Run with:
    /// `cargo test -- --ignored`
    #[test]
    #[ignore = "requires git and cargo on PATH"]
    fn external_toolchain_is_reachable() {
        let ctx = egui::Context::default();
        let (tx, _rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let pid = AtomicU32::new(0);
        for program in ["git", "cargo"] {
            let result = run_program(
                1,
                program,
                &["--version".into()],
                Path::new("."),
                &[],
                &tx,
                &ctx,
                &cancel,
                &pid,
            );
            assert!(result.is_ok(), "{program} should run: {result:?}");
        }
    }
}

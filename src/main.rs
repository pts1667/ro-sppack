#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod assets;
mod config_gen;
mod events;
mod jobs;
mod log_files;
mod plugins;
mod settings;

use events::AppEvent;
use jobs::{JobKind, JobRunner};
use settings::Settings;

use std::collections::VecDeque;
use std::sync::mpsc;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return Ok(());
    }
    if args
        .iter()
        .any(|arg| arg == "--setup" || arg == "--headless")
    {
        if let Err(error) = run_headless_setup(&args) {
            eprintln!("setup failed: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--generate-configs") {
        if let Err(error) = run_generate_configs(&args) {
            eprintln!("configuration failed: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let options = eframe::NativeOptions {
        // A launcher window: fixed size, and the app draws its own title bar.
        viewport: egui::ViewportBuilder::default()
            .with_title("ro-sppack — Ragnarok Online launcher")
            .with_inner_size([1120.0, 630.0])
            .with_decorations(false)
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "ro-sppack",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}

fn print_usage() {
    println!(
        "ro-sppack — setup dashboard and launcher for a local Ragnarok Online package\n\n\
         Usage:\n  \
         ro-sppack                       Open the setup/launch dashboard\n  \
         ro-sppack --setup [options]     Run the whole install pipeline without a window\n  \
         ro-sppack --generate-configs    Only regenerate config files (and optionally assets)\n\n\
         Options:\n  \
         --install-dir <path>   Override the install folder\n  \
         --assets-dir <path>    Override the kRO assets folder (DATA.INI + GRFs)\n  \
         --prepare-assets       With --generate-configs, also build the client data\n  \
         --force                With --setup, rebuild/recopy everything\n\n\
         Settings are loaded from and saved to ro-sppack.json in the working directory."
    );
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn apply_overrides(settings: &mut Settings, args: &[String]) {
    if let Some(value) = arg_value(args, "--install-dir") {
        settings.install_dir = value.into();
    }
    if let Some(value) = arg_value(args, "--assets-dir") {
        settings.assets_dir = value.into();
    }
}

fn run_generate_configs(args: &[String]) -> Result<(), String> {
    let mut settings = Settings::load();
    apply_overrides(&mut settings, args);

    if args.iter().any(|arg| arg == "--prepare-assets") {
        if settings.assets_dir.as_os_str().is_empty() {
            return Err(
                "--prepare-assets needs --assets-dir (or an assets folder in ro-sppack.json)"
                    .into(),
            );
        }
        run_plan(vec![(
            "Build client data (from the kRO assets)".into(),
            JobKind::BuildClientData {
                source: settings.assets_dir.clone(),
                server_dir: settings.server_dir(),
            },
        )])?;
    }

    let message = config_gen::generate_all(&settings)?;
    println!("{message}");
    Ok(())
}

fn run_headless_setup(args: &[String]) -> Result<(), String> {
    let mut settings = Settings::load();
    apply_overrides(&mut settings, args);
    let force = args.iter().any(|arg| arg == "--force");
    let plan = jobs::install_plan(&settings, force);
    if plan.is_empty() {
        println!("Everything is already installed.");
        return Ok(());
    }
    run_plan(plan)
}

fn run_plan(plan: Vec<(String, JobKind)>) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let ctx = egui::Context::default();
    let mut runner = JobRunner::new(tx.clone(), ctx);
    let mut queue: VecDeque<(String, JobKind)> = plan.into();
    let mut last_progress = -1.0f32;

    let (label, kind) = queue.pop_front().expect("plan is not empty");
    println!("== {label}");
    runner.start(label, kind);

    loop {
        match rx.recv() {
            Ok(AppEvent::JobLog { line, .. }) => println!("{line}"),
            Ok(AppEvent::JobProgress { fraction, note, .. }) => {
                if fraction - last_progress >= 0.05 {
                    last_progress = fraction;
                    println!("   {note} ({:.0}%)", fraction * 100.0);
                }
            }
            Ok(AppEvent::JobDone { label, result, .. }) => {
                match result {
                    Ok(message) => println!("-- {label}: {message}"),
                    Err(error) => return Err(format!("{label}: {error}")),
                }
                last_progress = -1.0;
                match queue.pop_front() {
                    Some((next_label, next_kind)) => {
                        println!("== {next_label}");
                        runner.start(next_label, next_kind);
                    }
                    None => return Ok(()),
                }
            }
            Ok(_) => {}
            Err(_) => return Ok(()),
        }
    }
}

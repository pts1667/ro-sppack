//! Preparing game archives for the remote client: DATA.INI parsing, copying and
//! progress reporting.

use crate::events::AppEvent;
use crate::settings::Settings;

use eframe::egui;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

struct CopyTask {
    src: PathBuf,
    dst: PathBuf,
    size: u64,
    skip: bool,
}

/// True when `resources/DATA.INI` exists and every archive it lists is present.
pub fn is_ready(settings: &Settings) -> bool {
    let resources = settings.resources_dir();
    let entries = read_data_ini(&resources);
    !entries.is_empty() && entries.iter().all(|path| path.is_file())
}

/// Resolve the DATA.INI entries in `resources/` to concrete file paths.
pub fn read_data_ini(resources: &Path) -> Vec<PathBuf> {
    let ini = resources.join("DATA.INI");
    let Ok(content) = fs::read_to_string(&ini) else {
        return Vec::new();
    };
    parse_data_ini(&content)
        .into_iter()
        .map(|entry| {
            let path = Path::new(&entry);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                resources.join(path)
            }
        })
        .collect()
}

/// The `[Data]` section of a DATA.INI in index order (lower index wins).
/// Mirrors the parser in the remote client so the generated file behaves the same.
pub fn parse_data_ini(content: &str) -> Vec<String> {
    let mut entries: Vec<(i64, String)> = Vec::new();
    let mut in_data = false;
    let mut fallback = 1_000_000i64;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_data = line[1..line.len() - 1].trim().eq_ignore_ascii_case("data");
            continue;
        }
        if !in_data {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let index = key.trim().parse::<i64>().unwrap_or_else(|_| {
            fallback += 1;
            fallback
        });
        entries.push((index, value.to_string()));
    }
    entries.sort_by_key(|(index, _)| *index);
    entries.into_iter().map(|(_, value)| value).collect()
}

#[allow(clippy::too_many_arguments)]
pub fn copy_assets(
    id: u64,
    source: &Path,
    webroot: &Path,
    reference: bool,
    copy_bgm_ai: bool,
    copy_loose_data: bool,
    tx: &Sender<AppEvent>,
    ctx: &egui::Context,
    cancel: &AtomicBool,
) -> Result<String, String> {
    if !source.is_dir() {
        return Err(format!("assets folder not found: {}", source.display()));
    }
    let resources = webroot.join("resources");
    fs::create_dir_all(&resources).map_err(|e| format!("create {}: {e}", resources.display()))?;

    let grfs = collect_grfs(source)?;
    if grfs.is_empty() {
        return Err(format!(
            "no GRF archives found: {} has no readable DATA.INI entries and no *.grf/*.gpf files",
            source.display()
        ));
    }

    let mut tasks: Vec<CopyTask> = Vec::new();
    let mut data_ini = String::from("[Data]\r\n");

    if reference {
        for (index, grf) in grfs.iter().enumerate() {
            data_ini.push_str(&format!("{index}={}\r\n", canonical_path(grf).display()));
        }
    } else {
        let mut used_names: Vec<String> = Vec::new();
        for (index, grf) in grfs.iter().enumerate() {
            let original = grf
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("archive_{index}.grf"));
            let name = unique_name(&original, &mut used_names);
            let dst = resources.join(&name);
            let size = fs::metadata(grf).map(|meta| meta.len()).unwrap_or(0);
            let skip = dst.is_file()
                && fs::metadata(&dst)
                    .map(|meta| meta.len() == size)
                    .unwrap_or(false);
            tasks.push(CopyTask {
                src: grf.clone(),
                dst,
                size,
                skip,
            });
            data_ini.push_str(&format!("{index}={name}\r\n"));
        }
    }

    if copy_loose_data {
        let data_src = source.join("data");
        if data_src.is_dir() {
            collect_tree(&data_src, &webroot.join("data"), &mut tasks)?;
        }
    }
    if copy_bgm_ai {
        for folder in ["BGM", "AI"] {
            let folder_src = source.join(folder);
            if folder_src.is_dir() {
                collect_tree(&folder_src, &webroot.join(folder), &mut tasks)?;
            }
        }
    }

    let ini_path = resources.join("DATA.INI");
    fs::write(&ini_path, data_ini).map_err(|e| format!("write {}: {e}", ini_path.display()))?;

    let total: u64 = tasks.iter().map(|task| task.size).sum();
    let mut done: u64 = tasks
        .iter()
        .filter(|task| task.skip)
        .map(|task| task.size)
        .sum();
    let mut last_report = Instant::now();
    report(id, done, total, "preparing assets", tx, ctx);

    for task in &tasks {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        if task.skip {
            continue;
        }
        copy_file(
            task,
            id,
            &mut done,
            total,
            tx,
            ctx,
            cancel,
            &mut last_report,
        )?;
    }

    let mode = if reference { "referenced" } else { "copied" };
    Ok(format!(
        "{mode} {} archive(s); {} auxiliary file(s); {} total",
        grfs.len(),
        tasks
            .len()
            .saturating_sub(if reference { 0 } else { grfs.len() }),
        human_bytes(total)
    ))
}

fn collect_grfs(source: &Path) -> Result<Vec<PathBuf>, String> {
    let ini = source.join("DATA.INI");
    let mut found: Vec<PathBuf> = Vec::new();

    if ini.is_file() {
        let content =
            fs::read_to_string(&ini).map_err(|e| format!("read {}: {e}", ini.display()))?;
        for entry in parse_data_ini(&content) {
            let path = Path::new(entry.trim());
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                source.join(path)
            };
            if resolved.is_file() {
                found.push(resolved);
            }
        }
    }

    if found.is_empty() {
        let entries =
            fs::read_dir(source).map_err(|e| format!("read {}: {e}", source.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && has_archive_extension(&path) {
                found.push(path);
            }
        }
        found.sort_by_key(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        });
    }

    // Remove duplicates that resolve to the same file.
    let mut seen: Vec<String> = Vec::new();
    found.retain(|path| {
        let key = canonical_path(path).display().to_string().to_lowercase();
        if seen.iter().any(|existing| existing == &key) {
            false
        } else {
            seen.push(key);
            true
        }
    });

    Ok(found)
}

fn collect_tree(src: &Path, dst_root: &Path, tasks: &mut Vec<CopyTask>) -> Result<(), String> {
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
            let file_type = entry
                .file_type()
                .map_err(|e| format!("stat {}: {e}", entry.path().display()))?;
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                let relative = path
                    .strip_prefix(src)
                    .map_err(|_| format!("{} is not under {}", path.display(), src.display()))?
                    .to_path_buf();
                let dst = dst_root.join(&relative);
                let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                let skip = dst.is_file()
                    && fs::metadata(&dst)
                        .map(|meta| meta.len() == size)
                        .unwrap_or(false);
                tasks.push(CopyTask {
                    src: path,
                    dst,
                    size,
                    skip,
                });
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn copy_file(
    task: &CopyTask,
    id: u64,
    done: &mut u64,
    total: u64,
    tx: &Sender<AppEvent>,
    ctx: &egui::Context,
    cancel: &AtomicBool,
    last_report: &mut Instant,
) -> Result<(), String> {
    if let Some(parent) = task.dst.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let mut reader =
        File::open(&task.src).map_err(|e| format!("open {}: {e}", task.src.display()))?;
    let mut writer =
        File::create(&task.dst).map_err(|e| format!("create {}: {e}", task.dst.display()))?;
    let mut buffer = vec![0u8; 4 * 1024 * 1024];

    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|e| format!("read {}: {e}", task.src.display()))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|e| format!("write {}: {e}", task.dst.display()))?;
        *done += read as u64;

        if last_report.elapsed() > Duration::from_millis(150) {
            *last_report = Instant::now();
            let name = task
                .src
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            report(id, *done, total, &format!("copying {name}"), tx, ctx);
        }
    }

    writer
        .flush()
        .map_err(|e| format!("flush {}: {e}", task.dst.display()))?;
    Ok(())
}

fn report(id: u64, done: u64, total: u64, note: &str, tx: &Sender<AppEvent>, ctx: &egui::Context) {
    let fraction = if total == 0 {
        1.0
    } else {
        (done as f32 / total as f32).clamp(0.0, 1.0)
    };
    let _ = tx.send(AppEvent::JobProgress {
        id,
        fraction,
        note: format!("{note} — {}/{}", human_bytes(done), human_bytes(total)),
    });
    ctx.request_repaint();
}

fn unique_name(original: &str, used: &mut Vec<String>) -> String {
    let lower = original.to_lowercase();
    if !used.iter().any(|existing| existing == &lower) {
        used.push(lower);
        return original.to_string();
    }
    let path = Path::new(original);
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".into());
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_string())
        .unwrap_or_default();
    for index in 2..10_000 {
        let candidate = if extension.is_empty() {
            format!("{stem}_{index}")
        } else {
            format!("{stem}_{index}.{extension}")
        };
        let lower = candidate.to_lowercase();
        if !used.iter().any(|existing| existing == &lower) {
            used.push(lower);
            return candidate;
        }
    }
    original.to_string()
}

fn has_archive_extension(path: &Path) -> bool {
    path.extension()
        .map(|extension| {
            let extension = extension.to_string_lossy().to_lowercase();
            extension == "grf" || extension == "gpf"
        })
        .unwrap_or(false)
}

fn canonical_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::time::SystemTime;

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("ro-sppack-{label}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn data_ini_is_sorted_by_numeric_index() {
        let parsed = parse_data_ini("[Data]\n2=data.grf\n0=custom.grf\n1=rdata.grf\n");
        assert_eq!(parsed, vec!["custom.grf", "rdata.grf", "data.grf"]);
    }

    #[test]
    fn data_ini_ignores_other_sections() {
        let parsed = parse_data_ini("[Data]\n0=yes.grf\n[Other]\n1=no.grf\n; comment\n");
        assert_eq!(parsed, vec!["yes.grf"]);
    }

    #[test]
    fn copy_assets_writes_manifest_and_archives() {
        let source = temp_dir("source");
        let install = temp_dir("install");
        fs::write(
            source.join("DATA.INI"),
            "[Data]\n1=data.grf\n0=custom.grf\n",
        )
        .unwrap();
        fs::write(source.join("data.grf"), b"fake-grf-one").unwrap();
        fs::write(source.join("custom.grf"), b"fake-grf-two").unwrap();
        fs::create_dir_all(source.join("data").join("luafiles514")).unwrap();
        fs::write(
            source.join("data").join("luafiles514").join("a.lua"),
            b"return 1",
        )
        .unwrap();

        let webroot = install.join("webroot");
        let (tx, _rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let message = copy_assets(
            1,
            &source,
            &webroot,
            false,
            false,
            true,
            &tx,
            &egui::Context::default(),
            &cancel,
        )
        .unwrap();

        assert!(message.contains("copied"));
        assert!(webroot.join("resources").join("custom.grf").is_file());
        assert!(webroot.join("resources").join("data.grf").is_file());
        assert!(
            webroot
                .join("data")
                .join("luafiles514")
                .join("a.lua")
                .is_file()
        );
        let manifest = fs::read_to_string(webroot.join("resources").join("DATA.INI")).unwrap();
        let entries = parse_data_ini(&manifest);
        assert_eq!(entries, vec!["custom.grf", "data.grf"]);
    }

    #[test]
    fn reference_mode_keeps_archives_in_place() {
        let source = temp_dir("reference-source");
        let install = temp_dir("reference-install");
        fs::write(source.join("DATA.INI"), "[Data]\n0=data.grf\n").unwrap();
        fs::write(source.join("data.grf"), b"fake").unwrap();

        let webroot = install.join("webroot");
        let (tx, _rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        copy_assets(
            1,
            &source,
            &webroot,
            true,
            false,
            false,
            &tx,
            &egui::Context::default(),
            &cancel,
        )
        .unwrap();

        assert!(!webroot.join("resources").join("data.grf").exists());
        let manifest = fs::read_to_string(webroot.join("resources").join("DATA.INI")).unwrap();
        let entries = parse_data_ini(&manifest);
        assert_eq!(entries.len(), 1);
        assert!(Path::new(&entries[0]).is_absolute());
    }
}

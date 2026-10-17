//! The English translation layers and the client data build.
//!
//! The native client reads loose files from `nostalro-client/data/extracted` first
//! (the translation lands there), then the client data served by the server under
//! `/data`. The server's `client-data-setup` tool builds that served tree from the
//! kRO archives, so the launcher no longer copies archives into the client.

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

/// True once `client-data-setup` has written its manifest into the server's client data root.
pub fn client_data_ready(settings: &Settings) -> bool {
    let root = settings.server_client_data_dir();
    root.join("manifest.json").is_file() && root.join("check.jsonl").is_file()
}

/// The `[Data]` section of a DATA.INI in index order (lower index wins).
/// Same semantics as the kRO client so an existing `DATA.INI` keeps working.
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

/// Copy translation layers' `data/` trees over the client's loose overrides.
/// Each source is a repository directory that contains `data/`; files are
/// copied without the leading `data/` component, matching the client's
/// `data_dir`. Layers are applied in order, so a later layer overwrites
/// earlier ones.
///
/// Every file is rewritten (no size-based skipping): the pre-renewal layer
/// replaces renewal files that may have the same size but different contents.
pub fn copy_translation(
    id: u64,
    sources: &[PathBuf],
    extracted: &Path,
    marker: &Path,
    tx: &Sender<AppEvent>,
    ctx: &egui::Context,
    cancel: &AtomicBool,
) -> Result<String, String> {
    if sources.is_empty() {
        return Err("no translation layers selected".into());
    }

    let mut tasks: Vec<CopyTask> = Vec::new();
    for source in sources {
        let data = source.join("data");
        if !data.is_dir() {
            return Err(format!("translation layer not found: {}", data.display()));
        }
        collect_tree(&data, extracted, &mut tasks)?;
    }
    if tasks.is_empty() {
        return Err(format!(
            "no files under {} in translation repository",
            sources[0].parent().unwrap_or(Path::new(".")).display()
        ));
    }
    for task in &mut tasks {
        task.skip = false;
    }

    let total: u64 = tasks.iter().map(|task| task.size).sum();
    let mut done: u64 = 0;
    let mut last_report = Instant::now();
    report(id, done, total, "applying translation", tx, ctx);

    for task in &tasks {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
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

    let details = serde_json::json!({
        "sources": sources
            .iter()
            .map(|source| source.display().to_string())
            .collect::<Vec<_>>(),
        "extracted": extracted.display().to_string(),
        "files": tasks.len(),
        "updated": tasks.len(),
        "total_bytes": total,
        "applied_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
    });
    if let Some(parent) = marker.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&details).unwrap_or_else(|_| "{}".into());
    fs::write(marker, text).map_err(|e| format!("write {}: {e}", marker.display()))?;

    Ok(format!(
        "translation: {} file(s), {} total",
        tasks.len(),
        human_bytes(total)
    ))
}

/// The archives of a kRO client folder in priority order: DATA.INI order, or every `.grf` when
/// DATA.INI lists none.
pub fn collect_grfs(source: &Path) -> Result<Vec<PathBuf>, String> {
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

fn has_archive_extension(path: &Path) -> bool {
    path.extension()
        .map(|extension| {
            let extension = extension.to_string_lossy().to_lowercase();
            extension == "grf" || extension == "gpf"
        })
        .unwrap_or(false)
}

fn canonical_path(path: &Path) -> PathBuf {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // Verbatim paths do not accept `/` as a separator and do not resolve `..`,
    // so the generated client config would contain paths it cannot open.
    #[cfg(windows)]
    {
        let text = resolved.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\")
            && rest.as_bytes().get(1) == Some(&b':')
            && !rest.starts_with("UNC\\")
        {
            return PathBuf::from(rest.to_string());
        }
    }
    resolved
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
    fn archives_follow_data_ini_order() {
        let root = temp_dir("assets-archives");
        fs::write(root.join("DATA.INI"), "[Data]\n0=data.grf\n1=rdata.grf\n").unwrap();
        fs::write(root.join("data.grf"), b"data").unwrap();
        fs::write(root.join("rdata.grf"), b"rdata").unwrap();
        fs::write(root.join("extra.grf"), b"not listed").unwrap();

        let names: Vec<String> = collect_grfs(&root)
            .unwrap()
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["data.grf", "rdata.grf"]);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn client_data_is_ready_once_the_setup_wrote_its_manifest() {
        let root = temp_dir("assets-ready");
        let settings = Settings {
            install_dir: root.clone(),
            ..Settings::default()
        };
        assert!(!client_data_ready(&settings));

        let tree = settings.server_client_data_dir();
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join("manifest.json"), "{}").unwrap();
        assert!(
            !client_data_ready(&settings),
            "check.jsonl is still missing"
        );
        fs::write(tree.join("check.jsonl"), "").unwrap();
        assert!(client_data_ready(&settings));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn translation_layers_override_loose_files_and_write_marker() {
        let root = temp_dir("translation");
        let repo = root.join("ROenglishRE");
        let renewal = repo.join("Translation").join("Renewal");
        let pre = repo.join("Translation").join("Pre-Renewal");
        let compat = repo
            .join("Translation")
            .join("Compatibility")
            .join("2012-04-10")
            .join("Pre-Renewal");
        let extracted = root.join("nostalro-client").join("data").join("extracted");
        let marker = root.join("translation.json");

        // Renewal base: message table and a lua table.
        fs::create_dir_all(renewal.join("data").join("lua files")).unwrap();
        fs::write(renewal.join("data").join("msgstringtable.txt"), "hello").unwrap();
        fs::write(
            renewal
                .join("data")
                .join("lua files")
                .join("skillinfolist.lub"),
            "renewal!",
        )
        .unwrap();

        // Pre-renewal overlay: a same-size replacement plus its own file.
        fs::create_dir_all(pre.join("data").join("lua files")).unwrap();
        fs::write(
            pre.join("data").join("lua files").join("skillinfolist.lub"),
            "prelayer",
        )
        .unwrap();
        fs::write(pre.join("data").join("skillnametable.txt"), "skills").unwrap();

        // Compatibility: the classic item tables a 2012-era client reads.
        fs::create_dir_all(compat.join("data")).unwrap();
        fs::write(
            compat.join("data").join("num2itemdesctable.txt"),
            "translated",
        )
        .unwrap();

        // Existing loose (Korean) files must be replaced by the translation.
        fs::create_dir_all(&extracted).unwrap();
        fs::write(extracted.join("num2itemdesctable.txt"), "korean").unwrap();
        fs::write(extracted.join("msgstringtable.txt"), "korean msg").unwrap();

        let sources = vec![renewal, pre, compat];
        let ctx = egui::Context::default();
        let (tx, _rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let summary =
            copy_translation(1, &sources, &extracted, &marker, &tx, &ctx, &cancel).unwrap();
        assert!(summary.contains("5 file(s)"), "{summary}");
        assert_eq!(
            fs::read_to_string(extracted.join("msgstringtable.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            fs::read_to_string(extracted.join("num2itemdesctable.txt")).unwrap(),
            "translated"
        );
        assert_eq!(
            fs::read_to_string(extracted.join("skillnametable.txt")).unwrap(),
            "skills"
        );
        // The pre-renewal layer wins over the renewal base even at equal size.
        assert_eq!(
            fs::read_to_string(extracted.join("lua files").join("skillinfolist.lub")).unwrap(),
            "prelayer"
        );

        let recorded: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&marker).unwrap()).unwrap();
        assert_eq!(recorded["sources"].as_array().unwrap().len(), 3);
        assert_eq!(recorded["extracted"], extracted.display().to_string());

        // Re-running keeps the same layering result.
        copy_translation(2, &sources, &extracted, &marker, &tx, &ctx, &cancel).unwrap();
        assert_eq!(
            fs::read_to_string(extracted.join("lua files").join("skillinfolist.lub")).unwrap(),
            "prelayer"
        );
        fs::remove_dir_all(&root).ok();
    }
}

//! Preparing the native client's data: GRF archives, loose overrides and the
//! English translation layers.
//!
//! The client resolves files in this order: `data_dir` (loose files that
//! override archive entries) first, then every archive in `grf_paths` in
//! order. Everything here works towards that layout.

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

/// Archive paths as written into the client `config.json`, read from the
/// manifest produced by [`copy_assets`].
pub fn grf_paths(settings: &Settings) -> Vec<String> {
    let path = settings.client_assets_manifest();
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    value
        .get("grf_paths")
        .and_then(|value| value.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve a manifest entry to a file path. Relative entries are relative to
/// the client directory, which is also its working directory.
pub fn resolve_grf(settings: &Settings, entry: &str) -> PathBuf {
    let path = Path::new(entry);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        settings.client_dir().join(path)
    }
}

/// True when the manifest exists and every archive it lists is present.
pub fn is_ready(settings: &Settings) -> bool {
    let paths = grf_paths(settings);
    !paths.is_empty()
        && paths
            .iter()
            .all(|entry| resolve_grf(settings, entry).is_file())
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

/// Prepare the client's `data/` folder: copy the archives (or reference them in
/// place), copy loose overrides into `data/extracted`, optionally copy BGM, and
/// write the manifest the configuration step reads.
#[allow(clippy::too_many_arguments)]
pub fn copy_assets(
    id: u64,
    source: &Path,
    client_dir: &Path,
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
    let data_dir = client_dir.join("data");
    fs::create_dir_all(&data_dir).map_err(|e| format!("create {}: {e}", data_dir.display()))?;

    let grfs = collect_grfs(source)?;
    if grfs.is_empty() {
        return Err(format!(
            "no GRF archives found: {} has no readable DATA.INI entries and no *.grf/*.gpf files",
            source.display()
        ));
    }

    let mut tasks: Vec<CopyTask> = Vec::new();
    let mut grf_paths: Vec<String> = Vec::new();

    if reference {
        for grf in &grfs {
            grf_paths.push(canonical_path(grf).display().to_string());
        }
    } else {
        let mut used_names: Vec<String> = Vec::new();
        for grf in &grfs {
            let original = grf
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "archive.grf".to_string());
            let name = unique_name(&original, &mut used_names);
            let dst = data_dir.join(&name);
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
            grf_paths.push(format!("data/{name}"));
        }
    }

    if copy_loose_data {
        // `data_dir` mirrors the archive's `data/` folder, so loose files are
        // copied without the leading `data/` component.
        let loose = source.join("data");
        if loose.is_dir() {
            collect_tree(&loose, &data_dir.join("extracted"), &mut tasks)?;
        }
    }
    if copy_bgm_ai {
        let bgm = source.join("BGM");
        if bgm.is_dir() {
            collect_tree(&bgm, &data_dir.join("BGM"), &mut tasks)?;
        }
    }

    let manifest = serde_json::json!({
        "mode": if reference { "reference" } else { "copy" },
        "source": source.display().to_string(),
        "grf_paths": grf_paths,
    });
    let manifest_path = data_dir.join("ro-sppack-assets.json");
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("serialize assets manifest: {e}"))?;
    fs::write(&manifest_path, text)
        .map_err(|e| format!("write {}: {e}", manifest_path.display()))?;

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

/// Copy a translation layer's `data/` tree over the client's loose overrides.
/// `source` is the repository root that contains `data/`; files are copied
/// without the leading `data/` component, matching the client's `data_dir`.
///
/// Every file is rewritten (no size-based skipping): the PRE layer replaces RE
/// files that may have the same size but different contents.
pub fn copy_translation(
    id: u64,
    source: &Path,
    extracted: &Path,
    marker: &Path,
    tx: &Sender<AppEvent>,
    ctx: &egui::Context,
    cancel: &AtomicBool,
) -> Result<String, String> {
    if !source.is_dir() {
        return Err(format!(
            "translation repository not found: {}",
            source.display()
        ));
    }
    let data = source.join("data");
    if !data.is_dir() {
        return Err(format!(
            "no data/ folder in translation repository: {}",
            source.display()
        ));
    }

    let mut tasks: Vec<CopyTask> = Vec::new();
    collect_tree(&data, extracted, &mut tasks)?;
    if tasks.is_empty() {
        return Err(format!(
            "no files under {} in translation repository",
            data.display()
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
        "source": source.display().to_string(),
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

    fn settings_for(install: &Path) -> Settings {
        Settings {
            install_dir: install.to_path_buf(),
            ..Settings::default()
        }
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
    fn copy_mode_writes_archives_loose_data_and_manifest() {
        let root = temp_dir("assets-copy");
        let source = root.join("kro");
        let client = root.join("nostalro-client");
        fs::create_dir_all(source.join("data").join("texture")).unwrap();
        fs::write(source.join("DATA.INI"), "[Data]\n0=data.grf\n1=rdata.grf\n").unwrap();
        fs::write(source.join("data.grf"), b"data").unwrap();
        fs::write(source.join("rdata.grf"), b"rdata").unwrap();
        fs::write(source.join("data").join("texture").join("x.bmp"), b"loose").unwrap();

        let ctx = egui::Context::default();
        let (tx, _rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let summary =
            copy_assets(1, &source, &client, false, false, true, &tx, &ctx, &cancel).unwrap();
        assert!(summary.contains("2 archive(s)"), "{summary}");
        assert!(client.join("data").join("data.grf").is_file());
        assert!(client.join("data").join("rdata.grf").is_file());
        // Loose files lose the leading `data/` component.
        assert_eq!(
            fs::read_to_string(
                client
                    .join("data")
                    .join("extracted")
                    .join("texture")
                    .join("x.bmp")
            )
            .unwrap(),
            "loose"
        );

        let settings = settings_for(&root);
        assert_eq!(
            grf_paths(&settings),
            vec!["data/data.grf", "data/rdata.grf"]
        );
        assert!(is_ready(&settings));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reference_mode_keeps_archives_in_place() {
        let root = temp_dir("assets-reference");
        let source = root.join("kro");
        let client = root.join("nostalro-client");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("DATA.INI"), "[Data]\n0=data.grf\n").unwrap();
        fs::write(source.join("data.grf"), b"data").unwrap();

        let ctx = egui::Context::default();
        let (tx, _rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        copy_assets(1, &source, &client, true, false, true, &tx, &ctx, &cancel).unwrap();

        let settings = settings_for(&root);
        let paths = grf_paths(&settings);
        assert_eq!(paths.len(), 1);
        assert!(Path::new(&paths[0]).is_absolute(), "{paths:?}");
        assert!(is_ready(&settings));
        assert!(!client.join("data").join("data.grf").exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn translation_copy_overrides_loose_files_and_writes_marker() {
        let root = temp_dir("translation");
        let source = root.join("ROenglishPRE");
        let extracted = root.join("nostalro-client").join("data").join("extracted");
        let marker = root.join("translation-pre.json");
        fs::create_dir_all(source.join("data").join("lua files")).unwrap();
        fs::create_dir_all(source.join("System")).unwrap();
        fs::write(
            source.join("data").join("num2itemdesctable.txt"),
            "translated",
        )
        .unwrap();
        fs::write(
            source
                .join("data")
                .join("lua files")
                .join("skillinfolist.lub"),
            "lub data",
        )
        .unwrap();
        fs::write(source.join("System").join("itemInfo.lua"), "item info").unwrap();

        // An existing loose (Korean) file must be replaced by the translation.
        fs::create_dir_all(&extracted).unwrap();
        fs::write(extracted.join("num2itemdesctable.txt"), "korean").unwrap();

        let ctx = egui::Context::default();
        let (tx, _rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let summary =
            copy_translation(1, &source, &extracted, &marker, &tx, &ctx, &cancel).unwrap();
        assert!(summary.contains("2 file(s)"), "{summary}");
        assert_eq!(
            fs::read_to_string(extracted.join("num2itemdesctable.txt")).unwrap(),
            "translated"
        );
        assert!(
            extracted
                .join("lua files")
                .join("skillinfolist.lub")
                .is_file()
        );
        // System/ is for the 2013+ official client; the native client ignores it.
        assert!(!extracted.join("System").exists());
        assert!(marker.is_file());

        // Same-size changes still win (this is how the PRE layer overrides RE).
        fs::write(
            source
                .join("data")
                .join("lua files")
                .join("skillinfolist.lub"),
            "LUB DATA",
        )
        .unwrap();
        copy_translation(2, &source, &extracted, &marker, &tx, &ctx, &cancel).unwrap();
        assert_eq!(
            fs::read_to_string(extracted.join("lua files").join("skillinfolist.lub")).unwrap(),
            "LUB DATA"
        );
        fs::remove_dir_all(&root).ok();
    }
}

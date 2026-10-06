//! Persistent copies of everything shown in the Logs tab, under `<install dir>/logs`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FLUSH_INTERVAL: Duration = Duration::from_secs(10);
const PROCESS_SOURCES: [&str; 2] = ["server", "web"];

struct State {
    dir: PathBuf,
    writers: HashMap<&'static str, BufWriter<File>>,
}

/// Appends log lines to `server.log`, `web.log` and `launcher.log`; a background thread flushes them every
/// ten seconds so the files can be read while the game runs. The previous run's file is kept as `<name>.previous.log`.
pub struct LogFiles {
    state: Mutex<State>,
}

impl LogFiles {
    pub fn new() -> Arc<Self> {
        let files = Arc::new(Self {
            state: Mutex::new(State {
                dir: PathBuf::new(),
                writers: HashMap::new(),
            }),
        });
        let weak = Arc::downgrade(&files);
        std::thread::spawn(move || {
            while let Some(files) = weak.upgrade() {
                files.flush();
                drop(files);
                std::thread::sleep(FLUSH_INTERVAL);
            }
        });
        files
    }

    pub fn write(&self, install_dir: &Path, source: &str, text: &str) -> Result<(), String> {
        let name = file_name(source);
        let line = format!("{} [{source}] {text}\n", timestamp());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let logs_dir = install_dir.join("logs");
        if state.dir != logs_dir {
            flush_all(&mut state);
            state.writers.clear();
            state.dir = logs_dir.clone();
        }
        if !state.writers.contains_key(name) {
            let writer = open(&logs_dir, name)
                .map_err(|error| format!("{}: {error}", logs_dir.display()))?;
            state.writers.insert(name, writer);
        }
        let writer = state.writers.get_mut(name).expect("writer inserted above");
        writer.write_all(line.as_bytes()).map_err(|error| {
            format!(
                "{}: {error}",
                logs_dir.join(format!("{name}.log")).display()
            )
        })
    }

    pub fn flush(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        flush_all(&mut state);
    }
}

fn file_name(source: &str) -> &'static str {
    PROCESS_SOURCES
        .into_iter()
        .find(|process| *process == source)
        .unwrap_or("launcher")
}

fn flush_all(state: &mut State) {
    for writer in state.writers.values_mut() {
        let _ = writer.flush();
    }
}

fn open(dir: &Path, name: &str) -> std::io::Result<BufWriter<File>> {
    fs::create_dir_all(dir)?;
    let current = dir.join(format!("{name}.log"));
    if current.exists() {
        let _ = fs::rename(&current, dir.join(format!("{name}.previous.log")));
    }
    Ok(BufWriter::new(File::create(current)?))
}

fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format_utc(seconds)
}

fn format_utc(epoch_seconds: u64) -> String {
    let days = (epoch_seconds / 86_400) as i64;
    let remainder = epoch_seconds % 86_400;
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        remainder / 3_600,
        remainder / 60 % 60,
        remainder % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ro-sppack-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn formats_utc_dates() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_utc(951_782_400 + 86_399), "2000-02-29 23:59:59Z");
        assert_eq!(format_utc(1_791_288_811), "2026-10-06 12:13:31Z");
    }

    #[test]
    fn routes_processes_to_their_own_file_and_everything_else_to_the_launcher_file() {
        assert_eq!(file_name("server"), "server");
        assert_eq!(file_name("web"), "web");
        assert_eq!(file_name("job"), "launcher");
        assert_eq!(file_name("setup"), "launcher");
    }

    #[test]
    fn lines_reach_the_file_on_flush_and_the_previous_run_is_kept() {
        let dir = temp_dir("flush");
        let first = LogFiles::new();
        first.write(&dir, "server", "first run").unwrap();
        first.flush();
        let log = dir.join("logs").join("server.log");
        let content = fs::read_to_string(&log).unwrap();
        assert!(content.ends_with("[server] first run\n"), "{content}");

        let second = LogFiles::new();
        second.write(&dir, "server", "second run").unwrap();
        second.flush();
        assert!(fs::read_to_string(&log).unwrap().contains("second run"));
        assert!(
            fs::read_to_string(dir.join("logs").join("server.previous.log"))
                .unwrap()
                .contains("first run")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn changing_the_install_dir_switches_the_target_folder() {
        let (first_dir, second_dir) = (temp_dir("dir-a"), temp_dir("dir-b"));
        let files = LogFiles::new();
        files.write(&first_dir, "web", "a").unwrap();
        files.write(&second_dir, "web", "b").unwrap();
        files.flush();
        assert!(
            fs::read_to_string(first_dir.join("logs").join("web.log"))
                .unwrap()
                .contains("] a")
        );
        assert!(
            fs::read_to_string(second_dir.join("logs").join("web.log"))
                .unwrap()
                .contains("] b")
        );
        let _ = fs::remove_dir_all(&first_dir);
        let _ = fs::remove_dir_all(&second_dir);
    }
}

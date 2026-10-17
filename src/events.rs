//! Events exchanged between worker threads and the UI thread.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Success,
    Warn,
    Error,
}

pub enum AppEvent {
    JobLog {
        id: u64,
        line: String,
    },
    JobProgress {
        id: u64,
        fraction: f32,
        note: String,
    },
    JobDone {
        id: u64,
        label: String,
        result: Result<String, String>,
    },
    /// A long-running child process (game server / native client) printed a line.
    ProcLog {
        name: String,
        line: String,
    },
    ProcExited {
        name: String,
        code: Option<i32>,
    },
    /// The game server (instance `generation`) accepts connections, or the wait for it gave up.
    ServerReady {
        generation: u64,
        result: Result<(), String>,
    },
}

//! Wire protocol spoken over the control socket: newline-delimited JSON,
//! one `Request`/`Response` object per line.

use serde::{Deserialize, Serialize};

use crate::stats::AppStats;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// List every app the daemon currently supervises.
    List,
    /// Get runtime statistics for a single app.
    Stats { name: String },
    /// Start a stopped app.
    Start { name: String },
    /// Start every currently stopped app.
    StartAll,
    /// Stop a running app: SIGTERM, or SIGKILL if `force` is set.
    Stop { name: String, force: bool },
}

/// A single app that failed to start as part of a `StartAll` request.
#[derive(Debug, Serialize, Deserialize)]
pub struct StartFailure {
    pub name: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub state: String,
    pub pid: Option<u32>,
    pub command: String,
    pub cwd: String,
    pub uid: u32,
    pub gid: u32,
    pub restart_count: u32,
    pub log_bytes: u64,
    pub memory_current: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub oneshot: bool,
    pub cpu_weight: Option<u64>,
    pub io_weight: Option<u16>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    AppList {
        apps: Vec<AppInfo>,
    },
    AppStats {
        name: String,
        stats: AppStats,
    },
    /// The requested app was successfully started.
    Started {
        name: String,
    },
    /// Result of a `StartAll` request; `failed` is empty on full success.
    StartedAll {
        started: Vec<String>,
        failed: Vec<StartFailure>,
    },
    Stopped {
        name: String,
        app_was_running: bool,
    },
    /// Requested app doesn't exist; `apps` lists the currently supervised names.
    AppNotFound {
        name: String,
        apps: Vec<String>,
    },
    Error {
        message: String,
    },
}

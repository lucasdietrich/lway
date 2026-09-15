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
    /// Requested app doesn't exist; `apps` lists the currently supervised names.
    AppNotFound {
        name: String,
        apps: Vec<String>,
    },
    Error {
        message: String,
    },
}

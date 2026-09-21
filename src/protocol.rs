//! Wire protocol spoken over the control socket: newline-delimited JSON,
//! one `Request`/`Response` object per line.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cgroups::AppCgroupConfig;
use crate::restart::RestartPolicy;
use crate::stats::AppStats;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// List every app the daemon currently supervises.
    List,
    /// Get full details (list info + stats + internal debug info) for a single app.
    Info { name: String },
    /// Reconstruct the full effective YAML configuration of a single app.
    Config { name: String },
    /// Start a stopped app.
    Start { name: String },
    /// Start every currently stopped app.
    StartAll,
    /// Stop a running app: SIGTERM, or SIGKILL if `force` is set.
    Stop { name: String, force: bool },
    /// Stop every currently running app: SIGTERM, or SIGKILL if `force` is set.
    StopAll { force: bool },
}

/// A single app that failed to start as part of a `StartAll` request.
#[derive(Debug, Serialize, Deserialize)]
pub struct StartFailure {
    pub name: String,
    pub message: String,
}

/// A single app that failed to stop as part of a `StopAll` request.
#[derive(Debug, Serialize, Deserialize)]
pub struct StopFailure {
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
    /// Current uptime if running, otherwise the duration of the last completed run.
    pub runtime: Option<Duration>,
}

/// Internal, implementation-level details about an app, meant for debugging.
#[derive(Debug, Serialize, Deserialize)]
pub struct AppDebugInfo {
    pub cgroup_config: AppCgroupConfig,
    /// Full cgroupfs path of the app's cgroup, if it's currently running.
    pub cgroup_path: Option<String>,
    pub log_buffer: LogBufferDebugInfo,
}

/// Implementation-level details about an app's captured stdout/stderr buffer.
#[derive(Debug, Serialize, Deserialize)]
pub struct LogBufferDebugInfo {
    /// Concrete buffer implementation, e.g. `"circular-mmap"`.
    pub kind: String,
    pub capacity: Option<u64>,
    /// Total bytes ever written; monotonically increasing (wraps for ring buffers).
    pub write_pos: u64,
    /// Oldest logical offset still retained; bytes before this were overwritten/evicted.
    pub start_pos: u64,
}

/// Full effective configuration of a running app, enough to rebuild its YAML entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct AppConfigSnapshot {
    pub name: String,
    pub command: String,
    pub workdir: String,
    pub uid: u32,
    pub gid: u32,
    pub env: Vec<String>,
    pub oneshot: bool,
    pub autostart: bool,
    pub restart: RestartPolicy,
    pub cgroup: AppCgroupConfig,
    pub log_buffer_size: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
// One-shot IPC responses, not a hot path; boxing the large variant isn't worth the churn.
#[allow(clippy::large_enum_variant)]
pub enum Response {
    AppList {
        apps: Vec<AppInfo>,
    },
    Info {
        name: String,
        info: AppInfo,
        stats: AppStats,
        debug: AppDebugInfo,
    },
    Config {
        name: String,
        config: AppConfigSnapshot,
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
    /// Result of a `StopAll` request; `failed` is empty on full success.
    StoppedAll {
        stopped: Vec<String>,
        failed: Vec<StopFailure>,
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

//! Point-in-time snapshot of an app's runtime statistics, meant for display
//! over the CLI or logging, as opposed to internal bookkeeping state.

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppStats {
    /// How long the current run has been alive; `None` if not running.
    pub uptime: Option<Duration>,
    /// Sum of the durations of all completed runs, excluding the current one.
    pub total_uptime: Duration,
    /// Duration of the most recently completed run, if any.
    pub last_run_duration: Option<Duration>,
    pub restart_count: u32,
    pub last_exit_code: Option<i32>,
    pub last_exit_reason: Option<String>,
    pub cpu_usage_usec: u64,
    pub memory_current: u64,
    pub memory_peak: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
}

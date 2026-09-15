//! `stats` CLI command: fetch and display a single app's runtime statistics.

use std::path::Path;
use std::time::Duration;

use crate::ipc::{IpcError, Result};
use crate::protocol::{Request, Response};
use crate::stats::AppStats;

use super::send_request;

/// Connects to the daemon's control socket and requests stats for `name`.
/// Blocking: this is a short-lived one-shot call.
pub fn get_stats(socket_path: &Path, name: &str) -> Result<AppStats> {
    let response = send_request(
        socket_path,
        &Request::Stats {
            name: name.to_string(),
        },
    )?;
    match response {
        Response::AppStats { stats, .. } => Ok(stats),
        Response::AppNotFound { name, apps } => Err(IpcError::AppNotFound { name, apps }),
        Response::Error { message } => Err(IpcError::Daemon(message)),
        _ => Err(IpcError::Daemon("unexpected response".to_string())),
    }
}

/// Prints `stats` as one stat per line, grouped by category.
pub fn print_stats(name: &str, stats: &AppStats) {
    println!("{}", name);

    println!("Lifecycle:");
    println!("  uptime            {}", format_duration(stats.uptime));
    println!(
        "  total_uptime      {}",
        format_duration(Some(stats.total_uptime))
    );
    println!("  restart_count     {}", stats.restart_count);
    println!("  last_exit_code    {}", format_opt(stats.last_exit_code));
    println!(
        "  last_exit_reason  {}",
        stats.last_exit_reason.as_deref().unwrap_or("-")
    );

    println!("Resource usage:");
    println!("  cpu_usage_usec    {}", stats.cpu_usage_usec);
    println!("  memory_current    {}", stats.memory_current);
    println!("  memory_peak       {}", stats.memory_peak);

    println!("I/O:");
    println!("  io_read_bytes     {}", stats.io_read_bytes);
    println!("  io_write_bytes    {}", stats.io_write_bytes);
    println!("  stdout_bytes      {}", stats.stdout_bytes);
    println!("  stderr_bytes      {}", stats.stderr_bytes);
}

fn format_duration(d: Option<Duration>) -> String {
    d.map_or_else(|| "-".to_string(), |d| format!("{:.2?}", d))
}

fn format_opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map_or_else(|| "-".to_string(), |v| v.to_string())
}

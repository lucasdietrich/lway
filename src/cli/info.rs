//! `info` CLI command: fetch and display full details for a single app.

use std::path::Path;
use std::time::Duration;

use crate::ipc::{IpcError, Result};
use crate::protocol::{AppDebugInfo, AppInfo, Request, Response};
use crate::stats::AppStats;
use crate::support::uidgid::{get_groupname, get_username};

use super::{format_bytes, format_duration_compact, format_id, send_request};

pub struct AppFullInfo {
    pub info: AppInfo,
    pub stats: AppStats,
    pub debug: AppDebugInfo,
}

/// Connects to the daemon's control socket, sends an `Info` request and
/// returns the reported app details. Blocking: this is a short-lived one-shot call.
pub fn get_info(socket_path: &Path, name: &str) -> Result<AppFullInfo> {
    let response = send_request(
        socket_path,
        &Request::Info {
            name: name.to_string(),
        },
    )?;
    match response {
        Response::Info {
            info, stats, debug, ..
        } => Ok(AppFullInfo { info, stats, debug }),
        Response::AppNotFound { name, apps } => Err(IpcError::AppNotFound { name, apps }),
        Response::Error { message } => Err(IpcError::Daemon(message)),
        _ => Err(IpcError::Daemon("unexpected response".to_string())),
    }
}

/// Prints `full` as one field per line, grouped by category.
pub fn print_info(name: &str, full: &AppFullInfo) {
    let AppFullInfo { info, stats, debug } = full;

    println!("{}", name);

    println!("General:");
    println!("  state             {}", info.state);
    println!("  pid               {}", format_opt(info.pid));
    println!("  command           {}", info.command);
    println!("  cwd               {}", info.cwd);
    println!("  uid               {}", format_id(info.uid, get_username));
    println!("  gid               {}", format_id(info.gid, get_groupname));
    println!("  oneshot           {}", info.oneshot);
    println!("  cpu_weight        {}", format_opt(info.cpu_weight));
    println!("  io_weight         {}", format_opt(info.io_weight));
    println!(
        "  runtime           {}",
        info.runtime
            .map_or_else(|| "-".to_string(), format_duration_compact)
    );

    println!("Lifecycle:");
    println!("  uptime            {}", format_duration(stats.uptime));
    println!(
        "  total_uptime      {}",
        format_duration(Some(stats.total_uptime))
    );
    println!(
        "  last_run_duration {}",
        format_duration(stats.last_run_duration)
    );
    println!("  restart_count     {}", stats.restart_count);
    println!("  last_exit_code    {}", format_opt(stats.last_exit_code));
    println!(
        "  last_exit_reason  {}",
        stats.last_exit_reason.as_deref().unwrap_or("-")
    );

    println!("Resource usage:");
    println!("  cpu_usage_usec    {}", stats.cpu_usage_usec);
    println!("  memory_current    {}", format_bytes(stats.memory_current));
    println!("  memory_peak       {}", format_bytes(stats.memory_peak));

    println!("I/O:");
    println!("  io_read_bytes     {}", format_bytes(stats.io_read_bytes));
    println!("  io_write_bytes    {}", format_bytes(stats.io_write_bytes));
    println!("  stdout_bytes      {}", format_bytes(stats.stdout_bytes));
    println!("  stderr_bytes      {}", format_bytes(stats.stderr_bytes));

    println!("Debug:");
    println!(
        "  cgroup_path         {}",
        debug.cgroup_path.as_deref().unwrap_or("-")
    );
    println!(
        "  memory_hard_limit   {}",
        format_opt(debug.cgroup_config.memory_hard_limit)
    );
    println!(
        "  memory_soft_limit   {}",
        format_opt(debug.cgroup_config.memory_soft_limit)
    );
    println!(
        "  memory_swap_limit   {}",
        format_opt(debug.cgroup_config.memory_swap_limit)
    );

    println!("Log buffer:");
    println!("  kind                {}", debug.log_buffer.kind);
    println!(
        "  capacity            {}",
        debug
            .log_buffer
            .capacity
            .map_or_else(|| "-".to_string(), format_bytes)
    );
    println!(
        "  retained            {}",
        format_bytes(debug.log_buffer.write_pos - debug.log_buffer.start_pos)
    );
    println!(
        "  write_pos           {}",
        format_bytes(debug.log_buffer.write_pos)
    );
    println!(
        "  start_pos           {}",
        format_bytes(debug.log_buffer.start_pos)
    );
}

fn format_duration(d: Option<Duration>) -> String {
    d.map_or_else(|| "-".to_string(), |d| format!("{:.2?}", d))
}

fn format_opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map_or_else(|| "-".to_string(), |v| v.to_string())
}

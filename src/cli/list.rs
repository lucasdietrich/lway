//! `list` CLI command: fetch and display the daemon's supervised apps.

use std::path::Path;

use crate::ipc::{IpcError, Result};
use crate::protocol::{AppInfo, Request, Response};
use crate::support::uidgid::{get_groupname, get_username};

use super::{format_bytes, format_duration_compact, format_id, send_request};

/// Connects to the daemon's control socket, sends a `List` request and
/// returns the reported apps. Blocking: this is a short-lived one-shot call.
pub fn list_apps(socket_path: &Path) -> Result<Vec<AppInfo>> {
    match send_request(socket_path, &Request::List)? {
        Response::AppList { apps } => Ok(apps),
        Response::Error { message } => Err(IpcError::Daemon(message)),
        Response::AppNotFound { name, apps } => Err(IpcError::AppNotFound { name, apps }),
        _ => Err(IpcError::Daemon("unexpected response".to_string())),
    }
}

/// Prints `apps` as a simple column-aligned table.
pub fn print_apps_table(apps: &[AppInfo]) {
    const HEADERS: [&str; 15] = [
        "NAME", "STATE", "PID", "RUNTIME", "COMMAND", "CWD", "UID", "GID", "REST", "LOG", "MEM",
        "IO (R/W)", "ONESHOT", "CPU_W", "IO_W",
    ];

    let dash = || "-".to_string();
    let rows: Vec<[String; 15]> = apps
        .iter()
        .map(|app| {
            [
                app.name.clone(),
                app.state.clone(),
                app.pid.map(|p| p.to_string()).unwrap_or_else(dash),
                app.runtime
                    .map(format_duration_compact)
                    .unwrap_or_else(dash),
                app.command.clone(),
                app.cwd.clone(),
                format_id(app.uid, get_username),
                format_id(app.gid, get_groupname),
                app.restart_count.to_string(),
                format_bytes(app.log_bytes),
                format_bytes(app.memory_current),
                format!(
                    "{} / {}",
                    format_bytes(app.io_read_bytes),
                    format_bytes(app.io_write_bytes)
                ),
                app.oneshot.to_string(),
                app.cpu_weight.map(|w| w.to_string()).unwrap_or_else(dash),
                app.io_weight.map(|w| w.to_string()).unwrap_or_else(dash),
            ]
        })
        .collect();

    let mut widths = HEADERS.map(str::len);
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row.iter()) {
            *width = (*width).max(cell.len());
        }
    }

    let print_row = |cells: &[String; 15]| {
        let line: Vec<String> = cells
            .iter()
            .zip(widths.iter())
            .map(|(cell, width)| format!("{:<width$}", cell, width = width))
            .collect();
        println!("{}", line.join("  ").trim_end());
    };

    print_row(&HEADERS.map(String::from));
    for row in &rows {
        print_row(row);
    }
}

//! CLI client commands: short-lived, blocking requests to a running daemon's
//! control socket (see `crate::ipc` for the daemon-side, non-blocking server).

use std::io::{self, BufRead, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::ipc::{IpcError, Result};
use crate::protocol::{Request, Response};

pub mod config;
pub mod info;
pub mod list;
pub mod start;
pub mod stop;

/// Connects to the daemon's control socket, sends `request` and returns its
/// single-line response. Blocking: this is a short-lived one-shot call.
fn send_request(socket_path: &Path, request: &Request) -> Result<Response> {
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|_| IpcError::NotRunning(socket_path.to_path_buf()))?;

    let mut line = serde_json::to_string(request)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;

    let mut reader = io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim().is_empty() {
        return Err(IpcError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "daemon closed the connection without responding",
        )));
    }

    Ok(serde_json::from_str::<Response>(line.trim())?)
}

/// Formats a byte count with the largest unit (B/kB/MB/GB) that keeps it >= 1.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "kB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = UNITS[0];
    for &next in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    if unit == "B" {
        format!("{} {}", bytes, unit)
    } else {
        format!("{:.1} {}", value, unit)
    }
}

/// Formats a duration with up to two units of precision, e.g. `"2h 16m"` or
/// `"3h"` (the second unit is omitted when it would be zero).
pub fn format_duration_compact(d: std::time::Duration) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;
    const UNITS: [(u64, &str); 7] = [
        (YEAR, "y"),
        (MONTH, "mo"),
        (WEEK, "w"),
        (DAY, "d"),
        (HOUR, "h"),
        (MINUTE, "m"),
        (1, "s"),
    ];

    let secs = d.as_secs();
    let idx = UNITS
        .iter()
        .position(|&(unit_secs, _)| secs >= unit_secs)
        .unwrap_or(UNITS.len() - 1);
    let (unit_secs, unit_name) = UNITS[idx];
    let mut result = format!("{}{}", secs / unit_secs, unit_name);

    if let Some(&(next_unit_secs, next_unit_name)) = UNITS.get(idx + 1) {
        let next_value = (secs % unit_secs) / next_unit_secs;
        if next_value > 0 {
            result.push_str(&format!(" {}{}", next_value, next_unit_name));
        }
    }

    result
}

/// Formats a uid/gid as `"1000 (name)"`, falling back to the bare id.
pub fn format_id(id: u32, resolve_name: impl Fn(u32) -> Option<String>) -> String {
    match resolve_name(id) {
        Some(name) => format!("{} ({})", name, id),
        None => id.to_string(),
    }
}

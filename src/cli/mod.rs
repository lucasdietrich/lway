//! CLI client commands: short-lived, blocking requests to a running daemon's
//! control socket (see `crate::ipc` for the daemon-side, non-blocking server).

use std::io::{self, BufRead, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::ipc::{IpcError, Result};
use crate::protocol::{Request, Response};

pub mod list;
pub mod start;
pub mod stats;
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

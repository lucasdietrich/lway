//! CLI client commands: short-lived, blocking requests to a running daemon's
//! control socket (see `crate::ipc` for the daemon-side, non-blocking server).

pub mod list;
pub mod stats;

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

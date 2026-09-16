//! `stop` CLI command: ask the daemon to stop a running app.

use std::path::Path;

use crate::ipc::{IpcError, Result};
use crate::protocol::{Request, Response, StopFailure};

use super::send_request;

/// Connects to the daemon's control socket and requests that `name` be
/// stopped, sending SIGKILL instead of SIGTERM if `force` is set. Blocking:
/// this is a short-lived one-shot call.
pub fn stop_app(socket_path: &Path, name: &str, force: bool) -> Result<bool> {
    let response = send_request(
        socket_path,
        &Request::Stop {
            name: name.to_string(),
            force,
        },
    )?;
    match response {
        Response::Stopped {
            app_was_running, ..
        } => Ok(app_was_running),
        Response::AppNotFound { name, apps } => Err(IpcError::AppNotFound { name, apps }),
        Response::Error { message } => Err(IpcError::Daemon(message)),
        _ => Err(IpcError::Daemon("unexpected response".to_string())),
    }
}

/// Connects to the daemon's control socket and requests that every running
/// app be stopped, sending SIGKILL instead of SIGTERM if `force` is set.
/// Blocking: this is a short-lived one-shot call.
pub fn stop_all_apps(socket_path: &Path, force: bool) -> Result<(Vec<String>, Vec<StopFailure>)> {
    let response = send_request(socket_path, &Request::StopAll { force })?;
    match response {
        Response::StoppedAll { stopped, failed } => Ok((stopped, failed)),
        Response::Error { message } => Err(IpcError::Daemon(message)),
        _ => Err(IpcError::Daemon("unexpected response".to_string())),
    }
}

/// Prints the outcome of a `StopAll` request: one line per stopped app,
/// followed by one line per failure.
pub fn print_stop_all_result(stopped: &[String], failed: &[StopFailure]) {
    for name in stopped {
        println!("stopped {}", name);
    }
    for failure in failed {
        eprintln!("error: {}: {}", failure.name, failure.message);
    }
}

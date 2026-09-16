//! `stop` CLI command: ask the daemon to stop a running app.

use std::path::Path;

use crate::ipc::{IpcError, Result};
use crate::protocol::{Request, Response};

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

//! `start` CLI command: ask the daemon to start one or all stopped apps.

use std::path::Path;

use crate::ipc::{IpcError, Result};
use crate::protocol::{Request, Response, StartFailure};

use super::send_request;

/// Connects to the daemon's control socket and requests that `name` be
/// started. Blocking: this is a short-lived one-shot call.
pub fn start_app(socket_path: &Path, name: &str) -> Result<()> {
    let response = send_request(
        socket_path,
        &Request::Start {
            name: name.to_string(),
        },
    )?;
    match response {
        Response::Started { .. } => Ok(()),
        Response::AppNotFound { name, apps } => Err(IpcError::AppNotFound { name, apps }),
        Response::Error { message } => Err(IpcError::Daemon(message)),
        _ => Err(IpcError::Daemon("unexpected response".to_string())),
    }
}

/// Connects to the daemon's control socket and requests that every stopped
/// app be started. Blocking: this is a short-lived one-shot call.
pub fn start_all_apps(socket_path: &Path) -> Result<(Vec<String>, Vec<StartFailure>)> {
    let response = send_request(socket_path, &Request::StartAll)?;
    match response {
        Response::StartedAll { started, failed } => Ok((started, failed)),
        Response::Error { message } => Err(IpcError::Daemon(message)),
        Response::AppNotFound { name, apps } => Err(IpcError::AppNotFound { name, apps }),
        _ => Err(IpcError::Daemon("unexpected response".to_string())),
    }
}

/// Prints the outcome of a `StartAll` request: one line per started app,
/// followed by one line per failure.
pub fn print_start_all_result(started: &[String], failed: &[StartFailure]) {
    for name in started {
        println!("started {}", name);
    }
    for failure in failed {
        eprintln!("error: {}: {}", failure.name, failure.message);
    }
}

//! `config` CLI command: reconstruct a running app's full YAML configuration,
//! so it can be copied into a config file as a new app entry.

use std::path::Path;

use serde::Serialize;

use crate::cgroups::AppCgroupConfig;
use crate::ipc::{IpcError, Result};
use crate::protocol::{AppConfigSnapshot, Request, Response};
use crate::restart::RestartPolicy;

use super::send_request;

/// Connects to the daemon's control socket, sends a `Config` request and
/// returns the app's effective configuration. Blocking: this is a
/// short-lived one-shot call.
pub fn get_config(socket_path: &Path, name: &str) -> Result<AppConfigSnapshot> {
    let response = send_request(
        socket_path,
        &Request::Config {
            name: name.to_string(),
        },
    )?;
    match response {
        Response::Config { config, .. } => Ok(config),
        Response::AppNotFound { name, apps } => Err(IpcError::AppNotFound { name, apps }),
        Response::Error { message } => Err(IpcError::Daemon(message)),
        _ => Err(IpcError::Daemon("unexpected response".to_string())),
    }
}

/// A YAML-serializable view of an app's config. Field order here is what
/// controls the field order in the printed YAML; it mirrors `parser::AppConfig`
/// so the output can be pasted as-is into an apps config file.
#[derive(Serialize)]
struct AppConfigYaml {
    name: String,
    command: String,
    workdir: String,
    user: u32,
    group: u32,
    #[serde(skip_serializing_if = "serde_yaml::Mapping::is_empty")]
    env: serde_yaml::Mapping,
    oneshot: bool,
    autostart: bool,
    restart: RestartPolicy,
    #[serde(flatten)]
    cgroup: AppCgroupConfig,
}

/// Prints `config` as a standalone YAML app entry.
pub fn print_config(config: AppConfigSnapshot) {
    let mut env = serde_yaml::Mapping::new();
    for entry in &config.env {
        if let Some((key, value)) = entry.split_once('=') {
            env.insert(key.into(), value.into());
        }
    }

    let yaml = AppConfigYaml {
        name: config.name,
        command: config.command,
        workdir: config.workdir,
        user: config.uid,
        group: config.gid,
        env,
        oneshot: config.oneshot,
        autostart: config.autostart,
        restart: config.restart,
        cgroup: config.cgroup,
    };

    print!(
        "{}",
        serde_yaml::to_string(&yaml).expect("serialize app config to yaml")
    );
}

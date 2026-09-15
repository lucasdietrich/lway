//! Wire protocol spoken over the control socket: newline-delimited JSON,
//! one `Request`/`Response` object per line.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// List every app the daemon currently supervises.
    List,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub pid: Option<u32>,
    pub state: String,
    pub oneshot: bool,
    pub cpu_weight: Option<u64>,
    pub io_weight: Option<u16>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    AppList { apps: Vec<AppInfo> },
    Error { message: String },
}

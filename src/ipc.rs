//! Control-socket connection handling.
//!
//! The daemon side (`Server`) is a non-blocking, single-threaded `mio` state
//! machine driven from the supervisor's own event loop: no thread-per-connection.
//! Every request currently supported (`Request::List`) is one-shot, so a
//! connection is closed as soon as its response has been fully flushed.
//!
//! The client side (see `crate::cli`) is a short-lived process making a
//! single blocking connection, so it uses `std::os::unix::net::UnixStream`
//! directly instead of `mio` -- there is no need for an event loop there.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use mio::net::{UnixListener, UnixStream};
use mio::{Interest, Poll, Token};
use thiserror::Error;

use crate::protocol::{AppInfo, Request, Response};
use crate::runtime::App;
use crate::support::mio_token_slab::MioTokenSlab;
use crate::UNIX_LISTENER_TOKEN;

/// Cap on concurrent control-socket connections; see `Server::bind`.
pub const DEFAULT_MAX_CONNECTIONS: usize = 32;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("failed to serialize/deserialize json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("daemon returned an error: {0}")]
    Daemon(String),
    #[error("no such app '{name}' (available: {})", if apps.is_empty() { "none".to_string() } else { apps.join(", ") })]
    AppNotFound { name: String, apps: Vec<String> },
    #[error("could not connect to daemon socket {0} (is lway running?)")]
    NotRunning(PathBuf),
}

pub type Result<T> = std::result::Result<T, IpcError>;

struct Connection {
    stream: UnixStream,
    read_buf: Vec<u8>,
    write_buf: VecDeque<u8>,
    // Every request handled today is one-shot: close once this response drains.
    close_after_flush: bool,
    writable_registered: bool,
}

impl Connection {
    fn new(stream: UnixStream) -> Self {
        Connection {
            stream,
            read_buf: Vec::new(),
            write_buf: VecDeque::new(),
            close_after_flush: false,
            writable_registered: false,
        }
    }

    fn queue_response(&mut self, response: &Response) -> Result<()> {
        let mut line = serde_json::to_string(response)?;
        line.push('\n');
        self.write_buf.extend(line.into_bytes());
        Ok(())
    }
}

/// Control-socket server, polled alongside app supervision from the daemon's
/// main event loop.
pub struct Server {
    listener: UnixListener,
    connections: HashMap<Token, Connection>,
    max_connections: usize,
}

impl Server {
    /// Binds the control socket, removing any stale socket file left behind by
    /// an unclean previous shutdown, and registers it with `poll`. Once
    /// `max_connections` are active, further connections are rejected.
    pub fn bind(socket_path: &Path, poll: &Poll, max_connections: usize) -> Result<Self> {
        if socket_path.exists() {
            std::fs::remove_file(socket_path)?;
        }

        let mut listener = UnixListener::bind(socket_path)?;
        poll.registry()
            .register(&mut listener, UNIX_LISTENER_TOKEN, Interest::READABLE)?;

        Ok(Server {
            listener,
            connections: HashMap::new(),
            max_connections,
        })
    }

    /// Accepts every pending connection until the listener would block.
    /// Connections beyond `max_connections` get a best-effort error reply
    /// and are dropped immediately instead of being tracked.
    pub fn accept_all(&mut self, poll: &Poll, token_slab: &mut MioTokenSlab) -> Result<()> {
        loop {
            match self.listener.accept() {
                Ok((mut stream, _addr)) => {
                    if self.connections.len() >= self.max_connections {
                        log::warn!(
                            "rejecting control connection: {} already active (max {})",
                            self.connections.len(),
                            self.max_connections
                        );
                        reject_over_capacity(&mut stream);
                        continue;
                    }
                    if let Some(token) = token_slab.allocate() {
                        poll.registry()
                            .register(&mut stream, token, Interest::READABLE)?;
                        self.connections.insert(token, Connection::new(stream));
                    } else {
                        log::warn!("rejecting control connection: no available token");
                        reject_over_capacity(&mut stream);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    pub fn is_known(&self, token: Token) -> bool {
        self.connections.contains_key(&token)
    }

    pub fn tokens(&self) -> Vec<Token> {
        self.connections.keys().copied().collect()
    }

    /// Reads and dispatches every complete request currently buffered on `token`.
    pub fn handle_readable(&mut self, token: Token, apps: &[App]) -> Result<()> {
        let conn = match self.connections.get_mut(&token) {
            Some(c) => c,
            None => return Ok(()),
        };

        let mut buf = [0u8; 4096];
        loop {
            match conn.stream.read(&mut buf) {
                Ok(0) => {
                    conn.close_after_flush = true;
                    break;
                }
                Ok(n) => conn.read_buf.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }

        while let Some(pos) = conn.read_buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = conn.read_buf.drain(..=pos).collect();
            let trimmed = String::from_utf8_lossy(&line).trim().to_string();
            if trimmed.is_empty() {
                continue;
            }

            let response = match serde_json::from_str::<Request>(&trimmed) {
                Ok(Request::List) => Response::AppList {
                    apps: apps.iter().map(app_info).collect(),
                },
                Ok(Request::Stats { name }) => match apps.iter().find(|app| app.name() == name) {
                    Some(app) => Response::AppStats {
                        name: app.name().to_string(),
                        stats: app.stats(),
                    },
                    None => Response::AppNotFound {
                        name,
                        apps: apps.iter().map(|app| app.name().to_string()).collect(),
                    },
                },
                Err(e) => Response::Error {
                    message: e.to_string(),
                },
            };
            conn.queue_response(&response)?;
            conn.close_after_flush = true;
        }

        Ok(())
    }

    pub fn handle_writable(&mut self, token: Token) -> Result<()> {
        let conn = match self.connections.get_mut(&token) {
            Some(c) => c,
            None => return Ok(()),
        };

        while !conn.write_buf.is_empty() {
            let (chunk, _) = conn.write_buf.as_slices();
            match conn.stream.write(chunk) {
                Ok(0) => break,
                Ok(n) => {
                    conn.write_buf.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Closes the connection once fully flushed, otherwise updates its
    /// registered interest depending on whether output is pending.
    pub fn reconcile(&mut self, token: Token, poll: &Poll, token_slab: &mut MioTokenSlab) {
        let should_close = match self.connections.get(&token) {
            Some(conn) => conn.close_after_flush && conn.write_buf.is_empty(),
            None => return,
        };

        if should_close {
            self.close(token, poll, token_slab);
            return;
        }

        let conn = match self.connections.get_mut(&token) {
            Some(c) => c,
            None => return,
        };
        let wants_writable = !conn.write_buf.is_empty();
        if wants_writable != conn.writable_registered {
            let interest = if wants_writable {
                Interest::READABLE | Interest::WRITABLE
            } else {
                Interest::READABLE
            };
            if poll
                .registry()
                .reregister(&mut conn.stream, token, interest)
                .is_ok()
            {
                conn.writable_registered = wants_writable;
            }
        }
    }

    pub fn close(&mut self, token: Token, poll: &Poll, token_slab: &mut MioTokenSlab) {
        if let Some(mut conn) = self.connections.remove(&token) {
            let _ = poll.registry().deregister(&mut conn.stream);
            token_slab.free(token);
        }
    }
}

/// Single best-effort, non-blocking write; the stream is dropped right
/// after regardless of whether the write succeeded.
fn reject_over_capacity(stream: &mut UnixStream) {
    if let Ok(mut line) = serde_json::to_string(&Response::Error {
        message: "too many control connections, try again later".to_string(),
    }) {
        line.push('\n');
        let _ = stream.write_all(line.as_bytes());
    }
}

fn app_info(app: &App) -> AppInfo {
    let cgroup = app.cgroup_config();
    let stats = app.stats();
    AppInfo {
        name: app.name().to_string(),
        state: app.status_string(),
        pid: app.pid(),
        command: app.command(),
        cwd: app.cwd().map(str::to_string),
        uid: app.uid(),
        gid: app.gid(),
        restart_count: stats.restart_count,
        log_bytes: stats.stdout_bytes + stats.stderr_bytes,
        memory_current: stats.memory_current,
        io_read_bytes: stats.io_read_bytes,
        io_write_bytes: stats.io_write_bytes,
        oneshot: app.is_oneshot(),
        cpu_weight: cgroup.cpu_weight,
        io_weight: cgroup.io_weight,
    }
}

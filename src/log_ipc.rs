//! Dedicated log-streaming socket, kept separate from the control socket (`crate::ipc`)
//! so raw stdout/stderr bytes never have to be wrapped in any framing or JSON.
//!
//! Wire format (client -> server, sent once): `follow: u8` `name_len: u8`
//! `name: [u8; name_len]` (`name` must be non-empty; only one app can be read per
//! connection). Everything the server ever writes back is that app's raw log
//! bytes, verbatim, with no header of any kind. If `name` doesn't match a known
//! app the connection is simply closed without sending anything.
//!
//! Like `crate::ipc::Server`, this is a non-blocking, single-threaded `mio` state
//! machine driven from the supervisor's main event loop. Log data is never copied
//! into an intermediate buffer: each connection just tracks a cursor into the
//! app's own ring buffer and writes directly from it whenever the socket is
//! writable, so the ring buffer itself is the only buffer involved.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::Path;

use mio::net::{UnixListener, UnixStream};
use mio::{Interest, Poll, Token};

use crate::ipc::Result;
use crate::runtime::App;
use crate::support::mio_token_slab::MioTokenSlab;

/// Cap on concurrent log-socket connections; see `LogServer::bind`.
pub const DEFAULT_MAX_CONNECTIONS: usize = 32;

/// Which app a connection is streaming, and how far it's gotten.
struct Subscription {
    name: String,
    follow: bool,
    /// Logical offset into the app's log buffer of the next byte to send.
    cursor: usize,
    /// For non-follow requests, the offset to stop at (the app's write position
    /// when the request was made). `None` when following.
    dump_until: Option<usize>,
}

enum ConnState {
    AwaitingRequest,
    Streaming(Subscription),
}

struct Connection {
    stream: UnixStream,
    read_buf: Vec<u8>,
    state: ConnState,
    close_after_flush: bool,
    /// Set when the last write attempt hit `WouldBlock`: more data is pending
    /// and we need a writable event to know when to retry.
    write_blocked: bool,
    writable_registered: bool,
}

impl Connection {
    fn new(stream: UnixStream) -> Self {
        Connection {
            stream,
            read_buf: Vec::new(),
            state: ConnState::AwaitingRequest,
            close_after_flush: false,
            write_blocked: false,
            writable_registered: false,
        }
    }
}

pub struct LogServer {
    listener: UnixListener,
    connections: HashMap<Token, Connection>,
    max_connections: usize,
}

impl LogServer {
    /// Binds the log socket, removing any stale socket file left behind by an unclean
    /// previous shutdown, and registers it with `poll`.
    pub fn bind(
        socket_path: &Path,
        poll: &Poll,
        token: Token,
        max_connections: usize,
    ) -> Result<Self> {
        if socket_path.exists() {
            std::fs::remove_file(socket_path)?;
        }

        let mut listener = UnixListener::bind(socket_path)?;
        poll.registry()
            .register(&mut listener, token, Interest::READABLE)?;

        Ok(LogServer {
            listener,
            connections: HashMap::new(),
            max_connections,
        })
    }

    /// Accepts every pending connection until the listener would block. Connections
    /// beyond `max_connections` are dropped immediately with no message, to keep the
    /// wire free of anything but raw log bytes.
    pub fn accept_all(&mut self, poll: &Poll, token_slab: &mut MioTokenSlab) -> Result<()> {
        loop {
            match self.listener.accept() {
                Ok((mut stream, _addr)) => {
                    if self.connections.len() >= self.max_connections {
                        log::warn!(
                            "rejecting log connection: {} already active (max {})",
                            self.connections.len(),
                            self.max_connections
                        );
                        continue;
                    }
                    if let Some(token) = token_slab.allocate() {
                        poll.registry()
                            .register(&mut stream, token, Interest::READABLE)?;
                        self.connections.insert(token, Connection::new(stream));
                    } else {
                        log::warn!("rejecting log connection: no available token");
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

    /// Reads whatever's pending on `token`; once a full request has arrived, starts
    /// streaming the requested app's log and immediately sends whatever's already
    /// available without waiting for a separate writable event.
    pub fn handle_readable(&mut self, token: Token, apps: &mut [App]) -> Result<()> {
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

        if matches!(conn.state, ConnState::AwaitingRequest) {
            // header (follow + name_len) plus the name itself must be fully buffered
            if conn.read_buf.len() < 2 {
                return Ok(());
            }
            let name_len = conn.read_buf[1] as usize;
            if conn.read_buf.len() < 2 + name_len {
                return Ok(());
            }

            let follow = conn.read_buf[0] != 0;
            let request: Vec<u8> = conn.read_buf.drain(..2 + name_len).collect();
            let name = String::from_utf8_lossy(&request[2..]).into_owned();

            let write_pos = if name.is_empty() {
                None
            } else {
                apps.iter_mut()
                    .find(|app| app.name() == name)
                    .map(|app| app.log_write_pos())
            };

            let Some(write_pos) = write_pos else {
                log::warn!("rejecting log request for unknown app '{}'", name);
                conn.close_after_flush = true;
                return Ok(());
            };

            conn.state = ConnState::Streaming(Subscription {
                name,
                follow,
                cursor: 0,
                dump_until: (!follow).then_some(write_pos),
            });
        }

        self.try_send(token, apps)
    }

    pub fn handle_writable(&mut self, token: Token, apps: &mut [App]) -> Result<()> {
        self.try_send(token, apps)
    }

    /// Writes as much of the subscribed app's retained log output (from the
    /// subscription's cursor onward) directly to the socket as it will currently
    /// accept, advancing the cursor by however many bytes actually went out. The
    /// app's ring buffer is the only buffer involved: nothing is copied out of it
    /// ahead of time.
    fn try_send(&mut self, token: Token, apps: &mut [App]) -> Result<()> {
        let Some(conn) = self.connections.get_mut(&token) else {
            return Ok(());
        };

        let ConnState::Streaming(sub) = &mut conn.state else {
            return Ok(());
        };

        let Some(app) = apps.iter_mut().find(|a| a.name() == &sub.name) else {
            conn.close_after_flush = true;
            return Ok(());
        };

        let mut blocked = false;
        loop {
            let chunk = app.log_bytes_from_cursor(sub.cursor);

            // Add any missed bytes to the cursor before attempting to write the buffer
            sub.cursor += chunk.missed;

            if let Some(buffer) = chunk.buffer {
                match conn.stream.write(buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        sub.cursor += n;
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        blocked = true;
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            } else {
                break;
            }
        }
        conn.write_blocked = blocked;

        if let ConnState::Streaming(sub) = &conn.state {
            if !blocked {
                if let Some(bound) = sub.dump_until {
                    if sub.cursor >= bound {
                        conn.close_after_flush = true;
                    }
                }
            }
        }

        Ok(())
    }

    /// Pushes any log output produced since the last call to every connection with an
    /// active follow subscription. Meant to be called once per iteration of the
    /// daemon's main loop, after apps have had a chance to splice freshly read
    /// stdout/stderr into their log buffers.
    pub fn push_updates(&mut self, apps: &mut [App], poll: &Poll, token_slab: &mut MioTokenSlab) {
        let tokens: Vec<Token> = self
            .connections
            .iter()
            .filter(|(_, conn)| matches!(&conn.state, ConnState::Streaming(sub) if sub.follow))
            .map(|(token, _)| *token)
            .collect();

        for token in tokens {
            if let Err(e) = self.try_send(token, apps) {
                log::error!("log connection write error: {}", e);
                self.close(token, poll, token_slab);
                continue;
            }
            self.reconcile(token, poll, token_slab);
        }
    }

    /// Closes the connection once fully flushed, otherwise updates its
    /// registered interest depending on whether output is pending.
    pub fn reconcile(&mut self, token: Token, poll: &Poll, token_slab: &mut MioTokenSlab) {
        let should_close = match self.connections.get(&token) {
            Some(conn) => conn.close_after_flush && !conn.write_blocked,
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
        let wants_writable = conn.write_blocked;
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

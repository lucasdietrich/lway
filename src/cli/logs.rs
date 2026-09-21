//! CLI client command: `lway log <name> [-f]` -- dumps (and optionally follows)
//! an app's captured stdout/stderr over the dedicated log socket (see `crate::log_ipc`).
//! The socket carries nothing but that app's raw log bytes: no framing, no JSON.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::ipc::{IpcError, Result};

/// Connects to the log socket, requests `name`'s logs, and prints whatever's
/// currently retained. If `follow` is set, keeps the connection open printing new
/// output as it arrives until the daemon closes it or the process exits.
pub fn run(log_socket_path: &Path, name: &str, follow: bool) -> Result<()> {
    let mut stream = UnixStream::connect(log_socket_path)
        .map_err(|_| IpcError::NotRunning(log_socket_path.to_path_buf()))?;

    let name_bytes = &name.as_bytes()[..name.len().min(u8::MAX as usize)];
    let mut request = Vec::with_capacity(2 + name_bytes.len());
    request.push(follow as u8);
    request.push(name_bytes.len() as u8);
    request.extend_from_slice(name_bytes);
    stream.write_all(&request)?;

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.write_all(&buf[..n])?;
                out.flush()?;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

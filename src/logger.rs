use std::{
    error::Error,
    io::{self},
    os::fd::{AsRawFd, RawFd},
};

use crate::support::pipe::splice;

pub trait LoggerSimple {
    fn log_str(
        &self,
        name: &str,
        pid: libc::pid_t,
        msg: &str,
        is_err: bool,
    ) -> Result<(), Box<dyn Error>> {
        self.log(name, pid, msg.as_bytes(), is_err)
    }

    /// `is_err` picks the daemon's own stdout vs stderr, mirroring where `bytes`
    /// came from on the app side.
    fn log(
        &self,
        name: &str,
        pid: libc::pid_t,
        bytes: &[u8],
        is_err: bool,
    ) -> Result<(), Box<dyn Error>>;
}

const DEFAULT_GUTTER_WIDTH: usize = 16;

pub struct StdoutLogger {
    gutter_width: usize,
}

impl StdoutLogger {
    pub fn new(gutter_width: usize) -> Self {
        Self { gutter_width }
    }
}

impl Default for StdoutLogger {
    fn default() -> Self {
        Self {
            gutter_width: DEFAULT_GUTTER_WIDTH,
        }
    }
}

impl LoggerSimple for StdoutLogger {
    fn log(
        &self,
        name: &str,
        pid: libc::pid_t,
        bytes: &[u8],
        is_err: bool,
    ) -> Result<(), Box<dyn Error>> {
        let string = String::from_utf8_lossy(bytes);
        for line in string.lines() {
            let prefix = format!("[{} {}]", pid, name);
            if is_err {
                eprintln!("{:width$} {}", prefix, line, width = self.gutter_width);
            } else {
                println!("{:width$} {}", prefix, line, width = self.gutter_width);
            }
        }
        Ok(())
    }
}

pub trait LogBuffer: AsRawFd {
    fn as_file(&self) -> &std::fs::File;

    /// Splices up to `max_len` bytes from `read_fd` (a pipe) into the buffer, returning
    /// the number of bytes written. A buffer may write fewer bytes than requested for
    /// reasons other than the source being empty (e.g. a ring buffer stopping at its
    /// wrap boundary); callers that need to fully drain a pipe should loop on this until
    /// it returns `Ok(0)`.
    fn splice_from(&mut self, read_fd: RawFd, len: usize) -> io::Result<usize> {
        splice(read_fd, self.as_raw_fd(), None, len)
    }

    fn filling(&mut self) -> usize;

    // Todo change signature to `fn capacity(&mut self) -> usize;`
    fn capacity(&mut self) -> Option<usize>;

    /// Short, human-readable name of the concrete buffer implementation (e.g.
    /// `"circular-mmap"`), for debug/introspection purposes only.
    fn kind(&self) -> &'static str;
}

pub struct LogChunk<'a> {
    pub buffer: Option<&'a [u8]>,
    pub missed: usize,
}

impl<'a> LogChunk<'a> {
    pub fn new(buffer: Option<&'a [u8]>, missed: usize) -> Self {
        Self { buffer, missed }
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_none() && self.missed == 0
    }
}

pub trait ViewableLogBuffer: LogBuffer {
    fn splice_from_and_view(
        &mut self,
        read_fd: RawFd,
        len: usize,
    ) -> io::Result<Option<&[u8]>>;

    /// Total bytes ever written to the buffer; monotonically increasing.
    fn write_pos(&mut self) -> usize {
        self.filling()
    }

    /// Oldest logical offset still retained; bytes before this were overwritten/evicted.
    fn start_pos(&mut self) -> usize {
        match self.capacity() {
            Some(cap) => self.write_pos().saturating_sub(cap),
            None => 0,
        }
    }

    /// Largest contiguous slice of retained data starting at logical offset `from`.
    /// May be shorter than everything available (e.g. stops at the ring buffer's
    /// wrap boundary); callers that want everything up to `write_pos()` should call
    /// this again with the advanced offset. Panics if `from` is older than
    /// `start_pos()`: the data was overwritten before the caller could read it.
    fn read_from_cursor(&mut self, from: usize) -> LogChunk<'_>;
}

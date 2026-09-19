use std::{
    fmt::Display,
    io::{self, Read},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        raw::c_int,
    },
};

use crate::support::to_ioresult;

#[derive(Debug)]
pub struct PipeReader(OwnedFd);

impl PipeReader {
    pub fn splice_to(&self, write_fd: &impl AsRawFd, size: usize) -> io::Result<usize> {
        splice(self.as_raw_fd(), write_fd.as_raw_fd(), None, size)
    }
}

impl AsRawFd for PipeReader {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl AsFd for PipeReader {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let ret = unsafe { libc::read(self.0.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len()) };
        if ret >= 0 {
            Ok(ret as usize)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

#[derive(Debug)]
pub struct PipeWriter(OwnedFd);

impl PipeWriter {
    pub fn splice_from(&self, read_fd: &impl AsRawFd, size: usize) -> io::Result<usize> {
        splice(read_fd.as_raw_fd(), self.as_raw_fd(), None, size)
    }
}

impl AsRawFd for PipeWriter {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl AsFd for PipeWriter {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

// [read, write]
pub struct Pipe([i32; 2]);

impl Pipe {
    pub fn new() -> io::Result<Pipe> {
        let mut pipefd = [-1; 2];
        let ret = unsafe { libc::pipe(&mut pipefd as *mut c_int) };
        to_ioresult(ret)?;
        Ok(Pipe(pipefd))
    }

    /// Convert the pipe into a read-only file descriptor, closing the write end.
    pub fn into_read_fd(self) -> io::Result<PipeReader> {
        let ret = unsafe { libc::close(self.0[1]) };
        to_ioresult(ret)?;
        Ok(PipeReader(unsafe { OwnedFd::from_raw_fd(self.0[0]) }))
    }

    /// Convert the pipe into a non-blocking read-only file descriptor, closing the write end.
    pub fn into_nonblocking_read_fd(self) -> io::Result<PipeReader> {
        let reader = self.into_read_fd()?;

        let fd = reader.0.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        let flags = to_ioresult(flags)?;
        let ret = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        to_ioresult(ret)?;
        Ok(reader)
    }

    /// Convert the pipe into a write-only file descriptor, closing the read end.
    pub fn into_write_fd(self) -> io::Result<PipeWriter> {
        // Close read end
        let ret = unsafe { libc::close(self.0[0]) };
        to_ioresult(ret)?;

        Ok(PipeWriter(unsafe { OwnedFd::from_raw_fd(self.0[1]) }))
    }
}

impl Display for Pipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pipe {{ read: {}, write: {} }}", self.0[0], self.0[1])
    }
}

/// Splices data from `read_fd` into `out_fd` at the given optional output offset.
/// If `out_offset` is `None`, the current file position of `out_fd` is used.
/// One of the file descriptors must be a pipe.
pub(crate) fn splice(
    read_fd: RawFd,
    out_fd: RawFd,
    out_offset: Option<libc::loff_t>,
    len: usize,
) -> io::Result<usize> {
    let off_out: *mut libc::loff_t = match out_offset {
        Some(offset) => &mut (offset as libc::loff_t) as *mut libc::loff_t,
        None => std::ptr::null_mut(),
    };

    let ret = unsafe {
        libc::splice(
            read_fd,
            std::ptr::null_mut(),
            out_fd,
            off_out,
            len,
            libc::SPLICE_F_MOVE | libc::SPLICE_F_NONBLOCK | libc::SPLICE_F_MORE,
        )
    };
    let result = to_ioresult(ret as c_int)? as usize;
    Ok(result)
}

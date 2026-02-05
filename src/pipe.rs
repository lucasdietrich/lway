use std::{
    fmt::Display,
    io::{self, Read},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        raw::c_int,
    },
};

use libc::pipe;

pub struct PipeReader(OwnedFd);
pub struct PipeWriter(OwnedFd);

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
        let ret = unsafe { pipe(&mut pipefd as *mut c_int) };
        if ret == 0 {
            Ok(Pipe(pipefd))
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    pub fn into_read_fd(self) -> io::Result<PipeReader> {
        // Close write end
        let ret = unsafe { libc::close(self.0[1]) };

        if ret == -1 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(PipeReader(unsafe { OwnedFd::from_raw_fd(self.0[0]) }))
    }

    pub fn into_nonblocking_read_fd(self) -> io::Result<PipeReader> {
        let reader = self.into_read_fd()?;

        let fd = reader.0.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let ret = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        if ret == -1 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(reader)
    }

    pub fn into_write_fd(self) -> io::Result<PipeWriter> {
        // Close read end
        let ret = unsafe { libc::close(self.0[0]) };

        if ret == -1 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(PipeWriter(unsafe { OwnedFd::from_raw_fd(self.0[1]) }))
    }
}

impl Display for Pipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pipe {{ read: {}, write: {} }}", self.0[0], self.0[1])
    }
}

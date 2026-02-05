use std::{
    ffi::CString,
    io::{self, Read},
    os::fd::AsRawFd,
    str::FromStr,
};

use libc::{c_char, dup2, waitpid, STDERR_FILENO, STDOUT_FILENO, WEXITSTATUS, WIFEXITED, WNOHANG};
use thiserror::Error;

use crate::{logger::Logger, pipe::{Pipe, PipeReader}};

#[derive(Debug, Error)]
pub enum AppErr {
    #[error("Fork failed {0}")]
    ForkFailed(i32),
    #[error("Execv failed {0}")]
    ExecvFailed(i32),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnState {
    // Tell whether the process returned normally
    Completed { ret: i32 },
    Abnormal, // Tell whether the process returned normally (call to exit or return from main)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Terminated(ReturnState),
}

pub struct App {
    name: String,
    pid: i32,
    state: State,
    stdout: PipeReader,
    stderr: PipeReader,
}

fn fd_dup(src: impl AsRawFd, dst: impl AsRawFd) -> io::Result<()> {
    let ret = unsafe { dup2(src.as_raw_fd(), dst.as_raw_fd()) };
    if ret == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl App {
    pub fn start<'a>(app_name: &str, app: &str, args: impl Iterator<Item = &'a str>) -> Result<Self, AppErr> {
        let pipe_stdout: Pipe = Pipe::new().expect("pipe stdout");
        let pipe_stderr = Pipe::new().expect("pipe stderr");

        let ret = unsafe { libc::fork() };

        if ret == 0 {
            // child
            let stdout = pipe_stdout.into_write_fd()?;
            fd_dup(stdout, STDOUT_FILENO)?;

            let stderr = pipe_stderr.into_write_fd()?;
            fd_dup(stderr, STDERR_FILENO)?;

            let prog = CString::from_str(app).expect("app name");
            let args: Vec<CString> = args
                .map(|arg| CString::from_str(arg).expect("arg"))
                .collect();
            let mut argv: Vec<*const c_char> = args
                .iter()
                .map(|cstring| cstring.as_ptr() as *const c_char)
                .collect();
            // execv expects a null-terminated array
            argv.push(std::ptr::null());

            let ret = unsafe { libc::execvp(prog.as_ptr() as *const c_char, argv.as_ptr()) };
            log::error!(
                "execv returned {} errno: {}",
                ret,
                std::io::Error::last_os_error()
            );
            Err(AppErr::ExecvFailed(ret))
        } else if ret > 0 {
            // parent
            log::info!("Child pid: {}", ret);
            Ok(App {
                name: app_name.to_string(),
                pid: ret,
                state: State::Running,
                stdout: pipe_stdout
                    .into_nonblocking_read_fd()
                    .expect("nonblocking stdout"),
                stderr: pipe_stderr
                    .into_nonblocking_read_fd()
                    .expect("nonblocking stderr"),
            })
        } else {
            Err(AppErr::ForkFailed(ret))
        }
    }

    pub fn poll(&mut self, logger: &impl Logger) {
        if self.state != State::Running {
            // Nothing to do
            return;
        }

        // Read stdout and stderr
        let mut buf = vec![0u8; 1024];
        if let Ok(rcvd) = self.stdout.read(&mut buf) {
            log::info!("app {} rcvd {} bytes from stdout", self.pid, rcvd);
            logger.log(&self.name, self.pid, &buf[..rcvd]).expect("log stdout");
        } else {
            log::debug!(
                "app {} no data from stdout: {}",
                self.pid,
                std::io::Error::last_os_error()
            );
        }

        if let Ok(rcvd) = self.stderr.read(&mut buf) {
            log::info!("app {} rcvd {} bytes from stderr", self.pid, rcvd);
            logger.log(&self.name, self.pid, &buf[..rcvd]).expect("log stderr");
            log::debug!(
                "app {} no data from stderr: {}",
                self.pid,
                std::io::Error::last_os_error()
            );
        }

        // Poll state
        let mut status: i32 = 0;
        let options: i32 = WNOHANG;
        let ret = unsafe { waitpid(self.pid, &mut status, options) };
        log::debug!("app: {} waitpid -> {} status: {}", self.pid, ret, status);

        if ret == self.pid {
            // children exited

            // parse status
            let normal = WIFEXITED(status);
            let return_state = match normal {
                true => ReturnState::Completed {
                    ret: WEXITSTATUS(status),
                },
                false => ReturnState::Abnormal,
            };
            self.state = State::Terminated(return_state);
            log::info!("app {} returned {:?}", self.pid, self.state);
        } else if ret == 0 {
            // waiting
        } else {
            panic!("waitpid failed")
        }
    }

    pub fn is_running(&self) -> bool {
        self.state == State::Running
    }
}

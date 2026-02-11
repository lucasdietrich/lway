use std::{
    ffi::CString,
    fmt::Display,
    io::{self, Error, Read},
    os::fd::AsRawFd,
    str::FromStr,
};

use libc::{c_char, dup2, waitpid, STDERR_FILENO, STDOUT_FILENO, WEXITSTATUS, WIFEXITED, WNOHANG};
use thiserror::Error;

use crate::{
    logger::Logger,
    pipe::{Pipe, PipeReader},
    utils::to_ioresult,
};

#[derive(Debug, Error)]
pub enum AppErr {
    #[error("Fork failed {0}")]
    ForkFailed(i32),
    #[error("Execv failed {0}")]
    ExecvFailed(Error),
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
    Running(i32), // Running with pid
    Terminated(ReturnState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppParams<'a> {
    pub cwd: Option<&'a str>,
    pub name: &'a str,
    pub prog: &'a str,
    pub args: &'a [&'a str],
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub env: Vec<String>,
}

pub struct App {
    name: String,
    state: State,
    stdout: PipeReader,
    stderr: PipeReader,
}

fn fd_dup(src: impl AsRawFd, dst: impl AsRawFd) -> io::Result<()> {
    let ret = unsafe { dup2(src.as_raw_fd(), dst.as_raw_fd()) };
    to_ioresult(ret)?;
    Ok(())
}

impl App {
    pub fn start<'a>(params: AppParams<'a>) -> Result<Self, AppErr> {
        let pipe_stdout: Pipe = Pipe::new().expect("pipe stdout");
        let pipe_stderr = Pipe::new().expect("pipe stderr");

        let ret = unsafe { libc::fork() };

        if ret == 0 {
            // child
            let stdout = pipe_stdout.into_write_fd()?;
            fd_dup(stdout, STDOUT_FILENO)?;

            let stderr = pipe_stderr.into_write_fd()?;
            fd_dup(stderr, STDERR_FILENO)?;

            let prog = CString::from_str(params.prog).expect("app name");
            let args: Vec<CString> = params
                .args
                .iter()
                .map(|arg| CString::from_str(arg).expect("arg"))
                .collect();
            let mut argv: Vec<*const c_char> = args
                .iter()
                .map(|cstring| cstring.as_ptr() as *const c_char)
                .collect();
            // execv expects a null-terminated array
            argv.push(std::ptr::null());

            // let ret = unsafe {
            //     libc::setsid()
            // };
            // if ret == -1 {
            //     let error = std::io::Error::last_os_error();
            //     log::error!("setsid failed: {}", error);
            //     return Err(AppErr::Io(error));
            // }

            // Set uid and gid if specified
            // let ret = unsafe { libc::setgroups(0, std::ptr::null()) };
            // to_ioresult(ret).map_err(|e| {
            //     log::error!("setgroups failed: {}", e);
            //     AppErr::Io(e)
            // })?;
            if let Some(gid) = params.gid {
                log::info!("Setting gid to {}", gid);
                let ret = unsafe { libc::setgid(gid) };
                to_ioresult(ret).map_err(|e| {
                    log::error!("setgid failed: {}", e);
                    AppErr::Io(e)
                })?;
            }
            if let Some(uid) = params.uid {
                log::info!("Setting uid to {}", uid);
                let ret = unsafe { libc::setuid(uid) };
                to_ioresult(ret).map_err(|e| {
                    log::error!("setuid failed: {}", e);
                    AppErr::Io(e)
                })?;
            }

            // Build environment variables
            let env: Vec<CString> = params
                .env
                .iter()
                .map(|env| CString::from_str(env).expect("env"))
                .collect();
            let mut envp: Vec<*const c_char> = env
                .iter()
                .map(|cstring| cstring.as_ptr() as *const c_char)
                .collect();
            envp.push(std::ptr::null());

            // Set working directory if specified
            if let Some(cwd) = params.cwd {
                log::info!("Changing working directory to {}", cwd);
                let cwd_cstr = CString::from_str(cwd).expect("cwd");
                let ret = unsafe { libc::chdir(cwd_cstr.as_ptr()) };
                to_ioresult(ret).map_err(|e| {
                    log::error!("chdir failed: {}", e);
                    AppErr::Io(e)
                })?;
            }

            let ret = unsafe {
                libc::execve(prog.as_ptr() as *const c_char, argv.as_ptr(), envp.as_ptr())
            };
            let error = std::io::Error::last_os_error();
            log::error!("execv returned {} errno: {}", ret, error,);
            Err(AppErr::ExecvFailed(error))
        } else if ret > 0 {
            // parent
            log::info!("Child pid: {}", ret);
            Ok(App {
                name: params.name.to_string(),
                state: State::Running(ret),
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
        let State::Running(pid) = self.state else {
            return;
        };

        // Read stdout and stderr
        let mut buf = vec![0u8; 1024];
        if let Ok(rcvd) = self.stdout.read(&mut buf) {
            log::debug!("app {} rcvd {} bytes from stdout", pid, rcvd);
            logger
                .log(&self.name, pid, &buf[..rcvd])
                .expect("log stdout");
        } else {
            log::debug!(
                "app {} no data from stdout: {}",
                pid,
                std::io::Error::last_os_error()
            );
        }

        if let Ok(rcvd) = self.stderr.read(&mut buf) {
            log::debug!("app {} rcvd {} bytes from stderr", pid, rcvd);
            logger
                .log(&self.name, pid, &buf[..rcvd])
                .expect("log stderr");
            log::debug!(
                "app {} no data from stderr: {}",
                pid,
                std::io::Error::last_os_error()
            );
        }

        // Poll state
        let mut status: i32 = 0;
        let options: i32 = WNOHANG;
        let ret = unsafe { waitpid(pid, &mut status, options) };
        log::debug!("app: {} waitpid -> {} status: {}", pid, ret, status);

        if ret == pid {
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
            log::info!("app {} returned {:?}", pid, self.state);
        } else if ret == 0 {
            // waiting
        } else {
            panic!("waitpid failed")
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, State::Running(_))
    }

    pub fn sigterm(&self) -> io::Result<()> {
        let State::Running(pid) = self.state else {
            return Ok(());
        };

        let ret = unsafe { libc::kill(pid, libc::SIGTERM) };
        log::info!("app {} kill -> {}", pid, ret);
        to_ioresult(ret)?;
        Ok(())
    }
}

impl Display for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "App {{ name: {}, state: {:?} }}", self.name, self.state)
    }
}

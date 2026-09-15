use std::{
    ffi::{c_int, CString},
    fmt::Display,
    io::{self, Error, Read},
    os::fd::{AsRawFd, RawFd},
    str::FromStr,
};

use cgroups_rs::fs::Cgroup;
use libc::{
    c_char, dup2, pid_t, waitpid, STDERR_FILENO, STDOUT_FILENO, WEXITSTATUS, WIFEXITED,
    WIFSIGNALED, WNOHANG, WTERMSIG,
};
use mio::unix::SourceFd;
use thiserror::Error;

use crate::{
    cgroups::{init_app_cgroup, AppCgroupConfig},
    logger::Logger,
    mio_token_slab::MioTokenSlab,
    pipe::{Pipe, PipeReader},
    support::{signal::signal_name, to_ioresult},
};

#[derive(Debug, Error)]
pub enum AppErr {
    #[error("Runtime error: {0}")]
    Runtime(#[from] AppRuntimeError),
    #[error("poll registration error: {0}")]
    PollRegistration(#[from] io::Error),
    #[error("Token allocation error")]
    TokenAllocation,
}

// remake <'a>
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppParams {
    pub cwd: Option<String>,
    pub name: String,
    pub prog: String,
    pub args: Vec<String>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub env: Vec<String>,
    pub oneshot: bool,
    pub cgroup: AppCgroupConfig,
}

#[derive(Debug)]
struct AppRtTokens {
    pub pidfd: mio::Token,
    pub stdout: mio::Token,
    pub stderr: mio::Token,
}

pub struct App {
    name: String,
    params: AppParams,
    state: State,
}

impl App {
    pub fn new(
        params: AppParams,
        poll: &mio::Poll,
        token_slab: &mut MioTokenSlab,
    ) -> Result<Self, AppErr> {
        let runtime = AppRuntime::start(&params, &poll, token_slab)?;

        Ok(App {
            name: params.name.to_string(),
            state: State::Running(runtime),
            params,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn pid(&self) -> Option<u32> {
        self.state.as_runtime().map(|rt| rt.pid as u32)
    }

    pub fn status_string(&self) -> String {
        match &self.state {
            State::Running(..) => "running".to_string(),
            State::Terminated(ReturnState::Completed { ret }) => format!("exited({})", ret),
            State::Terminated(ReturnState::Abnormal) => "abnormal".to_string(),
        }
    }

    pub fn is_oneshot(&self) -> bool {
        self.params.oneshot
    }

    pub fn cgroup_config(&self) -> &AppCgroupConfig {
        &self.params.cgroup
    }

    pub fn poll(
        &mut self,
        poll: &mio::Poll,
        token_slab: &mut MioTokenSlab,
        token: mio::Token,
        event: &mio::event::Event,
        logger: &dyn Logger,
        try_restart: bool,
    ) {
        let State::Running(rt) = &mut self.state else {
            return;
        };

        // logging: Read stdout and stderr
        let mut buf = vec![0u8; 1024];

        if token == rt.tokens.stdout {
            if event.is_readable() {
                if let Ok(rcvd) = rt.stdout.read(&mut buf) {
                    log::debug!("app {} rcvd {} bytes from stdout", rt.pid, rcvd);
                    logger
                        .log(&self.name, rt.pid, &buf[..rcvd])
                        .expect("log stdout");
                } else {
                    log::debug!(
                        "app {} no data from stdout: {}",
                        rt.pid,
                        std::io::Error::last_os_error()
                    );
                }
            }

            if event.is_read_closed() {
                log::info!("app {} stdout closed", rt.pid);
            }
        }

        if token == rt.tokens.stderr {
            if event.is_readable() {
                if let Ok(rcvd) = rt.stderr.read(&mut buf) {
                    log::debug!("app {} rcvd {} bytes from stderr", rt.pid, rcvd);
                    logger
                        .log(&self.name, rt.pid, &buf[..rcvd])
                        .expect("log stderr");
                } else {
                    log::debug!(
                        "app {} no data from stderr: {}",
                        rt.pid,
                        std::io::Error::last_os_error()
                    );
                }
            }

            if event.is_read_closed() {
                log::info!("app {} stderr closed", rt.pid);
            }
        }

        // handle process termination
        if token == rt.tokens.pidfd && event.is_readable() {
            if let Some(return_state) = poll_pid(rt.pid) {
                {
                    let terminated = State::Terminated(return_state);
                    let State::Running(rt) = std::mem::replace(&mut self.state, terminated) else {
                        unreachable!();
                    };

                    rt.stop(poll, token_slab)
                        .expect("Failed to stop app runtime");

                    self.state = State::Terminated(return_state);

                    // AppRuntime is getting dropped in the background (clearing cgroups, etc ...)
                }

                // evaluate the restart of the application
                if try_restart && !self.params.oneshot {
                    match AppRuntime::start(&self.params, &poll, token_slab) {
                        Ok(runtime) => self.state = State::Running(runtime),
                        Err(err) => log::error!("Failed to restart app {}: {}", self.name, err),
                    }
                }
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.state.is_running()
    }

    pub fn sigterm(&self) -> io::Result<()> {
        self.state
            .as_runtime()
            .map_or(Ok(()), |app_runtime| app_runtime.send_sigterm())
    }

    pub fn sigkill(&self) -> io::Result<()> {
        self.state
            .as_runtime()
            .map_or(Ok(()), |app_runtime| app_runtime.send_sigkill())
    }
}

impl Display for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "App {{ name: {}, state: {:?} }}", self.name, self.state)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnState {
    // Tell whether the process returned normally
    Completed { ret: i32 },
    Abnormal, // Tell whether the process returned normally (call to exit or return from main)
}

fn poll_pid(pid: pid_t) -> Option<ReturnState> {
    // Poll state
    let mut status: i32 = 0;
    let options: i32 = WNOHANG;
    let ret = unsafe { waitpid(pid as libc::pid_t, &mut status, options) };
    log::debug!("app: {} waitpid -> {} status: {}", pid, ret, status);

    if ret == pid as libc::pid_t {
        // children exited

        let signaled = WIFSIGNALED(status);
        if signaled {
            let termsig = WTERMSIG(status);
            log::info!(
                "app {} terminated by signal {} ({})",
                pid,
                signal_name(termsig as usize).unwrap_or("UNKNOWN"),
                termsig
            );
        }

        // parse status
        let normal = WIFEXITED(status);
        let return_state = match normal {
            true => ReturnState::Completed {
                ret: WEXITSTATUS(status),
            },
            false => ReturnState::Abnormal,
        };
        log::info!("app {} returned {:?}", pid, return_state);
        Some(return_state)
    } else if ret == 0 {
        // waiting
        None
    } else {
        panic!("waitpid failed")
    }
}

#[derive(Debug, Error)]
pub enum AppRuntimeError {
    #[error("Fork failed {0}")]
    ForkFailed(i32),
    #[error("Execv failed {0}")]
    ExecvFailed(Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Token allocation failed")]
    TokenAllocation,
}

#[derive(Debug)]
struct AppRuntime {
    pid: libc::pid_t,
    cgroup: Cgroup,
    stdout: PipeReader,
    stderr: PipeReader,
    pidfd: RawFd,
    tokens: AppRtTokens,

    /// Indicates whether AppRuntime::stop was called before the runtime was dropped
    proper_cleanup: bool,
}

impl AppRuntime {
    fn start(
        params: &AppParams,
        poll: &mio::Poll,
        token_slab: &mut MioTokenSlab,
    ) -> Result<Self, AppRuntimeError> {
        let pipe_stdout: Pipe = Pipe::new().expect("pipe stdout");
        let pipe_stderr = Pipe::new().expect("pipe stderr");

        let ret = unsafe { libc::fork() };

        if ret == 0 {
            // child
            let stdout = pipe_stdout.into_write_fd()?;
            fd_dup(stdout, STDOUT_FILENO)?;

            let stderr = pipe_stderr.into_write_fd()?;
            fd_dup(stderr, STDERR_FILENO)?;

            let prog = CString::from_str(&params.prog).expect("app name");
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

            // setgid must happen before setuid: once uid is dropped, permission to
            // change gid is lost.
            if let Some(gid) = params.gid {
                log::info!("Setting gid to {}", gid);
                let ret = unsafe { libc::setgid(gid) };
                to_ioresult(ret).map_err(|e| {
                    log::error!("setgid failed: {}", e);
                    AppRuntimeError::Io(e)
                })?;
            }

            if let Some(uid) = params.uid {
                log::info!("Setting uid to {}", uid);
                let ret = unsafe { libc::setuid(uid) };
                to_ioresult(ret).map_err(|e| {
                    log::error!("setuid failed: {}", e);
                    AppRuntimeError::Io(e)
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
            if let Some(cwd) = &params.cwd {
                log::info!("Changing working directory to {}", cwd);
                let cwd_cstr = CString::from_str(cwd).expect("cwd");
                let ret = unsafe { libc::chdir(cwd_cstr.as_ptr()) };
                to_ioresult(ret).map_err(|e| {
                    log::error!("chdir failed: {}", e);
                    AppRuntimeError::Io(e)
                })?;
            }

            let ret = unsafe {
                libc::execve(prog.as_ptr() as *const c_char, argv.as_ptr(), envp.as_ptr())
            };
            let error = std::io::Error::last_os_error();
            log::error!("execv returned {} errno: {}", ret, error,);
            Err(AppRuntimeError::ExecvFailed(error))
        } else if ret > 0 {
            let pid = ret as libc::pid_t;
            let cgroup = init_app_cgroup(&params.name, pid, &params.cgroup);

            let ret =
                unsafe { libc::syscall(libc::SYS_pidfd_open, pid, libc::PIDFD_NONBLOCK) } as c_int;
            let pidfd = to_ioresult(ret).map_err(|e| {
                log::error!("pidfd_open failed: {}", e);
                AppRuntimeError::Io(e)
            })?;

            let stdout = pipe_stdout
                .into_nonblocking_read_fd()
                .expect("nonblocking stdout");
            let stderr = pipe_stderr
                .into_nonblocking_read_fd()
                .expect("nonblocking stderr");

            let mut pidfd_sourcefd = SourceFd(&pidfd);
            let pidfd_token = token_slab
                .allocate()
                .ok_or(AppRuntimeError::TokenAllocation)?;
            poll.registry()
                .register(&mut pidfd_sourcefd, pidfd_token, mio::Interest::READABLE)?;

            let stdout_token = token_slab
                .allocate()
                .ok_or(AppRuntimeError::TokenAllocation)?;
            let mut stdout_sourcefd = SourceFd(&stdout.as_raw_fd());
            poll.registry().register(
                &mut stdout_sourcefd,
                stdout_token,
                mio::Interest::READABLE,
            )?;

            let stderr_token = token_slab
                .allocate()
                .ok_or(AppRuntimeError::TokenAllocation)?;
            let mut stderr_sourcefd = SourceFd(&stderr.as_raw_fd());
            poll.registry().register(
                &mut stderr_sourcefd,
                stderr_token,
                mio::Interest::READABLE,
            )?;

            Ok(AppRuntime {
                pid,
                cgroup,
                stdout,
                stderr,
                pidfd,
                tokens: AppRtTokens {
                    pidfd: pidfd_token,
                    stdout: stdout_token,
                    stderr: stderr_token,
                },
                proper_cleanup: false,
            })
        } else {
            Err(AppRuntimeError::ForkFailed(ret))
        }
    }

    fn stop(mut self, poll: &mio::Poll, token_slab: &mut MioTokenSlab) -> Result<(), AppRuntimeError> {
        let mut pidfd_sourcefd = SourceFd(&self.pidfd);
        poll.registry().deregister(&mut pidfd_sourcefd)?;
        token_slab.free(self.tokens.pidfd);

        let mut stdout_sourcefd = SourceFd(&self.stdout.as_raw_fd());
        poll.registry().deregister(&mut stdout_sourcefd)?;
        token_slab.free(self.tokens.stdout);

        let mut stderr_sourcefd = SourceFd(&self.stderr.as_raw_fd());
        poll.registry().deregister(&mut stderr_sourcefd)?;
        token_slab.free(self.tokens.stderr);

        // pipe fds are automatically closed

        if let Err(err) = self.cgroup.delete() {
            log::error!("Failed to delete app cgroup: {}", err);
        }

        self.proper_cleanup = true;

        Ok(())
    }

    pub fn send_sigterm(&self) -> io::Result<()> {
        let ret = unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGTERM) };
        log::info!("app {} kill -> {}", self.pid, ret);
        to_ioresult(ret)?;
        Ok(())
    }

    pub fn send_sigkill(&self) -> io::Result<()> {
        let ret = unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGKILL) };
        log::info!("app {} kill -> {}", self.pid, ret);
        if let Err(err) = to_ioresult(ret) {
            log::error!("Failed to send SIGKILL to app {}: {}", self.pid, err);
        }

        Ok(())
    }
}

impl Drop for AppRuntime {
    fn drop(&mut self) {
        if !self.proper_cleanup {
            panic!("AppRuntime dropped without proper cleanup");
        }
    }
}

#[derive(Debug)]
enum State {
    Running(AppRuntime),
    Terminated(ReturnState),
}

impl State {
    fn as_runtime(&self) -> Option<&AppRuntime> {
        if let State::Running(rt) = self {
            Some(rt)
        } else {
            None
        }
    }

    fn is_running(&self) -> bool {
        matches!(self, State::Running(..))
    }
}

fn fd_dup(src: impl AsRawFd, dst: impl AsRawFd) -> io::Result<()> {
    let ret = unsafe { dup2(src.as_raw_fd(), dst.as_raw_fd()) };
    to_ioresult(ret)?;
    Ok(())
}

use std::{
    ffi::CString,
    fmt::Display,
    io::{self, Error, Read},
    os::fd::AsRawFd,
    str::FromStr,
};

use cgroups_rs::fs::Cgroup;
use libc::{
    c_char, dup2, pid_t, waitpid, STDERR_FILENO, STDOUT_FILENO, WEXITSTATUS, WIFEXITED,
    WIFSIGNALED, WNOHANG, WTERMSIG,
};
use thiserror::Error;

use crate::{
    cgroups::{init_app_cgroup, AppCgroupConfig},
    logger::Logger,
    pipe::{Pipe, PipeReader},
    support::signal::signal_name,
    support::to_ioresult,
};

#[derive(Debug, Error)]
pub enum AppErr {
    #[error("Runtime error: {0}")]
    Runtime(#[from] AppRuntimeError),
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

pub struct App {
    name: String,
    params: AppParams,
    state: State,
}

impl Drop for AppRuntime {
    fn drop(&mut self) {
        // pipe fds are automatically closed

        if let Err(err) = self.cgroup.delete() {
            log::error!("Failed to delete app cgroup: {}", err);
        }
    }
}

impl App {
    pub fn start(params: AppParams) -> Result<Self, AppErr> {
        let runtime = AppRuntime::new(&params)?;

        Ok(App {
            name: params.name.to_string(),
            state: State::Running(runtime),
            params,
        })
    }

    pub fn restart(&mut self) -> Result<(), AppErr> {
        if let State::Terminated(_) = self.state {
            let runtime = AppRuntime::new(&self.params)?;
            self.state = State::Running(runtime);
        }
        Ok(())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn pid(&self) -> Option<u32> {
        self.state.as_runtime().map(|rt| rt.pid)
    }

    pub fn status_string(&self) -> String {
        match &self.state {
            State::Running(_) => "running".to_string(),
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

    pub fn poll(&mut self, logger: &dyn Logger, try_restart: bool) {
        // trick: `State::poll` consumes `self`, so swap in a placeholder to move the real
        // state out of the `&mut self` reference.
        let placeholder = State::Terminated(ReturnState::Abnormal);
        self.state = std::mem::replace(&mut self.state, placeholder).poll(&self.name, logger);

        // TODO evaluate the restart of the application
        if try_restart && !self.params.oneshot {
            if let Err(err) = self.restart() {
                log::error!("Failed to restart app {}: {}", self.name, err);
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.state.is_running()
    }

    pub fn sigterm(&self) -> io::Result<()> {
        self.state
            .as_runtime()
            .map_or(Ok(()), |app_runtime| app_runtime.sigterm())
    }

    pub fn terminate(&self) -> io::Result<()> {
        self.state
            .as_runtime()
            .map_or(Ok(()), |app_runtime| app_runtime.terminate())
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

#[derive(Debug, Error)]
pub enum AppRuntimeError {
    #[error("Fork failed {0}")]
    ForkFailed(i32),
    #[error("Execv failed {0}")]
    ExecvFailed(Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
struct AppRuntime {
    pid: u32,
    stdout: PipeReader,
    stderr: PipeReader,
    cgroup: Cgroup,
}

impl State {
    fn poll(self, app_name: &str, logger: &dyn Logger) -> Self {
        match self {
            State::Running(rt) => rt.poll(app_name, logger),
            State::Terminated(_) => self,
        }
    }

    fn as_runtime(&self) -> Option<&AppRuntime> {
        if let State::Running(rt) = self {
            Some(rt)
        } else {
            None
        }
    }

    fn as_runtime_mut(&mut self) -> Option<&mut AppRuntime> {
        if let State::Running(ref mut rt) = self {
            Some(rt)
        } else {
            None
        }
    }

    fn is_running(&self) -> bool {
        matches!(self, State::Running(_))
    }
}

fn fd_dup(src: impl AsRawFd, dst: impl AsRawFd) -> io::Result<()> {
    let ret = unsafe { dup2(src.as_raw_fd(), dst.as_raw_fd()) };
    to_ioresult(ret)?;
    Ok(())
}
#[derive(Debug)]
pub enum State {
    Running(AppRuntime),
    Terminated(ReturnState),
}

impl AppRuntime {
    fn new(params: &AppParams) -> Result<Self, AppRuntimeError> {
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
            let pid = ret as u32;
            let cgroup = init_app_cgroup(&params.name, pid, &params.cgroup);

            // parent
            log::info!("Child pid: {}", ret);

            Ok(AppRuntime {
                pid,
                stdout: pipe_stdout
                    .into_nonblocking_read_fd()
                    .expect("nonblocking stdout"),
                stderr: pipe_stderr
                    .into_nonblocking_read_fd()
                    .expect("nonblocking stderr"),
                cgroup,
            })
        } else {
            Err(AppRuntimeError::ForkFailed(ret))
        }
    }

    fn poll(mut self, name: &str, logger: &dyn Logger) -> State {
        // Read stdout and stderr
        let mut buf = vec![0u8; 1024];
        if let Ok(rcvd) = self.stdout.read(&mut buf) {
            log::debug!("app {} rcvd {} bytes from stdout", self.pid, rcvd);
            logger
                .log(name, self.pid, &buf[..rcvd])
                .expect("log stdout");
        } else {
            log::debug!(
                "app {} no data from stdout: {}",
                self.pid,
                std::io::Error::last_os_error()
            );
        }

        if let Ok(rcvd) = self.stderr.read(&mut buf) {
            log::debug!("app {} rcvd {} bytes from stderr", self.pid, rcvd);
            logger
                .log(name, self.pid, &buf[..rcvd])
                .expect("log stderr");
            log::debug!(
                "app {} no data from stderr: {}",
                self.pid,
                std::io::Error::last_os_error()
            );
        }

        // Poll state
        let mut status: i32 = 0;
        let options: i32 = WNOHANG;
        let ret = unsafe { waitpid(self.pid as pid_t, &mut status, options) };
        log::debug!("app: {} waitpid -> {} status: {}", self.pid, ret, status);

        if ret == self.pid as pid_t {
            // children exited

            let signaled = WIFSIGNALED(status);
            if signaled {
                let termsig = WTERMSIG(status);
                log::info!(
                    "app {} terminated by signal {} ({})",
                    self.pid,
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
            log::info!("app {} returned {:?}", self.pid, return_state);
            State::Terminated(return_state)
        } else if ret == 0 {
            // waiting
            State::Running(self)
        } else {
            panic!("waitpid failed")
        }
    }

    pub fn sigterm(&self) -> io::Result<()> {
        let ret = unsafe { libc::kill(self.pid as pid_t, libc::SIGTERM) };
        log::info!("app {} kill -> {}", self.pid, ret);
        to_ioresult(ret)?;
        Ok(())
    }

    pub fn terminate(&self) -> io::Result<()> {
        let ret = unsafe { libc::kill(self.pid as pid_t, libc::SIGKILL) };
        log::info!("app {} kill -> {}", self.pid, ret);
        if let Err(err) = to_ioresult(ret) {
            log::error!("Failed to send SIGKILL to app {}: {}", self.pid, err);
        }

        Ok(())
    }
}

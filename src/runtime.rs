use std::{
    ffi::{c_int, CString},
    fmt::Display,
    io::{self, Error, Read},
    os::fd::{AsRawFd, RawFd},
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, Instant},
};

use cgroups_rs::fs::Cgroup;
use libc::{
    c_char, dup2, pid_t, waitpid, STDERR_FILENO, STDOUT_FILENO, WEXITSTATUS, WIFEXITED,
    WIFSIGNALED, WNOHANG, WTERMSIG,
};
use mio::unix::SourceFd;
use thiserror::Error;

use crate::{
    cgroups::{init_app_cgroup, read_cgroup_usage, AppCgroupConfig, CgroupUsage},
    logger::Logger,
    restart::RestartPolicy,
    stats::AppStats,
    support::{
        mio_token_slab::MioTokenSlab,
        pipe::{Pipe, PipeReader},
        signal::signal_name,
        to_ioresult,
        uidgid::set_current_cwd,
    },
};

const STDIO_BUFFER_SIZE: usize = 4096;

#[derive(Debug, Error)]
pub enum AppErr {
    #[error("Runtime error: {0}")]
    Runtime(#[from] AppRuntimeError),
    #[error("App already running")]
    AlreadyRunning,
}

// remake <'a>
#[derive(Debug, Clone, PartialEq)]
pub struct AppParams {
    pub cwd: PathBuf,
    pub name: String,
    pub prog: String,
    pub args: Vec<String>,
    pub uid: u32,
    pub gid: u32,
    pub env: Vec<String>,
    pub oneshot: bool,
    pub cgroup: AppCgroupConfig,
    pub autostart: bool, // App automatically starts on creation
    pub restart: RestartPolicy,
}

#[derive(Debug)]
struct AppRtTokens {
    pub pidfd: mio::Token,
    pub stdout: mio::Token,
    pub stderr: mio::Token,
}

#[derive(Debug, Default)]
struct AppRuntimeStats {
    restart_count: u32,
    consecutive_failures: u32,
    last_exit_code: Option<i32>,
    last_exit_reason: Option<String>,
    stdout_bytes: u64,
    stderr_bytes: u64,
    /// Sum of the durations of all completed runs, excluding the current one.
    total_uptime: Duration,
}

pub struct App {
    name: String,
    params: AppParams,
    state: State,
    runtime_stats: AppRuntimeStats,
}

impl App {
    pub fn create(
        params: AppParams,
        poll: &mio::Poll,
        token_slab: &mut MioTokenSlab,
    ) -> Result<Self, AppErr> {
        let state = match params.autostart {
            true => {
                let runtime = AppRuntime::start(&params, &poll, token_slab)?;
                State::Running(runtime)
            }
            false => State::Stopped,
        };

        Ok(App {
            name: params.name.to_string(),
            state,
            params,
            runtime_stats: AppRuntimeStats::default(),
        })
    }

    pub fn start(&mut self, poll: &mio::Poll, token_slab: &mut MioTokenSlab) -> Result<(), AppErr> {
        if let State::Stopped = self.state {
            let runtime = AppRuntime::start(&self.params, &poll, token_slab)?;
            self.state = State::Running(runtime);
            self.runtime_stats.consecutive_failures = 0;
            Ok(())
        } else {
            Err(AppErr::AlreadyRunning)
        }
    }

    /// Deadline at which a scheduled restart should be attempted, if one is pending.
    pub fn pending_restart_deadline(&self) -> Option<Instant> {
        match &self.state {
            State::PendingRestart { deadline, .. } => Some(*deadline),
            _ => None,
        }
    }

    /// Attempts a restart scheduled by `schedule_restart` once its delay has elapsed.
    /// If `allow_restart` is false (e.g. the runtime is shutting down), the pending restart is
    /// cancelled and the app settles into `Terminated` instead.
    pub fn maybe_restart(
        &mut self,
        now: Instant,
        poll: &mio::Poll,
        token_slab: &mut MioTokenSlab,
        allow_restart: bool,
    ) {
        let (deadline, cause) = match &self.state {
            State::PendingRestart { deadline, cause } => (*deadline, *cause),
            _ => return,
        };

        if !allow_restart {
            self.state = State::Terminated(cause);
            return;
        }

        if now < deadline {
            return;
        }

        match AppRuntime::start(&self.params, poll, token_slab) {
            Ok(runtime) => {
                self.runtime_stats.restart_count += 1;
                self.state = State::Running(runtime);
            }
            Err(err) => {
                log::error!("Failed to restart app {}: {}", self.name, err);
                self.state = State::Terminated(cause);
            }
        }
    }

    /// Applies the app's restart policy to decide the delay (or give-up) after `return_state`,
    /// given how long the app ran (`uptime`) before exiting.
    fn schedule_restart(&mut self, return_state: ReturnState, uptime: Duration) {
        let policy = &self.params.restart;
        let success = matches!(return_state, ReturnState::Completed { ret: 0 });

        if success || uptime >= Duration::from_millis(policy.reset_after_ms) {
            self.runtime_stats.consecutive_failures = 0;
        }
        if !success {
            self.runtime_stats.consecutive_failures += 1;
        }

        if let Some(max) = policy.max_restart_attempts {
            if self.runtime_stats.consecutive_failures > max {
                log::error!(
                    "app {} reached max restart attempts ({}), giving up",
                    self.name,
                    max
                );
                return;
            }
        }

        let strategy = if success {
            &policy.on_success
        } else {
            &policy.on_error
        };
        if strategy.is_never() {
            log::info!(
                "app {} restart policy is 'never' for this outcome, staying stopped",
                self.name
            );
            return;
        }

        let delay = strategy.delay_for(self.runtime_stats.consecutive_failures.max(1));
        log::info!("app {} will restart in {:?}", self.name, delay);
        self.state = State::PendingRestart {
            deadline: Instant::now() + delay,
            cause: return_state,
        };
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn pid(&self) -> Option<u32> {
        self.state.as_runtime().map(|rt| rt.pid as u32)
    }

    pub fn status_string(&self) -> String {
        match &self.state {
            State::Stopped => "stopped".to_string(),
            State::Running(..) => "running".to_string(),
            State::PendingRestart { .. } => "restarting".to_string(),
            State::Terminated(ReturnState::Completed { ret }) => format!("exited({})", ret),
            State::Terminated(ReturnState::Abnormal { signal }) => format!(
                "abnormal(signal {})",
                signal_name(*signal as usize).unwrap_or("UNKNOWN")
            ),
        }
    }

    pub fn is_oneshot(&self) -> bool {
        self.params.oneshot
    }

    pub fn uid(&self) -> u32 {
        self.params.uid
    }

    pub fn gid(&self) -> u32 {
        self.params.gid
    }

    pub fn cwd(&self) -> &Path {
        &self.params.cwd
    }

    /// Full command line (program + args) as it was launched.
    pub fn command(&self) -> String {
        self.params.args.join(" ")
    }

    pub fn cgroup_config(&self) -> &AppCgroupConfig {
        &self.params.cgroup
    }

    pub fn stats(&self) -> AppStats {
        let (uptime, cgroup_usage) = match &self.state {
            State::Running(rt) => (Some(rt.started_at.elapsed()), read_cgroup_usage(&rt.cgroup)),
            _ => (None, CgroupUsage::default()),
        };

        AppStats {
            uptime,
            total_uptime: self.runtime_stats.total_uptime,
            restart_count: self.runtime_stats.restart_count,
            last_exit_code: self.runtime_stats.last_exit_code,
            last_exit_reason: self.runtime_stats.last_exit_reason.clone(),
            cpu_usage_usec: cgroup_usage.cpu_usage_usec,
            memory_current: cgroup_usage.memory_current,
            memory_peak: cgroup_usage.memory_peak,
            io_read_bytes: cgroup_usage.io_read_bytes,
            io_write_bytes: cgroup_usage.io_write_bytes,
            stdout_bytes: self.runtime_stats.stdout_bytes,
            stderr_bytes: self.runtime_stats.stderr_bytes,
        }
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
        // TODO replace this buffer to mmap per AppRuntime to avoid reallocating each loop
        let mut buf = vec![0u8; STDIO_BUFFER_SIZE];

        if token == rt.tokens.stdout {
            if event.is_readable() {
                if let Ok(rcvd) = rt.stdout.read(&mut buf) {
                    log::debug!("app {} rcvd {} bytes from stdout", rt.pid, rcvd);
                    self.runtime_stats.stdout_bytes += rcvd as u64;
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
                    self.runtime_stats.stderr_bytes += rcvd as u64;
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
                self.runtime_stats.last_exit_code = match return_state {
                    ReturnState::Completed { ret } => Some(ret),
                    ReturnState::Abnormal { .. } => None,
                };
                self.runtime_stats.last_exit_reason = Some(match return_state {
                    ReturnState::Completed { ret } => format!("exited({})", ret),
                    ReturnState::Abnormal { signal } => format!(
                        "signal {} ({})",
                        signal_name(signal as usize).unwrap_or("UNKNOWN"),
                        signal
                    ),
                });

                let terminated = State::Terminated(return_state);
                let State::Running(mut rt) = std::mem::replace(&mut self.state, terminated)
                else {
                    unreachable!();
                };

                // The process may have written its last output and closed its
                // pipes right before exiting, so their readable events can land
                // in the same poll batch as this pidfd event. Drain whatever is
                // still buffered now, since rt.stop() below deregisters and
                // closes the pipes, after which those pending events become
                // no-ops (state is no longer Running).
                Self::drain_pipe(
                    &mut rt.stdout,
                    rt.pid,
                    "stdout",
                    &self.name,
                    logger,
                    &mut self.runtime_stats.stdout_bytes,
                );
                Self::drain_pipe(
                    &mut rt.stderr,
                    rt.pid,
                    "stderr",
                    &self.name,
                    logger,
                    &mut self.runtime_stats.stderr_bytes,
                );

                let uptime = rt.started_at.elapsed();
                self.runtime_stats.total_uptime += uptime;

                rt.stop(poll, token_slab)
                    .expect("Failed to stop app runtime");

                self.state = State::Terminated(return_state);

                // evaluate the restart of the application
                if try_restart && !self.params.oneshot {
                    self.schedule_restart(return_state, uptime);
                }
            }
        }
    }

    /// Read a non-blocking pipe until it's empty (EAGAIN) or closed (EOF).
    fn drain_pipe(
        pipe: &mut PipeReader,
        pid: pid_t,
        stream: &str,
        name: &str,
        logger: &dyn Logger,
        byte_count: &mut u64,
    ) {
        let mut buf = [0u8; STDIO_BUFFER_SIZE];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(rcvd) => {
                    log::debug!("app {} drained {} bytes from {}", pid, rcvd, stream);
                    *byte_count += rcvd as u64;
                    logger.log(name, pid, &buf[..rcvd]).expect("log drain");
                }
                Err(_) => break,
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
    Abnormal { signal: i32 }, // Terminated by a signal (call to exit or return from main)
}

/// Non-blocking check of a child's exit status via `waitpid(WNOHANG)`.
///
/// Logs the signal name if the child was terminated by a signal, and the
/// exit code if it exited normally.
///
/// Returns:
/// - `Some(ReturnState::Completed { ret })` if the child exited normally with code `ret`.
/// - `Some(ReturnState::Abnormal)` if the child terminated abnormally (e.g. by a signal).
/// - `None` if the child is still running.
///
/// # Panics
/// Panics if `waitpid` fails (e.g. `pid` is not a valid/reapable child of this process).
fn poll_pid(pid: pid_t) -> Option<ReturnState> {
    // Poll state
    let mut status: i32 = 0;
    let options: i32 = WNOHANG;
    let ret = unsafe { waitpid(pid as libc::pid_t, &mut status, options) };
    log::debug!("app: {} waitpid -> {} status: {}", pid, ret, status);

    if ret == pid as libc::pid_t {
        // children exited

        let signaled = WIFSIGNALED(status);
        let termsig = if signaled {
            let termsig = WTERMSIG(status);
            log::info!(
                "app {} terminated by signal {} ({})",
                pid,
                signal_name(termsig as usize).unwrap_or("UNKNOWN"),
                termsig
            );
            termsig
        } else {
            0
        };

        // parse status
        let normal = WIFEXITED(status);
        let return_state = match normal {
            true => ReturnState::Completed {
                ret: WEXITSTATUS(status),
            },
            false => ReturnState::Abnormal { signal: termsig },
        };
        log::info!("app {} returned {:?}", pid, return_state);
        Some(return_state)
    } else if ret == 0 {
        // child still running
        None
    } else {
        panic!("waitpid failed")
    }
}

#[derive(Debug, Error)]
pub enum AppRuntimeError {
    #[error("Fork failed {0}")]
    ForkFailed(i32),
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
    started_at: Instant,

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
            // execve expects a null-terminated array
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

            let ret = unsafe { libc::setgid(params.gid) };
            to_ioresult(ret).map_err(|e| {
                eprintln!("setgid failed: {}", e);
                AppRuntimeError::Io(e)
            })?;

            let ret = unsafe { libc::setuid(params.uid) };
            to_ioresult(ret).map_err(|e| {
                eprintln!("setuid failed: {}", e);
                AppRuntimeError::Io(e)
            })?;

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
            set_current_cwd(&params.cwd).map_err(|e| {
                eprintln!("chdir failed: {}", e);
                AppRuntimeError::Io(e)
            })?;

            let ret = unsafe {
                libc::execve(prog.as_ptr() as *const c_char, argv.as_ptr(), envp.as_ptr())
            };
            let error = std::io::Error::last_os_error();
            eprintln!("execve returned {} errno: {}", ret, error,);
            
            // Exit the child process with failure status
            unsafe { libc::exit(libc::EXIT_FAILURE) }
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
                started_at: Instant::now(),
                proper_cleanup: false,
            })
        } else {
            Err(AppRuntimeError::ForkFailed(ret))
        }
    }

    fn stop(
        mut self,
        poll: &mio::Poll,
        token_slab: &mut MioTokenSlab,
    ) -> Result<(), AppRuntimeError> {
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
    Stopped,
    Running(AppRuntime),
    PendingRestart {
        deadline: Instant,
        cause: ReturnState,
    },
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

#[cfg(test)]
mod restart_tests {
    use crate::restart::RestartDelay;

    use super::*;

    fn make_params(restart: RestartPolicy) -> AppParams {
        AppParams {
            cwd: PathBuf::from("."),
            name: "test-app".to_string(),
            prog: "true".to_string(),
            args: vec!["true".to_string()],
            uid: 0,
            gid: 0,
            env: Vec::new(),
            oneshot: false,
            cgroup: AppCgroupConfig {
                cpu_weight: None,
                io_weight: None,
                memory_hard_limit: None,
                memory_soft_limit: None,
                memory_swap_limit: None,
            },
            autostart: false,
            restart,
        }
    }

    fn make_app(restart: RestartPolicy) -> App {
        App {
            name: "test-app".to_string(),
            params: make_params(restart),
            state: State::Stopped,
            runtime_stats: AppRuntimeStats::default(),
        }
    }

    fn pending_deadline(app: &App) -> Instant {
        match &app.state {
            State::PendingRestart { deadline, .. } => *deadline,
            other => panic!("expected PendingRestart, got {:?}", other),
        }
    }

    #[test]
    fn schedule_restart_uses_success_or_error_strategy() {
        let policy = RestartPolicy {
            on_success: RestartDelay::Constant { delay_ms: 100 },
            on_error: RestartDelay::Constant { delay_ms: 500 },
            reset_after_ms: 60_000,
            max_restart_attempts: None,
        };
        let mut app = make_app(policy);

        let before = Instant::now();
        app.schedule_restart(ReturnState::Completed { ret: 0 }, Duration::from_secs(5));
        let deadline = pending_deadline(&app);
        assert!(deadline >= before + Duration::from_millis(100));
        assert!(deadline < before + Duration::from_millis(500));
        assert_eq!(app.runtime_stats.consecutive_failures, 0);

        app.state = State::Stopped;
        let before = Instant::now();
        app.schedule_restart(ReturnState::Completed { ret: 1 }, Duration::from_secs(5));
        let deadline = pending_deadline(&app);
        assert!(deadline >= before + Duration::from_millis(500));
        assert_eq!(app.runtime_stats.consecutive_failures, 1);
    }

    #[test]
    fn restart_on_failure_only_stops_after_success() {
        let policy = RestartPolicy {
            on_success: RestartDelay::Never,
            on_error: RestartDelay::Constant { delay_ms: 100 },
            reset_after_ms: 60_000,
            max_restart_attempts: None,
        };
        let mut app = make_app(policy);

        // Fails: gets scheduled for restart.
        app.schedule_restart(ReturnState::Completed { ret: 1 }, Duration::from_secs(1));
        assert!(matches!(app.state, State::PendingRestart { .. }));

        // Succeeds: stays put, no restart is scheduled.
        app.state = State::Terminated(ReturnState::Completed { ret: 0 });
        app.schedule_restart(ReturnState::Completed { ret: 0 }, Duration::from_secs(1));
        assert!(matches!(
            app.state,
            State::Terminated(ReturnState::Completed { ret: 0 })
        ));
    }

    #[test]
    fn consecutive_failures_reset_on_success() {
        let policy = RestartPolicy {
            on_success: RestartDelay::default(),
            on_error: RestartDelay::ExponentialBackoff {
                initial_delay_ms: 10,
                max_delay_ms: 1000,
                multiplier: 2.0,
            },
            reset_after_ms: 60_000,
            max_restart_attempts: None,
        };
        let mut app = make_app(policy);

        app.schedule_restart(
            ReturnState::Abnormal { signal: 6 },
            Duration::from_millis(10),
        );
        assert_eq!(app.runtime_stats.consecutive_failures, 1);
        app.state = State::Stopped;

        app.schedule_restart(
            ReturnState::Abnormal { signal: 6 },
            Duration::from_millis(10),
        );
        assert_eq!(app.runtime_stats.consecutive_failures, 2);
        app.state = State::Stopped;

        // A clean exit resets the failure streak.
        app.schedule_restart(ReturnState::Completed { ret: 0 }, Duration::from_secs(1));
        assert_eq!(app.runtime_stats.consecutive_failures, 0);
    }

    #[test]
    fn consecutive_failures_reset_after_long_enough_uptime() {
        let policy = RestartPolicy {
            on_success: RestartDelay::default(),
            on_error: RestartDelay::default(),
            reset_after_ms: 1_000,
            max_restart_attempts: None,
        };
        let mut app = make_app(policy);

        app.schedule_restart(
            ReturnState::Completed { ret: 1 },
            Duration::from_millis(500),
        );
        assert_eq!(app.runtime_stats.consecutive_failures, 1);
        app.state = State::Stopped;

        // Ran longer than reset_after_ms before crashing again: streak resets, then this
        // failure brings it back to 1 rather than 2.
        app.schedule_restart(
            ReturnState::Completed { ret: 1 },
            Duration::from_millis(2_000),
        );
        assert_eq!(app.runtime_stats.consecutive_failures, 1);
    }

    #[test]
    fn max_restart_attempts_stops_scheduling_further_restarts() {
        let policy = RestartPolicy {
            on_success: RestartDelay::default(),
            on_error: RestartDelay::Constant { delay_ms: 0 },
            reset_after_ms: 60_000,
            max_restart_attempts: Some(2),
        };
        let mut app = make_app(policy);

        app.schedule_restart(ReturnState::Completed { ret: 1 }, Duration::ZERO);
        assert!(matches!(app.state, State::PendingRestart { .. }));
        app.state = State::Stopped;

        app.schedule_restart(ReturnState::Completed { ret: 1 }, Duration::ZERO);
        assert!(matches!(app.state, State::PendingRestart { .. }));
        app.state = State::Stopped;

        // Third consecutive failure exceeds max_restart_attempts: give up, no more restarts.
        app.schedule_restart(ReturnState::Completed { ret: 1 }, Duration::ZERO);
        assert_eq!(app.runtime_stats.consecutive_failures, 3);
        assert!(matches!(app.state, State::Stopped));
    }

    #[test]
    fn maybe_restart_waits_for_its_deadline() {
        let policy = RestartPolicy {
            on_success: RestartDelay::default(),
            on_error: RestartDelay::Constant { delay_ms: 60_000 },
            reset_after_ms: 60_000,
            max_restart_attempts: None,
        };
        let mut app = make_app(policy);
        app.schedule_restart(ReturnState::Abnormal { signal: 9 }, Duration::ZERO);
        let deadline = app.pending_restart_deadline().expect("pending restart");

        let poll = mio::Poll::new().expect("create poll");
        let mut token_slab = MioTokenSlab::new(8, 0);

        // Deadline is 60s out: nothing should happen yet.
        app.maybe_restart(Instant::now(), &poll, &mut token_slab, true);
        assert_eq!(app.pending_restart_deadline(), Some(deadline));
    }

    #[test]
    fn maybe_restart_cancelled_on_shutdown() {
        let policy = RestartPolicy {
            on_success: RestartDelay::default(),
            on_error: RestartDelay::Constant { delay_ms: 60_000 },
            reset_after_ms: 60_000,
            max_restart_attempts: None,
        };
        let mut app = make_app(policy);
        app.schedule_restart(ReturnState::Abnormal { signal: 9 }, Duration::ZERO);
        assert!(app.pending_restart_deadline().is_some());

        let poll = mio::Poll::new().expect("create poll");
        let mut token_slab = MioTokenSlab::new(8, 0);

        app.maybe_restart(Instant::now(), &poll, &mut token_slab, false);
        assert!(app.pending_restart_deadline().is_none());
        assert!(matches!(
            app.state,
            State::Terminated(ReturnState::Abnormal { signal: 9 })
        ));
    }
}

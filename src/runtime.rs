use std::{
    ffi::{c_int, CString},
    fmt::Display,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, Instant},
};

use cgroups_rs::fs::Cgroup;
use libc::{
    c_char, pid_t, waitpid, STDERR_FILENO, STDOUT_FILENO, WEXITSTATUS, WIFEXITED, WIFSIGNALED,
    WNOHANG, WTERMSIG,
};
use mio::unix::SourceFd;
use thiserror::Error;

use crate::{
    cgroups::{
        init_app_cgroup, read_cgroup_usage, spawn_into_cgroup, wait_for_cgroup_empty,
        AppCgroupConfig, CgroupUsage,
    },
    logger::{LogBuffer, LogChunk, LoggerSimple, StdoutLogger, ViewableLogBuffer},
    restart::RestartPolicy,
    stats::AppStats,
    support::{
        log_buffer::{CircularMappedMemFdBuffer, MemFdBuffer, DEFAULT_LOG_BUFFER_SIZE},
        mio_token_slab::MioTokenSlab,
        pipe::{Pipe, PipeReader},
        signal::signal_name,
        to_ioresult,
        uidgid::set_current_cwd,
    },
};

const SPLICE_MAX_XFER_SIZE: usize = 16384;

#[derive(Debug, Error)]
pub enum AppErr {
    #[error("Runtime error: {0}")]
    Runtime(#[from] AppRuntimeError),
    #[error("App already running")]
    AlreadyRunning,
    #[error("App not running")]
    NotRunning,
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
    /// Duration of the most recently completed run, if any.
    last_run_duration: Option<Duration>,
}

pub struct App {
    name: String,
    params: AppParams,
    state: State,
    runtime_stats: AppRuntimeStats,
    /// Set by `stop()` to suppress auto-restart of an app the user explicitly stopped.
    stop_requested: bool,
    log_buffer: Box<dyn ViewableLogBuffer>,
    /// When set, every byte spliced into `log_buffer` is also immediately printed
    /// to the daemon's own stdout/stderr (mirroring the app's own stream).
    echo_logs: bool,
}

impl App {
    pub fn create(
        params: AppParams,
        poll: &mio::Poll,
        token_slab: &mut MioTokenSlab,
        echo_logs: bool,
    ) -> Result<Self, AppErr> {
        let state = match params.autostart {
            true => {
                let runtime = AppRuntime::start(&params, poll, token_slab)?;
                State::Running(runtime)
            }
            false => State::default(),
        };

        let memfd_buffer =
            MemFdBuffer::new_sealed(&format!("logbuf-{}", params.name), DEFAULT_LOG_BUFFER_SIZE)
                .expect("Failed to create memfd buffer");
        let circ_buffer = CircularMappedMemFdBuffer::new_from_memfd_buffer(memfd_buffer)
            .expect("Failed to create circular log buffer");

        Ok(App {
            name: params.name.to_string(),
            state,
            params,
            runtime_stats: AppRuntimeStats::default(),
            stop_requested: false,
            log_buffer: Box::new(circ_buffer),
            echo_logs,
        })
    }

    pub fn start(&mut self, poll: &mio::Poll, token_slab: &mut MioTokenSlab) -> Result<(), AppErr> {
        match self.state {
            State::Stopped { .. } => {
                let runtime = AppRuntime::start(&self.params, poll, token_slab)?;
                self.state = State::Running(runtime);
                self.runtime_stats.consecutive_failures = 0;
                self.stop_requested = false;
                Ok(())
            }
            State::Running(..) => Err(AppErr::AlreadyRunning),
        }
    }

    /// Stops a running (or pending-restart) app: sends SIGTERM, or SIGKILL if `force` is set.
    /// Also cancels any pending restart and suppresses auto-restart once the app exits.
    ///
    /// Returns `Ok(true)` if the app was running and is now being stopped.
    /// Returns `Ok(false)` if the app was not running.
    /// Returns an `Err(AppErr)` if there was an error sending the termination signal.
    pub fn stop(&mut self, force: bool) -> Result<bool, AppErr> {
        match &self.state {
            State::Running(rt) => {
                let result = if force {
                    rt.send_sigkill()
                } else {
                    rt.send_sigterm()
                };
                self.stop_requested = true;
                result.map_err(AppRuntimeError::from)?;
                Ok(true)
            }
            State::Stopped(Some(LastExecutionInfo {
                cause,
                restart_deadline: Some(_),
            })) => {
                self.state = State::Stopped(Some(LastExecutionInfo {
                    cause: *cause,
                    restart_deadline: None,
                }));
                self.stop_requested = true;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub fn is_running(&self) -> bool {
        self.state.is_running()
    }

    /// Deadline at which a scheduled restart should be attempted, if one is pending.
    pub fn pending_restart_deadline(&self) -> Option<Instant> {
        match &self.state {
            State::Stopped(Some(LastExecutionInfo {
                restart_deadline, ..
            })) => *restart_deadline,
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
            State::Stopped(Some(LastExecutionInfo {
                cause,
                restart_deadline: Some(deadline),
            })) => (*deadline, *cause),
            _ => return,
        };

        if !allow_restart {
            self.state = State::stopped_with_cause(cause);
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
                self.state = State::stopped_with_cause(cause);
            }
        }
    }

    /// Applies the app's restart policy to decide the delay (or give-up) after `return_state`,
    /// given how long the app ran (`uptime`) before exiting.
    fn schedule_restart(&mut self, restart_state: ReturnState, uptime: Duration) {
        let policy = &self.params.restart;
        let success = matches!(restart_state, ReturnState::Completed { ret: 0 });

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
        self.state = State::Stopped(Some(LastExecutionInfo {
            cause: restart_state,
            restart_deadline: Some(Instant::now() + delay),
        }));
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
            State::Stopped(None) => "stopped".to_string(),
            State::Stopped(Some(LastExecutionInfo { cause, .. })) => match cause {
                ReturnState::Abnormal { signal } => format!(
                    "abnormal(signal {})",
                    signal_name(*signal as usize).unwrap_or("UNKNOWN")
                ),
                ReturnState::Completed { ret } => format!("exited({})", ret),
            },
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

    pub fn autostart(&self) -> bool {
        self.params.autostart
    }

    pub fn env(&self) -> &[String] {
        &self.params.env
    }

    pub fn restart_policy(&self) -> &RestartPolicy {
        &self.params.restart
    }

    /// Total bytes ever written to this app's log buffer; used as a follow cursor.
    pub fn log_write_pos(&mut self) -> usize {
        self.log_buffer.write_pos()
    }

    /// Oldest logical offset still retained in the log buffer; bytes before this
    /// were overwritten/evicted.
    pub fn log_start_pos(&mut self) -> usize {
        self.log_buffer.start_pos()
    }

    /// Capacity of the log buffer in bytes, if it's fixed-size (e.g. a ring buffer).
    pub fn log_buffer_capacity(&mut self) -> Option<usize> {
        self.log_buffer.capacity()
    }

    /// Short, human-readable name of the log buffer's concrete implementation.
    pub fn log_buffer_kind(&self) -> &'static str {
        self.log_buffer.kind()
    }

    /// Largest contiguous slice of the log buffer retained from `from` onward (no
    /// UTF-8 decoding, no copy). See `ViewableLogBuffer::read_from_cursor`.
    pub fn log_bytes_from_cursor(&mut self, from: usize) -> LogChunk<'_> {
        self.log_buffer.read_from_cursor(from)
    }

    /// Full cgroupfs path of the app's cgroup, if it's currently running.
    pub fn cgroup_path(&self) -> Option<String> {
        match &self.state {
            State::Running(rt) => Some(format!("/sys/fs/cgroup/{}", rt.cgroup.path())),
            _ => None,
        }
    }

    pub fn stats(&self) -> AppStats {
        let (uptime, cgroup_usage) = match &self.state {
            State::Running(rt) => (Some(rt.started_at.elapsed()), read_cgroup_usage(&rt.cgroup)),
            _ => (None, CgroupUsage::default()),
        };

        AppStats {
            uptime,
            total_uptime: self.runtime_stats.total_uptime,
            last_run_duration: self.runtime_stats.last_run_duration,
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
        try_restart: bool,
    ) {
        let State::Running(rt) = &mut self.state else {
            return;
        };

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Stdio {
            Stdout,
            Stderr,
        }

        impl Display for Stdio {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Stdio::Stdout => write!(f, "stdout"),
                    Stdio::Stderr => write!(f, "stderr"),
                }
            }
        }

        fn get_stdio_fd(token: mio::Token, rt: &AppRuntime) -> Option<(Stdio, RawFd)> {
            if token == rt.tokens.stdout {
                Some((Stdio::Stdout, rt.stdout.as_raw_fd()))
            } else if token == rt.tokens.stderr {
                Some((Stdio::Stderr, rt.stderr.as_raw_fd()))
            } else {
                None
            }
        }

        if let Some((stdio, stdio_fd)) = get_stdio_fd(token, rt) {
            if event.is_readable() {
                // mio uses edge-triggered epoll: keep splicing until the pipe is
                // drained (WouldBlock/EOF), otherwise leftover bytes (e.g. from the
                // ring buffer's per-call wraparound clamp) won't re-arm the event
                loop {
                    match self
                        .log_buffer
                        .splice_from_and_view(stdio_fd, SPLICE_MAX_XFER_SIZE)
                    {
                        Ok(None) => break,
                        Ok(Some(buf)) => {
                            if log::log_enabled!(log::Level::Trace) {
                                hexdump::hexdump(buf);
                            }

                            if self.echo_logs {
                                let _ = StdoutLogger::default().log(
                                    &self.name,
                                    rt.pid,
                                    buf,
                                    stdio == Stdio::Stderr,
                                );
                            }

                            match stdio {
                                Stdio::Stdout => {
                                    self.runtime_stats.stdout_bytes += buf.len() as u64
                                }
                                Stdio::Stderr => {
                                    self.runtime_stats.stderr_bytes += buf.len() as u64
                                }
                            }

                            let buf_len = buf.len();
                            let filling = self.log_buffer.filling();
                            let capacity = self.log_buffer.capacity().unwrap_or(0);
                            log::debug!(
                                "app {} spliced {} bytes from {} to log buffer ({} / {})",
                                rt.pid,
                                buf_len,
                                stdio,
                                filling,
                                capacity,
                            );
                        }
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                        // TODO on splice error, read into a buffer but dismiss the content
                        Err(err) => {
                            log::error!(
                                "app {} failed to splice {} to log buffer: {}",
                                rt.pid,
                                stdio,
                                err,
                            );
                            break;
                        }
                    }
                }
            }

            if event.is_read_closed() {
                log::info!("app {} {} closed", rt.pid, stdio);
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

                let terminated = State::stopped_with_cause(return_state);
                let State::Running(mut rt) = std::mem::replace(&mut self.state, terminated) else {
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
                    self.log_buffer.as_mut(),
                    &mut self.runtime_stats.stdout_bytes,
                );
                Self::drain_pipe(
                    &mut rt.stderr,
                    rt.pid,
                    "stderr",
                    self.log_buffer.as_mut(),
                    &mut self.runtime_stats.stderr_bytes,
                );

                let uptime = rt.started_at.elapsed();
                self.runtime_stats.total_uptime += uptime;
                self.runtime_stats.last_run_duration = Some(uptime);

                rt.stop(poll, token_slab)
                    .expect("Failed to stop app runtime");

                // evaluate the restart of the application
                if try_restart && !self.params.oneshot && !self.stop_requested {
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
        log_buffer: &mut dyn LogBuffer,
        byte_count: &mut u64,
    ) {
        loop {
            match log_buffer.splice_from(pipe.as_raw_fd(), SPLICE_MAX_XFER_SIZE) {
                Ok(0) => break,
                Ok(rcvd) => {
                    log::debug!("app {} spliced {} bytes from {}", pid, rcvd, stream);
                    *byte_count += rcvd as u64;
                }
                Err(_) => break,
            }
        }
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
        panic!("waitpid failed ret: {}", ret)
    }
}

#[derive(Debug, Error)]
pub enum AppRuntimeError {
    #[error("Fork failed {0}")]
    ForkFailed(std::io::Error),
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
    pidfd: OwnedFd,
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
        let pipe_stdout = Pipe::new().expect("pipe stdout");
        let pipe_stderr = Pipe::new().expect("pipe stderr");

        let cgroup = init_app_cgroup(&params.name, &params.cgroup);
        let cgroup_full_path = format!("/sys/fs/cgroup/{}", cgroup.path()); // TODO find a better way to obtain this full path

        let pid = spawn_into_cgroup(&cgroup_full_path).map_err(AppRuntimeError::ForkFailed)?;
        if pid == 0 {
            // child
            fn setup_child(
                params: &AppParams,
                pipe_stdout: Pipe,
                pipe_stderr: Pipe,
            ) -> Result<(), String> {
                let stdout = pipe_stdout
                    .into_write_fd()
                    .map_err(|e| format!("stdout into_write_fd failed: {}", e))?;
                fd_dup(stdout, STDOUT_FILENO)
                    .map_err(|e| format!("fd_dup stdout failed: {}", e))?;

                let stderr = pipe_stderr
                    .into_write_fd()
                    .map_err(|e| format!("stderr into_write_fd failed: {}", e))?;
                fd_dup(stderr, STDERR_FILENO)
                    .map_err(|e| format!("fd_dup stderr failed: {}", e))?;

                let prog = CString::from_str(&params.prog)
                    .map_err(|e| format!("prog into CString failed: {}", e))?;
                let args: Vec<CString> = params
                    .args
                    .iter()
                    .map(|arg| {
                        CString::from_str(arg)
                            .map_err(|e| format!("arg into CString failed: {}", e))
                    })
                    .collect::<Result<Vec<CString>, String>>()?;
                let mut argv: Vec<*const c_char> = args
                    .iter()
                    .map(|cstring| cstring.as_ptr() as *const c_char)
                    .collect();
                // execve expects a null-terminated array
                argv.push(std::ptr::null());

                let ret = unsafe { libc::setsid() };
                to_ioresult(ret).map_err(|e| format!("setsid failed: {}", e))?;

                // setgid must happen before setuid: once uid is dropped, permission to
                // change gid is lost.

                let ret = unsafe { libc::setgid(params.gid) };
                to_ioresult(ret).map_err(|e| format!("setgid failed: {}", e))?;

                let ret = unsafe { libc::setuid(params.uid) };
                to_ioresult(ret).map_err(|e| format!("setuid failed: {}", e))?;

                // Build environment variables
                let env: Vec<CString> = params
                    .env
                    .iter()
                    .map(|env| {
                        CString::from_str(env)
                            .map_err(|e| format!("env into CString failed: {}", e))
                    })
                    .collect::<Result<Vec<CString>, String>>()?;
                let mut envp: Vec<*const c_char> = env
                    .iter()
                    .map(|cstring| cstring.as_ptr() as *const c_char)
                    .collect();
                envp.push(std::ptr::null());

                // Set working directory if specified
                set_current_cwd(&params.cwd).map_err(|e| format!("chdir failed: {}", e))?;

                let _ret = unsafe {
                    libc::execve(prog.as_ptr() as *const c_char, argv.as_ptr(), envp.as_ptr())
                };
                let error = std::io::Error::last_os_error();
                Err(format!("execve failed: {}", error))
            }

            if let Err(e) = setup_child(params, pipe_stdout, pipe_stderr) {
                eprintln!("{}", e);
            }
            unsafe { libc::exit(libc::EXIT_FAILURE) }
        } else {
            // parent process
            let ret =
                unsafe { libc::syscall(libc::SYS_pidfd_open, pid, libc::PIDFD_NONBLOCK) } as c_int;
            let pidfd_raw = to_ioresult(ret).map_err(|e| {
                log::error!("pidfd_open failed: {}", e);
                AppRuntimeError::Io(e)
            })?;

            // wrap in OwnedFd so it's closed on drop, unlike a bare RawFd
            let pidfd = unsafe { OwnedFd::from_raw_fd(pidfd_raw) };

            let stdout = pipe_stdout
                .into_nonblocking_read_fd()
                .expect("nonblocking stdout");
            let stderr = pipe_stderr
                .into_nonblocking_read_fd()
                .expect("nonblocking stderr");

            let mut pidfd_sourcefd = SourceFd(&pidfd.as_raw_fd());
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
        }
    }

    fn stop(
        mut self,
        poll: &mio::Poll,
        token_slab: &mut MioTokenSlab,
    ) -> Result<(), AppRuntimeError> {
        let registry = poll.registry();
        let mut pidfd_sourcefd = SourceFd(&self.pidfd.as_raw_fd());
        registry.deregister(&mut pidfd_sourcefd)?;
        token_slab.free(self.tokens.pidfd);

        let mut stdout_sourcefd = SourceFd(&self.stdout.as_raw_fd());
        registry.deregister(&mut stdout_sourcefd)?;
        token_slab.free(self.tokens.stdout);

        let mut stderr_sourcefd = SourceFd(&self.stderr.as_raw_fd());
        registry.deregister(&mut stderr_sourcefd)?;
        token_slab.free(self.tokens.stderr);

        // pidfd/pipe fds are OwnedFd, automatically closed on drop

        // The tracked pid exited, but it may have left descendants behind (e.g. a wrapper
        // script's child, or a process that double-forked/setsid'd to daemonize itself).
        // Those aren't reachable by the process-group signals sent to the tracked pid, so
        // kill the whole cgroup to make sure nothing from this app survives it.
        if let Err(err) = self.cgroup.kill() {
            log::warn!("Failed to cgroup-kill app {}: {}", self.pid, err);
        } else if !wait_for_cgroup_empty(&self.cgroup, Duration::from_millis(200)) {
            // SIGKILL delivery is async; delete() below may still fail if something
            // (e.g. a process stuck in uninterruptible sleep) hasn't exited yet.
            log::warn!("app {} cgroup still populated after kill", self.pid);
        }

        if let Err(err) = self.cgroup.delete() {
            log::error!("Failed to delete app cgroup: {}", err);
        }

        self.proper_cleanup = true;

        Ok(())
    }

    fn send_signal(&self, signal: libc::c_int) -> io::Result<()> {
        let ret = unsafe { libc::kill(self.pid as libc::pid_t, signal) };
        log::info!(
            "app {} kill {} {} -> {}",
            self.pid,
            signal,
            signal_name(signal as usize).unwrap_or("UNKNOWN"),
            ret
        );
        let result = to_ioresult(ret);
        if let Err(err) = &result {
            log::error!(
                "Failed to send signal {} to app {}: {}",
                signal,
                self.pid,
                err
            );
        }
        result?;
        Ok(())
    }

    pub fn send_sigterm(&self) -> io::Result<()> {
        self.send_signal(libc::SIGTERM)
    }

    pub fn send_sigkill(&self) -> io::Result<()> {
        self.send_signal(libc::SIGKILL)
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
struct LastExecutionInfo {
    cause: ReturnState,
    restart_deadline: Option<Instant>,
}

#[derive(Debug)]
enum State {
    Stopped(Option<LastExecutionInfo>),
    Running(AppRuntime),
}

impl Default for State {
    fn default() -> Self {
        State::Stopped(None)
    }
}

impl State {
    fn stopped_with_cause(cause: ReturnState) -> Self {
        State::Stopped(Some(LastExecutionInfo {
            cause,
            restart_deadline: None,
        }))
    }

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
    let ret = unsafe { libc::dup2(src.as_raw_fd(), dst.as_raw_fd()) };
    to_ioresult(ret)?;
    Ok(())
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod restart_tests;

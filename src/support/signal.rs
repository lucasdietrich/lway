use std::{ffi::c_int, io, mem::MaybeUninit, os::fd::RawFd};

use libc::SFD_NONBLOCK;

use crate::support::to_ioresult;

/// Signal names indexed by signal number, for aarch64 (and generally
/// x86/ARM "most other architectures" numbering per signal(7)).
///
/// Index 0 is unused. Where a signal has an alias (e.g. SIGIOT for
/// SIGABRT), only the primary name is kept.
///
/// <https://man7.org/linux/man-pages/man7/signal.7.html>
pub static SIGNAL_NAMES: [&'static str; 32] = [
    "",          // 0 (unused)
    "SIGHUP",    // 1
    "SIGINT",    // 2
    "SIGQUIT",   // 3
    "SIGILL",    // 4
    "SIGTRAP",   // 5
    "SIGABRT",   // 6 (aka SIGIOT)
    "SIGBUS",    // 7
    "SIGFPE",    // 8
    "SIGKILL",   // 9
    "SIGUSR1",   // 10
    "SIGSEGV",   // 11
    "SIGUSR2",   // 12
    "SIGPIPE",   // 13
    "SIGALRM",   // 14
    "SIGTERM",   // 15
    "SIGSTKFLT", // 16
    "SIGCHLD",   // 17
    "SIGCONT",   // 18
    "SIGSTOP",   // 19
    "SIGTSTP",   // 20
    "SIGTTIN",   // 21
    "SIGTTOU",   // 22
    "SIGURG",    // 23
    "SIGXCPU",   // 24
    "SIGXFSZ",   // 25
    "SIGVTALRM", // 26
    "SIGPROF",   // 27
    "SIGWINCH",  // 28
    "SIGIO",     // 29 (aka SIGPOLL)
    "SIGPWR",    // 30
    "SIGSYS",    // 31 (aka SIGUNUSED)
];

/// Look up a signal name by number. Returns `None` for out-of-range
/// or undefined slots.
pub fn signal_name(sig: usize) -> Option<&'static str> {
    SIGNAL_NAMES.get(sig).filter(|s| !s.is_empty()).copied()
}

pub fn setup_signal_fd() -> io::Result<RawFd> {
    let mut mask = MaybeUninit::<libc::sigset_t>::uninit();

    unsafe { libc::sigemptyset(mask.as_mut_ptr()) };
    unsafe { libc::sigaddset(mask.as_mut_ptr(), libc::SIGINT) };
    // unsafe { libc::sigaddset(mask.as_mut_ptr(), libc::SIGHUP) };

    let ret = unsafe { libc::sigprocmask(libc::SIG_BLOCK, mask.as_ptr(), std::ptr::null_mut()) };
    to_ioresult(ret)?;

    let mask = unsafe { mask.assume_init() };

    let ret = unsafe { libc::signalfd(-1, &mask, SFD_NONBLOCK) };
    let res = to_ioresult(ret)?;
    Ok(res)
}

pub fn handle_signal_fd(signal_fd: &RawFd) -> libc::c_int {
    let mut siginfo = MaybeUninit::<libc::signalfd_siginfo>::uninit();
    let ret = unsafe {
        libc::read(
            *signal_fd,
            siginfo.as_mut_ptr() as *mut _,
            std::mem::size_of::<libc::signalfd_siginfo>(),
        )
    };
    if ret == std::mem::size_of::<libc::signalfd_siginfo>() as isize {
        let siginfo = unsafe { siginfo.assume_init() };
        let signo = siginfo.ssi_signo as usize;
        if let Some(name) = signal_name(signo) {
            log::info!("Received signal: {} ({})", name, signo);
        } else {
            log::warn!("Received unknown signal: {}", signo);
        }

        signo as c_int
    } else {
        -1
    }
}

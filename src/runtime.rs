use std::{ffi::CString, str::FromStr};

use libc::{c_char, waitpid, WEXITSTATUS, WIFEXITED, WNOHANG};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppErr {
    #[error("Fork failed {0}")]
    ForkFailed(i32),
    #[error("Execv failed {0}")]
    ExecvFailed(i32),
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
    pid: i32,
    state: State,
}

impl App {
    pub fn start<'a>(app: &str, args: impl Iterator<Item = &'a str>) -> Result<Self, AppErr> {
        let ret = unsafe { libc::fork() };

        if ret == 0 {
            // child
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

            let ret = unsafe {
                libc::execv(prog.as_ptr() as *const c_char, argv.as_ptr())
            };
            log::error!("execv returned {} errno: {}", ret, std::io::Error::last_os_error());
            Err(AppErr::ExecvFailed(ret))
        } else if ret > 0 {
            // parent
            log::info!("Child pid: {}", ret);
            Ok(App {
                pid: ret,
                state: State::Running,
            })
        } else {
            Err(AppErr::ForkFailed(ret))
        }
    }

    pub fn poll(&mut self) {
        if self.state != State::Running {
            // Nothing to do
            return;
        }

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

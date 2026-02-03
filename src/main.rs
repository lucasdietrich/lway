use std::{
    ffi::{c_char, CString},
    str::FromStr,
};

use libc::{WEXITSTATUS, WIFEXITED, WNOHANG, waitpid};
use thiserror::Error;

const APPS: &[&str] = &["/home/root/app 2", "/home/root/app 5"];

#[derive(Debug, Error)]
enum AppErr {
    #[error("Fork failed {0}")]
    ForkFailed(i32),
    #[error("Execv failed {0}")]
    ExecvFailed(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReturnState {
    // Tell whether the process returned normally
    Completed {
        ret: i32,
    },
    Abnormal // Tell whether the process returned normally (call to exit or return from main)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    Terminated(ReturnState)
}

struct App {
    pid: i32,
    state: State,
}

impl App {
    fn start<'a>(app: &str, args: impl Iterator<Item = &'a str>) -> Result<Self, AppErr> {
        let ret = unsafe { libc::fork() };

        if ret == 0 {
            // child
            let prog = CString::from_str(app).expect("app name");
            let args: Vec<CString> = args
                .map(|arg| CString::from_str(arg).expect("arg"))
                .collect();
            let argv: Vec<*const c_char> = args
                .iter()
                .map(|cstring| cstring.as_ptr() as *const c_char)
                .collect();
            let argv = argv.as_ptr();

            let ret =
                unsafe { libc::execv(prog.as_ptr() as *const c_char, argv as *const *const c_char) };
            Err(AppErr::ExecvFailed(ret))
        } else if ret > 0 {
            // parent
            println!("Child pid: {}", ret);
            Ok(App { pid: ret, state: State::Running})
        } else {
            Err(AppErr::ForkFailed(ret))
        }
    }

    fn poll(&mut self) {
        if self.state != State::Running {
            // Nothing to do
            return;
        }

        let mut status: i32 = 0;
        let options: i32=  WNOHANG;
        let ret = unsafe { waitpid(self.pid, &mut status, options) };
        println!("app: {} waitpid -> {} status: {}", self.pid, ret, status);

        if ret == self.pid {
            // children exited

            // parse status
            let normal = WIFEXITED(status);
            let return_state = match normal {
                true => ReturnState::Completed { ret: WEXITSTATUS(status) },
                false => ReturnState::Abnormal,
            };
            self.state = State::Terminated(return_state);
            println!("app {} returned {:#?}", self.pid, self.state);
            
        } else if ret == 0 {
            // waiting
        } else {
            panic!("waitpid failed")
        }
    }

    fn is_running(&self) -> bool{
        self.state == State::Running
    }
}

struct Runtime {
    apps: Vec<App>,
}

fn main() {
    println!("lway");

    let mut rt = Runtime {
        apps: Vec::new()
    };

    for app in APPS {
        println!("Starting {}", app);
        let parts: Vec<&str> = app.split(' ').collect();
        let app = App::start(parts[0], parts.into_iter()).expect("run_app");
        rt.apps.push(app);
    }

    loop {
        println!("lway loop");
        unsafe {
            libc::sleep(1);
        }

        for app in rt.apps.iter_mut() {
            app.poll();
        }

        if rt.apps.iter().all(|app| !app.is_running()) {
            println!("all apps returned, exiting ...");
            break
        }
    }
}

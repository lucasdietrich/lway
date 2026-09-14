use std::{sync::atomic, thread::sleep};

use libc::SIGINT;

use crate::{cgroups::init_main_cgroup, parser::Config, runtime::App};

pub mod cgroups;
pub mod config;
pub mod logger;
pub mod parser;
pub mod pipe;
pub mod runtime;
pub mod support;
pub mod utils;

const CONFIG: &str = "apps.yaml";

pub struct Runtime {
    pub apps: Vec<App>,
}

static SIGINT_COUNT: atomic::AtomicUsize = atomic::AtomicUsize::new(0);
const SIGINT_LIMIT: usize = 3;

extern "C" fn handler(signal: i32) {
    println!("Received signal: {}", signal);
    if signal == SIGINT {
        if SIGINT_COUNT.fetch_add(1, atomic::Ordering::SeqCst) >= (SIGINT_LIMIT - 1) {
            println!("Received SIGINT {} times, exiting immediately", SIGINT_LIMIT);
            std::process::exit(1);
        } else {
            println!("SIGINT received {} times", SIGINT_COUNT.load(atomic::Ordering::SeqCst));
        }
    }
}

impl Runtime {
    pub fn init() -> Self {
        Runtime { apps: Vec::new() }
    }
}

// impl Drop for Runtime {
//     fn drop(&mut self) {

//     }
// }

fn main() {
    // Parse command-line arguments to determine verbosity level
    let args: Vec<String> = std::env::args().collect();
    let verbosity = args
        .iter()
        .filter(|arg| arg.starts_with("-v"))
        .map(|arg| arg.chars().filter(|&c| c == 'v').count())
        .sum::<usize>();

    let log_level = match verbosity {
        0 => log::LevelFilter::Off,
        1 => log::LevelFilter::Error,
        2 => log::LevelFilter::Warn,
        3 => log::LevelFilter::Info,
        4 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    };

    simple_logger::SimpleLogger::new()
        .with_level(log_level)
        .init()
        .expect("init logger");

    let yaml = std::fs::read_to_string(CONFIG).expect("read config file");
    let cfg: Config = serde_yaml::from_str(&yaml).expect("parse config");

    log::info!("{:#?}", cfg);

    let ret = unsafe { libc::signal(SIGINT, handler as *const () as libc::sighandler_t) };
    if ret == libc::SIG_ERR {
        log::error!("Failed to set signal handler");
        std::process::exit(1);
    }

    let mut rt = Runtime::init();
    let logger = logger::StdoutLogger;

    let main_cg = init_main_cgroup();

    for app_cfg in cfg.apps.iter() {
        log::info!("Starting {}", app_cfg.command);
        let parts: Vec<&str> = app_cfg.command.split(' ').collect();
        let name = app_cfg.name.as_deref().unwrap_or(parts[0]);
        let env: Vec<String> = app_cfg
            .env
            .as_ref()
            .map(|env_map| {
                env_map
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect()
            })
            .unwrap_or_else(Vec::new);

        let params = runtime::AppParams {
            cwd: app_cfg.workdir.as_deref(),
            name,
            prog: parts[0],
            args: &parts,
            uid: app_cfg.uid,
            gid: app_cfg.gid,
            env,
            cpu_weight: app_cfg.cpu_weight,
        };

        let app = App::start(params).expect("run_app");
        rt.apps.push(app);
    }

    loop {
        unsafe {
            libc::sleep(1);
        }

        // If Ctrl+C (SIGINT) was received
        let sigint_count = SIGINT_COUNT.load(atomic::Ordering::SeqCst);
        if sigint_count > 0 {
            println!("SIGINT received {} times", sigint_count);
            if sigint_count >= SIGINT_LIMIT - 1 {
                // Send SIGKILL to all child processes
                for app in rt.apps.iter() {
                    if let Err(e) = app.terminate() {
                        log::error!("Failed to send SIGKILL to {}: {}", app, e);
                    }
                }
            } else {
                // Send SIGTERM to all child processes
                for app in rt.apps.iter() {
                    if let Err(e) = app.sigterm() {
                        log::error!("Failed to send SIGTERM to {}: {}", app, e);
                    }
                }
            }
        }

        for app in rt.apps.iter_mut() {
            app.poll(&logger);
        }

        // Remove all apps that are no longer running
        rt.apps.retain(|app| app.is_running());

        if rt.apps.is_empty() {
            log::info!("all apps returned, exiting ...");
            break;
        }
    }

    main_cg.delete().expect("Failed to delete main cgroup");
}

use std::sync::atomic;

use libc::SIGINT;

use crate::{parser::Config, runtime::App};

pub mod logger;
pub mod parser;
pub mod pipe;
pub mod runtime;
pub mod utils;

const CONFIG: &str = "apps.yaml";

pub struct Runtime {
    pub apps: Vec<App>,
}

const FLAG_INT_RECEIVED: usize = 1 << 0;
static FLAGS: atomic::AtomicUsize = atomic::AtomicUsize::new(0);

extern "C" fn handler(signal: i32) {
    println!("Received signal: {}", signal);
    if signal == SIGINT {
        if FLAGS.fetch_or(FLAG_INT_RECEIVED, atomic::Ordering::SeqCst) & FLAG_INT_RECEIVED != 0 {
            println!("Second SIGINT received, exiting immediately");
            std::process::exit(1);
        } else {
            println!("SIGINT received, sending SIGTERM to all child processes");
        }
    }
}

impl Runtime {
    pub fn init() -> Self {
        Runtime { apps: Vec::new() }
    }
}

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

    let ret = unsafe { libc::signal(SIGINT, handler as libc::sighandler_t) };
    if ret == libc::SIG_ERR {
        log::error!("Failed to set signal handler");
        std::process::exit(1);
    }

    let mut rt = Runtime::init();
    let logger = logger::StdoutLogger;

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
        };

        let app = App::start(params).expect("run_app");
        rt.apps.push(app);
    }

    while FLAGS.load(atomic::Ordering::SeqCst) & FLAG_INT_RECEIVED == 0 {
        unsafe {
            libc::sleep(1);
        }

        for app in rt.apps.iter_mut() {
            app.poll(&logger);
        }

        if rt.apps.iter().all(|app| !app.is_running()) {
            log::info!("all apps returned, exiting ...");
            break;
        }
    }

    // Send SIGTERM to all child processes
    for app in rt.apps.iter() {
        if let Err(e) = app.sigterm() {
            log::error!("Failed to send SIGTERM to {}: {}", app, e);
        }
    }
}

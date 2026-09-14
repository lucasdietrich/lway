use std::{path::PathBuf, sync::atomic};

use clap::Parser;
use libc::SIGINT;

use crate::{cgroups::init_main_cgroup, config::GlobalConfig, runtime::App};

pub mod cgroups;
pub mod config;
pub mod logger;
pub mod parser;
pub mod pipe;
pub mod runtime;
pub mod support;

const DEFAULT_CONFIG_PATH: &str = "lway.yaml";

/// lway - a tiny process supervisor
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Increase verbosity (repeat for more, e.g. -vvv)
    #[arg(short = 'v', action = clap::ArgAction::Count)]
    verbose: u8,

    /// Path to the global configuration file
    #[arg(short = 'c', long = "config")]
    config: Option<PathBuf>,
}

pub struct Runtime {
    apps: Vec<App>,
    stopping: bool,
}

static SIGINT_COUNT: atomic::AtomicUsize = atomic::AtomicUsize::new(0);
const SIGINT_LIMIT: usize = 3;

extern "C" fn handler(signal: i32) {
    println!("Received signal: {}", signal);
    if signal == SIGINT {
        let sigint_count = SIGINT_COUNT.fetch_add(1, atomic::Ordering::SeqCst);
        if sigint_count >= (SIGINT_LIMIT - 1) {
            println!(
                "Received SIGINT {} times, exiting immediately",
                sigint_count
            );
            std::process::exit(1);
        } else {
            println!("SIGINT received {} times", sigint_count);
        }
    }
}

impl Runtime {
    pub fn init() -> Self {
        Runtime {
            apps: Vec::new(),
            stopping: false,
        }
    }
}

fn main() {
    let cli = Cli::parse();

    let log_level = match cli.verbose {
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

    let config_path = cli
        .config
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
    let global_cfg = GlobalConfig::load(&config_path).unwrap_or_else(|e| {
        log::warn!(
            "Failed to load global config {}: {}",
            config_path.display(),
            e
        );
        GlobalConfig::default()
    });

    let apps = global_cfg.all_apps(&config_path);
    log::info!("{:#?}", apps);

    let ret = unsafe { libc::signal(SIGINT, handler as *const () as libc::sighandler_t) };
    if ret == libc::SIG_ERR {
        log::error!("Failed to set signal handler");
        std::process::exit(1);
    }

    let mut rt = Runtime::init();
    let logger = logger::StdoutLogger;

    let main_cg = init_main_cgroup();

    for app_cfg in apps.into_iter() {
        let uid = app_cfg.resolved_uid();
        let gid = app_cfg.resolved_gid();

        log::info!("Starting {}", app_cfg.command);
        let parts: Vec<String> = app_cfg.command.split(' ').map(|s| s.to_string()).collect();
        let name = app_cfg.name.unwrap_or(parts[0].clone());
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
            cwd: app_cfg.workdir,
            name,
            prog: parts[0].clone(),
            args: parts,
            uid,
            gid,
            env,
            oneshot: app_cfg.oneshot,
            cgroup: app_cfg.cgroup,
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
            rt.stopping = true;
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
            app.poll(&logger, !rt.stopping);
        }

        if rt.apps.iter().filter(|app| app.is_running()).count() == 0 {
            log::info!("all apps returned, exiting ...");
            break;
        }
    }

    main_cg.delete().expect("Failed to delete main cgroup");
}

use std::{io, path::PathBuf, time::Instant};

use clap::{CommandFactory, Parser, Subcommand};
use mio::{unix::SourceFd, Events, Poll, Token};

use crate::{
    cgroups::init_main_cgroup,
    config::GlobalConfig,
    runtime::App,
    support::{
        mio_token_slab::MioTokenSlab,
        signal::handle_signal_fd,
        uidgid::{get_current_cwd, get_current_gid, get_current_uid},
    },
};

pub mod cgroups;
pub mod cli;
pub mod config;
pub mod ipc;
pub mod logger;
pub mod parser;
pub mod protocol;
pub mod restart;
pub mod runtime;
pub mod stats;
pub mod support;

const DEFAULT_CONFIG_PATH: &str = "lway.yaml";
// world-writable /tmp is fine for local experimentation; a real deployment
// running as root should keep this under /run.
const DEFAULT_SOCKET_PATH: &str = "/run/lway.sock";

// Number of SIGINTs to tolerate before exiting
const SIGINT_LIMIT: usize = 2;

pub const UNIX_LISTENER_TOKEN: Token = Token(0);
pub const SIGNALFD_TOKEN: Token = Token(1);
pub const RESERVED_MIO_TOKENS: usize = 2;
pub const MAX_MIO_TOKENS: usize = 128;

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

    /// Path to the daemon's control socket
    #[arg(long = "socket")]
    socket: Option<PathBuf>,

    /// Run as the background supervisor instead of a CLI client
    #[arg(long = "daemon")]
    daemon: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List apps supervised by the running daemon
    List,
    /// Show runtime statistics for a single app
    Stats {
        /// Name of the app to query
        name: String,
    },
    /// Start a stopped app
    Start {
        /// Name of the app to start
        name: String,
    },
    /// Start every currently stopped app
    StartAll,
}

pub struct Runtime {
    apps: Vec<App>,
    stopping: bool,
    sigint_count: usize,
    mio_token_slab: MioTokenSlab,
}

impl Runtime {
    pub fn init() -> Self {
        Runtime {
            apps: Vec::new(),
            stopping: false,
            sigint_count: 0,
            mio_token_slab: MioTokenSlab::new(MAX_MIO_TOKENS, RESERVED_MIO_TOKENS),
        }
    }
}

fn main() {
    let cli = Cli::parse();

    let socket_path = cli
        .socket
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET_PATH));

    if !cli.daemon {
        let command = cli.command.unwrap_or_else(|| {
            Cli::command().print_help().expect("print help");
            println!();
            std::process::exit(1);
        });
        match command {
            Command::List => run_list_client(&socket_path),
            Command::Stats { name } => run_stats_client(&socket_path, &name),
            Command::Start { name } => run_start_client(&socket_path, &name),
            Command::StartAll => run_start_all_client(&socket_path),
        }
        return;
    }

    let log_level = match cli.verbose {
        0 => log::LevelFilter::Warn,
        1 => log::LevelFilter::Info,
        2 => log::LevelFilter::Debug,
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

    let signalfd = match support::signal::setup_signal_fd() {
        Ok(fd) => fd,
        Err(e) => {
            log::error!("Failed to set up signal fd: {}", e);
            std::process::exit(1);
        }
    };
    let mut signal_sourcefd = SourceFd(&signalfd);

    let mut rt = Runtime::init();
    let logger = logger::StdoutLogger::default();

    let main_cg = init_main_cgroup();

    let mut poll = Poll::new().expect("create mio poll");

    for app_cfg in apps.into_iter() {
        let cwd = app_cfg
            .workdir
            .clone()
            .map(|p| PathBuf::from(p))
            .unwrap_or_else(|| get_current_cwd().expect("get current cwd"));
        let uid = app_cfg.resolved_uid().unwrap_or_else(get_current_uid);
        let gid = app_cfg.resolved_gid().unwrap_or_else(get_current_gid);

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
            cwd,
            name,
            prog: parts[0].clone(),
            args: parts,
            uid,
            gid,
            env,
            oneshot: app_cfg.oneshot,
            cgroup: app_cfg.cgroup,
            autostart: app_cfg.autostart,
            restart: app_cfg.restart,
        };

        if params.oneshot
            && (!params.restart.on_success.is_never() || !params.restart.on_error.is_never())
        {
            log::warn!(
                "app {} is oneshot, its restart policy is ignored",
                params.name
            );
        }

        let app = App::create(params, &poll, &mut rt.mio_token_slab).expect("run_app");
        rt.apps.push(app);
    }

    let mut ipc_server = ipc::Server::bind(&socket_path, &poll, ipc::DEFAULT_MAX_CONNECTIONS)
        .unwrap_or_else(|e| {
            log::error!(
                "Failed to bind control socket {}: {}",
                socket_path.display(),
                e
            );
            std::process::exit(1);
        });

    poll.registry()
        .register(
            &mut signal_sourcefd,
            SIGNALFD_TOKEN,
            mio::Interest::READABLE,
        )
        .expect("register signal fd");

    let mut events = Events::with_capacity(MAX_MIO_TOKENS);

    loop {
        let now = Instant::now();
        let timeout = rt
            .apps
            .iter()
            .filter_map(|app| app.pending_restart_deadline())
            .min()
            .map(|deadline| deadline.saturating_duration_since(now));

        if let Err(e) = poll.poll(&mut events, timeout) {
            if e.kind() != io::ErrorKind::Interrupted {
                log::error!("control socket poll error: {}", e);
            }
        }

        for event in events.iter() {
            let token = event.token();
            log::debug!("poll ready for token: {:?} event: {:?}", token, event);

            if token == SIGNALFD_TOKEN {
                match handle_signal_fd(&signalfd) {
                    // Increment the SIGINT count and mark the runtime as stopping
                    libc::SIGINT => {
                        rt.sigint_count += 1;
                        rt.stopping = true;
                    }
                    _ => {}
                }
                continue;
            }

            if token == UNIX_LISTENER_TOKEN {
                if let Err(e) = ipc_server.accept_all(&poll, &mut rt.mio_token_slab) {
                    log::error!("failed to accept control connection: {}", e);
                }
                continue;
            }

            if ipc_server.is_known(token) {
                if event.is_readable() {
                    if let Err(e) = ipc_server.handle_readable(
                        token,
                        &mut rt.apps,
                        &poll,
                        &mut rt.mio_token_slab,
                    ) {
                        log::error!("control connection read error: {}", e);
                        ipc_server.close(token, &poll, &mut rt.mio_token_slab);
                        continue;
                    }
                }
                if event.is_writable() {
                    if let Err(e) = ipc_server.handle_writable(token) {
                        log::error!("control connection write error: {}", e);
                        ipc_server.close(token, &poll, &mut rt.mio_token_slab);
                        continue;
                    }
                }
                ipc_server.reconcile(token, &poll, &mut rt.mio_token_slab);
                continue;
            }

            // Create the equivalent is_known() for applications
            for app in rt.apps.iter_mut() {
                // TODO optimize this, as finding the app matching the token could be long if there is a lot of apps
                // also immediately exit the loop if the matching app is found
                app.poll(
                    &poll,
                    &mut rt.mio_token_slab,
                    token,
                    event,
                    &logger,
                    !rt.stopping,
                )
            }
        }

        // fire any scheduled restarts whose delay has elapsed
        let now = Instant::now();
        for app in rt.apps.iter_mut() {
            app.maybe_restart(now, &poll, &mut rt.mio_token_slab, !rt.stopping);
        }

        // If Ctrl+C (SIGINT) was received
        if rt.sigint_count > SIGINT_LIMIT {
            log::info!("Sending SIGKILL to all child processes");
            for app in rt.apps.iter() {
                if let Err(e) = app.sigkill() {
                    log::error!("Failed to send SIGKILL to {}: {}", app, e);
                }
            }
        } else if rt.sigint_count > 0 {
            log::info!("Sending SIGTERM to all child processes");
            for app in rt.apps.iter() {
                if let Err(e) = app.sigterm() {
                    log::error!("Failed to send SIGTERM to {}: {}", app, e);
                }
            }
        }

        if rt
            .apps
            .iter()
            .all(|app| !app.is_running() && app.pending_restart_deadline().is_none())
        {
            log::info!("all apps returned, exiting ...");
            break;
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    main_cg.delete().expect("Failed to delete main cgroup");
}

/// Connects to a running daemon, requests the app list and prints it as a table.
fn run_list_client(socket_path: &PathBuf) {
    match cli::list::list_apps(socket_path) {
        Ok(apps) => cli::list::print_apps_table(&apps),
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Connects to a running daemon, requests an app's stats and prints them.
fn run_stats_client(socket_path: &PathBuf, name: &str) {
    match cli::stats::get_stats(socket_path, name) {
        Ok(stats) => cli::stats::print_stats(name, &stats),
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Connects to a running daemon and requests that a stopped app be started.
fn run_start_client(socket_path: &PathBuf, name: &str) {
    match cli::start::start_app(socket_path, name) {
        Ok(()) => println!("started {}", name),
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Connects to a running daemon and requests that every stopped app be started.
fn run_start_all_client(socket_path: &PathBuf) {
    match cli::start::start_all_apps(socket_path) {
        Ok((started, failed)) => {
            cli::start::print_start_all_result(&started, &failed);
            if !failed.is_empty() {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

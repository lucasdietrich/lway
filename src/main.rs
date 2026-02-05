use crate::{parser::Config, runtime::App};

pub mod logger;
pub mod parser;
pub mod pipe;
pub mod runtime;

const CONFIG: &str = "apps.yaml";

pub struct Runtime {
    pub apps: Vec<App>,
}

impl Runtime {
    pub fn new() -> Self {
        Runtime { apps: Vec::new() }
    }
}

fn main() {
    // Parse command-line arguments to determine verbosity level
    let args: Vec<String> = std::env::args().collect();
    let verbosity = args.iter()
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

    let mut rt = Runtime::new();
    let logger = logger::StdoutLogger;

    for app_cfg in cfg.apps.iter() {
        log::info!("Starting {}", app_cfg.command);
        let parts: Vec<&str> = app_cfg.command.split(' ').collect();
        let app_name = app_cfg.name.as_deref().unwrap_or(parts[0]);
        let app = App::start(app_name, parts[0], parts.into_iter()).expect("run_app");
        rt.apps.push(app);
    }

    loop {
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
}

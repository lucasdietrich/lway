use crate::{parser::Config, runtime::App};

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
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .init()
        .expect("init logger");

    let yaml = std::fs::read_to_string(CONFIG).expect("read config file");
    let cfg: Config = serde_yaml::from_str(&yaml).expect("parse config");

    log::info!("{:#?}", cfg);

    let mut rt = Runtime::new();

    for app_cfg in cfg.apps.iter() {
        log::info!("Starting {}", app_cfg.command);
        let parts: Vec<&str> = app_cfg.command.split(' ').collect();
        let app = App::start(parts[0], parts.into_iter()).expect("run_app");
        rt.apps.push(app);
    }

    loop {
        unsafe {
            libc::sleep(1);
        }

        for app in rt.apps.iter_mut() {
            app.poll();
        }

        if rt.apps.iter().all(|app| !app.is_running()) {
            log::info!("all apps returned, exiting ...");
            break;
        }
    }
}

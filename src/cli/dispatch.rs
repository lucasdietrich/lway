//! CLI client dispatch: routes a parsed `Command` to the matching request
//! against the daemon's control socket.

use std::path::Path;

use crate::cli;
use crate::Command;

/// Runs the client-side handler for `command` against the daemon at `socket_path`.
pub(crate) fn run_client_command(command: Command, socket_path: &Path) {
    match command {
        Command::List => run_list_client(socket_path),
        Command::Info { name } => run_info_client(socket_path, &name),
        Command::Config { name } => run_config_client(socket_path, &name),
        Command::Start { name } => run_start_client(socket_path, &name),
        Command::StartAll => run_start_all_client(socket_path),
        Command::Stop { name, force } => run_stop_client(socket_path, &name, force),
        Command::StopAll { force } => run_stop_all_client(socket_path, force),
    }
}

/// Connects to a running daemon, requests the app list and prints it as a table.
fn run_list_client(socket_path: &Path) {
    match cli::list::list_apps(socket_path) {
        Ok(apps) => cli::list::print_apps_table(&apps),
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Connects to a running daemon, requests an app's full details and prints them.
fn run_info_client(socket_path: &Path, name: &str) {
    match cli::info::get_info(socket_path, name) {
        Ok(full) => cli::info::print_info(name, &full),
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Connects to a running daemon, requests an app's config and prints it as YAML.
fn run_config_client(socket_path: &Path, name: &str) {
    match cli::config::get_config(socket_path, name) {
        Ok(config) => cli::config::print_config(config),
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Connects to a running daemon and requests that a stopped app be started.
fn run_start_client(socket_path: &Path, name: &str) {
    match cli::start::start_app(socket_path, name) {
        Ok(()) => println!("started {}", name),
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Connects to a running daemon and requests that every stopped app be started.
fn run_start_all_client(socket_path: &Path) {
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

/// Connects to a running daemon and requests that a running app be stopped.
fn run_stop_client(socket_path: &Path, name: &str, force: bool) {
    match cli::stop::stop_app(socket_path, name, force) {
        Ok(app_was_running) => {
            if app_was_running {
                println!("stopped {}", name);
            } else {
                println!("{} was not running", name);
            }
        }
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Connects to a running daemon and requests that every running app be stopped.
fn run_stop_all_client(socket_path: &Path, force: bool) {
    match cli::stop::stop_all_apps(socket_path, force) {
        Ok((stopped, failed)) => {
            cli::stop::print_stop_all_result(&stopped, &failed);
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

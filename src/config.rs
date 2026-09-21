use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use thiserror::Error;

use crate::ipc::DEFAULT_MAX_CONNECTIONS as DEFAULT_MAX_CONTROL_CONNECTIONS;
use crate::log_ipc::DEFAULT_MAX_CONNECTIONS as DEFAULT_MAX_LOG_CONNECTIONS;
use crate::parser::AppConfig;
use crate::support::log_buffer::DEFAULT_LOG_BUFFER_SIZE;
use crate::MAX_MIO_TOKENS;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read global config {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("failed to parse global config {0}: {1}")]
    Parse(PathBuf, serde_yaml::Error),
    #[error("log_buffer_size {0} is not a multiple of the page size ({1}), try {2} instead")]
    InvalidLogBufferSize(usize, usize, usize),
}

/// Validates a (global or per-app override) `log_buffer_size` value; the ring
/// buffer backing it is mmap-based and requires page-aligned sizes.
pub fn validate_log_buffer_size(size: usize) -> Result<(), ConfigError> {
    let page_size = crate::support::mmap::page_size();
    if size % page_size != 0 {
        let suggested = size.div_ceil(page_size).max(1) * page_size;
        return Err(ConfigError::InvalidLogBufferSize(
            size, page_size, suggested,
        ));
    }
    Ok(())
}

/// Global lway configuration, loaded from the path given via the `-c` CLI option.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    /// Directory scanned for additional per-app configuration files.
    pub apps_dir: String,
    /// Apps declared inline in the global config.
    pub apps: Vec<AppConfig>,
    /// Default capacity (in bytes) of each app's captured stdout/stderr ring
    /// buffer; overridable per app via `AppConfig::log_buffer_size`. Must be a
    /// multiple of the page size.
    pub log_buffer_size: usize,
    /// Cap on total mio tokens (apps' fds + control/log connections).
    pub max_mio_tokens: usize,
    /// Cap on concurrent control-socket connections.
    pub max_control_connections: usize,
    /// Cap on concurrent log-socket connections.
    pub max_log_connections: usize,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        GlobalConfig {
            apps_dir: ".".to_string(),
            apps: Vec::new(),
            log_buffer_size: DEFAULT_LOG_BUFFER_SIZE,
            max_mio_tokens: MAX_MIO_TOKENS,
            max_control_connections: DEFAULT_MAX_CONTROL_CONNECTIONS,
            max_log_connections: DEFAULT_MAX_LOG_CONNECTIONS,
        }
    }
}

impl GlobalConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let yaml =
            std::fs::read_to_string(path).map_err(|e| ConfigError::Io(path.to_path_buf(), e))?;
        let cfg: GlobalConfig =
            serde_yaml::from_str(&yaml).map_err(|e| ConfigError::Parse(path.to_path_buf(), e))?;
        validate_log_buffer_size(cfg.log_buffer_size)?;
        Ok(cfg)
    }

    /// Apps declared inline plus one per yaml file found in `apps_dir`. `config_path` is
    /// excluded from the scan so the global config file itself isn't parsed as an app.
    /// Files that fail to parse only produce a warning.
    pub fn all_apps(self, config_path: &Path) -> Vec<AppConfig> {
        let mut apps = self.apps;
        apps.extend(scan_apps_dir(Path::new(&self.apps_dir), config_path));
        apps
    }
}

fn scan_apps_dir(dir: &Path, config_path: &Path) -> Vec<AppConfig> {
    let excluded = fs::canonicalize(config_path).ok();

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("Failed to read apps directory {}: {}", dir.display(), e);
            return Vec::new();
        }
    };

    let mut apps = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                log::warn!("Failed to read entry in {}: {}", dir.display(), e);
                continue;
            }
        };

        let path = entry.path();
        let is_yaml = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"));
        if !path.is_file() || !is_yaml || excluded == fs::canonicalize(&path).ok() {
            continue;
        }

        let yaml = match fs::read_to_string(&path) {
            Ok(yaml) => yaml,
            Err(e) => {
                log::warn!("Failed to read app config {}: {}", path.display(), e);
                continue;
            }
        };
        match serde_yaml::from_str::<AppConfig>(&yaml) {
            Ok(app) => apps.push(app),
            Err(e) => log::warn!("Failed to parse app config {}: {}", path.display(), e),
        }
    }
    apps
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{validate_log_buffer_size, ConfigError, GlobalConfig};

    /// Self-cleaning scratch directory for tests that need real files on disk
    /// (config loading / apps-dir scanning can't be exercised any other way).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "lway-test-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, contents).expect("write temp file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_global_config() -> Result<(), Box<dyn std::error::Error>> {
        let yaml = r#"
apps_dir: /etc/lway/apps.d
apps:
  - command: "app1 -a"
"#;
        let cfg: GlobalConfig = serde_yaml::from_str(yaml)?;
        assert_eq!(cfg.apps_dir, "/etc/lway/apps.d");
        assert_eq!(cfg.apps.len(), 1);
        assert_eq!(cfg.apps[0].command, "app1 -a");
        Ok(())
    }

    #[test]
    fn test_global_config_overrides() -> Result<(), Box<dyn std::error::Error>> {
        let yaml = r#"
log_buffer_size: 4096
max_mio_tokens: 16
max_control_connections: 2
max_log_connections: 3
"#;
        let cfg: GlobalConfig = serde_yaml::from_str(yaml)?;
        assert_eq!(cfg.log_buffer_size, 4096);
        assert_eq!(cfg.max_mio_tokens, 16);
        assert_eq!(cfg.max_control_connections, 2);
        assert_eq!(cfg.max_log_connections, 3);
        Ok(())
    }

    #[test]
    fn test_global_config_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let cfg: GlobalConfig = serde_yaml::from_str("{}")?;
        assert_eq!(cfg.apps_dir, ".");
        assert!(cfg.apps.is_empty());
        assert_eq!(cfg.log_buffer_size, super::DEFAULT_LOG_BUFFER_SIZE);
        assert_eq!(cfg.max_mio_tokens, crate::MAX_MIO_TOKENS);
        assert_eq!(
            cfg.max_control_connections,
            super::DEFAULT_MAX_CONTROL_CONNECTIONS
        );
        assert_eq!(cfg.max_log_connections, super::DEFAULT_MAX_LOG_CONNECTIONS);
        Ok(())
    }

    #[test]
    fn test_load_success() {
        let dir = TempDir::new("load-success");
        let path = dir.write(
            "lway.yaml",
            "apps_dir: apps.d\napps:\n  - command: \"app1\"\n",
        );

        let cfg = GlobalConfig::load(&path).expect("load config");
        assert_eq!(cfg.apps_dir, "apps.d");
        assert_eq!(cfg.apps.len(), 1);
    }

    #[test]
    fn test_load_missing_file_is_io_error() {
        let path = std::env::temp_dir().join("lway-test-does-not-exist.yaml");
        match GlobalConfig::load(&path) {
            Err(ConfigError::Io(p, _)) => assert_eq!(p, path),
            other => panic!("expected Io error, got {:?}", other),
        }
    }

    #[test]
    fn test_load_invalid_yaml_is_parse_error() {
        let dir = TempDir::new("load-parse-error");
        let path = dir.write("lway.yaml", "apps: \"not a list\"\n");

        match GlobalConfig::load(&path) {
            Err(ConfigError::Parse(p, _)) => assert_eq!(p, path),
            other => panic!("expected Parse error, got {:?}", other),
        }
    }

    #[test]
    fn test_load_misaligned_log_buffer_size_is_invalid() {
        let dir = TempDir::new("load-bad-log-buffer-size");
        let page_size = crate::support::mmap::page_size();
        let path = dir.write(
            "lway.yaml",
            &format!("log_buffer_size: {}\n", page_size + 1),
        );

        match GlobalConfig::load(&path) {
            Err(ConfigError::InvalidLogBufferSize(size, ps, suggested)) => {
                assert_eq!(size, page_size + 1);
                assert_eq!(ps, page_size);
                assert_eq!(suggested, page_size * 2);
            }
            other => panic!("expected InvalidLogBufferSize error, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_log_buffer_size() {
        let page_size = crate::support::mmap::page_size();
        assert!(validate_log_buffer_size(page_size).is_ok());
        assert!(validate_log_buffer_size(page_size * 4).is_ok());
        assert!(validate_log_buffer_size(0).is_ok());

        match validate_log_buffer_size(page_size + 1) {
            Err(ConfigError::InvalidLogBufferSize(_, _, suggested)) => {
                assert_eq!(suggested, page_size * 2)
            }
            other => panic!("expected InvalidLogBufferSize error, got {:?}", other),
        }
        match validate_log_buffer_size(page_size / 2) {
            Err(ConfigError::InvalidLogBufferSize(_, _, suggested)) => {
                assert_eq!(suggested, page_size)
            }
            other => panic!("expected InvalidLogBufferSize error, got {:?}", other),
        }
    }

    #[test]
    fn test_scan_apps_dir_finds_yaml_and_yml_case_insensitive() {
        let dir = TempDir::new("scan-extensions");
        dir.write("app1.yaml", "command: \"app1\"\n");
        dir.write("app2.YML", "command: \"app2\"\n");
        dir.write("notes.txt", "not an app config\n");

        let excluded = dir.path().join("lway.yaml"); // doesn't exist, just a placeholder path
        let apps = super::scan_apps_dir(dir.path(), &excluded);
        let mut commands: Vec<&str> = apps.iter().map(|a| a.command.as_str()).collect();
        commands.sort();
        assert_eq!(commands, vec!["app1", "app2"]);
    }

    #[test]
    fn test_scan_apps_dir_excludes_config_path() {
        let dir = TempDir::new("scan-excludes-self");
        let config_path = dir.write("lway.yaml", "apps: []\n");
        dir.write("app1.yaml", "command: \"app1\"\n");

        let apps = super::scan_apps_dir(dir.path(), &config_path);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].command, "app1");
    }

    #[test]
    fn test_scan_apps_dir_skips_unparsable_files() {
        let dir = TempDir::new("scan-skips-bad-yaml");
        dir.write("good.yaml", "command: \"app1\"\n");
        dir.write("bad.yaml", "command: [not, a, string]\n");

        let excluded = dir.path().join("lway.yaml");
        let apps = super::scan_apps_dir(dir.path(), &excluded);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].command, "app1");
    }

    #[test]
    fn test_scan_apps_dir_missing_directory_returns_empty() {
        let missing = std::env::temp_dir().join("lway-test-missing-apps-dir");
        let excluded = missing.join("lway.yaml");
        assert!(super::scan_apps_dir(&missing, &excluded).is_empty());
    }

    #[test]
    fn test_all_apps_merges_inline_and_scanned() {
        let dir = TempDir::new("all-apps-merge");
        dir.write("scanned.yaml", "command: \"app-from-dir\"\n");

        let cfg = GlobalConfig {
            apps_dir: dir.path().display().to_string(),
            apps: vec![serde_yaml::from_str("command: \"app-inline\"").unwrap()],
            ..GlobalConfig::default()
        };
        let config_path = dir.path().join("lway.yaml"); // doesn't exist, just excluded by path
        let mut commands: Vec<String> = cfg
            .all_apps(&config_path)
            .into_iter()
            .map(|a| a.command)
            .collect();
        commands.sort();
        assert_eq!(commands, vec!["app-from-dir", "app-inline"]);
    }
}

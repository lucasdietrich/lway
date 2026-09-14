use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use thiserror::Error;

use crate::parser::AppConfig;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read global config {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("failed to parse global config {0}: {1}")]
    Parse(PathBuf, serde_yaml::Error),
}

/// Global lway configuration, loaded from the path given via the `-c` CLI option.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    /// Directory scanned for additional per-app configuration files.
    pub apps_dir: String,
    /// Apps declared inline in the global config.
    pub apps: Vec<AppConfig>,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        GlobalConfig {
            apps_dir: ".".to_string(),
            apps: Vec::new(),
        }
    }
}

impl GlobalConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let yaml =
            std::fs::read_to_string(path).map_err(|e| ConfigError::Io(path.to_path_buf(), e))?;
        serde_yaml::from_str(&yaml).map_err(|e| ConfigError::Parse(path.to_path_buf(), e))
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
    use super::GlobalConfig;

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
    fn test_global_config_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let cfg: GlobalConfig = serde_yaml::from_str("{}")?;
        assert_eq!(cfg.apps_dir, ".");
        assert!(cfg.apps.is_empty());
        Ok(())
    }
}

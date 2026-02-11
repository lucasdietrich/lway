use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub name: Option<String>,
    pub command: String,
    pub workdir: Option<String>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub apps: Vec<AppConfig>,
}

#[cfg(test)]
mod tests {
    use crate::parser::Config;

    use super::AppConfig;

    #[test]
    fn test_app_config() -> Result<(), Box<dyn std::error::Error>> {
        // Example YAML config
        let yaml = r#"
command: "hello -v"
workdir: workdir
"#;
        let cfg: AppConfig = serde_yaml::from_str(yaml)?;
        assert_eq!(cfg.command, "hello -v");
        assert_eq!(cfg.workdir.as_deref(), Some("workdir"));

        Ok(())
    }

    #[test]
    fn test_config() -> Result<(), Box<dyn std::error::Error>> {
        // Example YAML config
        let yaml = r#"
apps:
  - command: "app1 -a"
    workdir: /path/to/app1
  - command: "app2 -b"
    workdir: /path/to/app2
    uid: 1000
    gid: 1000
"#;
        let cfg: Config = serde_yaml::from_str(yaml)?;
        assert_eq!(cfg.apps.len(), 2);
        assert_eq!(cfg.apps[0].command, "app1 -a");
        assert_eq!(cfg.apps[0].workdir.as_deref(), Some("/path/to/app1"));
        assert_eq!(cfg.apps[1].command, "app2 -b");
        assert_eq!(cfg.apps[1].workdir.as_deref(), Some("/path/to/app2"));
        assert_eq!(cfg.apps[1].uid, Some(1000));
        assert_eq!(cfg.apps[1].gid, Some(1000));
        Ok(())
    }
}

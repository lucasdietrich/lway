use std::collections::HashMap;

use serde::Deserialize;

use crate::{
    cgroups::AppCgroupConfig,
    support::uidgid::{get_gid, get_uid},
};

/// A user/group given either by numeric id or by name.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum UserId {
    Id(u32),
    Name(String),
}

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub name: Option<String>,
    pub command: String,
    pub workdir: Option<String>,
    pub user: Option<UserId>,
    pub group: Option<UserId>,
    pub env: Option<HashMap<String, String>>,
    #[serde(default)]
    pub oneshot: bool,
    #[serde(default = "default_autostart")]
    pub autostart: bool,
    #[serde(flatten)]
    pub cgroup: AppCgroupConfig,
}

fn default_autostart() -> bool {
    true
}

impl AppConfig {
    pub fn resolved_uid(&self) -> Option<u32> {
        match &self.user {
            Some(UserId::Id(uid)) => Some(*uid),
            Some(UserId::Name(name)) => get_uid(name).or_else(|| {
                log::warn!("user '{}' not found", name);
                None
            }),
            None => None,
        }
    }

    pub fn resolved_gid(&self) -> Option<u32> {
        match &self.group {
            Some(UserId::Id(gid)) => Some(*gid),
            Some(UserId::Name(name)) => get_gid(name).or_else(|| {
                log::warn!("group '{}' not found", name);
                None
            }),
            None => None,
        }
    }
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
    fn test_resolved_uid_gid_by_name() -> Result<(), Box<dyn std::error::Error>> {
        let yaml = r#"
command: "hello"
user: root
group: root
cpu_weight: 100
io_weight: 100
memory_hard_limit: 1073741824
memory_soft_limit: 0
memory_swap_limit: 268435456
"#;
        let cfg: AppConfig = serde_yaml::from_str(yaml)?;
        assert_eq!(cfg.resolved_uid(), Some(0));
        assert_eq!(cfg.resolved_gid(), Some(0));
        assert_eq!(cfg.cgroup.cpu_weight, Some(100));
        assert_eq!(cfg.cgroup.io_weight, Some(100));
        assert_eq!(cfg.cgroup.memory_hard_limit, Some(1073741824));
        assert_eq!(cfg.cgroup.memory_soft_limit, Some(0));
        assert_eq!(cfg.cgroup.memory_swap_limit, Some(268435456));

        Ok(())
    }

    #[test]
    fn test_resolved_uid_gid_by_id() -> Result<(), Box<dyn std::error::Error>> {
        let yaml = r#"
command: "hello"
user: 1234
group: 5678
"#;
        let cfg: AppConfig = serde_yaml::from_str(yaml)?;
        assert_eq!(cfg.resolved_uid(), Some(1234));
        assert_eq!(cfg.resolved_gid(), Some(5678));

        Ok(())
    }

    #[test]
    fn test_resolved_uid_gid_unknown_name() -> Result<(), Box<dyn std::error::Error>> {
        let yaml = r#"
command: "hello"
user: does-not-exist
group: does-not-exist
"#;
        let cfg: AppConfig = serde_yaml::from_str(yaml)?;
        assert_eq!(cfg.resolved_uid(), None);
        assert_eq!(cfg.resolved_gid(), None);

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
    user: 1000
    group: 1000
  - command: "app3"
    env:
      VAR1: value1
      VAR2: value2
"#;
        let cfg: Config = serde_yaml::from_str(yaml)?;
        assert_eq!(cfg.apps.len(), 3);
        assert_eq!(cfg.apps[0].command, "app1 -a");
        assert_eq!(cfg.apps[0].workdir.as_deref(), Some("/path/to/app1"));
        assert_eq!(cfg.apps[1].command, "app2 -b");
        assert_eq!(cfg.apps[1].workdir.as_deref(), Some("/path/to/app2"));
        assert_eq!(cfg.apps[1].resolved_uid(), Some(1000));
        assert_eq!(cfg.apps[1].resolved_gid(), Some(1000));
        assert_eq!(cfg.apps[2].command, "app3");
        assert_eq!(
            cfg.apps[2]
                .env
                .as_ref()
                .unwrap()
                .get("VAR1")
                .map(String::as_str),
            Some("value1")
        );
        assert_eq!(
            cfg.apps[2]
                .env
                .as_ref()
                .unwrap()
                .get("VAR2")
                .map(String::as_str),
            Some("value2")
        );
        Ok(())
    }
}

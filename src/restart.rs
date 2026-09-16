use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Delay strategy applied before a single restart attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum RestartDelay {
    /// Don't restart the app for this outcome; it settles into `Terminated`.
    Never,
    Constant {
        #[serde(default = "default_constant_delay_ms")]
        delay_ms: u64,
    },
    ExponentialBackoff {
        initial_delay_ms: u64,
        max_delay_ms: u64,
        #[serde(default = "default_backoff_multiplier")]
        multiplier: f64,
    },
}

fn default_constant_delay_ms() -> u64 {
    1000
}

fn default_backoff_multiplier() -> f64 {
    2.0
}

impl Default for RestartDelay {
    fn default() -> Self {
        RestartDelay::Constant {
            delay_ms: default_constant_delay_ms(),
        }
    }
}

impl RestartDelay {
    /// Whether this strategy means "don't restart" for the outcome it applies to.
    pub(crate) fn is_never(&self) -> bool {
        matches!(self, RestartDelay::Never)
    }

    /// Delay before the `attempt`-th (1-based) consecutive restart.
    pub(crate) fn delay_for(&self, attempt: u32) -> Duration {
        match self {
            RestartDelay::Never => Duration::ZERO,
            RestartDelay::Constant { delay_ms } => Duration::from_millis(*delay_ms),
            RestartDelay::ExponentialBackoff {
                initial_delay_ms,
                max_delay_ms,
                multiplier,
            } => {
                let factor = multiplier.powi(attempt.saturating_sub(1) as i32);
                let scaled_ms = (*initial_delay_ms as f64 * factor).min(*max_delay_ms as f64);
                Duration::from_millis(scaled_ms as u64)
            }
        }
    }
}

/// Configures how and whether an app is restarted after it exits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RestartPolicy {
    pub on_success: RestartDelay,
    pub on_error: RestartDelay,
    /// If the app stayed up at least this long before exiting, consecutive failures reset to 0.
    pub reset_after_ms: u64,
    /// Maximum number of consecutive failed restart attempts before giving up permanently.
    /// `None` means retry forever.
    pub max_restart_attempts: Option<u32>,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        RestartPolicy {
            on_success: RestartDelay::default(),
            on_error: RestartDelay::default(),
            reset_after_ms: 60_000,
            max_restart_attempts: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_delay_for_success_and_error() {
        let delay = RestartDelay::Constant { delay_ms: 250 };
        assert_eq!(delay.delay_for(1), Duration::from_millis(250));
        assert_eq!(delay.delay_for(5), Duration::from_millis(250));
    }

    #[test]
    fn exponential_backoff_grows_and_caps_at_max() {
        let delay = RestartDelay::ExponentialBackoff {
            initial_delay_ms: 100,
            max_delay_ms: 1000,
            multiplier: 2.0,
        };
        assert_eq!(delay.delay_for(1), Duration::from_millis(100));
        assert_eq!(delay.delay_for(2), Duration::from_millis(200));
        assert_eq!(delay.delay_for(3), Duration::from_millis(400));
        assert_eq!(delay.delay_for(4), Duration::from_millis(800));
        assert_eq!(delay.delay_for(5), Duration::from_millis(1000)); // capped
        assert_eq!(delay.delay_for(10), Duration::from_millis(1000));
    }

    #[test]
    fn restart_policy_deserializes_from_yaml() {
        let yaml = r#"
on_success:
  strategy: constant
  delay_ms: 1000
on_error:
  strategy: exponential_backoff
  initial_delay_ms: 500
  max_delay_ms: 30000
  multiplier: 2.0
reset_after_ms: 60000
max_restart_attempts: 5
"#;
        let policy: RestartPolicy = serde_yaml::from_str(yaml).expect("parse restart policy");
        assert_eq!(policy.on_success, RestartDelay::Constant { delay_ms: 1000 });
        assert_eq!(
            policy.on_error,
            RestartDelay::ExponentialBackoff {
                initial_delay_ms: 500,
                max_delay_ms: 30_000,
                multiplier: 2.0,
            }
        );
        assert_eq!(policy.reset_after_ms, 60_000);
        assert_eq!(policy.max_restart_attempts, Some(5));
    }

    #[test]
    fn restart_policy_defaults_to_immediate_unlimited_restarts() {
        let policy: RestartPolicy = serde_yaml::from_str("{}").expect("parse empty policy");
        assert_eq!(policy, RestartPolicy::default());
        assert_eq!(policy.on_success.delay_for(1), Duration::from_millis(1000));
        assert_eq!(policy.max_restart_attempts, None);
    }

    #[test]
    fn never_strategy_means_no_restart() {
        assert!(RestartDelay::Never.is_never());
        assert!(!RestartDelay::Constant { delay_ms: 0 }.is_never());
    }

    #[test]
    fn restart_on_failure_only_policy_deserializes() {
        // Common "restart on failure, stop on success" policy (like systemd's
        // Restart=on-failure / Docker's restart: on-failure).
        let yaml = r#"
on_success:
  strategy: never
on_error:
  strategy: constant
  delay_ms: 500
"#;
        let policy: RestartPolicy = serde_yaml::from_str(yaml).expect("parse restart policy");
        assert!(policy.on_success.is_never());
        assert_eq!(policy.on_error, RestartDelay::Constant { delay_ms: 500 });
    }
}

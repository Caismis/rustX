//! User-process resource policy. Never part of durable Session selections.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AppServerPolicy {
    pub max_resident_runtimes: usize,
    pub max_connections: usize,
    pub max_external_attachments: usize,
    pub idle_grace_ms: u64,
    pub shutdown_deadline_ms: u64,
}
impl Default for AppServerPolicy {
    fn default() -> Self {
        Self {
            max_resident_runtimes: 8,
            max_connections: 32,
            max_external_attachments: 64,
            idle_grace_ms: 300_000,
            shutdown_deadline_ms: 30_000,
        }
    }
}
impl AppServerPolicy {
    /// # Errors
    /// Rejects zero and operationally unreasonable process budgets.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=256).contains(&self.max_resident_runtimes)
            || !(1..=1024).contains(&self.max_connections)
            || !(1..=4096).contains(&self.max_external_attachments)
            || !(1..=86_400_000).contains(&self.idle_grace_ms)
            || !(1..=3_600_000).contains(&self.shutdown_deadline_ms)
        {
            return Err("invalid app_server policy: positive bounded resource limits and deadlines required".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_budgets_are_positive_bounded_and_strict() {
        assert!(AppServerPolicy::default().validate().is_ok());
        for field in [
            "max_resident_runtimes",
            "max_connections",
            "max_external_attachments",
            "idle_grace_ms",
            "shutdown_deadline_ms",
        ] {
            let mut value = serde_json::to_value(AppServerPolicy::default()).unwrap();
            value[field] = 0.into();
            assert!(
                serde_json::from_value::<AppServerPolicy>(value)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        assert!(toml::from_str::<AppServerPolicy>("unknown = 1").is_err());
        let impossible = AppServerPolicy {
            max_resident_runtimes: usize::MAX,
            ..AppServerPolicy::default()
        };
        assert!(impossible.validate().is_err());
    }
}

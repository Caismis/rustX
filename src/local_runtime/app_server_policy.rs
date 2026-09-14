//! User-process resource policy. Never part of durable Session selections.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AppServerPolicy {
    #[schemars(range(min = 1, max = 256))]
    pub max_resident_runtimes: usize,
    #[schemars(range(min = 1, max = 1024))]
    pub max_connections: usize,
    #[schemars(range(min = 1, max = 4096))]
    pub max_external_attachments: usize,
    #[schemars(range(min = 1, max = 86_400_000))]
    pub idle_grace_ms: u64,
    #[schemars(range(min = 1, max = 3_600_000))]
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
    #[test]
    fn schema_and_startup_accept_the_same_policy_bounds() {
        let schema = serde_json::to_value(schemars::schema_for!(AppServerPolicy)).unwrap();
        for (field, max) in [
            ("max_resident_runtimes", 256_u64),
            ("max_connections", 1024),
            ("max_external_attachments", 4096),
            ("idle_grace_ms", 86_400_000),
            ("shutdown_deadline_ms", 3_600_000),
        ] {
            assert_eq!(schema["properties"][field]["minimum"], 1);
            assert_eq!(schema["properties"][field]["maximum"], max);
            for (number, valid) in [(0, false), (1, true), (max, true), (max + 1, false)] {
                let mut value = serde_json::to_value(AppServerPolicy::default()).unwrap();
                value[field] = number.into();
                assert_eq!(
                    serde_json::from_value::<AppServerPolicy>(value)
                        .unwrap()
                        .validate()
                        .is_ok(),
                    valid
                );
            }
        }
    }
}

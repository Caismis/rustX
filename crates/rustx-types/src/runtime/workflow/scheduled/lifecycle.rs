//! Workflow-owned durable start identity.

use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct ScheduledWorkflowStartId(String);
impl ScheduledWorkflowStartId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Derives the start identity of one admitted trigger occurrence.
    pub fn for_trigger(trigger: &impl Serialize) -> Self {
        use sha2::{Digest, Sha256};
        Self(format!(
            "swf_{:x}",
            Sha256::digest(serde_json::to_vec(trigger).expect("native TriggerId"))
        ))
    }
}
impl std::fmt::Display for ScheduledWorkflowStartId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl TryFrom<String> for ScheduledWorkflowStartId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        if value.len() == 68
            && value.starts_with("swf_")
            && value[4..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            Ok(Self(value))
        } else {
            Err("invalid native scheduled start identity".into())
        }
    }
}
impl From<ScheduledWorkflowStartId> for String {
    fn from(value: ScheduledWorkflowStartId) -> Self {
        value.0
    }
}

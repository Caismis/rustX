//! Caller semantics and close-fenced submission ownership. Neither identity nor
//! fingerprint is authority. The transport owns the permit until native transfer.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::AuxiliaryFingerprint;
use crate::runtime::identity::{ConversationId, MessageId};

/// Finite admission/read failures, independent of model and consumer outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliaryFailure {
    Busy,
    Conflict,
    ScopeClosed,
    Unavailable,
    InputTooLarge,
}

pub const SUBMISSION_ENCODING_VERSION: u16 = 1;
pub const INPUT_TEXT_BYTES_MAX: usize = 16_384;
pub const LABEL_COUNT_MAX: usize = 32;
pub const LABEL_BYTES_MAX: usize = 128;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct CorrelationId(String);
impl CorrelationId {
    /// Validate the exact caller identity without normalization.
    /// # Errors
    /// Rejects empty, oversized or non-ASCII identifiers.
    pub fn new(value: String) -> Result<Self, String> {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
        {
            return Err("auxiliary correlation must contain 1..=128 ASCII letters, digits, '-', '_', '.' or ':'".into());
        }
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for CorrelationId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<CorrelationId> for String {
    fn from(value: CorrelationId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClassificationInput {
    Text { text: String },
    History { message_id: MessageId },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClassificationSubmission {
    pub correlation: CorrelationId,
    pub input: ClassificationInput,
    pub labels: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BtwSubmission {
    pub correlation: CorrelationId,
    pub question: String,
}

fn validate_text(text: &str) -> Result<(), String> {
    if text.is_empty() || text.len() > INPUT_TEXT_BYTES_MAX {
        return Err(format!(
            "auxiliary text must contain 1..={INPUT_TEXT_BYTES_MAX} UTF-8 bytes"
        ));
    }
    Ok(())
}
fn fingerprint(value: &impl Serialize) -> Result<AuxiliaryFingerprint, String> {
    // All input components are validated before serialization. Tuple order is
    // the versioned canonical encoding; text and array order remain exact.
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    AuxiliaryFingerprint::new(format!("{:x}", Sha256::digest(bytes)))
}
impl ClassificationSubmission {
    /// Validate caller semantics and fingerprint the exact authorized command.
    /// # Errors
    /// Rejects invalid text, references, label counts and duplicate labels.
    pub fn fingerprint(
        &self,
        conversation: &ConversationId,
    ) -> Result<AuxiliaryFingerprint, String> {
        match &self.input {
            ClassificationInput::Text { text } => validate_text(text)?,
            ClassificationInput::History { message_id } => {
                if message_id.as_str().is_empty() || message_id.as_str().len() > 128 {
                    return Err("classification historical message identity is invalid".into());
                }
            }
        }
        if self.labels.is_empty()
            || self.labels.len() > LABEL_COUNT_MAX
            || self
                .labels
                .iter()
                .any(|label| label.is_empty() || label.len() > LABEL_BYTES_MAX)
            || self
                .labels
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.labels.len()
        {
            return Err(
                "classification requires 1..=32 unique labels of 1..=128 UTF-8 bytes".into(),
            );
        }
        fingerprint(&(
            SUBMISSION_ENCODING_VERSION,
            "auxiliary/classify",
            conversation,
            &self.input,
            &self.labels,
        ))
    }
}
impl BtwSubmission {
    /// Validate and fingerprint an explicit question; current context is excluded.
    /// # Errors
    /// Rejects empty or oversized questions.
    pub fn fingerprint(
        &self,
        conversation: &ConversationId,
    ) -> Result<AuxiliaryFingerprint, String> {
        validate_text(&self.question)?;
        fingerprint(&(
            SUBMISSION_ENCODING_VERSION,
            "auxiliary/btw",
            conversation,
            &self.question,
        ))
    }
}

/// A bounded native evidence window for original request scopes, including
/// requests still parked before Runtime Client handoff. Retired evidence is
/// evicted; a miss is Unresolved, never proof of non-admission.
const SCOPE_LIMIT: usize = 128;
const RETIRED_SCOPE_LIMIT: usize = 128;
#[derive(Clone)]
struct ScopeBinding {
    id: crate::model::AuxiliaryAttribution,
    conversation: ConversationId,
    correlation: CorrelationId,
}
#[derive(Default)]
struct ScopeRegistry {
    active: std::collections::BTreeMap<String, ScopeBinding>,
    retired: std::collections::VecDeque<ScopeBinding>,
}
fn registry() -> &'static Mutex<ScopeRegistry> {
    static SCOPES: std::sync::OnceLock<Mutex<ScopeRegistry>> = std::sync::OnceLock::new();
    SCOPES.get_or_init(Mutex::default)
}
pub(crate) enum ScopeEvidence {
    Pending(crate::model::AuxiliaryAttribution),
    Closed,
    Unknown,
}
pub(crate) fn scope_evidence(
    conversation: &ConversationId,
    correlation: &CorrelationId,
    scope: Option<&crate::model::AuxiliaryAttribution>,
) -> ScopeEvidence {
    let registry = registry().lock().expect("submission scope evidence");
    let matches = |binding: &&ScopeBinding| {
        binding.conversation == *conversation
            && binding.correlation == *correlation
            && scope.is_none_or(|id| binding.id == *id)
    };
    if let Some(binding) = registry.active.values().find(matches) {
        return ScopeEvidence::Pending(binding.id.clone());
    }
    if scope.is_some() && registry.retired.iter().any(|binding| matches(&binding)) {
        return ScopeEvidence::Closed;
    }
    ScopeEvidence::Unknown
}
fn retire(binding: &ScopeBinding) {
    let mut registry = registry().lock().expect("submission scope evidence");
    if registry.active.remove(binding.id.as_str()).is_some() {
        if registry.retired.len() == RETIRED_SCOPE_LIMIT {
            registry.retired.pop_front();
        }
        registry.retired.push_back(binding.clone());
    }
}
/// One scope is carried unchanged across every asynchronous forwarding boundary.
#[derive(Clone)]
pub(crate) struct SubmissionPermit(Arc<Permit>);
struct Permit {
    state: Mutex<PermitState>,
    binding: ScopeBinding,
    authority: Mutex<Option<Arc<Mutex<bool>>>>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum PermitState {
    Open,
    Transferred,
    Closed,
}
impl SubmissionPermit {
    pub(crate) fn new_for(
        conversation: ConversationId,
        correlation: CorrelationId,
    ) -> Result<Self, String> {
        let binding = ScopeBinding {
            id: crate::model::AuxiliaryAttribution::new(format!("scope_{}", uuid::Uuid::now_v7()))
                .expect("bounded native scope"),
            conversation,
            correlation,
        };
        let mut registry = registry().lock().expect("submission scope evidence");
        if registry.active.len() >= SCOPE_LIMIT {
            return Err("auxiliary submission scope capacity is full".into());
        }
        registry
            .active
            .insert(binding.id.as_str().to_owned(), binding.clone());
        Ok(Self(Arc::new(Permit {
            state: Mutex::new(PermitState::Open),
            binding,
            authority: Mutex::new(None),
        })))
    }
    #[cfg(test)]
    fn new() -> Self {
        Self::new_for(
            ConversationId::new("conv_01900000-0000-7000-8000-000000000001"),
            CorrelationId::new(format!("test-{}", uuid::Uuid::now_v7())).unwrap(),
        )
        .unwrap()
    }
    pub(crate) fn id(&self) -> crate::model::AuxiliaryAttribution {
        self.0.binding.id.clone()
    }
    pub(crate) fn bind_authority(&self, authority: Arc<Mutex<bool>>) {
        let mut current = self.0.authority.lock().expect("submission authority");
        if current.is_none() {
            *current = Some(authority);
        }
    }
    /// No await or storage operation may run inside the transfer closure.
    pub(crate) fn transfer<T>(
        &self,
        admit: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        #[cfg(test)]
        {
            let hook = transfer_hooks()
                .lock()
                .unwrap()
                .remove(self.0.binding.correlation.as_str());
            if let Some(hook) = hook {
                hook()?;
            }
        }
        let mut state = self.0.state.lock().expect("submission fence");
        if *state != PermitState::Open {
            return Err("original auxiliary submission scope is closed".into());
        }
        let authority = self
            .0
            .authority
            .lock()
            .expect("submission authority")
            .clone();
        let authority = authority
            .as_ref()
            .map(|gate| gate.lock().expect("attachment authority"));
        if authority.as_ref().is_some_and(|current| !**current) {
            return Err("auxiliary control attachment was detached".into());
        }
        let result = admit()?;
        *state = PermitState::Transferred;
        Ok(result)
    }
    pub(crate) fn close(&self) {
        let mut state = self.0.state.lock().expect("submission fence");
        if *state == PermitState::Open {
            *state = PermitState::Closed;
            retire(&self.0.binding);
        }
    }
    /// Called only after the native worker and all required writers exited.
    pub(crate) fn finish(&self) {
        *self.0.state.lock().expect("submission fence") = PermitState::Closed;
        retire(&self.0.binding);
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        // An untransferred scope cannot ever acquire a replacement permit.
        // A transferred scope is retained by the physically supervised consumer.
        retire(&self.binding);
    }
}

#[cfg(test)]
type TransferHook = Box<dyn FnOnce() -> Result<(), String> + Send>;
#[cfg(test)]
fn transfer_hooks() -> &'static std::sync::Mutex<std::collections::BTreeMap<String, TransferHook>> {
    static HOOKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<String, TransferHook>>,
    > = std::sync::OnceLock::new();
    HOOKS.get_or_init(std::sync::Mutex::default)
}
#[cfg(test)]
pub(crate) fn install_transfer_hook(correlation: String, hook: TransferHook) {
    assert!(
        transfer_hooks()
            .lock()
            .unwrap()
            .insert(correlation, hook)
            .is_none()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    fn classify() -> ClassificationSubmission {
        ClassificationSubmission {
            correlation: CorrelationId::new("command-1".into()).unwrap(),
            input: ClassificationInput::Text {
                text: "Exact text\n".into(),
            },
            labels: vec!["Yes".into(), "No".into()],
        }
    }
    #[test]
    fn canonical_encoding_preserves_semantics_and_excludes_correlation() {
        let target = ConversationId::new("conv_01900000-0000-7000-8000-000000000001");
        let original = classify();
        let expected = original.fingerprint(&target).unwrap();
        let mut changed = original.clone();
        changed.correlation = CorrelationId::new("another-command".into()).unwrap();
        assert_eq!(changed.fingerprint(&target).unwrap(), expected);
        changed.labels.reverse();
        assert_ne!(changed.fingerprint(&target).unwrap(), expected);
        changed = original.clone();
        changed.input = ClassificationInput::Text {
            text: "Exact text".into(),
        };
        assert_ne!(changed.fingerprint(&target).unwrap(), expected);
        assert_ne!(
            original
                .fingerprint(&ConversationId::new(
                    "conv_01900000-0000-7000-8000-000000000002"
                ))
                .unwrap(),
            expected
        );
        changed.labels = vec!["Yes".into(), "Yes".into()];
        assert!(changed.fingerprint(&target).is_err());
    }
    #[test]
    fn closure_and_native_transfer_have_one_ordering() {
        let permit = SubmissionPermit::new();
        let delayed = permit.clone();
        permit.close();
        assert!(delayed.transfer(|| Ok(())).is_err());
        let permit = SubmissionPermit::new();
        assert!(permit.transfer(|| Ok(())).is_ok());
        permit.close();
        assert!(
            permit.transfer(|| Ok(())).is_err(),
            "no replacement permit or second handoff"
        );
    }
}

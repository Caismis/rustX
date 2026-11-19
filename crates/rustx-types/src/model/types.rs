//! Shared model vocabulary types.
use serde::{Deserialize, Serialize};

/// The model interaction protocol an adapter must speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ModelProtocol {
    /// `OpenAI` Chat Completions API.
    #[serde(rename = "openai_chat_completions")]
    OpenAiChatCompletions,
    /// `OpenAI` Responses API.
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
    /// `Anthropic` Messages API.
    AnthropicMessages,
}

/// Ordered semantic content identity, independent of canonical/projected/request occurrence IDs.
/// # Panics
/// Runtime-owned semantic content must serialize.
#[must_use]
pub fn semantic_messages_fingerprint<'a>(
    messages: impl IntoIterator<Item = &'a crate::model::input::ModelInputMessage>,
) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for message in messages {
        append_semantic_message(&mut digest, message);
    }
    format!("{:x}", digest.finalize())
}

pub fn append_semantic_message(
    digest: &mut sha2::Sha256,
    message: &crate::model::input::ModelInputMessage,
) {
    append_semantic_view(digest, &message.view());
}

pub fn semantic_view_fingerprint(view: &crate::model::projected::ModelMessageView<'_>) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    append_semantic_view(&mut digest, view);
    format!("{:x}", digest.finalize())
}

fn append_semantic_view(
    digest: &mut sha2::Sha256,
    view: &crate::model::projected::ModelMessageView<'_>,
) {
    use sha2::Digest;
    struct SemanticWriter<'a>(&'a mut sha2::Sha256);
    impl std::io::Write for SemanticWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            #[cfg(any(test, feature = "test-fixtures"))]
            crate::model::hash_work::record(0, bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(SemanticWriter(&mut *digest), &SemanticView(view))
        .expect("semantic input serializes");
    digest.update([0]);
}

// Match the insertion order of the original json! objects. preserve_order is
// explicit in Cargo.toml because this byte representation is durable evidence.
pub struct SemanticView<'a>(pub &'a crate::model::projected::ModelMessageView<'a>);

impl Serialize for SemanticView<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use crate::model::projected::ModelMessageView;
        use crate::tools::types::ModelToolResultContent;
        use serde::ser::SerializeMap;

        let mut object = serializer.serialize_map(None)?;
        match self.0 {
            ModelMessageView::User(content) => {
                object.serialize_entry("role", &"user")?;
                object.serialize_entry("content", content)?;
            }
            ModelMessageView::Assistant(content) => {
                object.serialize_entry("role", &"assistant")?;
                object.serialize_entry("content", content)?;
            }
            ModelMessageView::Tool(call_id, result) => {
                object.serialize_entry("role", &"tool")?;
                object.serialize_entry("call_id", call_id)?;
                match result.model_visible_content() {
                    Ok(ModelToolResultContent::Parts(parts)) => {
                        object.serialize_entry("parts", &parts)?;
                    }
                    Ok(ModelToolResultContent::Text(text)) => {
                        object.serialize_entry("text", &text)?;
                        object.serialize_entry("media", &[(); 0])?;
                    }
                    Err(_) => object.serialize_entry("unsupported_content", &true)?,
                }
            }
            ModelMessageView::RequestOnly(context) => {
                let role = if context.is_assistant() {
                    "assistant"
                } else {
                    "user"
                };
                object.serialize_entry("role", &role)?;
                match context {
                    super::input::RequestOnlyModelContext::AuxiliaryQuestion { text }
                    | super::input::RequestOnlyModelContext::AuxiliaryAnswer { text } => {
                        object.serialize_entry("text", text)?;
                    }
                    super::input::RequestOnlyModelContext::UnresolvedOutputCarryover(carryover) => {
                        object.serialize_entry("text", &carryover.render())?;
                    }
                }
            }
        }
        object.end()
    }
}

/// Providers do not expose identical token metrics; this is the stable
/// common core. Provider SDK usage objects never appear here.
/// Missing provider metrics remain absent; a reported zero is `Some(0)`.
/// These are native u64 quantities, not byte offsets. Observation transports
/// use the existing exact decimal codec without narrowing provider evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModelUsage {
    /// Input tokens consumed by the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Output tokens produced by the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Total tokens, where the provider reports or can derive them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// Optional normalized usage details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<UsageDetails>,
}

impl ModelUsage {
    /// Whether all normalized core metrics were observed or exactly derived.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.input_tokens.is_some() && self.output_tokens.is_some() && self.total_tokens.is_some()
    }
}

/// Optional normalized token details where providers expose them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UsageDetails {
    /// Tokens consumed by reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// Input tokens served from cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
}

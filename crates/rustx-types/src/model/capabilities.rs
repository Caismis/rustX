//! Provider-neutral model capability and compatibility contracts.
//!
//! These are pure data: what input/output modalities a model accepts, and
//! which Chat Completions/Responses wire spellings an endpoint supports.
//! Catalog validation and credential binding own how they are resolved;
//! this module only holds the validated values.

use std::collections::BTreeSet;

use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One semantic content modality of a model capability set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum Modality {
    /// Textual content.
    Text,
    /// Image content.
    Image,
    /// File/document content.
    File,
}

/// The semantic capability set of a model, an adapter/protocol, or the
/// runtime.
///
/// Modalities are structured sets rather than one boolean per modality, so
/// adding a modality never reshapes the contract. The client-visible
/// capability is always an [`intersection`](ModelCapabilities::intersect) of
/// the model claim, the adapter/protocol capability, and the current runtime
/// capability — a raw catalog claim is never advertised.
/// Tool calls are supported by every model and are not a configurable capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ModelCapabilities {
    /// The accepted input modalities.
    pub input_modalities: BTreeSet<Modality>,
    /// The produced output modalities.
    pub output_modalities: BTreeSet<Modality>,
    /// Native API-enforced JSON Schema output for this model, endpoint and protocol.
    /// An omitted claim is conservative; ordinary text requests never opt in implicitly.
    #[serde(default)]
    pub json_schema_output: bool,
}

impl ModelCapabilities {
    /// The capability set of a text-only model.
    #[must_use]
    pub fn text_only() -> Self {
        Self {
            input_modalities: BTreeSet::from([Modality::Text]),
            output_modalities: BTreeSet::from([Modality::Text]),
            json_schema_output: false,
        }
    }

    /// The pointwise intersection of two capability sets.
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Self {
        Self {
            input_modalities: self
                .input_modalities
                .intersection(&other.input_modalities)
                .copied()
                .collect(),
            output_modalities: self
                .output_modalities
                .intersection(&other.output_modalities)
                .copied()
                .collect(),
            json_schema_output: self.json_schema_output && other.json_schema_output,
        }
    }

    /// Whether the set can carry the ordinary rustX text conversation path.
    #[must_use]
    pub fn supports_text_conversation(&self) -> bool {
        self.input_modalities.contains(&Modality::Text)
            && self.output_modalities.contains(&Modality::Text)
    }
}

/// Which max-token field spelling a Chat Completions service accepts.
///
/// This is a real structural translation difference between
/// OpenAI-compatible services, not a provider wire value: the two spellings
/// are mutually exclusive and both are runtime-protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ChatMaxTokensField {
    /// The current `max_completion_tokens` field.
    #[default]
    MaxCompletionTokens,
    /// The `max_tokens` field used by `DeepSeek`, GLM, Kimi and Qwen APIs.
    MaxTokens,
}

impl ChatMaxTokensField {
    /// The wire field name this spelling writes.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::MaxCompletionTokens => "max_completion_tokens",
            Self::MaxTokens => "max_tokens",
        }
    }
}

/// How the `OpenAI` Responses protocol operates with provider storage.
///
/// Stored requests provider retention and continues by `previous_response_id`.
/// If the response reports `store: false`, the original output items are used
/// instead. Stateless always continues by preserved output items (encrypted
/// reasoning or a provider's original plaintext reasoning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ResponsesStorageMode {
    /// Request provider-side storage (`store: true`).
    #[default]
    Stored,
    /// Zero provider retention (`store: false`).
    Stateless,
}

/// Assistant-message field used to replay canonical reasoning through an
/// OpenAI-compatible Chat Completions dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ChatReasoningReplay {
    /// vLLM and `OpenRouter`'s plaintext reasoning field.
    Reasoning,
    /// `DeepSeek` V4, GLM, Qwen, and compatible preserved-thinking APIs.
    #[default]
    ReasoningContent,
    /// Do not replay historical canonical reasoning to the provider.
    Omit,
}

impl ChatReasoningReplay {
    /// Returns the assistant-message wire field, if reasoning is replayed.
    #[must_use]
    pub const fn wire_name(self) -> Option<&'static str> {
        match self {
            Self::Reasoning => Some("reasoning"),
            Self::ReasoningContent => Some("reasoning_content"),
            Self::Omit => None,
        }
    }
}

/// The in-band tool protocol a Chat Completions model speaks.
///
/// This is a real protocol difference between OpenAI-compatible services,
/// not a provider wire value. Most services emit tool calls only through the
/// structured `tool_calls` field. Some model families additionally have a
/// *reserved in-band* tool syntax that the serving stack is supposed to parse
/// out of the generated text; when that parse fails, the reserved markup
/// leaks into ordinary content or reasoning and the request terminates as if
/// the model had simply answered.
///
/// Declaring the dialect is what allows the adapter to recognize such a leak
/// as malformed tool intent instead of guessing from arbitrary text. Nothing
/// is ever inferred from a provider name or a base URL hostname.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ChatToolProtocol {
    /// Tool calls exist only as structured `tool_calls`. Generated text is
    /// never inspected for tool-protocol markup.
    #[default]
    Native,
    /// The Qwen XML tool dialect (`<tool_call>`, `<function=…>`,
    /// `<parameter=…>`), as served by vLLM and compatible stacks.
    QwenXml,
}

impl ChatToolProtocol {
    /// The human-readable dialect name used in runtime diagnostics. The
    /// stable wire value is the serde representation, not this string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::QwenXml => "Qwen XML",
        }
    }
}

/// The bounded structural protocol-translation metadata of one model.
///
/// Every field here corresponds to a translation branch the current
/// adapters actually take. There is intentionally no strategy trait, no
/// plugin registry, no translator factory, and no JSON-driven
/// transformation. Optional overrides take precedence over the built-in
/// protocol defaults and endpoint/model rules resolved at the binding boundary.
#[derive(Debug, Clone, Copy, Default, schemars::JsonSchema)]
#[schemars(with = "ModelCompatDocument")]
pub struct ModelCompat {
    /// Chat Completions: which max-token field spelling is legal.
    pub chat_max_tokens_field: ChatMaxTokensField,
    /// Chat Completions: whether the service supports
    /// `stream_options.include_usage`.
    pub chat_stream_usage: ChatStreamUsage,
    /// Chat Completions: how previous assistant reasoning is replayed.
    /// Omission uses `reasoning_content`; this does not enable thinking.
    pub chat_reasoning_replay: Option<ChatReasoningReplay>,
    /// Chat Completions: the model's in-band tool protocol, if it has one.
    pub chat_tool_protocol: ChatToolProtocol,
    /// Responses: the provider storage/continuation mode.
    pub responses_storage: ResponsesStorageMode,
    /// Bitset recording which compat fields were explicitly present in the
    /// catalog, including fields equal to their defaults.
    #[doc(hidden)]
    pub explicit_fields: u8,
}

impl PartialEq for ModelCompat {
    fn eq(&self, other: &Self) -> bool {
        self.chat_max_tokens_field == other.chat_max_tokens_field
            && self.chat_stream_usage == other.chat_stream_usage
            && self.chat_reasoning_replay == other.chat_reasoning_replay
            && self.chat_tool_protocol == other.chat_tool_protocol
            && self.responses_storage == other.responses_storage
    }
}

impl Eq for ModelCompat {}

impl Serialize for ModelCompat {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let chat_max_tokens_field_is_present = self.chat_max_tokens_field_is_explicit();
        let chat_stream_usage_is_present = self.chat_stream_usage_is_explicit();
        let chat_reasoning_replay_is_present = self.chat_reasoning_replay.is_some();
        let chat_tool_protocol_is_present = self.chat_tool_protocol_is_explicit();
        let responses_storage_is_present = self.responses_storage_is_explicit();
        let field_count = usize::from(chat_max_tokens_field_is_present)
            + usize::from(chat_stream_usage_is_present)
            + usize::from(chat_reasoning_replay_is_present)
            + usize::from(chat_tool_protocol_is_present)
            + usize::from(responses_storage_is_present);
        let mut state = serializer.serialize_struct("ModelCompat", field_count)?;
        if chat_max_tokens_field_is_present {
            state.serialize_field("chatMaxTokensField", &self.chat_max_tokens_field)?;
        }
        if chat_stream_usage_is_present {
            state.serialize_field("chatStreamUsage", &self.chat_stream_usage)?;
        }
        if let Some(chat_reasoning_replay) = self.chat_reasoning_replay {
            state.serialize_field("chatReasoningReplay", &chat_reasoning_replay)?;
        }
        if chat_tool_protocol_is_present {
            state.serialize_field("chatToolProtocol", &self.chat_tool_protocol)?;
        }
        if responses_storage_is_present {
            state.serialize_field("responsesStorage", &self.responses_storage)?;
        }
        state.end()
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelCompatDocument {
    #[serde(default)]
    chat_max_tokens_field: Option<ChatMaxTokensField>,
    #[serde(default)]
    chat_stream_usage: Option<ChatStreamUsage>,
    #[serde(default)]
    chat_reasoning_replay: Option<ChatReasoningReplay>,
    #[serde(default)]
    chat_tool_protocol: Option<ChatToolProtocol>,
    #[serde(default)]
    responses_storage: Option<ResponsesStorageMode>,
}

impl<'de> Deserialize<'de> for ModelCompat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let document = ModelCompatDocument::deserialize(deserializer)?;
        let explicit_fields = u8::from(document.chat_max_tokens_field.is_some())
            | (u8::from(document.chat_stream_usage.is_some()) << 1)
            | (u8::from(document.chat_reasoning_replay.is_some()) << 2)
            | (u8::from(document.responses_storage.is_some()) << 3)
            | (u8::from(document.chat_tool_protocol.is_some()) << 4);
        Ok(Self {
            chat_max_tokens_field: document.chat_max_tokens_field.unwrap_or_default(),
            chat_stream_usage: document.chat_stream_usage.unwrap_or_default(),
            chat_reasoning_replay: document.chat_reasoning_replay,
            chat_tool_protocol: document.chat_tool_protocol.unwrap_or_default(),
            responses_storage: document.responses_storage.unwrap_or_default(),
            explicit_fields,
        })
    }
}

impl ModelCompat {
    const CHAT_MAX_TOKENS_EXPLICIT: u8 = 1;
    const CHAT_STREAM_USAGE_EXPLICIT: u8 = 1 << 1;
    const CHAT_REASONING_REPLAY_EXPLICIT: u8 = 1 << 2;
    const RESPONSES_STORAGE_EXPLICIT: u8 = 1 << 3;
    const CHAT_TOOL_PROTOCOL_EXPLICIT: u8 = 1 << 4;

    /// Resolve optional authoring overrides once, before an invocation is
    /// frozen. Endpoint/model rules never change an explicit declaration.
    #[must_use]
    pub fn chat_max_tokens_field_is_explicit(self) -> bool {
        self.explicit_fields & Self::CHAT_MAX_TOKENS_EXPLICIT != 0
            || self.chat_max_tokens_field != ChatMaxTokensField::default()
    }

    #[must_use]
    pub fn chat_stream_usage_is_explicit(self) -> bool {
        self.explicit_fields & Self::CHAT_STREAM_USAGE_EXPLICIT != 0
            || self.chat_stream_usage != ChatStreamUsage::default()
    }

    #[must_use]
    pub fn chat_reasoning_replay_is_explicit(self) -> bool {
        self.explicit_fields & Self::CHAT_REASONING_REPLAY_EXPLICIT != 0
            || self.chat_reasoning_replay.is_some()
    }

    #[must_use]
    pub fn chat_tool_protocol_is_explicit(self) -> bool {
        self.explicit_fields & Self::CHAT_TOOL_PROTOCOL_EXPLICIT != 0
            || self.chat_tool_protocol != ChatToolProtocol::default()
    }

    #[must_use]
    pub fn responses_storage_is_explicit(self) -> bool {
        self.explicit_fields & Self::RESPONSES_STORAGE_EXPLICIT != 0
            || self.responses_storage != ResponsesStorageMode::default()
    }
}

/// Whether a Chat Completions service supports streaming usage options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ChatStreamUsage {
    /// `stream_options.include_usage` is supported and requested.
    #[default]
    Supported,
    /// The service rejects `stream_options`; the field is omitted.
    Unsupported,
}

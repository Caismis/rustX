//! The runtime-owned validated model catalog (Issue #42).
//!
//! The catalog is the *only* way a rustX local runtime learns that a model
//! exists, which protocol speaks to it, where its provider endpoint is, and
//! which credential authorizes it. There is deliberately no other path:
//!
//! ```text
//! rustx.toml -> RuntimeLayer -> ModelCatalogDocument -> ModelCatalog
//!               (strict TOML)       (native typed)          (validated)
//! ModelCatalog -> ResolvedModelCatalog (host credentials bound at admission)
//! ```
//!
//! # Explicit provider binding
//!
//! A provider declares its `base_url` and its `api_key` source explicitly. A
//! provider *name* carries no endpoint semantics: naming a provider
//! `openai` or `anthropic` never selects an official endpoint, and no
//! adapter constructor can reach the network without a base URL that came
//! from this catalog. The bounded credential syntax is exactly a literal
//! string or a `$ENV_VAR` reference — no shell commands, no OAuth, no
//! keychain, no credential plugins, and no auth profiles.
//!
//! # Credential redaction
//!
//! A resolved credential lives in [`ResolvedCredential`], which has no
//! `Serialize`, a redacted `Debug`, and a redacted `Display`. The only way
//! to read it is [`ResolvedCredential::expose`], which exists solely for the
//! adapter construction boundary. Every client-facing view carries at most
//! the credential *source kind* and the environment variable *name*
//! ([`CredentialSourceView`]), never the value.
//!
//! # What the catalog does not own
//!
//! Provider wire parameters are opaque ([`crate::model::invocation`] owns
//! the overlay and protected-key contract). Reasoning is expressed as
//! model-declared named profiles whose behaviour is exactly their configured
//! `request_params`; the runtime assigns no meaning to a profile name.
//! Structural translation behaviour lives in the bounded [`ModelCompat`],
//! which is deliberately *not* a strategy framework and is never inferred
//! from a hostname. Historical Chat reasoning replay is an explicit
//! model/provider wire contract; it is not a generation-time reasoning
//! control.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::model::invocation::{RequestParams, RequestParamsLayer, validate_request_params_layer};
use crate::model::types::ModelProtocol;

/// The only model-catalog schema version this runtime accepts.
pub const MODEL_CATALOG_SCHEMA_VERSION: u32 = 1;

/// The identity of one catalog provider.
///
/// A provider identity is an opaque local name. It never implies an
/// endpoint, a protocol, a credential store, or a vendor.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(String);

/// The identity of one catalog model within its provider.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelId(String);

/// The identity of one reasoning profile declared by a model.
///
/// The runtime assigns no meaning to the name: `off`, `on`, `low`,
/// `thinking-32k`, and `deep` are all just names whose wire behaviour is
/// exactly the profile's configured `request_params`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[derive(schemars::JsonSchema)]
pub struct ReasoningProfileId(String);

macro_rules! catalog_identity {
    ($name:ident, $what:literal, $reject_slash:literal, $require_non_empty_segments:literal) => {
        impl $name {
            /// Creates the identity without validation.
            ///
            /// Catalog loading always goes through the validating
            /// constructor; this exists for tests and for identities the
            /// runtime already validated.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Validates and creates the identity.
            ///
            /// # Errors
            ///
            /// Returns [`ModelCatalogError::InvalidIdentity`] when the value
            /// is empty, contains whitespace, or contains the `/` reference
            /// separator when this identity type reserves it. Model IDs may
            /// use `/`, but every segment must be non-empty.
            pub fn parse(value: impl Into<String>) -> Result<Self, ModelCatalogError> {
                let value = value.into();
                if value.is_empty()
                    || ($reject_slash && value.contains('/'))
                    || ($require_non_empty_segments && value.split('/').any(str::is_empty))
                    || value.chars().any(char::is_whitespace)
                {
                    return Err(ModelCatalogError::InvalidIdentity { kind: $what, value });
                }
                Ok(Self(value))
            }

            /// The identity as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

catalog_identity!(ProviderId, "provider", true, false);
catalog_identity!(ModelId, "model", false, true);
catalog_identity!(ReasoningProfileId, "reasoning profile", true, false);

/// An authored Model identity. Its spelling has no provider or wire semantics.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, schemars::JsonSchema)]
#[schemars(with = "String")]
pub struct ModelRef(String);

impl ModelRef {
    /// Constructs an explicitly qualified name; qualification is naming only.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)] // Own both explicit naming components at construction.
    pub fn new(provider: ProviderId, model: ModelId) -> Self {
        Self(format!("{provider}/{model}"))
    }

    /// Parses a nonempty catalog identity, independently of Provider bindings.
    /// # Errors
    /// Rejects invalid identity segments.
    pub fn parse(value: &str) -> Result<Self, ModelCatalogError> {
        ModelId::parse(value).map_err(|_| ModelCatalogError::InvalidModelRef {
            value: value.to_owned(),
        })?;
        Ok(Self(value.to_owned()))
    }
}
impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ModelRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ModelRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// The declared source of one provider credential.
///
/// The syntax is exactly two forms and is never extended implicitly: a
/// literal string, or `$ENV_VAR`.
#[derive(Clone, PartialEq, Eq, schemars::JsonSchema)]
#[schemars(with = "String")]
pub enum CredentialSource {
    /// A literal credential written into the catalog file.
    Literal(String),
    /// A credential read from the named process environment variable at
    /// startup.
    Environment(String),
}

impl CredentialSource {
    /// Parses the bounded credential syntax.
    ///
    /// # Errors
    ///
    /// Returns [`ModelCatalogError::InvalidCredentialSource`] for an empty
    /// value or a malformed environment-variable name.
    pub fn parse(value: &str, provider: &ProviderId) -> Result<Self, ModelCatalogError> {
        if value.is_empty() {
            return Err(ModelCatalogError::InvalidCredentialSource {
                provider: provider.clone(),
                detail: "the api_key source must not be empty".to_owned(),
            });
        }
        let Some(name) = value.strip_prefix('$') else {
            return Ok(Self::Literal(value.to_owned()));
        };
        let valid = crate::credentials::valid_environment_name(name);
        if !valid {
            return Err(ModelCatalogError::InvalidCredentialSource {
                provider: provider.clone(),
                detail: "expected a valid $ENV_VAR credential reference".into(),
            });
        }
        Ok(Self::Environment(name.to_owned()))
    }

    /// The redacted client-facing view of this source.
    #[must_use]
    pub fn view(&self) -> CredentialSourceView {
        match self {
            Self::Literal(_) => CredentialSourceView::Literal,
            Self::Environment(name) => CredentialSourceView::Environment {
                variable: name.clone(),
            },
        }
    }
}

impl fmt::Debug for CredentialSource {
    /// Redacted: a literal credential never appears in debug output, and an
    /// environment reference shows only the variable name.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal(_) => f.write_str("CredentialSource::Literal(<redacted>)"),
            Self::Environment(name) => write!(f, "CredentialSource::Environment({name})"),
        }
    }
}

impl Serialize for CredentialSource {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Environment(name) => serializer.serialize_str(&format!("${name}")),
            Self::Literal(_) => serde_json::json!({"redacted": true}).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for CredentialSource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw, &ProviderId::new("configured")).map_err(serde::de::Error::custom)
    }
}

/// The redacted client-facing description of a credential source.
///
/// This is safe to place in Runtime Client results: it carries the source
/// *kind* and, for an environment reference, the variable *name*. It never
/// carries a credential value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum CredentialSourceView {
    /// The credential is a literal catalog value.
    Literal,
    /// The credential is read from the named environment variable.
    Environment {
        /// The environment variable name (never its value).
        variable: String,
    },
}

/// A resolved provider credential.
///
/// The value is deliberately unreachable except through
/// [`ResolvedCredential::expose`]: the type has no `Serialize`, its `Debug`
/// and `Display` are redacted, and it never appears in an error, an event,
/// a snapshot, or a panic message.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedCredential(String);

impl ResolvedCredential {
    /// Wraps a resolved credential value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Exposes the credential for adapter construction.
    ///
    /// This is the one intentional read boundary. Callers must pass the
    /// value straight into a provider client and must never log, format, or
    /// re-expose it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ResolvedCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for ResolvedCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ModelCapabilities {
    /// The accepted input modalities.
    pub input_modalities: BTreeSet<Modality>,
    /// The produced output modalities.
    pub output_modalities: BTreeSet<Modality>,
    /// Whether the model can be given tool definitions and can call them.
    pub tool_calls: bool,
    /// Whether the model semantically supports reasoning.
    pub reasoning: bool,
}

impl ModelCapabilities {
    /// The capability set of a text-only tool-calling reasoning model.
    #[must_use]
    pub fn text_only(tool_calls: bool, reasoning: bool) -> Self {
        Self {
            input_modalities: BTreeSet::from([Modality::Text]),
            output_modalities: BTreeSet::from([Modality::Text]),
            tool_calls,
            reasoning,
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
            tool_calls: self.tool_calls && other.tool_calls,
            reasoning: self.reasoning && other.reasoning,
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
    /// The legacy `max_tokens` field.
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
/// This is continuation *structure*, not a wire value: Stored continues by
/// `previous_response_id`, Stateless continues by preserved output items and
/// requires the encrypted-reasoning `include` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ResponsesStorageMode {
    /// Provider-side storage enabled (`store: true`).
    #[default]
    Stored,
    /// Zero provider retention (`store: false`).
    Stateless,
}

/// Assistant-message field used to replay canonical reasoning through an
/// OpenAI-compatible Chat Completions dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ChatReasoningReplay {
    /// vLLM and `OpenRouter`'s plaintext reasoning field.
    Reasoning,
    /// `DeepSeek` V4, GLM, Qwen, and compatible preserved-thinking APIs.
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
/// transformation, and nothing here is ever inferred from a provider name or
/// a base URL hostname.
#[derive(Debug, Clone, Copy, Default, schemars::JsonSchema)]
#[schemars(with = "ModelCompatDocument")]
pub struct ModelCompat {
    /// Chat Completions: which max-token field spelling is legal.
    pub chat_max_tokens_field: ChatMaxTokensField,
    /// Chat Completions: whether the service supports
    /// `stream_options.include_usage`.
    pub chat_stream_usage: ChatStreamUsage,
    /// Chat Completions: how previous assistant reasoning is replayed. This
    /// is required for catalog models using Chat Completions and is absent
    /// for models whose protocol does not use this dialect.
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

impl From<super::authoring::Compat> for ModelCompat {
    fn from(document: super::authoring::Compat) -> Self {
        let explicit_fields = u8::from(document.chat_max_tokens_field.is_some())
            | (u8::from(document.chat_stream_usage.is_some()) << 1)
            | (u8::from(document.chat_reasoning_replay.is_some()) << 2)
            | (u8::from(document.responses_storage.is_some()) << 3)
            | (u8::from(document.chat_tool_protocol.is_some()) << 4);
        Self {
            chat_max_tokens_field: document.chat_max_tokens_field.unwrap_or_default(),
            chat_stream_usage: document.chat_stream_usage.unwrap_or_default(),
            chat_reasoning_replay: document.chat_reasoning_replay,
            chat_tool_protocol: document.chat_tool_protocol.unwrap_or_default(),
            responses_storage: document.responses_storage.unwrap_or_default(),
            explicit_fields,
        }
    }
}

impl ModelCompat {
    const CHAT_MAX_TOKENS_EXPLICIT: u8 = 1;
    const CHAT_STREAM_USAGE_EXPLICIT: u8 = 1 << 1;
    const CHAT_REASONING_REPLAY_EXPLICIT: u8 = 1 << 2;
    const RESPONSES_STORAGE_EXPLICIT: u8 = 1 << 3;
    const CHAT_TOOL_PROTOCOL_EXPLICIT: u8 = 1 << 4;

    fn chat_max_tokens_field_is_explicit(self) -> bool {
        self.explicit_fields & Self::CHAT_MAX_TOKENS_EXPLICIT != 0
            || self.chat_max_tokens_field != ChatMaxTokensField::default()
    }

    fn chat_stream_usage_is_explicit(self) -> bool {
        self.explicit_fields & Self::CHAT_STREAM_USAGE_EXPLICIT != 0
            || self.chat_stream_usage != ChatStreamUsage::default()
    }

    fn chat_reasoning_replay_is_explicit(self) -> bool {
        self.explicit_fields & Self::CHAT_REASONING_REPLAY_EXPLICIT != 0
            || self.chat_reasoning_replay.is_some()
    }

    fn chat_tool_protocol_is_explicit(self) -> bool {
        self.explicit_fields & Self::CHAT_TOOL_PROTOCOL_EXPLICIT != 0
            || self.chat_tool_protocol != ChatToolProtocol::default()
    }

    fn responses_storage_is_explicit(self) -> bool {
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

/// One declared reasoning profile of a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ReasoningProfile {
    /// Whether this profile semantically enables reasoning.
    pub enabled: bool,
    /// The exact provider-owned request parameters of this profile.
    ///
    /// The profile owns every top-level key it declares: a session override
    /// may not also declare one of them.
    #[serde(default)]
    pub request_params: RequestParams,
}

/// The reasoning configuration of one model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ReasoningConfig {
    /// The profile selected when the session does not choose one.
    pub default_profile: ReasoningProfileId,
    /// The declared profiles.
    pub profiles: BTreeMap<ReasoningProfileId, ReasoningProfile>,
}

/// One validated catalog model definition.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDefinition {
    /// Explicit provider binding, independent of the catalog name.
    pub provider: ProviderId,
    /// The model identity within its provider.
    pub id: ModelId,
    /// The protocol an adapter must speak to this model.
    pub protocol: ModelProtocol,
    /// The model context window in tokens.
    pub context_window: u64,
    /// The configured maximum output tokens.
    pub max_output_tokens: u32,
    /// The capabilities the catalog claims for this model.
    pub capabilities: ModelCapabilities,
    /// The model-level default provider request parameters.
    pub request_params: RequestParams,
    /// The declared reasoning profiles, when the model exposes any.
    pub reasoning: Option<ReasoningConfig>,
    /// The bounded structural translation metadata.
    pub compat: ModelCompat,
}

impl ModelDefinition {
    /// The reasoning profile the session selects by default, when the model
    /// declares reasoning profiles.
    #[must_use]
    pub fn default_reasoning_profile(&self) -> Option<&ReasoningProfileId> {
        self.reasoning
            .as_ref()
            .map(|config| &config.default_profile)
    }

    /// Resolves one declared profile by identity.
    #[must_use]
    pub fn reasoning_profile(&self, id: &ReasoningProfileId) -> Option<&ReasoningProfile> {
        self.reasoning
            .as_ref()
            .and_then(|config| config.profiles.get(id))
    }
}

/// One validated catalog provider (before credential resolution).
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderDefinition {
    /// The provider identity.
    pub id: ProviderId,
    /// The mandatory explicit provider endpoint.
    pub base_url: String,
    /// The declared credential source.
    pub api_key: CredentialSource,
}

/// The validated model catalog.
///
/// Validation is complete at this point: every provider has an explicit
/// endpoint and credential source, every model has a known protocol, sane
/// limits, coherent capabilities, a valid reasoning configuration, and
/// request parameters that collide with no runtime-protected wire key.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCatalog {
    providers: BTreeMap<ProviderId, ProviderDefinition>,
    models: BTreeMap<ModelRef, Arc<ModelDefinition>>,
}

impl ModelCatalog {
    /// Canonical credential-free catalog projection for authoring and live selection.
    #[must_use]
    pub fn view(&self) -> ModelCatalogView {
        let mut models = Vec::new();
        for reference in self.model_refs() {
            let model = &self.models[&reference];
            let provider = &self.providers[&model.provider];
            models.push(CatalogModelView {
                model: reference,
                protocol: model.protocol,
                context_window: model.context_window,
                max_output_tokens: model.max_output_tokens,
                declared_capabilities: model.capabilities.clone(),
                effective_capabilities: super::invocation::effective_capabilities(
                    &model.capabilities,
                    model.protocol,
                ),
                reasoning_profiles: model
                    .reasoning
                    .as_ref()
                    .map(|reasoning| {
                        reasoning
                            .profiles
                            .iter()
                            .map(|(id, profile)| ReasoningProfileView {
                                id: id.clone(),
                                enabled: profile.enabled,
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                default_reasoning_profile: model.default_reasoning_profile().cloned(),
                credential_source: provider.api_key.view(),
            });
        }
        ModelCatalogView { models }
    }

    /// Parses and validates a catalog from TOML bytes.
    ///
    /// Strict `snake_case` authoring types resolve into native configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ModelCatalogError::Syntax`] for malformed TOML (including
    /// unknown fields and duplicate provider identities) and a specific
    /// validation error otherwise.
    pub fn from_toml_slice(bytes: &[u8]) -> Result<Self, ModelCatalogError> {
        let document: super::authoring::Catalog = crate::toml_authoring::parse(bytes)
            .map_err(|detail| ModelCatalogError::Syntax { detail })?;
        Self::from_document(document.into())
    }

    /// Validates an already-parsed catalog document.
    ///
    /// # Errors
    ///
    /// Returns the first validation failure.
    pub fn from_document(document: ModelCatalogDocument) -> Result<Self, ModelCatalogError> {
        if document.schema_version != MODEL_CATALOG_SCHEMA_VERSION {
            return Err(ModelCatalogError::UnsupportedSchemaVersion {
                supported: MODEL_CATALOG_SCHEMA_VERSION,
                found: document.schema_version,
            });
        }
        if document.providers.is_empty() {
            return Err(ModelCatalogError::EmptyCatalog);
        }
        let mut providers = BTreeMap::new();
        for (raw_id, provider) in document.providers {
            let id = ProviderId::parse(raw_id)?;
            let definition = validate_provider(&id, provider)?;
            providers.insert(id, definition);
        }
        let mut models = BTreeMap::new();
        for (name, document) in document.models {
            let reference = ModelRef::parse(&name)?;
            let provider = ProviderId::parse(&document.provider)?;
            if !providers.contains_key(&provider) {
                return Err(ModelCatalogError::UnknownProvider { provider });
            }
            models.insert(
                reference.clone(),
                Arc::new(validate_model(&reference, document)?),
            );
        }
        if models.is_empty() {
            return Err(ModelCatalogError::EmptyCatalog);
        }
        Ok(Self { providers, models })
    }

    /// The providers in deterministic identity order.
    pub fn providers(&self) -> impl Iterator<Item = &ProviderDefinition> {
        self.providers.values()
    }

    /// Resolves one model reference.
    ///
    /// # Errors
    ///
    /// Returns [`ModelCatalogError::UnknownProvider`] or
    /// [`ModelCatalogError::UnknownModel`].
    pub fn model(&self, reference: &ModelRef) -> Result<&Arc<ModelDefinition>, ModelCatalogError> {
        self.models
            .get(reference)
            .ok_or_else(|| ModelCatalogError::UnknownModel {
                model: reference.clone(),
            })
    }

    /// Every model reference in deterministic presentation order.
    pub fn model_refs(&self) -> impl Iterator<Item = ModelRef> + '_ {
        self.models.keys().cloned()
    }

    /// Captures credential inputs for every provider without requiring unused keys.
    ///
    /// # Errors
    ///
    /// Kept as a fallible binding boundary; missing references are diagnosed
    /// lazily by `ResolvedProvider::credential` when a provider is admitted.
    pub fn resolve(
        &self,
        environment: &dyn CredentialEnvironment,
    ) -> Result<ResolvedModelCatalog, ModelCatalogError> {
        let mut providers = BTreeMap::new();
        let captured = environment.snapshot();
        for (id, provider) in &self.providers {
            providers.insert(
                id.clone(),
                ResolvedProvider {
                    id: id.clone(),
                    base_url: provider.base_url.clone(),
                    credential: std::sync::Arc::new(std::sync::OnceLock::new()),
                    captured: captured.clone(),
                    source: provider.api_key.clone(),
                },
            );
        }
        Ok(ResolvedModelCatalog {
            catalog: self.clone(),
            providers,
        })
    }
}

/// One provider with its credential bound.
#[derive(Clone)]
pub struct ResolvedProvider {
    id: ProviderId,
    base_url: String,
    credential: std::sync::Arc<std::sync::OnceLock<Result<ResolvedCredential, ModelCatalogError>>>,
    captured: crate::credentials::CredentialSnapshot,
    source: CredentialSource,
}

impl ResolvedProvider {
    /// The provider identity.
    #[must_use]
    pub const fn id(&self) -> &ProviderId {
        &self.id
    }

    /// The explicit provider endpoint.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The bound credential.
    ///
    /// # Errors
    /// Returns a provider-specific missing-reference error on first admitted use.
    pub fn credential(&self) -> Result<&ResolvedCredential, ModelCatalogError> {
        self.credential
            .get_or_init(|| match &self.source {
                CredentialSource::Literal(value) => Ok(ResolvedCredential::new(value.clone())),
                CredentialSource::Environment(name) => self
                    .captured
                    .var(name)
                    .filter(|value| !value.is_empty())
                    .map(ResolvedCredential::new)
                    .ok_or_else(|| ModelCatalogError::MissingEnvironmentCredential {
                        provider: self.id.clone(),
                        variable: name.clone(),
                    }),
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The redacted credential source view.
    #[must_use]
    pub fn credential_source(&self) -> CredentialSourceView {
        self.source.view()
    }

    /// The **declared** credential source of this provider.
    ///
    /// This is catalog configuration, not a resolved secret: it is the same
    /// bounded two-form value the catalog file declared. It exists so a
    /// frozen provider binding can carry the declaration to another process
    /// that resolves it through its own [`CredentialEnvironment`], instead
    /// of that process reopening the mutable catalog. Use
    /// [`ResolvedProvider::credential_source`] for anything client-facing.
    #[must_use]
    pub const fn credential_declaration(&self) -> &CredentialSource {
        &self.source
    }
}

impl fmt::Debug for ResolvedProvider {
    /// Redacted: the bound credential never appears in debug output.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedProvider")
            .field("id", &self.id)
            .field("base_url", &self.base_url)
            .field("credential", &"<redacted>")
            .field("captured", &self.captured)
            .field("credential_source", &self.source)
            .finish()
    }
}

/// The catalog with every provider credential bound.
#[derive(Clone, Debug)]
pub struct ResolvedModelCatalog {
    catalog: ModelCatalog,
    providers: BTreeMap<ProviderId, ResolvedProvider>,
}

impl ResolvedModelCatalog {
    /// The underlying validated catalog.
    #[must_use]
    pub const fn catalog(&self) -> &ModelCatalog {
        &self.catalog
    }

    /// Resolves one provider binding.
    ///
    /// # Errors
    ///
    /// Returns [`ModelCatalogError::UnknownProvider`].
    pub fn provider(&self, id: &ProviderId) -> Result<&ResolvedProvider, ModelCatalogError> {
        self.providers
            .get(id)
            .ok_or_else(|| ModelCatalogError::UnknownProvider {
                provider: id.clone(),
            })
    }

    /// Resolves one model reference to its provider binding and definition.
    ///
    /// # Errors
    ///
    /// Returns [`ModelCatalogError::UnknownProvider`] or
    /// [`ModelCatalogError::UnknownModel`].
    pub fn binding(
        &self,
        reference: &ModelRef,
    ) -> Result<(&ResolvedProvider, &Arc<ModelDefinition>), ModelCatalogError> {
        let model = self.catalog.model(reference)?;
        let provider = self.provider(&model.provider)?;
        Ok((provider, model))
    }

    /// Every model reference in deterministic order.
    pub fn model_refs(&self) -> impl Iterator<Item = ModelRef> + '_ {
        self.catalog.model_refs()
    }
}

/// The process-environment lookup used to resolve `$ENV_VAR` credentials.
pub trait CredentialEnvironment: Send + Sync {
    /// Capture a stable host input without resolving a particular reference.
    fn snapshot(&self) -> crate::credentials::CredentialSnapshot;
    /// Reads one environment variable.
    fn var(&self, name: &str) -> Option<String>;
}

/// The real process environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessCredentialEnvironment;

impl CredentialEnvironment for ProcessCredentialEnvironment {
    fn snapshot(&self) -> crate::credentials::CredentialSnapshot {
        crate::credentials::CredentialSnapshot::capture()
    }
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// An explicit in-memory environment, used by deterministic tests and by
/// callers that resolve credentials from an already-collected map.
#[derive(Clone, Default)]
pub struct MapCredentialEnvironment {
    variables: BTreeMap<String, String>,
}

impl fmt::Debug for MapCredentialEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MapCredentialEnvironment(<redacted>)")
    }
}

impl MapCredentialEnvironment {
    /// Creates an environment from name/value pairs.
    #[must_use]
    pub fn new(variables: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            variables: variables.into_iter().collect(),
        }
    }
}

impl CredentialEnvironment for MapCredentialEnvironment {
    fn snapshot(&self) -> crate::credentials::CredentialSnapshot {
        crate::credentials::CredentialSnapshot::new(self.variables.clone())
    }
    fn var(&self, name: &str) -> Option<String> {
        self.variables.get(name).cloned()
    }
}

// ---------------------------------------------------------------------------
// Document (wire) shapes
// ---------------------------------------------------------------------------

/// The native programmatic catalog document. TOML authoring is owned by
/// [`super::authoring::Catalog`] and resolves into this typed representation.
///
/// Unknown fields are rejected everywhere: a typo must fail loudly rather
/// than silently changing runtime semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ModelCatalogDocument {
    /// The catalog schema version.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// The providers, keyed by provider identity. Duplicate keys are
    /// rejected rather than silently resolved by last-write-wins.
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub providers: BTreeMap<String, ProviderDocument>,
    /// Independently named complete Model definitions.
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub models: BTreeMap<String, ModelDocument>,
}

const fn default_schema_version() -> u32 {
    MODEL_CATALOG_SCHEMA_VERSION
}

/// One provider entry of the catalog document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ProviderDocument {
    /// The mandatory explicit provider endpoint.
    pub base_url: String,
    /// The mandatory credential source: a literal or `$ENV_VAR`.
    pub api_key: CredentialSource,
}

/// One model entry of the catalog document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ModelDocument {
    /// Explicit Provider identity.
    pub provider: String,
    /// The model identity within its provider.
    pub id: String,
    /// The protocol an adapter must speak.
    pub protocol: ModelProtocol,
    /// The model context window in tokens.
    pub context_window: u64,
    /// The configured maximum output tokens.
    pub max_output_tokens: u32,
    /// The claimed capabilities.
    pub capabilities: ModelCapabilities,
    /// The model-level default provider request parameters.
    #[serde(default)]
    pub request_params: RequestParams,
    /// The declared reasoning profiles.
    #[serde(default)]
    pub reasoning: Option<ReasoningConfig>,
    /// The bounded structural translation metadata.
    #[serde(default)]
    pub compat: ModelCompat,
}

/// Deserializes a map, rejecting duplicate keys instead of keeping the last.
fn deserialize_unique_map<'de, D, V>(deserializer: D) -> Result<BTreeMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct UniqueMap<V>(std::marker::PhantomData<V>);

    impl<'de, V: Deserialize<'de>> Visitor<'de> for UniqueMap<V> {
        type Value = BTreeMap<String, V>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a map with unique keys")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut out = BTreeMap::new();
            while let Some((key, value)) = access.next_entry::<String, V>()? {
                if out.contains_key(&key) {
                    return Err(serde::de::Error::custom(format!("duplicate key {key:?}")));
                }
                out.insert(key, value);
            }
            Ok(out)
        }
    }

    deserializer.deserialize_map(UniqueMap(std::marker::PhantomData))
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

fn validate_provider(
    id: &ProviderId,
    document: ProviderDocument,
) -> Result<ProviderDefinition, ModelCatalogError> {
    validate_base_url(id, &document.base_url)?;
    let api_key = document.api_key.clone();
    Ok(ProviderDefinition {
        id: id.clone(),
        base_url: document.base_url,
        api_key,
    })
}

/// Validates that the endpoint is an explicit absolute HTTP(S) URL.
///
/// The check is deliberately structural only: nothing about a hostname ever
/// changes runtime behaviour.
fn validate_base_url(provider: &ProviderId, base_url: &str) -> Result<(), ModelCatalogError> {
    let valid = url::Url::parse(base_url).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && !url.cannot_be_a_base()
    });
    if !valid {
        return Err(ModelCatalogError::InvalidBaseUrl {
            provider: provider.clone(),
            base_url: base_url.to_owned(),
        });
    }
    Ok(())
}

fn validate_model(
    reference: &ModelRef,
    document: ModelDocument,
) -> Result<ModelDefinition, ModelCatalogError> {
    let id = ModelId::parse(document.id)?;
    let reference = reference.clone();

    if document.context_window == 0 || document.max_output_tokens == 0 {
        return Err(ModelCatalogError::InvalidLimits {
            model: reference,
            detail: "context_window and max_output_tokens must both be positive".to_owned(),
        });
    }
    if u64::from(document.max_output_tokens) >= document.context_window {
        return Err(ModelCatalogError::InvalidLimits {
            model: reference,
            detail: format!(
                "max_output_tokens {} must be smaller than context_window {}",
                document.max_output_tokens, document.context_window
            ),
        });
    }
    if document.capabilities.input_modalities.is_empty()
        || document.capabilities.output_modalities.is_empty()
    {
        return Err(ModelCatalogError::InvalidCapabilities {
            model: reference,
            detail:
                "input_modalities and output_modalities must each declare at least one modality"
                    .to_owned(),
        });
    }
    if !document.capabilities.supports_text_conversation() {
        return Err(ModelCatalogError::InvalidCapabilities {
            model: reference,
            detail: "a rustX conversation model must declare text input and text output".to_owned(),
        });
    }

    validate_compat(&reference, document.protocol, document.compat)?;

    validate_request_params_layer(
        &document.request_params,
        document.protocol,
        RequestParamsLayer::ModelDefaults,
    )
    .map_err(|collision| ModelCatalogError::ProtectedKey {
        model: reference.clone(),
        key: collision.key,
        layer: collision.layer,
    })?;

    if let Some(reasoning) = &document.reasoning {
        validate_reasoning(
            &reference,
            document.protocol,
            &document.capabilities,
            reasoning,
        )?;
    }

    Ok(ModelDefinition {
        provider: ProviderId::parse(document.provider)?,
        id,
        protocol: document.protocol,
        context_window: document.context_window,
        max_output_tokens: document.max_output_tokens,
        capabilities: document.capabilities,
        request_params: document.request_params,
        reasoning: document.reasoning,
        compat: document.compat,
    })
}

fn validate_compat(
    reference: &ModelRef,
    protocol: ModelProtocol,
    compat: ModelCompat,
) -> Result<(), ModelCatalogError> {
    let invalid_detail = match protocol {
        ModelProtocol::OpenAiChatCompletions if compat.chat_reasoning_replay.is_none() => Some(
            "compat.chat_reasoning_replay is required for openai_chat_completions and must be one of reasoning, reasoning_content, or omit"
                .to_owned(),
        ),
        ModelProtocol::OpenAiChatCompletions if compat.responses_storage_is_explicit() => Some(
            format!("compat declares fields that do not apply to protocol {protocol:?}"),
        ),
        ModelProtocol::OpenAiResponses
            if compat.chat_max_tokens_field_is_explicit()
                || compat.chat_stream_usage_is_explicit()
                || compat.chat_reasoning_replay_is_explicit()
                || compat.chat_tool_protocol_is_explicit() =>
        {
            Some(format!(
                "compat declares fields that do not apply to protocol {protocol:?}"
            ))
        }
        ModelProtocol::AnthropicMessages
            if compat.chat_max_tokens_field_is_explicit()
                || compat.chat_stream_usage_is_explicit()
                || compat.chat_reasoning_replay_is_explicit()
                || compat.chat_tool_protocol_is_explicit()
                || compat.responses_storage_is_explicit() =>
        {
            Some(format!(
                "compat declares fields that do not apply to protocol {protocol:?}"
            ))
        }
        ModelProtocol::OpenAiChatCompletions
        | ModelProtocol::OpenAiResponses
        | ModelProtocol::AnthropicMessages => None,
    };
    if let Some(detail) = invalid_detail {
        return Err(ModelCatalogError::InvalidCompat {
            model: reference.clone(),
            detail,
        });
    }
    Ok(())
}

fn validate_reasoning(
    reference: &ModelRef,
    protocol: ModelProtocol,
    capabilities: &ModelCapabilities,
    reasoning: &ReasoningConfig,
) -> Result<(), ModelCatalogError> {
    if reasoning.profiles.is_empty() {
        return Err(ModelCatalogError::InvalidReasoning {
            model: reference.clone(),
            detail: "a declared reasoning block must declare at least one profile".to_owned(),
        });
    }
    if !reasoning.profiles.contains_key(&reasoning.default_profile) {
        return Err(ModelCatalogError::InvalidReasoning {
            model: reference.clone(),
            detail: format!(
                "default_profile {:?} is not declared in profiles",
                reasoning.default_profile.as_str()
            ),
        });
    }
    for (id, profile) in &reasoning.profiles {
        if profile.enabled && !capabilities.reasoning {
            return Err(ModelCatalogError::InvalidReasoning {
                model: reference.clone(),
                detail: format!(
                    "profile {:?} enables reasoning but capabilities.reasoning is false",
                    id.as_str()
                ),
            });
        }
        validate_request_params_layer(
            &profile.request_params,
            protocol,
            RequestParamsLayer::ReasoningProfile,
        )
        .map_err(|collision| ModelCatalogError::ProtectedKey {
            model: reference.clone(),
            key: collision.key,
            layer: collision.layer,
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A model-catalog load, validation, or resolution failure.
///
/// No variant ever carries a credential value; an environment-backed
/// credential is identified by its variable name only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCatalogError {
    /// The catalog input is malformed or does not satisfy its typed schema.
    Syntax {
        /// The parser detail.
        detail: String,
    },
    /// The document declares a schema version this runtime does not speak.
    UnsupportedSchemaVersion {
        /// The version this runtime supports.
        supported: u32,
        /// The version found in the document.
        found: u32,
    },
    /// The catalog declares no provider.
    EmptyCatalog,
    /// A provider or reasoning-profile identity is empty, contains whitespace,
    /// or contains `/`. Model identities may contain `/` because the first
    /// slash in a model reference is the provider separator, but their
    /// slash-separated segments must be non-empty.
    InvalidIdentity {
        /// What kind of identity failed.
        kind: &'static str,
        /// The offending value.
        value: String,
    },
    /// A model reference is not of the form `provider-id/model-id`.
    InvalidModelRef {
        /// The offending value.
        value: String,
    },
    /// A provider endpoint is missing or is not an absolute HTTP(S) URL.
    InvalidBaseUrl {
        /// The provider.
        provider: ProviderId,
        /// The offending value.
        base_url: String,
    },
    /// A provider credential source is malformed.
    InvalidCredentialSource {
        /// The provider.
        provider: ProviderId,
        /// The failure detail (never a credential value).
        detail: String,
    },
    /// A provider declares no model.
    ProviderWithoutModels {
        /// The provider.
        provider: ProviderId,
    },
    /// Two models of one provider share an identity.
    DuplicateModel {
        /// The duplicated reference.
        model: ModelRef,
    },
    /// A model declares impossible context/output limits.
    InvalidLimits {
        /// The model.
        model: ModelRef,
        /// The failure detail.
        detail: String,
    },
    /// A model declares an unusable capability set.
    InvalidCapabilities {
        /// The model.
        model: ModelRef,
        /// The failure detail.
        detail: String,
    },
    /// A model declares an invalid reasoning configuration.
    InvalidReasoning {
        /// The model.
        model: ModelRef,
        /// The failure detail.
        detail: String,
    },
    /// A model declares bounded compatibility fields for a foreign protocol.
    InvalidCompat {
        /// The model.
        model: ModelRef,
        /// The validation detail.
        detail: String,
    },
    /// A configured request-parameter layer collides with a runtime-owned
    /// protected wire key.
    ProtectedKey {
        /// The model.
        model: ModelRef,
        /// The colliding key.
        key: String,
        /// Which configuration layer declared it.
        layer: RequestParamsLayer,
    },
    /// A referenced provider does not exist.
    UnknownProvider {
        /// The referenced provider.
        provider: ProviderId,
    },
    /// A referenced model does not exist.
    UnknownModel {
        /// The referenced model.
        model: ModelRef,
    },
    /// A `$ENV_VAR` credential is absent or empty in the process
    /// environment.
    MissingEnvironmentCredential {
        /// The provider whose credential is unresolved.
        provider: ProviderId,
        /// The environment variable name (never a value).
        variable: String,
    },
}

impl fmt::Display for ModelCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { detail } => write!(f, "malformed model catalog: {detail}"),
            Self::UnsupportedSchemaVersion { supported, found } => write!(
                f,
                "unsupported model catalog schema_version {found}; this runtime speaks {supported}"
            ),
            Self::EmptyCatalog => f.write_str("the model catalog declares no provider"),
            Self::InvalidIdentity { kind, value } => {
                write!(f, "invalid {kind} identity {value:?}")
            }
            Self::InvalidModelRef { value } => write!(
                f,
                "invalid model reference {value:?}; expected \"provider-id/model-id\""
            ),
            Self::InvalidBaseUrl { provider, base_url } => write!(
                f,
                "provider {provider} declares an invalid base_url {base_url:?}; \
                 an explicit absolute http(s) endpoint is mandatory"
            ),
            Self::InvalidCredentialSource { provider, detail } => {
                write!(f, "provider {provider} api_key source is invalid: {detail}")
            }
            Self::ProviderWithoutModels { provider } => {
                write!(f, "provider {provider} declares no model")
            }
            Self::DuplicateModel { model } => write!(f, "duplicate model identity {model}"),
            Self::InvalidLimits { model, detail } => {
                write!(f, "model {model} declares invalid limits: {detail}")
            }
            Self::InvalidCapabilities { model, detail } => {
                write!(f, "model {model} declares invalid capabilities: {detail}")
            }
            Self::InvalidReasoning { model, detail } => {
                write!(f, "model {model} declares invalid reasoning: {detail}")
            }
            Self::InvalidCompat { model, detail } => {
                write!(f, "model {model} declares invalid compat: {detail}")
            }
            Self::ProtectedKey { model, key, layer } => write!(
                f,
                "model {model} {layer} (request_params) declares runtime-owned protected wire key {key:?}"
            ),
            Self::UnknownProvider { provider } => {
                write!(f, "unknown catalog provider {provider}")
            }
            Self::UnknownModel { model } => write!(f, "unknown catalog model {model}"),
            Self::MissingEnvironmentCredential { provider, variable } => write!(
                f,
                "provider {provider} credential environment variable {variable} is not set"
            ),
        }
    }
}

impl std::error::Error for ModelCatalogError {}

/// The safe public catalog view served to Runtime Clients.
///
/// A client selects a model and a reasoning profile from this view; it never
/// reads `rustx.toml` itself and never sees a credential, an adapter, or a
/// provider HTTP client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ModelCatalogView {
    /// Every selectable model in deterministic reference order.
    #[serde(default)]
    pub models: Vec<CatalogModelView>,
}

/// One selectable model of the public catalog view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct CatalogModelView {
    /// The fully qualified model reference.
    pub model: ModelRef,
    /// The protocol an adapter speaks to this model.
    pub protocol: ModelProtocol,
    /// The model context window in tokens.
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub context_window: u64,
    /// The configured maximum output tokens.
    pub max_output_tokens: u32,
    /// The capabilities the catalog claims.
    pub declared_capabilities: ModelCapabilities,
    /// The capabilities the runtime can actually deliver today.
    pub effective_capabilities: ModelCapabilities,
    /// The declared reasoning profiles in deterministic order.
    #[serde(default)]
    pub reasoning_profiles: Vec<ReasoningProfileView>,
    /// The profile selected when a session does not choose one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_profile: Option<ReasoningProfileId>,
    /// The redacted credential source of the model's provider.
    pub credential_source: CredentialSourceView,
}

/// One selectable reasoning profile of the public catalog view.
///
/// Only the identity and the semantic enabled state are exposed: the
/// profile's provider request parameters are provider-owned wire config that
/// a client never needs to select a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ReasoningProfileView {
    /// The profile identity.
    pub id: ReasoningProfileId,
    /// Whether the profile semantically enables reasoning.
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn document() -> Value {
        json!({
            "providers": {"p": {"baseUrl":"https://gateway.example/v1", "apiKey":"$RUSTX_KEY"}},
            "models": {"chosen": {
                "provider":"p", "id":"wire/model", "protocol":"openai_chat_completions",
                "contextWindow":1000, "maxOutputTokens":100,
                "capabilities":{"inputModalities":["text"],"outputModalities":["text"],"toolCalls":true,"reasoning":false},
                "compat":{"chatReasoningReplay":"omit"}
            }}
        })
    }
    fn catalog(value: Value) -> Result<ModelCatalog, ModelCatalogError> {
        let doc = serde_json::from_value(value).map_err(|e| ModelCatalogError::Syntax {
            detail: e.to_string(),
        })?;
        ModelCatalog::from_document(doc)
    }
    fn selected() -> ModelRef {
        ModelRef::parse("chosen").unwrap()
    }
    fn registry(value: Value) -> crate::model::invocation::ModelBindingRegistry {
        crate::model::invocation::ModelBindingRegistry::new(
            catalog(value)
                .unwrap()
                .resolve(&MapCredentialEnvironment::new([(
                    "RUSTX_KEY".into(),
                    "fixture-key".into(),
                )]))
                .unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn provider_and_model_names_never_supply_semantics() {
        let mut value = document();
        let p = value["providers"]
            .as_object_mut()
            .unwrap()
            .remove("p")
            .unwrap();
        value["providers"]["openai"] = p;
        value["models"]["chosen"]["provider"] = "openai".into();
        let catalog = catalog(value).unwrap();
        assert_eq!(
            catalog.providers().next().unwrap().base_url,
            "https://gateway.example/v1"
        );
        assert_eq!(
            catalog.model(&selected()).unwrap().id.as_str(),
            "wire/model"
        );
        assert!(matches!(
            catalog.model(&ModelRef::parse("openai/wire/model").unwrap()),
            Err(ModelCatalogError::UnknownModel { .. })
        ));
    }
    #[test]
    fn provider_required_fields_and_unknown_fields_fail() {
        for field in ["baseUrl", "apiKey"] {
            let mut value = document();
            value["providers"]["p"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(matches!(
                catalog(value),
                Err(ModelCatalogError::Syntax { .. })
            ));
        }
        for pointer in [
            "/providers/p",
            "/models/chosen",
            "/models/chosen/capabilities",
            "/models/chosen/compat",
        ] {
            let mut value = document();
            value.pointer_mut(pointer).unwrap()["unknown"] = true.into();
            assert!(catalog(value).is_err(), "{pointer}");
        }
        let mut value = document();
        value["providers"]["p"]["models"] = json!([]);
        assert!(
            matches!(catalog(value), Err(ModelCatalogError::Syntax { .. })),
            "Providers cannot contain Models"
        );
    }
    #[test]
    fn invalid_base_url_fails() {
        for url in [
            "",
            "gateway.example",
            "ftp://x/y",
            "https://",
            "https://u:p@example.com/v1",
            "https://example.com/v1?x=1",
            "https://example.com/v1#fragment",
            "https:// example.com",
        ] {
            let mut value = document();
            value["providers"]["p"]["baseUrl"] = url.into();
            assert!(
                matches!(
                    catalog(value),
                    Err(ModelCatalogError::InvalidBaseUrl { .. })
                ),
                "{url}"
            );
        }
    }
    #[test]
    fn independent_model_provider_reference_is_required() {
        let mut value = document();
        value["models"]["chosen"]["provider"] = "missing".into();
        assert!(catalog(value).is_err());
        let mut value = document();
        value["models"]["chosen"]
            .as_object_mut()
            .unwrap()
            .remove("provider");
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::Syntax { .. })
        ));
    }
    #[test]
    fn environment_credentials_resolve_lazily_or_fail_without_disclosing_values() {
        let catalog = catalog(document()).unwrap();
        let empty = catalog
            .resolve(&MapCredentialEnvironment::default())
            .unwrap();
        let error = empty
            .provider(&ProviderId::new("p"))
            .unwrap()
            .credential()
            .unwrap_err();
        assert!(matches!(
            error,
            ModelCatalogError::MissingEnvironmentCredential { .. }
        ));
        assert!(error.to_string().contains("RUSTX_KEY"));
        let resolved = catalog
            .resolve(&MapCredentialEnvironment::new([(
                "RUSTX_KEY".into(),
                "sk-do-not-print".into(),
            )]))
            .unwrap();
        let provider = resolved.provider(&ProviderId::new("p")).unwrap();
        assert_eq!(provider.credential().unwrap().expose(), "sk-do-not-print");
        assert_eq!(
            provider.credential_source(),
            CredentialSourceView::Environment {
                variable: "RUSTX_KEY".into()
            }
        );
        assert!(!format!("{resolved:?}").contains("sk-do-not-print"));
        let secret = ResolvedCredential::new("sk-do-not-print");
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(secret.to_string(), "<redacted>");
        assert!(
            !format!("{:?}", CredentialSource::Literal("sk-do-not-print".into()))
                .contains("sk-do-not-print")
        );
    }
    #[test]
    fn invalid_protocol_and_limits_fail() {
        let mut value = document();
        value["models"]["chosen"]["protocol"] = "guessed".into();
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::Syntax { .. })
        ));
        for (window, output) in [(0, 100), (100, 0), (100, 101)] {
            let mut value = document();
            value["models"]["chosen"]["contextWindow"] = window.into();
            value["models"]["chosen"]["maxOutputTokens"] = output.into();
            assert!(catalog(value).is_err());
        }
    }
    #[test]
    fn malformed_capabilities_fail() {
        for capabilities in [
            json!({"inputModalities":[],"outputModalities":["text"],"toolCalls":true,"reasoning":false}),
            json!({"inputModalities":["text"],"outputModalities":[],"toolCalls":true,"reasoning":false}),
        ] {
            let mut value = document();
            value["models"]["chosen"]["capabilities"] = capabilities;
            assert!(matches!(
                catalog(value),
                Err(ModelCatalogError::InvalidCapabilities { .. })
            ));
        }
    }
    #[test]
    fn model_references_are_independent_names_with_nonempty_segments() {
        for name in ["chosen", "a/b", "a/b/c"] {
            assert!(ModelRef::parse(name).is_ok());
        }
        for name in ["", "a/", "/b", "a//b", "a/b/", "white space"] {
            assert!(ModelRef::parse(name).is_err());
        }
        assert!(catalog(document()).unwrap().model(&selected()).is_ok());
        assert!(matches!(
            catalog(document())
                .unwrap()
                .model(&ModelRef::parse("missing").unwrap()),
            Err(ModelCatalogError::UnknownModel { .. })
        ));
    }
    #[test]
    fn chat_reasoning_replay_is_explicit_and_round_trips() {
        let mut value = document();
        value["models"]["chosen"]["compat"] = json!({});
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::InvalidCompat { .. })
        ));
        for replay in ["reasoning", "reasoning_content", "omit"] {
            let mut value = document();
            value["models"]["chosen"]["compat"]["chatReasoningReplay"] = replay.into();
            let doc: ModelCatalogDocument = serde_json::from_value(value).unwrap();
            assert!(catalog(serde_json::to_value(doc).unwrap()).is_ok());
        }
    }
    #[test]
    fn protocol_specific_compat_rejects_foreign_even_default_valued_members() {
        for protocol in ["openai_responses", "anthropic_messages"] {
            for (field, setting) in [
                ("chatReasoningReplay", "omit"),
                ("chatToolProtocol", "native"),
                ("chatMaxTokensField", "max_completion_tokens"),
            ] {
                let mut value = document();
                value["models"]["chosen"]["protocol"] = protocol.into();
                value["models"]["chosen"]["compat"] = json!({field:setting});
                assert!(
                    matches!(catalog(value), Err(ModelCatalogError::InvalidCompat { .. })),
                    "{protocol} {field}"
                );
            }
        }
        let mut value = document();
        value["models"]["chosen"]["compat"]["responsesStorage"] = "stored".into();
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::InvalidCompat { .. })
        ));
    }
    #[test]
    fn absent_compat_never_serializes_foreign_defaults() {
        let mut value = document();
        value["models"]["chosen"]["protocol"] = "anthropic_messages".into();
        value["models"]["chosen"]["compat"] = json!({});
        let doc: ModelCatalogDocument = serde_json::from_value(value).unwrap();
        let value = serde_json::to_value(doc).unwrap();
        assert_eq!(value["models"]["chosen"]["compat"], json!({}));
        assert!(catalog(value).is_ok());
    }
    #[test]
    fn chat_tool_protocol_defaults_and_explicit_variants() {
        let default = catalog(document()).unwrap();
        assert_eq!(
            default
                .model(&selected())
                .unwrap()
                .compat
                .chat_tool_protocol,
            ChatToolProtocol::Native
        );
        for (name, expected) in [
            ("native", ChatToolProtocol::Native),
            ("qwen_xml", ChatToolProtocol::QwenXml),
        ] {
            let mut value = document();
            value["models"]["chosen"]["compat"]["chatToolProtocol"] = name.into();
            assert_eq!(
                catalog(value)
                    .unwrap()
                    .model(&selected())
                    .unwrap()
                    .compat
                    .chat_tool_protocol,
                expected
            );
        }
    }
    #[test]
    fn invalid_reasoning_default_and_capability_contradiction_fail() {
        for reasoning in [
            json!({"defaultProfile":"missing","profiles":{"off":{"enabled":false}}}),
            json!({"defaultProfile":"on","profiles":{"on":{"enabled":true}}}),
        ] {
            let mut value = document();
            value["models"]["chosen"]["reasoning"] = reasoning;
            assert!(matches!(
                catalog(value),
                Err(ModelCatalogError::InvalidReasoning { .. })
            ));
        }
    }
    #[test]
    fn request_defaults_and_reasoning_profiles_cannot_replace_protected_structure() {
        let mut value = document();
        value["models"]["chosen"]["requestParams"] = json!({"messages":[]});
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::ProtectedKey { .. })
        ));
        let mut value = document();
        value["models"]["chosen"]["reasoning"] = json!({"defaultProfile":"off","profiles":{"off":{"enabled":false,"requestParams":{"messages":[]}}}});
        assert!(matches!(
            catalog(value),
            Err(ModelCatalogError::ProtectedKey { .. })
        ));
    }
    #[test]
    fn always_on_reasoning_and_opaque_parameters_remain_adapter_owned() {
        let mut value = document();
        value["models"]["chosen"]["capabilities"]["reasoning"] = true.into();
        value["models"]["chosen"]["requestParams"] =
            json!({"provider_reasoning":{"mode":"default"}, "opaque":[null, true, {"future":42}]});
        let bindings = registry(value);
        let invocation = bindings
            .resolve(&crate::model::invocation::ModelSelection::of(selected()))
            .unwrap();
        assert!(invocation.reasoning_enabled());
        assert_eq!(invocation.reasoning_profile(), None);
        assert_eq!(invocation.request_params()["opaque"][2]["future"], 42);
        assert_eq!(
            invocation.request_params()["provider_reasoning"]["mode"],
            "default"
        );
    }
}

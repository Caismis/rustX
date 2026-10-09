//! Canonical TOML model catalog authoring. Provider-native request parameters
//! cross exactly one JSON-string source boundary
//! ([`AuthoredRequestParams`]); provider translation remains adapter-owned.
use super::ModelProtocol;
use super::catalog::{
    ChatMaxTokensField, ChatReasoningReplay, ChatStreamUsage, ChatToolProtocol, CredentialSource,
    MODEL_CATALOG_SCHEMA_VERSION, Modality, ModelCapabilities, ModelCatalogDocument, ModelDocument,
    ModelProfileDocument, ModelProfileId, ProviderDocument, ResponsesStorageMode,
};
use crate::toml_authoring::AuthoredRequestParams;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    #[serde(default = "version")]
    pub schema_version: u32,
    pub providers: BTreeMap<String, Provider>,
    pub models: BTreeMap<String, Model>,
}
fn version() -> u32 {
    MODEL_CATALOG_SCHEMA_VERSION
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub base_url: String,
    /// A literal credential or an explicit $`ENV_VAR` reference. Never inferred.
    pub api_key: CredentialSource,
}
/// One authored Model. A Model declares either model-level `request_params`
/// or a nonempty `profiles` collection with a `default_profile`, never both.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub provider: String,
    pub id: String,
    pub protocol: ModelProtocol,
    pub context_window: u64,
    /// The hard maximum output budget of every invocation of this Model.
    pub max_output_tokens: u32,
    pub capabilities: Capabilities,
    /// Native request parameters of a Model without profiles. Presence is
    /// retained: a Model with profiles must not declare it, even as `{}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_params: Option<AuthoredRequestParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<ModelProfileId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<BTreeMap<ModelProfileId, Profile>>,
    #[serde(default)]
    pub compat: Compat,
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub input_modalities: BTreeSet<Modality>,
    pub output_modalities: BTreeSet<Modality>,
    pub tool_calls: bool,
    pub reasoning: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_max_tokens_field: Option<ChatMaxTokensField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_stream_usage: Option<ChatStreamUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_reasoning_replay: Option<ChatReasoningReplay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_tool_protocol: Option<ChatToolProtocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responses_storage: Option<ResponsesStorageMode>,
}
/// One authored Model Profile: a complete, independent invocation preset.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Required for every profile of a reasoning-capable Model; omission
    /// means `false` for a non-reasoning Model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_enabled: Option<bool>,
    /// Default output budget; at most the Model's `max_output_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    // The complete native request parameters of this profile.
    #[serde(default)]
    pub request_params: AuthoredRequestParams,
}
impl From<Catalog> for ModelCatalogDocument {
    fn from(value: Catalog) -> Self {
        Self {
            schema_version: value.schema_version,
            providers: value
                .providers
                .into_iter()
                .map(|(id, p)| {
                    (
                        id,
                        ProviderDocument {
                            base_url: p.base_url,
                            api_key: p.api_key,
                        },
                    )
                })
                .collect(),
            models: value
                .models
                .into_iter()
                .map(|(name, model)| (name, model.into()))
                .collect(),
        }
    }
}
impl From<Model> for ModelDocument {
    fn from(value: Model) -> Self {
        Self {
            provider: value.provider,
            id: value.id,
            protocol: value.protocol,
            context_window: value.context_window,
            max_output_tokens: value.max_output_tokens,
            capabilities: ModelCapabilities {
                input_modalities: value.capabilities.input_modalities,
                output_modalities: value.capabilities.output_modalities,
                tool_calls: value.capabilities.tool_calls,
                reasoning: value.capabilities.reasoning,
            },
            request_params: value.request_params.map(|params| params.0),
            default_profile: value.default_profile,
            profiles: value.profiles.map(|profiles| {
                profiles
                    .into_iter()
                    .map(|(id, p)| {
                        (
                            id,
                            ModelProfileDocument {
                                reasoning_enabled: p.reasoning_enabled,
                                max_output_tokens: p.max_output_tokens,
                                request_params: p.request_params.0,
                            },
                        )
                    })
                    .collect()
            }),
            compat: value.compat.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::catalog::{
        MapCredentialEnvironment, ModelCatalog, ModelCatalogError, ModelRef,
    };
    use crate::model::invocation::{ModelBindingRegistry, ModelSelection};
    use serde_json::json;

    const CATALOG: &str = r#"
schema_version = 1
[providers.p]
base_url = "https://example.invalid/v1"
api_key = "$NOT_CAPTURED"

[models."p/plain"]
provider = "p"
id = "plain"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 4096
request_params = '{"future":{"nested":[1,"text",{"new":true},null]},"temperature":0.1}'
[models."p/plain".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = true
[models."p/plain".compat]
chat_reasoning_replay = "omit"

[models."p/m"]
provider = "p"
id = "m"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 4096
default_profile = "balanced"
[models."p/m".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = true
[models."p/m".compat]
chat_reasoning_replay = "omit"
[models."p/m".profiles.fast]
reasoning_enabled = true
max_output_tokens = 1024
request_params = '{"reasoning_effort":"low"}'
[models."p/m".profiles.balanced]
reasoning_enabled = true
request_params = '''
{
  "reasoning_effort": "medium",
  "vendor": [false, {"enabled": null}]
}
'''
[models."p/m".profiles.off]
reasoning_enabled = false
request_params = '{"thinking":{"type":"disabled"}}'
"#;
    const PLAIN_PARAMS: &str = r#"request_params = '{"future":{"nested":[1,"text",{"new":true},null]},"temperature":0.1}'"#;

    fn registry(catalog: &ModelCatalog) -> ModelBindingRegistry {
        ModelBindingRegistry::new(
            catalog
                .resolve(&MapCredentialEnvironment::new([(
                    "NOT_CAPTURED".into(),
                    "test".into(),
                )]))
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn json_string_params_resolve_to_independent_profiles_and_opaque_values() {
        let catalog = ModelCatalog::from_toml_slice(CATALOG.as_bytes()).unwrap();
        let plain = catalog.model(&ModelRef::parse("p/plain").unwrap()).unwrap();
        assert_eq!(
            plain.request_params["future"]["nested"][3],
            serde_json::Value::Null
        );
        assert!(plain.profiles.is_empty() && plain.default_profile.is_none());
        let model = catalog.model(&ModelRef::parse("p/m").unwrap()).unwrap();
        assert!(model.request_params.is_empty());
        assert_eq!(
            model.profiles[&ModelProfileId::new("balanced")].request_params["vendor"][1]["enabled"],
            serde_json::Value::Null
        );
        let registry = registry(&catalog);
        let mut selection = ModelSelection::of(ModelRef::parse("p/m").unwrap());
        // The default profile applies; its budget defaults to the hard maximum.
        let invocation = registry.resolve(&selection).unwrap();
        assert_eq!(invocation.profile().unwrap().as_str(), "balanced");
        assert_eq!(invocation.max_output_tokens(), 4096);
        assert_eq!(
            serde_json::Value::Object(invocation.request_params().clone()),
            json!({"reasoning_effort":"medium","vendor":[false,{"enabled":null}]})
        );
        // An explicit profile is a complete preset with its own budget default.
        selection.profile = Some(ModelProfileId::new("fast"));
        selection.request_params =
            serde_json::from_value(json!({"future":{"replacement":[null]}})).unwrap();
        let invocation = registry.resolve(&selection).unwrap();
        assert_eq!(invocation.max_output_tokens(), 1024);
        assert_eq!(
            serde_json::Value::Object(invocation.request_params().clone()),
            json!({"reasoning_effort":"low","future":{"replacement":[null]}})
        );
        // An explicit budget replaces the profile default.
        selection.max_output_tokens = Some(4000);
        assert_eq!(
            registry.resolve(&selection).unwrap().max_output_tokens(),
            4000
        );
        selection.profile = Some(ModelProfileId::new("off"));
        assert!(!registry.resolve(&selection).unwrap().reasoning_enabled());
    }

    #[test]
    fn written_catalogs_keep_json_strings_and_reread_to_the_same_catalog() {
        let document: Catalog = crate::toml_authoring::parse(CATALOG.as_bytes()).unwrap();
        let written = crate::toml_authoring::write(&document).unwrap();
        let table = written.parse::<toml::Table>().unwrap();
        assert!(
            table["models"]["p/plain"]["request_params"].is_str(),
            "{written}"
        );
        assert!(
            table["models"]["p/m"]["profiles"]["balanced"]["request_params"].is_str(),
            "{written}"
        );
        assert!(
            table["models"]["p/m"].get("request_params").is_none(),
            "{written}"
        );
        assert_eq!(
            ModelCatalog::from_toml_slice(written.as_bytes()).unwrap(),
            ModelCatalog::from_toml_slice(CATALOG.as_bytes()).unwrap()
        );
        // Client JSON carries the structured objects.
        let value = serde_json::to_value(&document).unwrap();
        assert_eq!(
            value["models"]["p/m"]["profiles"]["fast"]["request_params"],
            json!({"reasoning_effort":"low"})
        );
        assert_eq!(value["models"]["p/m"]["default_profile"], "balanced");
        assert_eq!(serde_json::from_value::<Catalog>(value).unwrap(), document);
    }

    #[test]
    fn obsolete_and_invalid_authoring_fails_with_field_paths() {
        let fast = r#"request_params = '{"reasoning_effort":"low"}'"#;
        for (original, replacement, expected) in [
            (
                fast,
                r#"request_params = { reasoning_effort = "low" }"#,
                "models.p/m.profiles.fast.request_params must be a JSON-encoded string",
            ),
            (
                fast,
                r#"request_params = '{"reasoning_effort":"low","reasoning_effort":"high"}'"#,
                "models.p/m.profiles.fast.request_params repeats a JSON object key at $.reasoning_effort",
            ),
            (
                fast,
                r#"request_params = '["low"]'"#,
                "models.p/m.profiles.fast.request_params must encode a JSON object",
            ),
            (
                PLAIN_PARAMS,
                r#"request_params = '{"future":'"#,
                "models.p/plain.request_params is not valid JSON",
            ),
            (
                PLAIN_PARAMS,
                "request_params_json = '{}'",
                "unknown field `request_params_json`",
            ),
            (
                "default_profile = \"balanced\"",
                "default_profile = \"balanced\"\n[models.\"p/m\".reasoning]\ndefault_profile = \"off\"",
                "unknown field `reasoning`",
            ),
            (
                "reasoning_enabled = false",
                "enabled = false",
                "unknown field `enabled`",
            ),
        ] {
            let text = CATALOG.replace(original, replacement);
            assert_ne!(text, CATALOG);
            let error = ModelCatalog::from_toml_slice(text.as_bytes()).unwrap_err();
            assert!(matches!(error, ModelCatalogError::Syntax { .. }), "{error}");
            assert!(error.to_string().contains(expected), "{expected}: {error}");
        }
    }

    #[test]
    fn semantic_toml_diagnostics_use_authoring_field_names() {
        for (old, new, expected) in [
            ("https://example.invalid/v1", "relative", "base_url"),
            ("$NOT_CAPTURED", "", "api_key"),
            (
                "max_output_tokens = 4096\ndefault_profile",
                "max_output_tokens = 128000\ndefault_profile",
                "max_output_tokens",
            ),
            (
                "default_profile = \"balanced\"",
                "default_profile = \"missing\"",
                "default_profile",
            ),
            (
                "default_profile = \"balanced\"",
                "",
                "must declare default_profile",
            ),
            (
                "default_profile = \"balanced\"",
                "default_profile = \"balanced\"\nrequest_params = '{}'",
                "must not declare model-level request_params",
            ),
            (
                "reasoning_enabled = true\nmax_output_tokens = 1024",
                "max_output_tokens = 1024",
                "must declare reasoning_enabled",
            ),
            (
                "max_output_tokens = 1024",
                "max_output_tokens = 4097",
                "max_output_tokens 4097",
            ),
        ] {
            let text = CATALOG.replace(old, new);
            assert_ne!(text, CATALOG, "{old}");
            let error = ModelCatalog::from_toml_slice(text.as_bytes()).unwrap_err();
            assert!(error.to_string().contains(expected), "{expected}: {error}");
        }
    }

    #[test]
    fn protected_keys_fail_at_their_owned_layer() {
        for key in ["model", "messages", "stream", "max_completion_tokens"] {
            for (original, layer) in [
                (PLAIN_PARAMS, "model request parameters"),
                (
                    r#"request_params = '{"thinking":{"type":"disabled"}}'"#,
                    "model profile request parameters",
                ),
            ] {
                let text =
                    CATALOG.replace(original, &format!("request_params = '{{\"{key}\":true}}'"));
                assert_ne!(text, CATALOG);
                let error = ModelCatalog::from_toml_slice(text.as_bytes()).unwrap_err();
                assert!(matches!(error, ModelCatalogError::ProtectedKey { .. }));
                let message = error.to_string();
                assert!(message.contains(layer), "{message}");
                assert!(
                    message.contains(&format!("protected wire key {key:?}")),
                    "{message}"
                );
            }
        }
    }
}

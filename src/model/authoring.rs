//! Canonical TOML model catalog. Provider-native overlays cross exactly one
//! structured TOML normalization boundary; provider translation remains adapter-owned.
use super::ModelProtocol;
use super::catalog::{
    ChatMaxTokensField, ChatReasoningReplay, ChatStreamUsage, ChatToolProtocol, CredentialSource,
    MODEL_CATALOG_SCHEMA_VERSION, Modality, ModelCapabilities, ModelCatalogDocument, ModelDocument,
    ProviderDocument, ReasoningConfig, ReasoningProfile, ReasoningProfileId, ResponsesStorageMode,
};
use crate::toml_authoring::RequestParamsToml;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    #[serde(default = "version")]
    pub schema_version: u32,
    pub providers: BTreeMap<String, Provider>,
}
fn version() -> u32 {
    MODEL_CATALOG_SCHEMA_VERSION
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub base_url: String,
    /// A literal credential or an explicit $`ENV_VAR` reference. Never inferred.
    pub api_key: CredentialSource,
    pub models: Vec<Model>,
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub protocol: ModelProtocol,
    pub context_window: u64,
    pub max_output_tokens: u32,
    pub capabilities: Capabilities,
    #[serde(default)]
    pub request_params: RequestParamsToml,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(default)]
    pub compat: Compat,
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub input_modalities: BTreeSet<Modality>,
    pub output_modalities: BTreeSet<Modality>,
    pub tool_calls: bool,
    pub reasoning: bool,
}
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
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
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reasoning {
    pub default_profile: ReasoningProfileId,
    pub profiles: BTreeMap<ReasoningProfileId, Profile>,
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub enabled: bool,
    #[serde(default)]
    pub request_params: RequestParamsToml,
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
                            models: p.models.into_iter().map(Into::into).collect(),
                        },
                    )
                })
                .collect(),
        }
    }
}
impl From<Model> for ModelDocument {
    fn from(value: Model) -> Self {
        Self {
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
            request_params: value.request_params.0,
            compat: value.compat.into(),
            reasoning: value.reasoning.map(|r| ReasoningConfig {
                default_profile: r.default_profile,
                profiles: r
                    .profiles
                    .into_iter()
                    .map(|(id, p)| {
                        (
                            id,
                            ReasoningProfile {
                                enabled: p.enabled,
                                request_params: p.request_params.0,
                            },
                        )
                    })
                    .collect(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
    const CATALOG: &str = r#"
schema_version = 1
[providers.p]
base_url = "https://example.invalid/v1"
api_key = "$NOT_CAPTURED"
[[providers.p.models]]
id = "m"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 4096
request_params = { future = { nested = [1, "text", { new = true }] }, temperature = 0.1 }
[providers.p.models.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = true
[providers.p.models.compat]
chat_reasoning_replay = "omit"
[providers.p.models.reasoning]
default_profile = "off"
[providers.p.models.reasoning.profiles.off]
enabled = false
request_params = { vendor_reasoning = [false, { enabled = false }] }
"#;
    #[test]
    fn catalog_and_profile_json_remain_opaque_and_shallow_overlays_remain_owned() {
        let catalog = ModelCatalog::from_toml_slice(CATALOG.as_bytes()).unwrap();
        let model_ref = ModelRef::parse("p/m").unwrap();
        let definition = catalog.model(&model_ref).unwrap();
        assert_eq!(
            definition.request_params["future"]["nested"][1],
            serde_json::json!("text")
        );
        assert_eq!(
            definition.reasoning.as_ref().unwrap().profiles[&ReasoningProfileId::new("off")]
                .request_params["vendor_reasoning"][0],
            serde_json::json!(false)
        );
        let mut selection = crate::model::invocation::ModelSelection::of(model_ref);
        selection.request_params = serde_json::from_str(
            r#"{"future":{"replacement":[null]},"unreleased":[true,1,"s",null]}"#,
        )
        .unwrap();
        crate::model::invocation::analyze_selection(
            definition,
            &selection,
            crate::model::invocation::RequestParamsLayer::SessionOverrides,
        )
        .unwrap();
        // Concrete overlays are resolved by the existing invocation owner, not TOML.
        let registry = crate::model::invocation::ModelBindingRegistry::new(
            catalog
                .resolve(&MapCredentialEnvironment::new([(
                    "NOT_CAPTURED".into(),
                    "test".into(),
                )]))
                .unwrap(),
        )
        .unwrap();
        let invocation = registry.resolve(&selection).unwrap();
        assert!(
            invocation.request_params()["future"]
                .get("nested")
                .is_none()
        );
        assert_eq!(
            invocation.request_params()["future"]["replacement"][0],
            serde_json::Value::Null
        );
        assert_eq!(
            invocation.request_params()["unreleased"][3],
            serde_json::Value::Null
        );
    }
    #[test]
    fn catalog_and_reasoning_reject_toml_only_values_with_full_paths() {
        for (original, path) in [
            (
                r#"{ future = { nested = [1, "text", { new = true }] }, temperature = 0.1 }"#,
                "providers.p.models[0].request_params.items[1].when",
            ),
            (
                "{ vendor_reasoning = [false, { enabled = false }] }",
                "providers.p.models[0].reasoning.profiles.off.request_params.items[1].when",
            ),
        ] {
            for value in [
                "1979-05-27",
                "07:32:00",
                "1979-05-27T07:32:00Z",
                "nan",
                "+inf",
                "-inf",
            ] {
                let text = CATALOG.replace(
                    original,
                    &format!("{{items = [{{when = true}}, {{when = {value}}}]}}"),
                );
                let error = ModelCatalog::from_toml_slice(text.as_bytes())
                    .unwrap_err()
                    .to_string();
                assert!(error.contains(path), "{error}");
            }
        }
    }
    #[test]
    fn semantic_toml_diagnostics_use_authoring_field_names() {
        for (old, new, expected) in [
            ("https://example.invalid/v1", "relative", "base_url"),
            ("$NOT_CAPTURED", "", "api_key"),
            (
                "context_window = 128000",
                "context_window = 0",
                "context_window",
            ),
            (
                "max_output_tokens = 4096",
                "max_output_tokens = 128000",
                "max_output_tokens",
            ),
            (
                "default_profile = \"off\"",
                "default_profile = \"missing\"",
                "default_profile",
            ),
            (
                "chat_reasoning_replay = \"omit\"",
                "",
                "compat.chat_reasoning_replay",
            ),
        ] {
            let text = CATALOG.replace(old, new);
            assert_ne!(text, CATALOG);
            text.parse::<toml_edit::DocumentMut>().expect("valid TOML");
            let error = ModelCatalog::from_toml_slice(text.as_bytes()).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
    #[test]
    fn invalid_roots_unknown_toml_and_protected_keys_fail_at_their_owned_boundaries() {
        for replacement in ["[]", "42", "true", "'SECRET_VALUE'"] {
            let text = CATALOG.replace(
                r#"{ future = { nested = [1, "text", { new = true }] }, temperature = 0.1 }"#,
                replacement,
            );
            text.parse::<toml_edit::DocumentMut>()
                .expect("valid outer TOML");
            let error = ModelCatalog::from_toml_slice(text.as_bytes()).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("request_params must be a TOML table"),
                "{error}"
            );
        }
        for field in ["unknown", "request_params_json"] {
            let text = format!("{field} = {{}}\n{CATALOG}");
            text.parse::<toml_edit::DocumentMut>().expect("valid TOML");
            let error = ModelCatalog::from_toml_slice(text.as_bytes()).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("unknown field `{field}`")),
                "{error}"
            );
        }
        let obsolete = CATALOG.replace("request_params =", "request_params_json =");
        assert!(
            ModelCatalog::from_toml_slice(obsolete.as_bytes())
                .unwrap_err()
                .to_string()
                .contains("unknown field `request_params_json`")
        );
        for key in ["model", "messages", "stream"] {
            let protected = format!("{{{key} = true}}");
            for (original, layer) in [
                (
                    r#"{ future = { nested = [1, "text", { new = true }] }, temperature = 0.1 }"#,
                    "model default",
                ),
                (
                    r"{ vendor_reasoning = [false, { enabled = false }] }",
                    "reasoning profile",
                ),
            ] {
                let text = CATALOG.replace(original, &protected);
                assert_ne!(text, CATALOG);
                text.parse::<toml_edit::DocumentMut>().expect("valid TOML");
                let error = ModelCatalog::from_toml_slice(text.as_bytes()).unwrap_err();
                assert!(matches!(
                    error,
                    crate::model::catalog::ModelCatalogError::ProtectedKey { .. }
                ));
                let message = error.to_string();
                assert!(message.contains("request_params"), "{message}");
                assert!(message.contains(layer), "{message}");
                assert!(
                    message.contains(&format!("protected wire key {key:?}")),
                    "{message}"
                );
            }
        }
    }
}

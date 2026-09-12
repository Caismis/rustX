//! Canonical TOML model catalog. Provider-native overlays cross exactly one
//! JSON-string boundary; provider translation remains adapter-owned.
use super::ModelProtocol;
use super::catalog::{
    ChatMaxTokensField, ChatReasoningReplay, ChatStreamUsage, ChatToolProtocol, CredentialSource,
    MODEL_CATALOG_SCHEMA_VERSION, Modality, ModelCapabilities, ModelCatalogDocument, ModelDocument,
    ProviderDocument, ReasoningConfig, ReasoningProfile, ReasoningProfileId, ResponsesStorageMode,
};
use crate::toml_authoring::RequestParamsJson;
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
    pub request_params_json: RequestParamsJson,
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
    pub request_params_json: RequestParamsJson,
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
            request_params: value.request_params_json.0,
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
                                request_params: p.request_params_json.0,
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
request_params_json = '''{"future":{"nested":[1,null,{"new":true}]},"temperature":0.1}'''
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
request_params_json = '''{"vendor_reasoning":[null,{"enabled":false}]}'''
"#;
    #[test]
    fn catalog_and_profile_json_remain_opaque_and_shallow_overlays_remain_owned() {
        let catalog = ModelCatalog::from_toml_slice(CATALOG.as_bytes()).unwrap();
        let model_ref = ModelRef::parse("p/m").unwrap();
        let definition = catalog.model(&model_ref).unwrap();
        assert_eq!(
            definition.request_params["future"]["nested"][1],
            serde_json::Value::Null
        );
        assert_eq!(
            definition.reasoning.as_ref().unwrap().profiles[&ReasoningProfileId::new("off")]
                .request_params["vendor_reasoning"][0],
            serde_json::Value::Null
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
    fn invalid_json_unknown_toml_and_protected_keys_fail_at_their_owned_boundaries() {
        for replacement in ["[]", "null", "42", "true", "{bad"] {
            let text = CATALOG.replace(
                r#"{"future":{"nested":[1,null,{"new":true}]},"temperature":0.1}"#,
                replacement,
            );
            assert!(ModelCatalog::from_toml_slice(text.as_bytes()).is_err());
        }
        for field in ["unknown = true\n", "request_params = {}\n"] {
            let text = CATALOG.replace(
                "schema_version = 1",
                &format!("schema_version = 1\n{field}"),
            );
            assert!(ModelCatalog::from_toml_slice(text.as_bytes()).is_err());
        }
        for key in ["model", "messages", "stream"] {
            let protected = format!("{{\"{key}\":null}}");
            let text = CATALOG.replace(
                r#"{"future":{"nested":[1,null,{"new":true}]},"temperature":0.1}"#,
                &protected,
            );
            assert!(
                ModelCatalog::from_toml_slice(text.as_bytes()).is_err(),
                "{key}"
            );
        }
    }
}

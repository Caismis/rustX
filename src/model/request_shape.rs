//! Configuration-induced request comparison at the adapter boundary.
//!
//! History and continuation are held fixed outside this comparison. Equality
//! describes preservation of configuration contributions, never a cache hit.
use crate::model::frozen::FrozenProviderBinding;
use crate::model::{ModelProtocol, ModelRequest};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CacheImpact {
    Preserved,
    PrefixChanged,
    CacheNamespaceChanged,
    Unproven,
}

/// Private comparison evidence. Provider credentials never enter projections.
#[derive(Clone)]
pub struct ConfigurationRequestShape {
    binding: FrozenProviderBinding,
    request: ModelRequest,
    wire: serde_json::Value,
    summary: Option<Box<Self>>,
}

impl ConfigurationRequestShape {
    /// Use the actual adapter translator, with no natural history growth.
    ///
    /// # Errors
    /// Returns the same validation/translation error as actual construction.
    pub fn capture(
        binding: FrozenProviderBinding,
        mut request: ModelRequest,
    ) -> Result<Self, crate::model::ModelError> {
        request.messages.clear();
        request.continuation = None;
        let tools =
            crate::model::adapter::validation::validate_request(&request, request.protocol())?;
        let wire = match request.protocol() {
            ModelProtocol::OpenAiChatCompletions => {
                crate::model::adapter::openai::chat_completions::configuration_request(&request)?
            }
            ModelProtocol::OpenAiResponses => {
                crate::model::adapter::openai::responses::translate_request(
                    &request,
                    &tools,
                    request.invocation.compat.responses_storage,
                )?
            }
            ModelProtocol::AnthropicMessages => {
                crate::model::adapter::anthropic::mapping::configuration_request(&request, &tools)?
            }
        };
        Ok(Self {
            binding,
            request,
            wire,
            summary: None,
        })
    }

    #[must_use]
    pub fn with_summary(mut self, summary: Self) -> Self {
        self.summary = Some(Box::new(summary));
        self
    }

    #[must_use]
    pub fn compare(&self, candidate: &Self) -> CacheImpact {
        if self.binding != candidate.binding
            || self.binding.resolved_credential != candidate.binding.resolved_credential
            || self.request.protocol() != candidate.request.protocol()
            || self.request.model() != candidate.request.model()
        {
            return CacheImpact::CacheNamespaceChanged;
        }
        // Compat can change translation of existing history even when the empty
        // configuration probe emits identical JSON. Never infer preservation.
        if self.request.invocation.compat != candidate.request.invocation.compat {
            return CacheImpact::Unproven;
        }
        let prefix_fields: &[&str] = match self.request.protocol() {
            ModelProtocol::OpenAiChatCompletions => &["messages", "tools"],
            ModelProtocol::OpenAiResponses => &["instructions", "tools"],
            ModelProtocol::AnthropicMessages => &["system", "tools"],
        };
        if prefix_fields
            .iter()
            .any(|field| self.wire.get(field) != candidate.wire.get(field))
        {
            return CacheImpact::PrefixChanged;
        }
        if self.wire == candidate.wire
            && self.request.invocation.capabilities == candidate.request.invocation.capabilities
        {
            match (&self.summary, &candidate.summary) {
                (Some(old), Some(new)) => old.compare(new),
                (None, None) => CacheImpact::Preserved,
                _ => CacheImpact::Unproven,
            }
        } else {
            // Opaque provider parameters may affect caching. No whitelist is
            // invented for parameters whose preservation is not proven here.
            CacheImpact::Unproven
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::catalog::{CredentialSource, ProviderId};
    use crate::scripted_suites::common::simple_request;
    use serde_json::Value;

    fn binding() -> FrozenProviderBinding {
        FrozenProviderBinding {
            provider: ProviderId::new("fixture"),
            base_url: "http://provider.invalid/v1".into(),
            credential: CredentialSource::Literal("fixture-only".into()),
            resolved_credential: None,
        }
    }

    fn actual(request: &ModelRequest) -> Value {
        let tools =
            crate::model::adapter::validation::validate_request(request, request.protocol())
                .unwrap();
        match request.protocol() {
            ModelProtocol::OpenAiChatCompletions => {
                crate::model::adapter::openai::chat_completions::translate_request(request)
            }
            ModelProtocol::OpenAiResponses => {
                crate::model::adapter::openai::responses::translate_request(
                    request,
                    &tools,
                    request.invocation.compat.responses_storage,
                )
            }
            ModelProtocol::AnthropicMessages => {
                crate::model::adapter::anthropic::mapping::translate_request(request, &tools)
            }
        }
        .unwrap()
    }

    #[test]
    fn t10_configuration_evidence_matches_all_actual_adapter_prefixes() {
        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
            ModelProtocol::AnthropicMessages,
        ] {
            let mut request = simple_request(protocol, "fixture", "canonical history");
            request.effective_system_prompt = "instructions P1".into();
            request.tools = ["alpha", "beta"]
                .into_iter()
                .map(|name| {
                    crate::tools::schema::compile_model_definition(
                        &crate::scripted_suites::common::tool(name, name),
                    )
                    .unwrap()
                })
                .collect();
            let baseline = ConfigurationRequestShape::capture(binding(), request.clone()).unwrap();
            let wire = actual(&request);
            assert_eq!(wire.get("tools"), baseline.wire.get("tools"));
            let field = match protocol {
                ModelProtocol::OpenAiChatCompletions => "messages",
                ModelProtocol::OpenAiResponses => "instructions",
                ModelProtocol::AnthropicMessages => "system",
            };
            if protocol == ModelProtocol::OpenAiChatCompletions {
                assert_eq!(wire[field][0], baseline.wire[field][0]);
            } else {
                assert_eq!(wire.get(field), baseline.wire.get(field));
            }
            let mut changed = request.clone();
            changed.effective_system_prompt = "instructions P2".into();
            assert_ne!(actual(&changed).get(field), wire.get(field));
            assert_eq!(
                baseline.compare(&ConfigurationRequestShape::capture(binding(), changed).unwrap()),
                CacheImpact::PrefixChanged
            );
            let mut reordered = request.clone();
            reordered.tools.reverse();
            assert_ne!(actual(&reordered).get("tools"), wire.get("tools"));
            assert_eq!(
                baseline
                    .compare(&ConfigurationRequestShape::capture(binding(), reordered).unwrap()),
                CacheImpact::PrefixChanged
            );
            let mut grown = request.clone();
            grown.messages.extend(request.messages.clone());
            assert_ne!(actual(&grown), wire);
            assert_eq!(
                baseline.compare(&ConfigurationRequestShape::capture(binding(), grown).unwrap()),
                CacheImpact::Preserved
            );
            let mut endpoint = binding();
            endpoint.base_url = "http://another.invalid/v1".into();
            assert_eq!(
                baseline.compare(
                    &ConfigurationRequestShape::capture(endpoint, request.clone()).unwrap()
                ),
                CacheImpact::CacheNamespaceChanged
            );
            let mut budget = request;
            budget.invocation.max_output_tokens += 1;
            assert_ne!(actual(&budget), wire);
            assert_eq!(
                baseline.compare(&ConfigurationRequestShape::capture(binding(), budget).unwrap()),
                CacheImpact::Unproven
            );
        }
    }
}

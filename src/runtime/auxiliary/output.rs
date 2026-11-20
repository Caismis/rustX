//! Consumer-owned output contracts prepared once before native Call admission.

use crate::model::structured::{PreparedResponseSchema, ResponseSchema, StructuredOutputStrategy};
use crate::model::{
    AuxiliaryBusinessSource, ModelError, ModelErrorKind, ModelFinishReason, ModelRequest,
};
use crate::runtime::identity::CompactionOperationId;

use super::stream::{AuxiliaryTextOutput, local_error};

/// The narrow internal consumer seam. A capability alone never opts text in.
pub(crate) enum AuxiliaryOutputFormat {
    Text,
    JsonSchema(ResponseSchema),
}

impl AuxiliaryOutputFormat {
    /// Complete preparation before fingerprinting, context fitting or submission.
    pub(crate) fn prepare(self, mut request: ModelRequest) -> Result<ModelRequest, ModelError> {
        if request.response_schema.is_some() {
            return Err(local_error(
                ModelErrorKind::InvalidRequest,
                "auxiliary output contract has already been prepared",
            ));
        }
        if let Self::JsonSchema(schema) = self {
            let strategy = if request.invocation.capabilities.json_schema_output {
                StructuredOutputStrategy::Native
            } else {
                if !request.effective_system_prompt.is_empty() {
                    request.effective_system_prompt.push_str("\n\n");
                }
                request
                    .effective_system_prompt
                    .push_str(&schema.prompt_instruction());
                StructuredOutputStrategy::Prompt
            };
            request.response_schema = Some(PreparedResponseSchema { schema, strategy });
        }
        Ok(request)
    }

    /// Freeze an explicitly authorized Compaction-local Native-to-Prompt
    /// downgrade. Call only after the owning operation observes a classified
    /// Native-format rejection or completed contract violation.
    pub(crate) fn prepare_prompt_fallback(
        self,
        mut request: ModelRequest,
        operation_id: CompactionOperationId,
    ) -> Result<ModelRequest, ModelError> {
        if request.response_schema.is_some()
            || !request.invocation.capabilities.json_schema_output
            || operation_id.as_str().is_empty()
        {
            return Err(local_error(
                ModelErrorKind::InvalidRequest,
                "native format fallback requires an unprepared native-capable request",
            ));
        }
        let Self::JsonSchema(schema) = self else {
            return Err(local_error(
                ModelErrorKind::InvalidRequest,
                "native format fallback requires a structured output contract",
            ));
        };
        if !request.effective_system_prompt.is_empty() {
            request.effective_system_prompt.push_str("\n\n");
        }
        request
            .effective_system_prompt
            .push_str(&schema.prompt_instruction());
        request.response_schema = Some(PreparedResponseSchema {
            schema,
            strategy: StructuredOutputStrategy::PromptFallback { operation_id },
        });
        Ok(request)
    }
}

/// Only schema-validated JSON can cross this native consumer boundary.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AuxiliaryOutput {
    Text(AuxiliaryTextOutput),
    JsonSchema(serde_json::Value),
}

impl AuxiliaryOutput {
    pub(crate) fn into_text(self) -> Result<AuxiliaryTextOutput, ModelError> {
        match self {
            Self::Text(text) => Ok(text),
            Self::JsonSchema(_) => Err(local_error(
                ModelErrorKind::InvalidRequest,
                "structured auxiliary output cannot be accepted as text",
            )),
        }
    }
}

/// Runs only after the physical stream and required settlement have joined.
pub(super) fn accept_output(
    request: &ModelRequest,
    output: AuxiliaryTextOutput,
) -> Result<AuxiliaryOutput, ModelError> {
    let Some(prepared) = &request.response_schema else {
        return Ok(AuxiliaryOutput::Text(output));
    };
    if output.refused || output.finish_reason != ModelFinishReason::Stop {
        return Err(local_error(
            ModelErrorKind::InvalidRequest,
            "structured auxiliary output was refused or did not complete normally",
        ));
    }
    if output.text.trim().is_empty() {
        return Err(local_error(
            ModelErrorKind::InvalidRequest,
            "structured auxiliary output contained no visible JSON",
        ));
    }
    prepared.schema.validate_output(&output.text).map_or_else(
        |mut error| {
            if prepared.is_native() && error.kind == ModelErrorKind::InvalidRequest {
                error.kind = ModelErrorKind::StructuredOutputContractViolation;
            }
            Err(error)
        },
        |value| Ok(AuxiliaryOutput::JsonSchema(value)),
    )
}

/// The operation may downgrade only the Native request that actually failed
/// with typed format evidence after its physical execution was settled.
pub(crate) fn can_fallback_native_format(request: &ModelRequest, error: &ModelError) -> bool {
    request
        .response_schema
        .as_ref()
        .is_some_and(PreparedResponseSchema::is_native)
        && request.invocation.capabilities.json_schema_output
        && matches!(
            error.kind,
            ModelErrorKind::NativeFormatUnsupported
                | ModelErrorKind::StructuredOutputContractViolation
        )
}

pub(super) fn validate_prepared(
    request: &ModelRequest,
    business_source: &AuxiliaryBusinessSource,
) -> Result<(), ModelError> {
    let Some(prepared) = &request.response_schema else {
        return Ok(());
    };
    let fallback_matches_source = match &prepared.strategy {
        StructuredOutputStrategy::PromptFallback { operation_id } => matches!(
            business_source,
            AuxiliaryBusinessSource::Compaction { operation_id: admitted, .. }
                if admitted == operation_id && !operation_id.as_str().is_empty()
        ),
        StructuredOutputStrategy::Native | StructuredOutputStrategy::Prompt => true,
    };
    if !prepared.matches_capability(request.invocation.capabilities.json_schema_output)
        || !fallback_matches_source
        || (!prepared.is_native()
            && !request
                .effective_system_prompt
                .ends_with(&prepared.schema.prompt_instruction()))
    {
        return Err(local_error(
            ModelErrorKind::InvalidRequest,
            "auxiliary output strategy does not match its frozen request",
        ));
    }
    Ok(())
}

/// Only Auxiliary admission may install an in-memory proof after validating
/// the admitted Compaction identity and the complete prepared request.
pub(super) fn authorize_prepared(
    request: &mut ModelRequest,
    business_source: &AuxiliaryBusinessSource,
) -> Result<(), ModelError> {
    validate_prepared(request, business_source)?;
    let operation =
        request
            .response_schema
            .as_ref()
            .and_then(|prepared| match &prepared.strategy {
                StructuredOutputStrategy::PromptFallback { operation_id } => {
                    Some(operation_id.clone())
                }
                _ => None,
            });
    if let Some(operation_id) = operation {
        request.authorize_prompt_fallback(operation_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        AuxiliaryOutput, AuxiliaryOutputFormat, accept_output, authorize_prepared,
        can_fallback_native_format, validate_prepared,
    };
    use crate::model::structured::ResponseSchema;
    use crate::model::{AuxiliaryBusinessSource, ModelErrorKind, ModelFinishReason, ModelProtocol};
    use crate::runtime::auxiliary::stream::{AuxiliaryTextOutput, local_error};
    use crate::runtime::identity::CompactionOperationId;

    fn schema() -> ResponseSchema {
        ResponseSchema::new(
            "test_summary".into(),
            json!({"type":"object","properties":{"goal":{"type":"string"}},"required":["goal"],"additionalProperties":false}),
            128,
        )
        .unwrap()
    }

    fn request(native: bool) -> crate::model::ModelRequest {
        let mut request = crate::scripted_suites::common::simple_request(
            ModelProtocol::OpenAiChatCompletions,
            "test",
            "source",
        );
        request.invocation.capabilities.json_schema_output = native;
        request
    }

    fn completion(
        text: &str,
        refused: bool,
        finish_reason: ModelFinishReason,
    ) -> AuxiliaryTextOutput {
        AuxiliaryTextOutput {
            text: text.to_owned(),
            reasoning: String::new(),
            refused,
            finish_reason,
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One authorization proof is challenged by serialization and semantic tampering.
    fn fallback_is_frozen_bound_to_compaction_and_keeps_its_prompt_contract() {
        let operation_id = CompactionOperationId::new("operation");
        let source = AuxiliaryBusinessSource::Compaction {
            operation_id: operation_id.clone(),
            preceding_call: None,
        };
        let mut fallback = AuxiliaryOutputFormat::JsonSchema(schema())
            .prepare_prompt_fallback(request(true), operation_id.clone())
            .unwrap();
        let fingerprint = fallback.semantic_input_fingerprint();
        let encoded = serde_json::to_value(&fallback).unwrap();
        let mut decoded: crate::model::ModelRequest = serde_json::from_value(encoded).unwrap();
        assert_eq!(fallback, decoded);
        assert!(validate_prepared(&fallback, &source).is_ok());
        assert!(
            crate::model::adapter::validation::validate_request(
                &fallback,
                ModelProtocol::OpenAiChatCompletions,
            )
            .is_err()
        );
        assert!(authorize_prepared(&mut fallback, &AuxiliaryBusinessSource::Independent).is_err());
        authorize_prepared(&mut fallback, &source).unwrap();
        assert_eq!(fingerprint, fallback.semantic_input_fingerprint());
        assert!(
            crate::model::adapter::validation::validate_request(
                &fallback,
                ModelProtocol::OpenAiChatCompletions,
            )
            .is_ok()
        );
        assert!(
            crate::model::adapter::validation::validate_request(
                &decoded,
                ModelProtocol::OpenAiChatCompletions,
            )
            .is_err()
        );
        let mut tampered = fallback.clone();
        tampered.invocation.max_output_tokens -= 1;
        assert!(
            crate::model::adapter::validation::validate_request(
                &tampered,
                ModelProtocol::OpenAiChatCompletions,
            )
            .is_err()
        );
        let mut wrong_operation = fallback.clone();
        wrong_operation.response_schema.as_mut().unwrap().strategy =
            crate::model::structured::StructuredOutputStrategy::PromptFallback {
                operation_id: CompactionOperationId::new("other"),
            };
        assert!(
            crate::model::adapter::validation::validate_request(
                &wrong_operation,
                ModelProtocol::OpenAiChatCompletions,
            )
            .is_err()
        );
        let encoded_authorized = serde_json::to_value(&fallback).unwrap();
        assert!(
            encoded_authorized
                .get("prompt_fallback_authorization")
                .is_none()
        );
        let reloaded: crate::model::ModelRequest =
            serde_json::from_value(encoded_authorized).unwrap();
        assert!(
            crate::model::adapter::validation::validate_request(
                &reloaded,
                ModelProtocol::OpenAiChatCompletions,
            )
            .is_err()
        );
        assert!(validate_prepared(&fallback, &AuxiliaryBusinessSource::Independent).is_err());
        assert!(
            validate_prepared(
                &fallback,
                &AuxiliaryBusinessSource::Compaction {
                    operation_id: CompactionOperationId::new("other"),
                    preceding_call: None,
                },
            )
            .is_err()
        );
        let another_operation = AuxiliaryOutputFormat::JsonSchema(schema())
            .prepare_prompt_fallback(request(true), CompactionOperationId::new("other"))
            .unwrap();
        assert_ne!(fingerprint, another_operation.semantic_input_fingerprint());
        decoded.effective_system_prompt.push_str(" altered");
        assert!(validate_prepared(&decoded, &source).is_err());
        assert!(
            crate::model::adapter::validation::validate_request(
                &decoded,
                ModelProtocol::OpenAiChatCompletions,
            )
            .is_err()
        );
        assert_ne!(fingerprint, decoded.semantic_input_fingerprint());
        assert!(
            AuxiliaryOutputFormat::JsonSchema(schema())
                .prepare_prompt_fallback(request(false), operation_id.clone())
                .is_err()
        );
        assert!(
            AuxiliaryOutputFormat::Text
                .prepare_prompt_fallback(request(true), operation_id)
                .is_err()
        );
    }

    #[test]
    fn only_completed_native_format_failures_can_authorize_downgrade() {
        let native = AuxiliaryOutputFormat::JsonSchema(schema())
            .prepare(request(true))
            .unwrap();
        let prompt = AuxiliaryOutputFormat::JsonSchema(schema())
            .prepare(request(false))
            .unwrap();
        let invalid = completion("not json", false, ModelFinishReason::Stop);
        let error = accept_output(&native, invalid.clone()).unwrap_err();
        assert_eq!(
            error.kind,
            ModelErrorKind::StructuredOutputContractViolation
        );
        assert!(can_fallback_native_format(&native, &error));
        assert!(!can_fallback_native_format(&prompt, &error));
        assert_eq!(
            accept_output(&prompt, invalid).unwrap_err().kind,
            ModelErrorKind::InvalidRequest,
        );
        for kind in [
            ModelErrorKind::InvalidRequest,
            ModelErrorKind::Unsupported,
            ModelErrorKind::Authentication,
            ModelErrorKind::ContextWindowExceeded,
            ModelErrorKind::RateLimit,
            ModelErrorKind::Transport,
            ModelErrorKind::Timeout,
            ModelErrorKind::Cancelled,
            ModelErrorKind::GenerationBudgetExceeded,
        ] {
            assert!(!can_fallback_native_format(
                &native,
                &local_error(kind, "test")
            ));
        }
        assert!(can_fallback_native_format(
            &native,
            &local_error(ModelErrorKind::NativeFormatUnsupported, "test"),
        ));
        for output in [
            completion("  ", false, ModelFinishReason::Stop),
            completion("not json", true, ModelFinishReason::Stop),
            completion("not json", false, ModelFinishReason::Length),
            completion("not json", false, ModelFinishReason::Refusal),
        ] {
            let error = accept_output(&native, output).unwrap_err();
            assert!(!can_fallback_native_format(&native, &error));
        }
        let valid = completion(r#"{"goal":"ok"}"#, false, ModelFinishReason::Stop);
        assert!(matches!(
            accept_output(&native, valid.clone()),
            Ok(AuxiliaryOutput::JsonSchema(_))
        ));
        let fallback = AuxiliaryOutputFormat::JsonSchema(schema())
            .prepare_prompt_fallback(request(true), CompactionOperationId::new("operation"))
            .unwrap();
        assert!(matches!(
            accept_output(&fallback, valid),
            Ok(AuxiliaryOutput::JsonSchema(_))
        ));
        let fallback_error = accept_output(
            &fallback,
            completion("not json", false, ModelFinishReason::Stop),
        )
        .unwrap_err();
        assert_eq!(fallback_error.kind, ModelErrorKind::InvalidRequest);
        assert!(!can_fallback_native_format(&fallback, &fallback_error));
    }
}

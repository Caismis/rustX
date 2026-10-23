//! Consumer-owned output contracts prepared once before native Call admission.

use crate::model::structured::{PreparedResponseSchema, ResponseSchema, StructuredOutputStrategy};
use crate::model::{ModelError, ModelErrorKind, ModelFinishReason, ModelRequest};

use super::stream::{AuxiliaryTextOutput, local_error};

/// The narrow internal consumer seam. A capability alone never opts text in.
pub(crate) enum AuxiliaryOutputFormat {
    Text,
    #[cfg_attr(not(test), allow(dead_code))] // Internal structured consumers opt in at this seam.
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
    prepared
        .schema
        .validate_output(&output.text)
        .map(AuxiliaryOutput::JsonSchema)
}

pub(super) fn validate_prepared(request: &ModelRequest) -> Result<(), ModelError> {
    let Some(prepared) = &request.response_schema else {
        return Ok(());
    };
    if prepared.is_native() != request.invocation.capabilities.json_schema_output
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

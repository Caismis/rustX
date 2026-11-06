//! Editable selection spelling is distinct from admitted native identities.
//! Conversion reuses the existing constructors; no discovery or preparation.
use crate::capabilities::selection::ExactToolSelector;
use crate::model::catalog::{ModelProfileId, ModelRef};
use crate::model::invocation::RequestParams;
use crate::model::session::AuxiliaryModelPolicy;
use crate::runtime::workflow::WorkflowCompileError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowModelPolicy {
    #[default]
    Session,
    Explicit {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
        #[serde(default)]
        request_params: RequestParams,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_output_tokens: Option<u32>,
    },
}

impl WorkflowModelPolicy {
    pub(in crate::runtime::workflow) fn lower(
        &self,
    ) -> Result<AuxiliaryModelPolicy, WorkflowCompileError> {
        match self {
            Self::Session => Ok(AuxiliaryModelPolicy::Session),
            Self::Explicit {
                model,
                profile,
                request_params,
                max_output_tokens,
            } => {
                let model = ModelRef::parse(model).map_err(|_| {
                    WorkflowCompileError::InvalidField(
                        "Model selection must name a valid registered-model identity".into(),
                    )
                    .at("model")
                })?;
                let profile = profile
                    .as_deref()
                    .map(ModelProfileId::parse)
                    .transpose()
                    .map_err(|_| {
                        WorkflowCompileError::InvalidField(
                            "Model Profile must name a valid profile identity".into(),
                        )
                        .at("profile")
                    })?;
                Ok(AuxiliaryModelPolicy::Explicit {
                    model,
                    profile,
                    request_params: request_params.clone(),
                    max_output_tokens: *max_output_tokens,
                })
            }
        }
    }
}

impl From<AuxiliaryModelPolicy> for WorkflowModelPolicy {
    fn from(policy: AuxiliaryModelPolicy) -> Self {
        match policy {
            AuxiliaryModelPolicy::Session => Self::Session,
            AuxiliaryModelPolicy::Explicit {
                model,
                profile,
                request_params,
                max_output_tokens,
            } => Self::Explicit {
                model: model.to_string(),
                profile: profile.map(|profile| profile.to_string()),
                request_params,
                max_output_tokens,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowToolSelector {
    Builtin { name: String },
    Source { source_id: String, name: String },
}

impl WorkflowToolSelector {
    pub(in crate::runtime::workflow) fn lower(
        &self,
    ) -> Result<ExactToolSelector, WorkflowCompileError> {
        let name = match self {
            Self::Builtin { name } | Self::Source { name, .. } => name,
        };
        if name.is_empty() {
            return Err(WorkflowCompileError::InvalidField(
                "Tool selection requires one exact name".into(),
            )
            .at("name"));
        }
        Ok(match self {
            Self::Builtin { name } => ExactToolSelector::Builtin { name: name.clone() },
            Self::Source { source_id, name } => ExactToolSelector::Source {
                source_id: crate::capabilities::ToolSourceId::try_from(source_id.clone()).map_err(
                    |_| {
                        WorkflowCompileError::InvalidField(
                            "Tool selection requires a valid source identity".into(),
                        )
                        .at("source_id")
                    },
                )?,
                name: name.clone(),
            },
        })
    }
}

impl From<ExactToolSelector> for WorkflowToolSelector {
    fn from(selector: ExactToolSelector) -> Self {
        match selector {
            ExactToolSelector::Builtin { name } => Self::Builtin { name },
            ExactToolSelector::Source { source_id, name } => Self::Source {
                source_id: source_id.to_string(),
                name,
            },
        }
    }
}

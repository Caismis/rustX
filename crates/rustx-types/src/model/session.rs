//! Direct (non-Auxiliary) model invocation policy for Workflow Model nodes
//! and other direct calls.
use serde::{Deserialize, Serialize};

use crate::model::catalog::{ModelProfileId, ModelRef};
use crate::model::request_params::RequestParams;

/// Follow the Session primary invocation, or resolve an explicit catalog model.
///
/// Used by Workflow Model nodes and other direct model calls. These are not
/// Auxiliary consumers and do not apply Lowest-profile selection (#543).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum DirectModelPolicy {
    /// Generation follows the primary invocation captured by the consumer.
    #[default]
    Session,
    /// Generation uses an explicitly configured catalog model.
    Explicit {
        /// The catalog model reference.
        model: ModelRef,
        /// The selected Model Profile; the model's `default_profile` is used
        /// when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<ModelProfileId>,
        /// The explicit request-parameter overrides.
        #[serde(default)]
        request_params: RequestParams,
        /// The explicit output-budget override.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_output_tokens: Option<u32>,
    },
}

/// One desired model selection: the mutable configuration a session owns and
/// a Runtime Client sets as a whole.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelection {
    /// The selected catalog model.
    pub model: crate::model::catalog::ModelRef,
    /// The selected Model Profile; the model's `default_profile` is used
    /// when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<crate::model::catalog::ModelProfileId>,
    /// The explicit request-parameter overrides.
    #[serde(default)]
    pub request_params: crate::model::request_params::RequestParams,
    /// The explicit output-budget override; the selected profile's default,
    /// else the model's hard maximum, is used when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

impl ModelSelection {
    /// A selection of one model with every default.
    #[must_use]
    pub fn of(model: crate::model::catalog::ModelRef) -> Self {
        Self {
            model,
            profile: None,
            request_params: crate::model::request_params::RequestParams::new(),
            max_output_tokens: None,
        }
    }
}

impl DirectModelPolicy {
    /// Select the same native catalog semantics used by primary invocations.
    #[must_use]
    pub fn selection(&self) -> Option<ModelSelection> {
        match self {
            Self::Session => None,
            Self::Explicit {
                model,
                profile,
                request_params,
                max_output_tokens,
            } => Some(ModelSelection {
                model: model.clone(),
                profile: profile.clone(),
                request_params: request_params.clone(),
                max_output_tokens: *max_output_tokens,
            }),
        }
    }
}

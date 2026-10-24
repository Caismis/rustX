//! Finite registered Model/Profile discovery over the existing source catalog.
//! No credentials, bindings or provider availability are resolved here.
use super::UserConfigManager;
use crate::local_runtime::configuration::settings::{SettingsError, SourceTarget};
use crate::model::catalog::{ModelProfileId, ModelRef};
use serde::{Deserialize, Serialize};

const PAGE_SIZE: usize = 32;
// The compiler rejects longer selector identities, so they are not Workflow
// choices even when the general Model Catalog permits their spelling.
const SELECTOR_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowModelChoicesQuery {
    Models {
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
    },
    Profiles {
        model: ModelRef,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        offset: u64,
    },
}

/// At most 32 identities of at most 256 UTF-8 bytes in one exact source cut.
/// Continuation checks `revision`; a replaced source never silently retargets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowModelChoices {
    Models {
        revision: String,
        models: Vec<ModelRef>,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        next_offset: Option<u64>,
    },
    Profiles {
        revision: String,
        model: ModelRef,
        profiles: Vec<ModelProfileId>,
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        next_offset: Option<u64>,
    },
}

fn page<T>(
    values: impl Iterator<Item = T>,
    offset: u64,
) -> Result<(Vec<T>, Option<u64>), SettingsError> {
    let offset_index = usize::try_from(offset).map_err(|_| SettingsError::Invalid)?;
    let mut values = values
        .skip(offset_index)
        .take(PAGE_SIZE + 1)
        .collect::<Vec<_>>();
    let more = values.len() > PAGE_SIZE;
    values.truncate(PAGE_SIZE);
    let next = more.then(|| offset.checked_add(PAGE_SIZE as u64)).flatten();
    Ok((values, next))
}

impl UserConfigManager {
    /// Read one bounded page of registered choices in the file's exact scope.
    /// # Errors
    /// Refuses redirected scopes, stale source revisions and invalid queries.
    pub fn workflow_model_choices(
        &self,
        target: &SourceTarget,
        query: WorkflowModelChoicesQuery,
        expected_revision: Option<&str>,
    ) -> Result<WorkflowModelChoices, SettingsError> {
        let (source, revision) = self.capture_source_settings(target)?;
        if let Some(expected) = expected_revision
            && expected != revision
        {
            return Err(SettingsError::Conflict {
                scope: target.scope(),
                expected: expected.into(),
                actual: revision,
            });
        }
        let models = source
            .resolved
            .and_then(|source| source.models)
            .unwrap_or_default();
        match query {
            WorkflowModelChoicesQuery::Models { offset } => {
                let (models, next_offset) = page(
                    models
                        .keys()
                        .filter(|name| name.len() <= SELECTOR_BYTES)
                        .filter_map(|name| ModelRef::parse(name).ok()),
                    offset,
                )?;
                Ok(WorkflowModelChoices::Models {
                    revision,
                    models,
                    next_offset,
                })
            }
            WorkflowModelChoicesQuery::Profiles { model, offset } => {
                if model.as_str().len() > SELECTOR_BYTES {
                    return Err(SettingsError::Invalid);
                }
                let definition = models.get(model.as_str()).ok_or(SettingsError::Invalid)?;
                let (profiles, next_offset) = page(
                    definition
                        .profiles
                        .iter()
                        .flat_map(|profiles| profiles.keys())
                        .filter(|id| id.as_str().len() <= SELECTOR_BYTES)
                        .cloned(),
                    offset,
                )?;
                Ok(WorkflowModelChoices::Profiles {
                    revision,
                    model,
                    profiles,
                    next_offset,
                })
            }
        }
    }
}

//! Structured authoring on the bound native sources. No transport or browser policy.
pub use super::super::authoring::ModelLayer as AuthoredModelSelection;
use super::{Origin, SessionConfigInput, UserConfigManager};
use crate::model::{
    authoring::{Catalog, Model, Provider},
    catalog::{CredentialSource, CredentialSourceView, ModelCatalog},
    session::SessionModelConfig,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Write, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    User,
    Workspace,
    Catalog,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderDraft {
    pub base_url: String,
    /// Literal means retain the existing native secret, never supply/read one.
    pub credential: CredentialSourceView,
    pub models: Vec<Model>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CatalogSettings {
    pub document: String,
    pub revision: String,
    pub providers: BTreeMap<String, ProviderDraft>,
    pub models: crate::model::catalog::ModelCatalogView,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SelectionSource {
    pub document: String,
    pub revision: String,
    pub active: bool,
    pub authored: Option<AuthoredModelSelection>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SourceSettings {
    pub catalog: CatalogSettings,
    pub user: SelectionSource,
    pub workspace: SelectionSource,
    pub effective: Option<SessionModelConfig>,
    pub effective_request: Option<crate::model::invocation::ModelInvocationView>,
    pub effective_summary: Option<crate::model::invocation::ModelInvocationView>,
    pub provenance: BTreeMap<String, Origin>,
    /// Invalid/incomplete current sources do not prevent repairing the catalog.
    pub resolution_available: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceMutation {
    Catalog {
        providers: BTreeMap<String, ProviderDraft>,
    },
    UserModel {
        authored: Option<AuthoredModelSelection>,
    },
    WorkspaceModel {
        authored: Option<AuthoredModelSelection>,
    },
}
impl SourceMutation {
    fn scope(&self) -> SourceScope {
        match self {
            Self::Catalog { .. } => SourceScope::Catalog,
            Self::UserModel { .. } => SourceScope::User,
            Self::WorkspaceModel { .. } => SourceScope::Workspace,
        }
    }
}
#[derive(Debug)]
pub enum SettingsError {
    Conflict {
        scope: SourceScope,
        expected: String,
        actual: String,
    },
    UntrustedWorkspace,
    Invalid,
    Io,
    Committed,
}
fn read(path: &Path) -> Result<Option<Vec<u8>>, SettingsError> {
    super::super::settings::read_document(path).map_err(|_| SettingsError::Io)
}
fn revision(bytes: Option<&[u8]>) -> String {
    super::super::settings::revision(bytes)
}
fn catalog(bytes: &[u8]) -> Result<Catalog, SettingsError> {
    crate::toml_authoring::parse_detailed(bytes).map_err(|_| SettingsError::Invalid)
}
fn selection(
    path: &Path,
    bytes: Option<&[u8]>,
    project: bool,
    active: bool,
) -> Result<SelectionSource, SettingsError> {
    let layer = super::parse_layer(path, bytes.unwrap_or(b""), project)
        .map_err(|_| SettingsError::Invalid)?;
    let authored = layer.agent.as_ref().and_then(|a| a.model.clone());
    Ok(SelectionSource {
        document: path.display().to_string(),
        revision: revision(bytes),
        active,
        authored,
    })
}
impl UserConfigManager {
    fn settings_paths(
        &self,
        input: &SessionConfigInput,
    ) -> Result<[std::path::PathBuf; 3], SettingsError> {
        let (locations, _) = self
            .resolve_locations(input)
            .map_err(|_| SettingsError::Invalid)?;
        let project = input.config.as_ref().map_or_else(
            || locations.workspace.join("rustx.toml"),
            |p| super::absolute(&locations.workspace, p),
        );
        Ok([
            self.sources.settings.clone(),
            super::normalize_missing(&project).map_err(|_| SettingsError::Invalid)?,
            self.sources.models.clone(),
        ])
    }
    /// Serialize native source writers, including the existing default writer.
    fn settings_locks(
        paths: &[std::path::PathBuf; 3],
        trusted: bool,
    ) -> Result<Vec<std::fs::File>, SettingsError> {
        let mut paths = vec![paths[0].clone(), paths[2].clone()]
            .into_iter()
            .chain(trusted.then(|| paths[1].clone()))
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        paths
            .iter()
            .map(|p| super::super::settings::lock_document(p).map_err(|_| SettingsError::Io))
            .collect()
    }
    /// Coherent projection under the same source locks used for publication.
    /// # Errors
    /// Unreadable or malformed authoring is rejected without exposing file content.
    pub fn read_source_settings(
        &self,
        input: &SessionConfigInput,
    ) -> Result<SourceSettings, SettingsError> {
        let paths = self.settings_paths(input)?;
        #[cfg(test)]
        self.test_hooks.reach("before_trust");
        let epoch = self
            .trust_epoch(input)
            .map_err(|_| SettingsError::Invalid)?;
        let trusted = epoch.trusted();
        #[cfg(test)]
        self.test_hooks.reach("trust_acquired");
        #[cfg(test)]
        self.test_hooks.reach("before_documents");
        let _locks = Self::settings_locks(&paths, trusted)?;
        self.source_settings_locked(input, &paths, trusted)
    }
    fn source_settings_locked(
        &self,
        input: &SessionConfigInput,
        paths: &[std::path::PathBuf; 3],
        trusted: bool,
    ) -> Result<SourceSettings, SettingsError> {
        let user = read(&paths[0])?;
        // Do not parse or expose untrusted project content.
        let workspace = if trusted { read(&paths[1])? } else { None };
        let bytes = read(&paths[2])?;
        let document = match &bytes {
            Some(bytes) => catalog(bytes)?,
            None => Catalog {
                schema_version: 1,
                providers: BTreeMap::new(),
            },
        };
        let models = if bytes.is_none() {
            crate::model::catalog::ModelCatalogView { models: Vec::new() }
        } else {
            ModelCatalog::from_document(document.clone().into())
                .map_err(|_| SettingsError::Invalid)?
                .view()
        };
        let providers = document
            .providers
            .into_iter()
            .map(|(id, p)| {
                (
                    id,
                    ProviderDraft {
                        base_url: p.base_url,
                        credential: p.api_key.view(),
                        models: p.models,
                    },
                )
            })
            .collect();
        let resolved = self.resolve_model_candidate(input, None, trusted).ok();
        // A noncooperating editor may change a source while it is being resolved.
        // Do not return a mixed projection assembled from different content.
        for (index, captured, scope) in [
            (0, user.as_deref(), SourceScope::User),
            (2, bytes.as_deref(), SourceScope::Catalog),
        ]
        .into_iter()
        .chain(trusted.then_some((1, workspace.as_deref(), SourceScope::Workspace)))
        {
            let expected = revision(captured);
            let actual = revision(read(&paths[index])?.as_deref());
            if expected != actual {
                return Err(SettingsError::Conflict {
                    scope,
                    expected,
                    actual,
                });
            }
        }

        Ok(SourceSettings {
            catalog: CatalogSettings {
                document: paths[2].display().to_string(),
                revision: revision(bytes.as_deref()),
                providers,
                models,
            },
            user: selection(&paths[0], user.as_deref(), false, true)?,
            workspace: selection(&paths[1], workspace.as_deref(), true, trusted)?,
            effective_request: resolved.as_ref().and_then(|r| {
                crate::model::session::analyze_session_model_config(
                    &r.models,
                    r.config.initial_model(),
                )
                .ok()
                .map(|(primary, _)| primary)
            }),
            effective_summary: resolved.as_ref().and_then(|r| {
                crate::model::session::analyze_session_model_config(
                    &r.models,
                    r.config.initial_model(),
                )
                .ok()
                .map(|(primary, summary)| summary.unwrap_or(primary))
            }),
            effective: resolved.as_ref().map(|r| r.config.initial_model().clone()),
            provenance: resolved
                .as_ref()
                .map(|r| r.provenance.clone())
                .unwrap_or_default(),
            resolution_available: resolved.is_some(),
        })
    }
    /// Compare and publish under one native document lock. Source changes apply
    /// to fresh/cold resolution; admitted runtimes and attempts remain frozen.
    /// # Errors
    /// Stale revisions, untrusted projects and invalid candidates never publish.
    pub fn write_source_settings(
        &self,
        input: &SessionConfigInput,
        expected: &str,
        mutation: SourceMutation,
    ) -> Result<SourceSettings, SettingsError> {
        let paths = self.settings_paths(input)?;
        #[cfg(test)]
        self.test_hooks.reach("before_trust");
        let epoch = self
            .trust_epoch(input)
            .map_err(|_| SettingsError::Invalid)?;
        let trusted = epoch.trusted();
        #[cfg(test)]
        self.test_hooks.reach("trust_acquired");
        let scope = mutation.scope();
        if scope == SourceScope::Workspace && !trusted {
            return Err(SettingsError::UntrustedWorkspace);
        }
        #[cfg(test)]
        self.test_hooks.reach("before_documents");
        let _locks = Self::settings_locks(&paths, trusted)?;
        let target = &paths[match scope {
            SourceScope::User => 0,
            SourceScope::Workspace => 1,
            SourceScope::Catalog => 2,
        }];
        let original = read(target)?;
        let actual = revision(original.as_deref());
        if expected != actual {
            return Err(SettingsError::Conflict {
                scope,
                expected: expected.into(),
                actual,
            });
        }
        let candidate = match mutation {
            SourceMutation::Catalog { providers } => {
                catalog_candidate(original.as_deref(), providers)?
            }
            SourceMutation::UserModel { authored }
            | SourceMutation::WorkspaceModel { authored } => self.selection_candidate(
                input,
                &paths,
                scope,
                original.as_deref(),
                authored,
                trusted,
            )?,
        };
        let mut staged = tempfile::NamedTempFile::new_in(target.parent().ok_or(SettingsError::Io)?)
            .map_err(|_| SettingsError::Io)?;
        staged
            .write_all(&candidate)
            .map_err(|_| SettingsError::Io)?;
        staged.as_file().sync_all().map_err(|_| SettingsError::Io)?;
        let actual = revision(read(target)?.as_deref());
        if actual != expected {
            return Err(SettingsError::Conflict {
                scope,
                expected: expected.into(),
                actual,
            });
        }
        #[cfg(test)]
        self.test_hooks.reach("before_publication");
        staged.persist(target).map_err(|_| SettingsError::Io)?;
        self.source_settings_locked(input, &paths, trusted)
            .map_err(|_| SettingsError::Committed)
    }
    fn selection_candidate(
        &self,
        input: &SessionConfigInput,
        paths: &[std::path::PathBuf; 3],
        scope: SourceScope,
        original: Option<&[u8]>,
        authored: Option<AuthoredModelSelection>,
        trusted: bool,
    ) -> Result<Vec<u8>, SettingsError> {
        let target = &paths[usize::from(scope != SourceScope::User)];
        let mut tree: toml_edit::DocumentMut = std::str::from_utf8(original.unwrap_or(b""))
            .map_err(|_| SettingsError::Invalid)?
            .parse()
            .map_err(|_| SettingsError::Invalid)?;
        if !tree.contains_key("agent") {
            tree["agent"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let agent = tree["agent"]
            .as_table_like_mut()
            .ok_or(SettingsError::Invalid)?;
        let selecting = authored.is_some();
        if let Some(authored) = authored {
            let model: toml_edit::DocumentMut = toml::to_string(&authored)
                .map_err(|_| SettingsError::Invalid)?
                .parse()
                .map_err(|_| SettingsError::Invalid)?;
            agent.insert("model", toml_edit::Item::Table(model.as_table().clone()));
        } else {
            agent.remove("model");
        }
        let bytes = tree.to_string().into_bytes();
        super::parse_layer(target, &bytes, scope == SourceScope::Workspace)
            .map_err(|_| SettingsError::Invalid)?;
        // A partial User layer may rely on Workspace/Session for model identity.
        // Missing identity is repairable; all other canonical validation fails closed.
        if selecting {
            let mut prospective = input.clone();
            prospective.model = None;
            if let Err(error) = self.resolve_model_candidate(
                &prospective,
                Some((target, &bytes)),
                trusted && scope == SourceScope::Workspace,
            ) && !error.incomplete
            {
                return Err(SettingsError::Invalid);
            }
        }
        Ok(bytes)
    }
}

fn catalog_candidate(
    original: Option<&[u8]>,
    providers: BTreeMap<String, ProviderDraft>,
) -> Result<Vec<u8>, SettingsError> {
    let old = original.map(catalog).transpose()?;
    let mut authored = BTreeMap::new();
    for (id, draft) in providers {
        let api_key = match draft.credential {
            CredentialSourceView::Environment { variable } => CredentialSource::parse(
                &format!("${variable}"),
                &crate::model::catalog::ProviderId::new(&id),
            )
            .map_err(|_| SettingsError::Invalid)?,
            CredentialSourceView::Literal => old
                .as_ref()
                .and_then(|c| c.providers.get(&id))
                .map(|p| p.api_key.clone())
                .ok_or(SettingsError::Invalid)?,
        };
        authored.insert(
            id,
            Provider {
                base_url: draft.base_url,
                api_key,
                models: draft.models,
            },
        );
    }
    let document = Catalog {
        schema_version: 1,
        providers: authored,
    };
    ModelCatalog::from_document(document.clone().into()).map_err(|_| SettingsError::Invalid)?;
    // Canonical serializer never serializes literal secrets; restore only
    // native-retained credential strings within this private write buffer.
    let mut serializable = document.clone();
    for provider in serializable.providers.values_mut() {
        provider.api_key = CredentialSource::Environment("RUSTX_RETAINED".into());
    }
    let mut tree: toml::Value =
        toml::from_str(&toml::to_string(&serializable).map_err(|_| SettingsError::Invalid)?)
            .map_err(|_| SettingsError::Invalid)?;
    for (id, provider) in document.providers {
        tree["providers"][&id]["api_key"] = toml::Value::String(match provider.api_key {
            CredentialSource::Literal(value) => value,
            CredentialSource::Environment(name) => format!("${name}"),
        });
    }
    Ok(toml::to_string(&tree)
        .map_err(|_| SettingsError::Invalid)?
        .into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn fixture() -> (tempfile::TempDir, UserConfigManager, SessionConfigInput) {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let home = root_path.join("home");
        let config = home.join("config");
        let state = home.join("state");
        let workspace = root_path.join("workspace");
        for p in [&config, &state, &workspace] {
            std::fs::create_dir_all(p).unwrap();
        }
        let models = include_str!("../../../examples/local-runtime/minimal/models.toml");
        let mut document: Catalog = crate::toml_authoring::parse(models.as_bytes()).unwrap();
        let provider = document.providers.get_mut("example").unwrap();
        let mut second = provider.models[0].clone();
        second.id = "second".into();
        provider.models.push(second);
        std::fs::write(
            config.join("models.toml"),
            toml::to_string(&document).unwrap(),
        )
        .unwrap();
        std::fs::write(
            config.join("settings.toml"),
            "[agent.model]\nmodel = 'example/demo-model'\n",
        )
        .unwrap();
        let owner = UserConfigManager::new(super::super::UserConfigSources {
            home_directory: home,
            config_directory: config.clone(),
            state_directory: state,
            settings: config.join("settings.toml"),
            models: config.join("models.toml"),
            runtime_root: root_path.join("runtime"),
        })
        .unwrap();
        let input = SessionConfigInput::new(workspace);
        (root, owner, input)
    }
    pub(super) fn selected(id: &str) -> SessionModelConfig {
        SessionModelConfig::of(crate::model::catalog::ModelRef::parse(id).unwrap())
    }
    pub(super) fn authored(id: &str) -> AuthoredModelSelection {
        AuthoredModelSelection {
            model: Some(crate::model::catalog::ModelRef::parse(id).unwrap()),
            ..Default::default()
        }
    }
    pub(super) fn trust(owner: &UserConfigManager, input: &SessionConfigInput) {
        owner
            .trust_epoch(input)
            .unwrap()
            .change(crate::local_runtime::launch::TrustAction::Grant)
            .unwrap();
    }
    #[test]
    fn native_precedence_provenance_reset_and_separate_cas_domains() {
        let (_root, owner, mut input) = fixture();
        trust(&owner, &input);
        let first = owner.read_source_settings(&input).unwrap();
        assert_eq!(
            first.effective.as_ref().unwrap().model.to_string(),
            "example/demo-model"
        );
        assert!(matches!(
            first.provenance["agent.model.model"],
            Origin::User { .. }
        ));
        let project = owner
            .write_source_settings(
                &input,
                &first.workspace.revision,
                SourceMutation::WorkspaceModel {
                    authored: Some(authored("example/second")),
                },
            )
            .unwrap();
        assert_eq!(
            project.effective.as_ref().unwrap().model.to_string(),
            "example/second"
        );
        assert!(matches!(
            project.provenance["agent.model.model"],
            Origin::Project { .. }
        ));
        assert_eq!(project.user.revision, first.user.revision);
        input.model = Some(selected("example/demo-model"));
        let session = owner.read_source_settings(&input).unwrap();
        assert_eq!(
            session.effective.unwrap().model.to_string(),
            "example/demo-model"
        );
        assert!(matches!(
            session.provenance["agent.model.model"],
            Origin::Explicit { .. }
        ));
        input.model = None;
        assert_eq!(
            owner
                .read_source_settings(&input)
                .unwrap()
                .effective
                .unwrap()
                .model
                .to_string(),
            "example/second"
        );
        let cleared = owner
            .write_source_settings(
                &input,
                &project.workspace.revision,
                SourceMutation::WorkspaceModel { authored: None },
            )
            .unwrap();
        assert!(cleared.workspace.authored.is_none());
        assert_eq!(
            cleared.effective.unwrap().model.to_string(),
            "example/demo-model"
        );
        assert!(matches!(
            owner.write_source_settings(
                &input,
                &first.workspace.revision,
                SourceMutation::WorkspaceModel {
                    authored: Some(authored("example/second"))
                }
            ),
            Err(SettingsError::Conflict {
                scope: SourceScope::Workspace,
                ..
            })
        ));
        assert_eq!(
            owner
                .read_source_settings(&input)
                .unwrap()
                .workspace
                .revision,
            cleared.workspace.revision
        );
    }
    #[test]
    fn untrusted_workspace_and_unknown_model_fail_without_mutation() {
        let (_root, owner, input) = fixture();
        std::fs::write(
            input.cwd.join("rustx.toml"),
            "[agent.model]\nmodel = 'example/second'\n",
        )
        .unwrap();
        let first = owner.read_source_settings(&input).unwrap();
        assert!(!first.workspace.active);
        assert_eq!(
            first.effective.unwrap().model.to_string(),
            "example/demo-model"
        );
        assert!(matches!(
            owner.write_source_settings(
                &input,
                "missing",
                SourceMutation::WorkspaceModel {
                    authored: Some(authored("example/second"))
                }
            ),
            Err(SettingsError::UntrustedWorkspace)
        ));
        assert!(matches!(
            owner.write_source_settings(
                &input,
                &first.user.revision,
                SourceMutation::UserModel {
                    authored: Some(authored("example/unknown"))
                }
            ),
            Err(SettingsError::Invalid)
        ));
        assert_eq!(
            owner.read_source_settings(&input).unwrap().user.revision,
            first.user.revision
        );
        assert!(!owner.project_trusted(&input).unwrap());
    }
    #[test]
    fn catalog_roundtrip_validates_before_commit_and_never_returns_literal_secret() {
        let (_root, owner, input) = fixture();
        let bytes = std::fs::read_to_string(&owner.sources.models)
            .unwrap()
            .replace("$RUSTX_EXAMPLE_API_KEY", "SECRET_SENTINEL");
        std::fs::write(&owner.sources.models, bytes).unwrap();
        let first = owner.read_source_settings(&input).unwrap();
        assert!(
            !serde_json::to_string(&first)
                .unwrap()
                .contains("SECRET_SENTINEL")
        );
        for invalid in 0..3 {
            let mut providers = first.catalog.providers.clone();
            let model = &mut providers.get_mut("example").unwrap().models[0];
            match invalid {
                0 => model.capabilities.input_modalities.clear(),
                1 => model.protocol = crate::model::ModelProtocol::OpenAiResponses,
                _ => model.max_output_tokens = 0,
            }
            assert!(matches!(
                owner.write_source_settings(
                    &input,
                    &first.catalog.revision,
                    SourceMutation::Catalog { providers }
                ),
                Err(SettingsError::Invalid)
            ));
            assert_eq!(
                owner.read_source_settings(&input).unwrap().catalog.revision,
                first.catalog.revision
            );
        }
        let mut providers = first.catalog.providers.clone();
        providers.get_mut("example").unwrap().models[0].context_window = 256_000;
        let saved = owner
            .write_source_settings(
                &input,
                &first.catalog.revision,
                SourceMutation::Catalog {
                    providers: providers.clone(),
                },
            )
            .unwrap();
        assert_ne!(saved.catalog.revision, first.catalog.revision);
        assert_eq!(
            saved.catalog.providers["example"].models[0].context_window,
            256_000
        );
        assert!(matches!(
            owner.write_source_settings(
                &input,
                &first.catalog.revision,
                SourceMutation::Catalog {
                    providers: providers.clone()
                }
            ),
            Err(SettingsError::Conflict {
                scope: SourceScope::Catalog,
                ..
            })
        ));
        providers.get_mut("example").unwrap().base_url = "invalid".into();
        assert!(matches!(
            owner.write_source_settings(
                &input,
                &saved.catalog.revision,
                SourceMutation::Catalog { providers }
            ),
            Err(SettingsError::Invalid)
        ));
        assert_eq!(
            owner.read_source_settings(&input).unwrap().catalog.revision,
            saved.catalog.revision
        );
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    #[test]
    fn competing_catalog_writers_have_one_publication_and_one_conflict() {
        let (_root, owner, input) = super::tests::fixture();
        let first = owner.read_source_settings(&input).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles = [256_000, 512_000].map(|limit| {
            let owner = owner.clone();
            let input = input.clone();
            let barrier = barrier.clone();
            let first = first.clone();
            std::thread::spawn(move || {
                let mut providers = first.catalog.providers;
                providers.get_mut("example").unwrap().models[0].context_window = limit;
                barrier.wait();
                owner.write_source_settings(
                    &input,
                    &first.catalog.revision,
                    SourceMutation::Catalog { providers },
                )
            })
        });
        let results = handles.map(|handle| handle.join().unwrap());
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(SettingsError::Conflict {
                        scope: SourceScope::Catalog,
                        ..
                    })
                ))
                .count(),
            1
        );
        let winner = results.into_iter().find_map(Result::ok).unwrap();
        assert_eq!(
            owner.read_source_settings(&input).unwrap().catalog,
            winner.catalog
        );
    }
}

#[cfg(test)]
mod roundtrip_tests {
    use super::tests::{authored, fixture, selected, trust};
    use super::*;
    #[test]
    fn structured_provider_add_delete_preserves_secret_and_external_changes_conflict() {
        let (_root, owner, input) = super::tests::fixture();
        let original = std::fs::read_to_string(&owner.sources.models)
            .unwrap()
            .replace("$RUSTX_EXAMPLE_API_KEY", "SECRET_SENTINEL");
        std::fs::write(&owner.sources.models, &original).unwrap();
        let first = owner.read_source_settings(&input).unwrap();
        assert_eq!(
            first.catalog.providers["example"].credential,
            CredentialSourceView::Literal
        );
        let mut providers = first.catalog.providers.clone();
        let mut added = providers["example"].clone();
        added.credential = CredentialSourceView::Environment {
            variable: "NEW_KEY".into(),
        };
        providers.insert("added".into(), added);
        let added = owner
            .write_source_settings(
                &input,
                &first.catalog.revision,
                SourceMutation::Catalog { providers },
            )
            .unwrap();
        assert!(
            std::fs::read_to_string(&owner.sources.models)
                .unwrap()
                .contains("SECRET_SENTINEL")
        );
        assert!(
            !serde_json::to_string(&added)
                .unwrap()
                .contains("SECRET_SENTINEL")
        );
        let mut providers = added.catalog.providers;
        providers.remove("added");
        let deleted = owner
            .write_source_settings(
                &input,
                &added.catalog.revision,
                SourceMutation::Catalog {
                    providers: providers.clone(),
                },
            )
            .unwrap();
        assert_eq!(deleted.catalog.providers.len(), 1);
        let external = format!(
            "# external edit\n{}",
            std::fs::read_to_string(&owner.sources.models).unwrap()
        );
        std::fs::write(&owner.sources.models, &external).unwrap();
        assert!(matches!(
            owner.write_source_settings(
                &input,
                &deleted.catalog.revision,
                SourceMutation::Catalog { providers }
            ),
            Err(SettingsError::Conflict { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(&owner.sources.models).unwrap(),
            external
        );
    }
    #[test]
    fn partial_layers_roundtrip_omissions_defaults_and_reset() {
        let (_root, owner, mut input) = fixture();
        trust(&owner, &input);
        std::fs::write(
            &owner.sources.settings,
            "[agent.model.request_params]\ntemperature = 0.3\n[environment]\nKEEP = 'unchanged'\n",
        )
        .unwrap();
        std::fs::write(
            input.cwd.join("rustx.toml"),
            "[agent.model]\nmodel = 'example/demo-model'\n",
        )
        .unwrap();
        let before = owner.read_source_settings(&input).unwrap();
        let partial = before.user.authored.clone().unwrap();
        assert!(partial.model.is_none());
        let after = owner
            .write_source_settings(
                &input,
                &before.user.revision,
                SourceMutation::UserModel {
                    authored: Some(partial.clone()),
                },
            )
            .unwrap();
        assert_eq!(after.user.authored, Some(partial));
        assert_eq!(
            after.effective.as_ref().unwrap().request_params["temperature"],
            0.3
        );
        let bytes = std::fs::read_to_string(&owner.sources.settings).unwrap();
        assert!(bytes.contains("KEEP = 'unchanged'"));
        assert!(!bytes.contains("reasoning_profile"));
        assert!(!bytes.contains("max_output_tokens"));
        assert!(!bytes.contains("model ="));
        let mut layer = after.workspace.authored.clone().unwrap();
        layer.max_output_tokens =
            Some(super::super::super::authoring::ModelOutput::CatalogDefault {});
        let explicit = owner
            .write_source_settings(
                &input,
                &after.workspace.revision,
                SourceMutation::WorkspaceModel {
                    authored: Some(layer.clone()),
                },
            )
            .unwrap();
        assert_eq!(explicit.workspace.authored, Some(layer));
        assert!(
            explicit
                .workspace
                .authored
                .as_ref()
                .unwrap()
                .reasoning_profile
                .is_none()
        );
        assert!(
            serde_json::to_value(&explicit.workspace.authored)
                .unwrap()
                .get("reasoning_profile")
                .is_none()
        );
        assert_eq!(
            serde_json::to_value(&explicit.workspace.authored).unwrap()["max_output_tokens"]["mode"],
            "catalog_default"
        );
        input.model = Some(selected("example/second"));
        let whole = owner.read_source_settings(&input).unwrap();
        assert!(whole.effective.unwrap().request_params.is_empty());
        input.model = None;
        let reset_user = owner
            .write_source_settings(
                &input,
                &explicit.user.revision,
                SourceMutation::UserModel { authored: None },
            )
            .unwrap();
        assert!(reset_user.user.authored.is_none());
        assert!(reset_user.effective.unwrap().request_params.is_empty());
        assert!(
            std::fs::read_to_string(&owner.sources.settings)
                .unwrap()
                .contains("KEEP = 'unchanged'")
        );
        let reset_workspace = owner
            .write_source_settings(
                &input,
                &explicit.workspace.revision,
                SourceMutation::WorkspaceModel { authored: None },
            )
            .unwrap();
        assert!(reset_workspace.workspace.authored.is_none());
        assert!(!reset_workspace.resolution_available); // No invented default identity.
    }

    #[test]
    fn workspace_policy_without_identity_preserves_omitted_and_explicit_defaults() {
        let (_root, owner, input) = fixture();
        trust(&owner, &input);
        let before = owner.read_source_settings(&input).unwrap();
        let partial = AuthoredModelSelection {
            request_params: Some(crate::toml_authoring::RequestParamsToml(
                serde_json::from_value(serde_json::json!({"temperature": 0.4})).unwrap(),
            )),
            ..Default::default()
        };
        let first = owner
            .write_source_settings(
                &input,
                &before.workspace.revision,
                SourceMutation::WorkspaceModel {
                    authored: Some(partial.clone()),
                },
            )
            .unwrap();
        assert_eq!(first.workspace.authored, Some(partial.clone()));
        assert!(matches!(
            first.provenance["agent.model.model"],
            Origin::User { .. }
        ));
        assert!(matches!(
            first.provenance["agent.model.request_params"],
            Origin::Project { .. }
        ));
        let mut explicit = partial;
        explicit.reasoning_profile =
            Some(super::super::super::authoring::ReasoningSelection::CatalogDefault {});
        let second = owner
            .write_source_settings(
                &input,
                &first.workspace.revision,
                SourceMutation::WorkspaceModel {
                    authored: Some(explicit.clone()),
                },
            )
            .unwrap();
        assert_eq!(second.workspace.authored, Some(explicit));
        let bytes = std::fs::read_to_string(input.cwd.join("rustx.toml")).unwrap();
        assert!(!bytes.contains("model ="));
        assert!(!bytes.contains("max_output_tokens"));
        assert!(!bytes.contains("summary_model"));
        assert!(bytes.contains("catalog_default"));
        let reset = owner
            .write_source_settings(
                &input,
                &second.workspace.revision,
                SourceMutation::WorkspaceModel { authored: None },
            )
            .unwrap();
        assert!(reset.effective.unwrap().request_params.is_empty());
        assert!(matches!(
            reset.provenance["agent.model.model"],
            Origin::User { .. }
        ));
    }

    #[test]
    fn model_domain_remains_usable_with_invalid_runtime_workflow_semantics() {
        let (_root, owner, mut input) = fixture();
        trust(&owner, &input);
        let path = input.cwd.join("rustx.toml");
        std::fs::write(&path, "[agent]\nworkflows = ['check', 'check']\n[agent.model]\nmax_output_tokens = { mode = 'limit', tokens = 1024 }\n").unwrap();
        let first = owner.read_source_settings(&input).unwrap();
        assert!(first.resolution_available);
        assert_eq!(
            first.effective.as_ref().unwrap().model.to_string(),
            "example/demo-model"
        );
        assert_eq!(
            first.effective_request.as_ref().unwrap().max_output_tokens,
            1024
        );
        assert!(matches!(
            first.provenance["agent.model.model"],
            Origin::User { .. }
        ));
        assert!(matches!(
            first.provenance["agent.model.max_output_tokens"],
            Origin::Project { .. }
        ));
        assert_eq!(first.user.authored, Some(authored("example/demo-model")));
        assert!(first.workspace.authored.as_ref().unwrap().model.is_none());
        let partial = AuthoredModelSelection {
            max_output_tokens: Some(crate::local_runtime::authoring::ModelOutput::Limit {
                tokens: 2048,
            }),
            ..Default::default()
        };
        let saved = owner
            .write_source_settings(
                &input,
                &first.workspace.revision,
                SourceMutation::WorkspaceModel {
                    authored: Some(partial.clone()),
                },
            )
            .unwrap();
        assert!(saved.resolution_available);
        assert_eq!(saved.workspace.authored, Some(partial));
        assert_eq!(saved.effective_request.unwrap().max_output_tokens, 2048);
        let bytes = std::fs::read(&path).unwrap();
        let parsed = super::super::parse_layer(&path, &bytes, true).unwrap();
        assert!(parsed.agent.unwrap().model.unwrap().model.is_none());
        let error = owner.resolve_session(&input).err().unwrap().to_string();
        assert!(error.contains("duplicate"), "{error}");
        input.model = Some(selected("example/second"));
        let selected = owner.resolve_model_configuration(&input).unwrap();
        assert_eq!(
            selected.config.initial_model(),
            input.model.as_ref().unwrap()
        );
        assert!(
            owner
                .resolve_session(&input)
                .err()
                .unwrap()
                .to_string()
                .contains("duplicate")
        );
    }

    #[test]
    fn model_projection_does_not_prepare_invalid_workflows() {
        let (_root, owner, input) = fixture();
        trust(&owner, &input);
        std::fs::create_dir_all(input.cwd.join(".agents/workflows")).unwrap();
        std::fs::write(
            input.cwd.join(".agents/workflows/broken.yaml"),
            "invalid: [",
        )
        .unwrap();
        std::fs::write(
            input.cwd.join("rustx.toml"),
            "[agent.model]\nmodel = 'example/second'\n",
        )
        .unwrap();
        let projection = owner.read_source_settings(&input).unwrap();
        assert!(projection.resolution_available);
        assert!(matches!(
            projection.provenance["agent.model.model"],
            Origin::Project { .. }
        ));
        assert_eq!(
            projection.effective.unwrap().model.to_string(),
            "example/second"
        );
        assert!(owner.resolve_session(&input).is_err());
        owner
            .write_source_settings(
                &input,
                &projection.workspace.revision,
                SourceMutation::WorkspaceModel {
                    authored: Some(authored("example/demo-model")),
                },
            )
            .unwrap();
    }

    fn pause(
        owner: &UserConfigManager,
        point: &'static str,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (entered, waiting) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        owner.test_hooks.insert(point, move || {
            entered.send(()).unwrap();
            resume.recv().unwrap();
        });
        (waiting, release)
    }
    fn change_trust(
        owner: &UserConfigManager,
        input: &SessionConfigInput,
        action: crate::local_runtime::launch::TrustAction,
    ) {
        owner.trust_epoch(input).unwrap().change(action).unwrap();
    }
    #[test]
    fn revoke_wins_before_workspace_publication_authority() {
        use crate::local_runtime::launch::TrustAction;
        let (_root, owner, input) = fixture();
        change_trust(&owner, &input, TrustAction::Grant);
        let before = owner.read_source_settings(&input).unwrap();
        let (waiting, resume) = pause(&owner, "before_trust");
        std::thread::scope(|threads| {
            let save = threads.spawn(|| {
                owner.write_source_settings(
                    &input,
                    &before.workspace.revision,
                    SourceMutation::WorkspaceModel {
                        authored: Some(authored("example/second")),
                    },
                )
            });
            waiting.recv().unwrap();
            change_trust(&owner, &input, TrustAction::Revoke);
            resume.send(()).unwrap();
            assert!(matches!(
                save.join().unwrap(),
                Err(SettingsError::UntrustedWorkspace)
            ));
        });
        assert!(!input.cwd.join("rustx.toml").exists());
    }
    #[test]
    fn workspace_publication_owns_trust_until_after_commit() {
        use crate::local_runtime::launch::TrustAction;
        let (_root, owner, input) = fixture();
        change_trust(&owner, &input, TrustAction::Grant);
        let before = owner.read_source_settings(&input).unwrap();
        let (waiting, resume) = pause(&owner, "before_publication");
        std::thread::scope(|threads| {
            let save = threads.spawn(|| {
                owner.write_source_settings(
                    &input,
                    &before.workspace.revision,
                    SourceMutation::WorkspaceModel {
                        authored: Some(authored("example/second")),
                    },
                )
            });
            waiting.recv().unwrap();
            // The actual persistent authority lock is held at publication.
            let (_, identity) = owner.resolve_locations(&input).unwrap();
            let target = super::super::TrustEpoch::lock_target(&owner.sources, &identity).unwrap();
            let name = target.file_name().unwrap().to_string_lossy();
            let path = target.with_file_name(format!(".{name}.lock"));
            let probe = std::fs::File::open(path).unwrap();
            assert!(probe.try_lock().is_err());
            let revoke = threads.spawn(|| change_trust(&owner, &input, TrustAction::Revoke));
            resume.send(()).unwrap();
            let saved = save.join().unwrap().unwrap();
            assert!(saved.workspace.active);
            assert!(matches!(
                saved.provenance["agent.model.model"],
                Origin::Project { .. }
            ));
            revoke.join().unwrap();
        });
        assert!(!owner.project_trusted(&input).unwrap());
        assert!(input.cwd.join("rustx.toml").exists());
    }
    #[test]
    fn reads_keep_one_trust_epoch_across_grant_and_revoke() {
        use crate::local_runtime::launch::TrustAction;
        let (_root, owner, input) = fixture();
        std::fs::write(
            input.cwd.join("rustx.toml"),
            "[agent.model]\nmodel = 'example/second'\n",
        )
        .unwrap();
        for (initial, action) in [(false, TrustAction::Grant), (true, TrustAction::Revoke)] {
            let (waiting, resume) = pause(&owner, "trust_acquired");
            std::thread::scope(|threads| {
                let read = threads.spawn(|| owner.read_source_settings(&input));
                waiting.recv().unwrap();
                let mutation = threads.spawn(|| change_trust(&owner, &input, action));
                resume.send(()).unwrap();
                let read = read.join().unwrap().unwrap();
                assert_eq!(read.workspace.active, initial);
                assert_eq!(
                    matches!(read.provenance["agent.model.model"], Origin::Project { .. }),
                    initial
                );
                mutation.join().unwrap();
            });
            let after = owner.read_source_settings(&input).unwrap();
            assert_eq!(after.workspace.active, !initial);
            assert_eq!(
                matches!(
                    after.provenance["agent.model.model"],
                    Origin::Project { .. }
                ),
                !initial
            );
        }
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    #[test]
    fn model_authoring_cannot_publish_an_incompatible_context_budget() {
        let (_root, owner, input) = super::tests::fixture();
        let mut document = catalog(&std::fs::read(&owner.sources.models).unwrap()).unwrap();
        document.providers.get_mut("example").unwrap().models[1].context_window = 8192;
        std::fs::write(&owner.sources.models, toml::to_string(&document).unwrap()).unwrap();
        std::fs::write(&owner.sources.settings, "[agent.model]\nmodel = 'example/demo-model'\n[context]\nreserve_tokens = 10000\nkeep_recent_tokens = 4000\n").unwrap();
        let before = owner.read_source_settings(&input).unwrap();
        assert!(before.resolution_available);
        let selected = SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("example/second").unwrap(),
        );
        assert!(matches!(
            owner.write_source_settings(
                &input,
                &before.user.revision,
                SourceMutation::UserModel {
                    authored: Some(AuthoredModelSelection {
                        model: Some(selected.model),
                        ..Default::default()
                    })
                }
            ),
            Err(SettingsError::Invalid)
        ));
        assert_eq!(
            owner.read_source_settings(&input).unwrap().user,
            before.user
        );
    }
}

//! Closed integration authoring and content-free native source projection.
use super::settings::{SettingsError, SourceScope};
use super::{Origin, SessionConfigInput, UserConfigManager};
use crate::local_runtime::{authoring::McpAuthoring, config::McpTransportType};
use crate::runtime::identity::McpServerId;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

/// Exactly the two persistent configuration authorities. Never Session or Effective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationScope {
    User,
    Workspace,
}
impl IntegrationScope {
    pub(super) fn source(self) -> SourceScope {
        match self {
            Self::User => SourceScope::User,
            Self::Workspace => SourceScope::Workspace,
        }
    }
}

/// Native transport fields plus reference-only credentials. Existing ordinary
/// environment/header values stay private and may only be retained or removed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpDraft {
    pub enabled: Option<bool>,
    pub transport: Option<McpTransportType>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub url: Option<String>,
    pub retained_env: Vec<String>,
    pub retained_headers: Vec<String>,
    pub sensitive_env: BTreeMap<String, crate::credentials::EnvironmentReference>,
    pub sensitive_headers: BTreeMap<String, crate::credentials::EnvironmentReference>,
}
impl McpDraft {
    fn view(entry: &McpAuthoring) -> Self {
        Self {
            enabled: entry.enabled,
            transport: entry.transport_type,
            command: entry.command.clone(),
            args: entry.args.clone(),
            cwd: entry.cwd.clone(),
            url: entry.url.clone(),
            retained_env: entry.env.keys().cloned().collect(),
            retained_headers: entry.headers.keys().cloned().collect(),
            sensitive_env: entry.sensitive_env.clone().unwrap_or_default(),
            sensitive_headers: entry.sensitive_headers.clone().unwrap_or_default(),
        }
    }
    pub(super) fn author(
        self,
        old: Option<&McpAuthoring>,
        scope: IntegrationScope,
    ) -> Result<McpAuthoring, SettingsError> {
        fn retained(
            keys: Vec<String>,
            old: Option<&BTreeMap<String, String>>,
        ) -> Result<BTreeMap<String, String>, SettingsError> {
            let mut result = BTreeMap::new();
            for key in keys {
                let value = old
                    .and_then(|map| map.get(&key))
                    .ok_or(SettingsError::Invalid)?;
                if result.insert(key, value.clone()).is_some() {
                    return Err(SettingsError::Invalid);
                }
            }
            Ok(result)
        }
        if scope == IntegrationScope::Workspace
            && (!self.sensitive_env.is_empty() || !self.sensitive_headers.is_empty())
        {
            return Err(SettingsError::Invalid);
        }
        Ok(McpAuthoring {
            enabled: self.enabled,
            transport_type: self.transport,
            command: self.command,
            args: self.args,
            cwd: self.cwd,
            url: self.url,
            env: retained(self.retained_env, old.map(|e| &e.env))?,
            headers: retained(self.retained_headers, old.map(|e| &e.headers))?,
            sensitive_env: (!self.sensitive_env.is_empty()).then_some(self.sensitive_env),
            sensitive_headers: (!self.sensitive_headers.is_empty())
                .then_some(self.sensitive_headers),
        })
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpIdentityView {
    pub id: McpServerId,
    pub user: Option<McpDraft>,
    pub workspace: Option<McpDraft>,
    pub winning: Option<Origin>,
    pub activation: Option<crate::capabilities::activation::SourceActivation>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IntegrationSelections {
    pub agents: Option<Vec<crate::runtime::subagent::SubagentName>>,
    pub workflows: Option<Vec<crate::runtime::workflow::WorkflowId>>,
    pub disabled_skills: Option<Vec<String>>,
    pub skill_sources: Option<Vec<crate::skills::AutomaticSkillSource>>,
    pub extensions: Option<crate::extensions::NativeAgentExtensionsDocument>,
}
impl IntegrationSelections {
    pub(super) fn from_layer(layer: &crate::local_runtime::authoring::RuntimeLayer) -> Self {
        let agent = layer.agent.as_ref();
        Self {
            agents: agent.and_then(|a| a.agents.clone()),
            workflows: agent.and_then(|a| a.workflows.clone()),
            disabled_skills: agent.and_then(|a| a.disabled_skills.clone()),
            extensions: agent.and_then(|a| a.extensions.clone()),
            skill_sources: layer.skills.as_ref().and_then(|s| s.sources.clone()),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IntegrationControl {
    Agents {
        selected: Option<Vec<crate::runtime::subagent::SubagentName>>,
    },
    Workflows {
        selected: Option<Vec<crate::runtime::workflow::WorkflowId>>,
    },
    SkillVisibility {
        disabled: Option<Vec<String>>,
    },
    SkillSources {
        selected: Option<Vec<crate::skills::AutomaticSkillSource>>,
    },
    Extension {
        identity: crate::runtime::capability_inspection::NativeExtension,
        enabled: bool,
    },
    ResetExtensions,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IntegrationSettings {
    /// User-owned policy, independent of MCP definition precedence.
    pub mcp_tool_policies:
        BTreeMap<McpServerId, crate::local_runtime::config::InvocationPolicyDocument>,
    pub mcp: Vec<McpIdentityView>,
    pub mcp_valid: bool,
    pub user: IntegrationSelections,
    pub workspace: IntegrationSelections,
    pub prospective: IntegrationSelections,
    pub provenance: BTreeMap<String, Origin>,
    /// Static native resource projection, not runtime admission or connection proof.
    pub inventory: Option<crate::runtime::capability_inspection::CapabilityInspection>,
    pub agents: Vec<AgentDefinitionSource>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDefinitionSource {
    pub name: crate::runtime::subagent::SubagentName,
    pub selected: PathBuf,
    pub shadowed: Option<PathBuf>,
}
impl UserConfigManager {
    /// Same authority/overlay seam as full resolution, validating only MCP.
    pub(super) fn resolve_mcp_candidate(
        &self,
        input: &SessionConfigInput,
        candidate: Option<(&std::path::Path, &[u8])>,
        trusted: bool,
    ) -> Result<(), SettingsError> {
        let captured = self
            .capture_layers(input, candidate, trusted)
            .map_err(|_| SettingsError::Invalid)?;
        crate::local_runtime::config::resolve_mcp_bindings(
            &captured
                .merged
                .mcp_servers
                .unwrap_or_default()
                .into_iter()
                .map(|(id, entry)| (id, entry.resolve()))
                .collect(),
            &captured.merged.mcp_tool_policies.unwrap_or_default(),
        )
        .map_err(|_| SettingsError::Invalid)?;
        Ok(())
    }
    pub(super) fn integration_settings(
        &self,
        input: &SessionConfigInput,
        user: &[u8],
        workspace: &[u8],
        trusted: bool,
    ) -> Result<IntegrationSettings, SettingsError> {
        let captured = self
            .capture_layers(input, None, trusted)
            .map_err(|_| SettingsError::Invalid)?;
        let user = super::parse_layer(&self.sources.settings, user, false)
            .map_err(|_| SettingsError::Invalid)?;
        let workspace = super::parse_layer(&input.cwd.join("rustx.toml"), workspace, true)
            .map_err(|_| SettingsError::Invalid)?;
        let mut ids = std::collections::BTreeSet::new();
        for layer in [&user, &workspace] {
            ids.extend(layer.mcp_servers.iter().flat_map(|map| map.keys()).cloned());
        }
        let mcp = ids
            .into_iter()
            .map(|id| {
                let effective = captured
                    .merged
                    .mcp_servers
                    .as_ref()
                    .and_then(|m| m.get(&id));
                McpIdentityView {
                    user: user
                        .mcp_servers
                        .as_ref()
                        .and_then(|m| m.get(&id))
                        .map(McpDraft::view),
                    workspace: workspace
                        .mcp_servers
                        .as_ref()
                        .and_then(|m| m.get(&id))
                        .map(McpDraft::view),
                    winning: effective.and_then(|_| {
                        captured
                            .provenance
                            .get(&format!("mcp_servers.{id}"))
                            .cloned()
                    }),
                    activation: effective.map(|e| e.clone().resolve().activation()),
                    id,
                }
            })
            .collect();
        let resources = self
            .resolve_model_candidate(input, None, trusted)
            .and_then(Self::resolve_runtime_configuration)
            .and_then(|capture| self.resolve_resources(input, capture))
            .ok();
        Ok(IntegrationSettings {
            mcp_tool_policies: user.mcp_tool_policies.clone().unwrap_or_default(),
            mcp,
            mcp_valid: self.resolve_mcp_candidate(input, None, trusted).is_ok(),
            user: IntegrationSelections::from_layer(&user),
            workspace: IntegrationSelections::from_layer(&workspace),
            prospective: IntegrationSelections::from_layer(&captured.merged),
            provenance: captured
                .provenance
                .into_iter()
                .filter(|(key, _)| key.starts_with("agent.") || key.starts_with("skills"))
                .collect(),
            agents: resources
                .as_ref()
                .map(|r| {
                    r.role_sources
                        .iter()
                        .map(|(name, source)| AgentDefinitionSource {
                            name: name.clone(),
                            selected: source.selected.clone(),
                            shadowed: source.overridden.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            inventory: resources.map(|r| r.inspection),
        })
    }
}

#[cfg(test)]
#[allow(clippy::too_many_lines)] // End-to-end source and deterministic authority scenarios.
mod tests {
    use super::super::settings::{
        SourceMutation,
        tests::{fixture, trust},
    };
    use super::*;
    fn draft(command: &str) -> McpDraft {
        McpDraft {
            enabled: Some(false),
            transport: Some(McpTransportType::Stdio),
            command: Some(command.into()),
            ..Default::default()
        }
    }
    fn mutation(scope: IntegrationScope, id: &str, authored: Option<McpDraft>) -> SourceMutation {
        SourceMutation::Mcp {
            scope,
            id: McpServerId::new(id),
            authored,
        }
    }
    #[test]
    fn web09_both_scopes_roundtrip_whole_entries_provenance_and_independent_cas() {
        let (_root, owner, input) = fixture();
        trust(&owner, &input);
        let original = std::fs::read_to_string(&owner.sources.settings).unwrap();
        std::fs::write(
            &owner.sources.settings,
            format!("{original}\n[environment]\nKEEP = 'unchanged'\n"),
        )
        .unwrap();
        let first = owner.read_source_settings(&input).unwrap();
        let mut user = draft("user-command");
        user.args = vec!["user-only".into()];
        let added = owner
            .write_source_settings(
                &input,
                &first.user.revision,
                mutation(IntegrationScope::User, "same", Some(user)),
            )
            .unwrap();
        let project = owner
            .write_source_settings(
                &input,
                &first.workspace.revision,
                mutation(
                    IntegrationScope::Workspace,
                    "same",
                    Some(draft("workspace-command")),
                ),
            )
            .unwrap();
        let item = &project.integrations.mcp[0];
        assert!(matches!(item.winning, Some(Origin::Project { .. })));
        assert_eq!(item.user.as_ref().unwrap().args, ["user-only"]);
        assert!(item.workspace.as_ref().unwrap().args.is_empty());
        let native = owner.capture_layers(&input, None, true).unwrap();
        assert!(
            native.merged.mcp_servers.unwrap()[&McpServerId::new("same")]
                .args
                .is_empty()
        );
        let changed = owner
            .write_source_settings(
                &input,
                &added.user.revision,
                mutation(IntegrationScope::User, "same", Some(draft("edited-shadow"))),
            )
            .unwrap();
        assert_eq!(changed.workspace.revision, project.workspace.revision);
        assert_eq!(
            changed.integrations.mcp[0]
                .workspace
                .as_ref()
                .unwrap()
                .command
                .as_deref(),
            Some("workspace-command")
        );
        for (scope, revision) in [
            (IntegrationScope::User, first.user.revision),
            (IntegrationScope::Workspace, first.workspace.revision),
        ] {
            assert!(matches!(
                owner.write_source_settings(&input, &revision, mutation(scope, "same", None)),
                Err(SettingsError::Conflict { .. })
            ));
        }
        let other = owner
            .write_source_settings(
                &input,
                &changed.user.revision,
                mutation(IntegrationScope::User, "other", Some(draft("other"))),
            )
            .unwrap();
        assert_eq!(other.integrations.mcp.len(), 2);
        let removed = owner
            .write_source_settings(
                &input,
                &other.user.revision,
                mutation(IntegrationScope::User, "same", None),
            )
            .unwrap();
        assert_eq!(removed.workspace.revision, project.workspace.revision);
        assert!(
            removed
                .integrations
                .mcp
                .iter()
                .find(|m| m.id.as_str() == "same")
                .unwrap()
                .user
                .is_none()
        );
        let edited = owner
            .write_source_settings(
                &input,
                &removed.workspace.revision,
                mutation(
                    IntegrationScope::Workspace,
                    "same",
                    Some(draft("edited-project")),
                ),
            )
            .unwrap();
        let removed = owner
            .write_source_settings(
                &input,
                &edited.workspace.revision,
                mutation(IntegrationScope::Workspace, "same", None),
            )
            .unwrap();
        assert_eq!(removed.integrations.mcp.len(), 1);
        assert!(
            std::fs::read_to_string(&owner.sources.settings)
                .unwrap()
                .contains("KEEP = 'unchanged'")
        );
    }
    #[test]
    fn web09_mcp_domain_rejects_invalid_transport_without_requiring_workflows_or_models() {
        let (_root, owner, input) = fixture();
        trust(&owner, &input);
        std::fs::write(
            input.cwd.join("rustx.toml"),
            "[agent]\nworkflows = ['check', 'check']\n",
        )
        .unwrap();
        let before = owner.read_source_settings(&input).unwrap();
        let saved = owner
            .write_source_settings(
                &input,
                &before.user.revision,
                mutation(IntegrationScope::User, "ok", Some(draft("inert-no-spawn"))),
            )
            .unwrap();
        assert!(saved.integrations.mcp_valid);
        assert!(saved.integrations.inventory.is_none());
        assert!(owner.resolve_session(&input).is_err());
        let bytes = std::fs::read(&owner.sources.settings).unwrap();
        let mut both = draft("x");
        both.url = Some("http://localhost/mcp".into());
        let mut empty = draft("");
        empty.enabled = None;
        let mut invalid_url = draft("x");
        invalid_url.transport = Some(McpTransportType::Http);
        invalid_url.command = None;
        invalid_url.url = Some("file:///private".into());
        let mut nul = draft("bad\0command");
        nul.enabled = Some(true);
        for invalid in [both, empty, invalid_url, nul] {
            assert!(matches!(
                owner.write_source_settings(
                    &input,
                    &saved.user.revision,
                    mutation(IntegrationScope::User, "bad", Some(invalid))
                ),
                Err(SettingsError::Invalid)
            ));
            assert_eq!(std::fs::read(&owner.sources.settings).unwrap(), bytes);
        }
        let mut catalog: crate::model::authoring::Catalog =
            crate::toml_authoring::parse(&std::fs::read(&owner.sources.models).unwrap()).unwrap();
        catalog.providers.get_mut("example").unwrap().models[0].context_window = 0;
        std::fs::write(&owner.sources.models, toml::to_string(&catalog).unwrap()).unwrap();
        let invalid_model = owner.read_source_settings(&input).unwrap();
        assert!(!invalid_model.catalog.valid);
        owner
            .write_source_settings(
                &input,
                &invalid_model.user.revision,
                mutation(IntegrationScope::User, "independent", Some(draft("inert"))),
            )
            .unwrap();
        std::fs::write(&owner.sources.settings, "").unwrap();
        assert!(owner.resolve_mcp_candidate(&input, None, true).is_ok());
    }
    #[test]
    fn web09_untrusted_workspace_is_not_read_or_authorized_and_session_has_no_mcp_authority() {
        let (_root, owner, input) = fixture();
        std::fs::write(
            input.cwd.join("rustx.toml"),
            "private invalid untrusted bytes",
        )
        .unwrap();
        let view = owner.read_source_settings(&input).unwrap();
        assert!(!view.workspace.active);
        assert!(view.integrations.mcp.is_empty());
        assert!(matches!(
            owner.write_source_settings(
                &input,
                &view.workspace.revision,
                mutation(IntegrationScope::Workspace, "x", Some(draft("x")))
            ),
            Err(SettingsError::UntrustedWorkspace)
        ));
        assert!(!owner.project_trusted(&input).unwrap());
        assert!(
            serde_json::from_value::<SourceMutation>(
                serde_json::json!({"kind":"mcp","scope":"session","id":"x","authored":null})
            )
            .is_err()
        );
    }
    #[test]
    fn web09_secret_readback_and_workspace_security_ceiling() {
        let (_root, owner, input) = fixture();
        trust(&owner, &input);
        let original = std::fs::read_to_string(&owner.sources.settings).unwrap();
        std::fs::write(&owner.sources.settings, format!("{original}\n[mcp_servers.private]\nenabled = false\ncommand = 'private'\n[mcp_servers.private.env]\nTOKEN = 'SECRET_SENTINEL'\n[mcp_servers.private.sensitive_env]\nKEY = '$NATIVE_KEY'\n[mcp_tool_policies.private]\napproval = 'always'\n")).unwrap();
        let first = owner.read_source_settings(&input).unwrap();
        assert!(
            !serde_json::to_string(&first)
                .unwrap()
                .contains("SECRET_SENTINEL")
        );
        let retained = first.integrations.mcp[0].user.clone().unwrap();
        let saved = owner
            .write_source_settings(
                &input,
                &first.user.revision,
                mutation(IntegrationScope::User, "private", Some(retained)),
            )
            .unwrap();
        assert!(
            std::fs::read_to_string(&owner.sources.settings)
                .unwrap()
                .contains("SECRET_SENTINEL")
        );
        let project = owner
            .write_source_settings(
                &input,
                &saved.workspace.revision,
                mutation(
                    IntegrationScope::Workspace,
                    "private",
                    Some(draft("project")),
                ),
            )
            .unwrap();
        assert!(
            project.integrations.mcp[0]
                .workspace
                .as_ref()
                .unwrap()
                .sensitive_env
                .is_empty()
        );
        let capture = owner.capture_layers(&input, None, true).unwrap();
        assert!(
            capture.merged.mcp_servers.unwrap()[&McpServerId::new("private")]
                .sensitive_env
                .is_none()
        );
        assert!(matches!(
            capture.provenance["mcp_tool_policies.private"],
            Origin::User { .. }
        ));
        for bytes in [
            "approval_mode = 'never'",
            "[mcp_tool_policies.private]\napproval = 'never'",
            "[native_tools.bash]\napproval = 'never'",
            "[mcp_servers.x.sensitive_env]\nKEY = '$NATIVE_KEY'",
        ] {
            assert!(
                super::super::parse_layer(&input.cwd.join("rustx.toml"), bytes.as_bytes(), true)
                    .is_err()
            );
        }
        assert_eq!(
            project.integrations.mcp_tool_policies[&McpServerId::new("private")].approval,
            crate::local_runtime::config::ApprovalPolicyDocument::Always
        );
        let session = crate::local_runtime::session::SessionPersistentState::from_input(&input);
        for field in [
            "mcp_servers",
            "mcp_tool_policies",
            "approval_mode",
            "native_tools",
        ] {
            let mut wire = serde_json::to_value(&session).unwrap();
            wire[field] = serde_json::json!({});
            assert!(
                serde_json::from_value::<crate::local_runtime::session::SessionPersistentState>(
                    wire
                )
                .is_err()
            );
        }
        assert!(serde_json::from_value::<SourceMutation>(serde_json::json!({"kind":"mcp_policy","scope":"workspace","id":"private","authored":null})).is_err());
        let reset = owner
            .write_source_settings(
                &input,
                &project.user.revision,
                SourceMutation::McpPolicy {
                    id: McpServerId::new("private"),
                    authored: None,
                },
            )
            .unwrap();
        assert!(reset.integrations.mcp_tool_policies.is_empty());
        assert_eq!(reset.workspace.revision, project.workspace.revision);
        assert!(
            serde_json::from_value::<McpDraft>(
                serde_json::json!({"sensitive_env":{"KEY":"literal-secret"}})
            )
            .is_err()
        );
    }
    #[test]
    fn web09_extension_toggle_preserves_same_scope_siblings_and_replaces_cross_scope() {
        use crate::runtime::capability_inspection::NativeExtension;
        let (_root, owner, input) = fixture();
        trust(&owner, &input);
        let mut state = owner.read_source_settings(&input).unwrap();
        for (scope, identity) in [
            (IntegrationScope::User, NativeExtension::Todo),
            (IntegrationScope::User, NativeExtension::Goal),
            (IntegrationScope::Workspace, NativeExtension::AgentStatus),
        ] {
            let revision = match scope {
                IntegrationScope::User => &state.user.revision,
                IntegrationScope::Workspace => &state.workspace.revision,
            };
            state = owner
                .write_source_settings(
                    &input,
                    revision,
                    SourceMutation::Integration {
                        scope,
                        control: IntegrationControl::Extension {
                            identity,
                            enabled: true,
                        },
                    },
                )
                .unwrap();
        }
        let user = state.integrations.user.extensions.unwrap();
        assert!(user.todo.enabled && user.goal.enabled);
        let project = state.integrations.prospective.extensions.unwrap();
        assert!(project.agent_status.enabled);
        assert!(!project.todo.enabled && !project.goal.enabled);
        state = owner
            .write_source_settings(
                &input,
                &state.workspace.revision,
                SourceMutation::Integration {
                    scope: IntegrationScope::Workspace,
                    control: IntegrationControl::ResetExtensions,
                },
            )
            .unwrap();
        assert!(
            state
                .integrations
                .prospective
                .extensions
                .unwrap()
                .goal
                .enabled
        );
        let source = std::fs::read_to_string(&owner.sources.settings).unwrap();
        assert!(!source.contains("tasks"));
        assert!(!source.contains("objective"));
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
    #[test]
    fn web09_revoke_before_authority_prevents_mcp_publication() {
        let (_root, owner, input) = fixture();
        trust(&owner, &input);
        let before = owner.read_source_settings(&input).unwrap();
        let (waiting, resume) = pause(&owner, "before_trust");
        std::thread::scope(|threads| {
            let save = threads.spawn(|| {
                owner.write_source_settings(
                    &input,
                    &before.workspace.revision,
                    mutation(IntegrationScope::Workspace, "x", Some(draft("x"))),
                )
            });
            waiting.recv().unwrap();
            owner
                .trust_epoch(&input)
                .unwrap()
                .change(crate::local_runtime::launch::TrustAction::Revoke)
                .unwrap();
            resume.send(()).unwrap();
            assert!(matches!(
                save.join().unwrap(),
                Err(SettingsError::UntrustedWorkspace)
            ));
        });
        assert!(!input.cwd.join("rustx.toml").exists());
    }
    #[test]
    fn web09_publication_holds_trust_epoch_and_reread_after_revoke_is_coherent() {
        let (_root, owner, input) = fixture();
        trust(&owner, &input);
        let before = owner.read_source_settings(&input).unwrap();
        let (waiting, resume) = pause(&owner, "before_publication");
        std::thread::scope(|threads| {
            let save = threads.spawn(|| {
                owner.write_source_settings(
                    &input,
                    &before.workspace.revision,
                    mutation(IntegrationScope::Workspace, "x", Some(draft("x"))),
                )
            });
            waiting.recv().unwrap();
            let (_, identity) = owner.resolve_locations(&input).unwrap();
            let target = super::super::TrustEpoch::lock_target(&owner.sources, &identity).unwrap();
            let path = target.with_file_name(format!(
                ".{}.lock",
                target.file_name().unwrap().to_string_lossy()
            ));
            assert!(std::fs::File::open(path).unwrap().try_lock().is_err());
            let revoke = threads.spawn(|| {
                owner
                    .trust_epoch(&input)
                    .unwrap()
                    .change(crate::local_runtime::launch::TrustAction::Revoke)
                    .unwrap();
            });
            resume.send(()).unwrap();
            let saved = save.join().unwrap().unwrap();
            assert!(saved.workspace.active);
            assert!(matches!(
                saved.integrations.mcp[0].winning,
                Some(Origin::Project { .. })
            ));
            revoke.join().unwrap();
        });
        let after = owner.read_source_settings(&input).unwrap();
        assert!(!after.workspace.active);
        assert!(after.integrations.mcp.is_empty());
    }
    #[test]
    fn web09_read_spanning_trust_changes_keeps_matching_mcp_provenance() {
        use crate::local_runtime::launch::TrustAction;
        let (_root, owner, input) = fixture();
        std::fs::write(
            input.cwd.join("rustx.toml"),
            "[mcp_servers.local]\ncommand = 'inert'\nenabled = false\n",
        )
        .unwrap();
        for (trusted, action) in [(false, TrustAction::Grant), (true, TrustAction::Revoke)] {
            let (waiting, resume) = pause(&owner, "trust_acquired");
            std::thread::scope(|threads| {
                let read = threads.spawn(|| owner.read_source_settings(&input));
                waiting.recv().unwrap();
                let change = threads.spawn(|| {
                    owner.trust_epoch(&input).unwrap().change(action).unwrap();
                });
                resume.send(()).unwrap();
                let read = read.join().unwrap().unwrap();
                assert_eq!(read.workspace.active, trusted);
                assert_eq!(!read.integrations.mcp.is_empty(), trusted);
                if trusted {
                    assert!(matches!(
                        read.integrations.mcp[0].winning,
                        Some(Origin::Project { .. })
                    ));
                }
                change.join().unwrap();
            });
            let next = owner.read_source_settings(&input).unwrap();
            assert_eq!(next.workspace.active, !trusted);
            assert_eq!(next.integrations.mcp.is_empty(), trusted);
        }
    }
    #[test]
    fn web09_invalid_header_env_and_reference_combinations_fail_before_publication() {
        let (_root, owner, input) = fixture();
        for entry in [
            "command = 'x'\n[env]\n'BAD=KEY' = 'ordinary'",
            "url = 'http://localhost/mcp'\n[headers]\n'bad header' = 'ordinary'",
            "url = 'http://localhost/mcp'\n[headers]\nX = 'one'\nx = 'two'",
            "command = 'x'\n[env]\nKEY = 'ordinary'\n[sensitive_env]\nKEY = '$KEY'",
            "command = 'x'\n[sensitive_headers]\nAuthorization = '$KEY'",
        ] {
            let prefix = "[mcp_servers.invalid]\n";
            let entry = entry
                .replace("[env]", "[mcp_servers.invalid.env]")
                .replace("[headers]", "[mcp_servers.invalid.headers]")
                .replace("[sensitive_env]", "[mcp_servers.invalid.sensitive_env]")
                .replace(
                    "[sensitive_headers]",
                    "[mcp_servers.invalid.sensitive_headers]",
                );
            let bytes = format!("{prefix}{entry}");
            assert!(
                owner
                    .resolve_mcp_candidate(
                        &input,
                        Some((&owner.sources.settings, bytes.as_bytes())),
                        false
                    )
                    .is_err()
            );
        }
    }
    #[test]
    fn web09_resource_definition_and_selection_owners_remain_distinct() {
        use crate::runtime::subagent::SubagentName;
        let (_root, owner, mut input) = fixture();
        trust(&owner, &input);
        let user_agents = owner.sources.config_directory.join("agents");
        let project_agents = input.cwd.join(".agents/agents");
        for root in [&user_agents, &project_agents] {
            std::fs::create_dir_all(root).unwrap();
        }
        std::fs::write(
            user_agents.join("worker.toml"),
            "description = 'user'\ninstructions = 'user'\nskills = ['user-only']\n",
        )
        .unwrap();
        std::fs::write(
            project_agents.join("worker.toml"),
            "description = 'project'\ninstructions = 'project'\n",
        )
        .unwrap();
        let global_skill = owner.sources.home_directory.join(".agents/skills/shared");
        let project_skill = input.cwd.join(".agents/skills/shared");
        for root in [&global_skill, &project_skill] {
            std::fs::create_dir_all(root).unwrap();
            std::fs::write(
                root.join("SKILL.md"),
                "---\nname: shared\ndescription: A local skill\n---\nLocal instructions.\n",
            )
            .unwrap();
        }
        let workflows = input.cwd.join(".agents/workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        let program = "description: Return a literal\nblock:\n  input: {type: object, properties: {}, additionalProperties: false}\n  output: {type: object, properties: {}, additionalProperties: false}\n  entry: done\n  nodes:\n    done:\n      type: return\n      output: {type: literal, value: {}}\n";
        std::fs::write(workflows.join("check.yaml"), program).unwrap();
        let before = owner.read_source_settings(&input).unwrap();
        let facts = &before.integrations;
        assert_eq!(facts.agents[0].selected, project_agents.join("worker.toml"));
        assert_eq!(
            facts.agents[0].shadowed,
            Some(user_agents.join("worker.toml"))
        );
        assert!(facts.user.agents.is_none());
        let inventory = facts.inventory.as_ref().unwrap();
        let name = SubagentName::parse("worker").unwrap();
        assert!(inventory.agents[&name].skills.is_empty());
        assert_eq!(
            inventory.skills[0].source,
            crate::skills::SkillSource::Workspace
        );
        assert_eq!(
            inventory.skills[0].shadowed[0].source,
            crate::skills::SkillSource::Global
        );
        assert!(
            inventory
                .workflows
                .contains_key(&crate::runtime::workflow::WorkflowId::parse("check").unwrap())
        );
        let selected = owner
            .write_source_settings(
                &input,
                &before.user.revision,
                SourceMutation::Integration {
                    scope: IntegrationScope::User,
                    control: IntegrationControl::Agents {
                        selected: Some(vec![name]),
                    },
                },
            )
            .unwrap();
        assert!(matches!(
            selected.integrations.provenance["agent.agents"],
            Origin::User { .. }
        ));
        assert_eq!(selected.integrations.agents, facts.agents);
        assert_eq!(
            std::fs::read_to_string(workflows.join("check.yaml")).unwrap(),
            program
        );
        input.no_automatic_skills = true;
        let explicit = owner.read_source_settings(&input).unwrap();
        assert!(explicit.integrations.inventory.unwrap().skills.is_empty());
        assert!(explicit.integrations.user.skill_sources.is_none());
    }
}

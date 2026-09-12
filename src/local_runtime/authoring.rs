//! Strict TOML layers and explicit native composition. No dynamic value merge.
use super::{
    config::{
        ContextPolicyDocument, CurrentRuntimeConfig, InvocationPolicyDocument, McpServerDocument,
        McpTransportType, NativePolicyOverrideDocument,
    },
    launch::Origin,
};
use crate::model::catalog::{ModelRef, ReasoningProfileId};
use crate::model::session::{SessionModelConfig, SummaryModelPolicy};
use crate::toml_authoring::RequestParamsJson;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

type Origins = BTreeMap<String, Origin>;
fn replace_origin(origins: &mut Origins, path: &str, origin: &Origin) {
    origins.retain(|old, _| old != path && !old.starts_with(&format!("{path}.")));
    origins.insert(path.into(), origin.clone());
}
fn replace<T>(
    target: &mut Option<T>,
    layer: Option<T>,
    path: &str,
    origin: &Origin,
    origins: &mut Origins,
) {
    if let Some(value) = layer {
        *target = Some(value);
        replace_origin(origins, path, origin);
    }
}
fn named<K: Ord + std::fmt::Display, V>(
    target: &mut Option<BTreeMap<K, V>>,
    layer: Option<BTreeMap<K, V>>,
    path: &str,
    origin: &Origin,
    origins: &mut Origins,
) {
    if let Some(layer) = layer {
        let target = target.get_or_insert_default();
        if layer.is_empty() {
            target.clear();
            replace_origin(origins, path, origin);
        } else {
            for (name, entry) in layer {
                replace_origin(origins, &format!("{path}.{name}"), origin);
                target.insert(name, entry);
            }
        }
    }
}
macro_rules! partial {
    ($name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub(super) struct $name { $(#[serde(default, skip_serializing_if = "Option::is_none")]
            pub $field: Option<$ty>),* }
    };
}
// Schemars derives concrete field schemas below; optional fields mean omission,
// never a TOML null value.
partial!(RuntimeLayer {
    models: PathBuf, runtime_root: PathBuf, schema_version: u32,
    agent_id: crate::runtime::identity::AgentId,
    approval_mode: crate::runtime::ApprovalMode, agent: AgentProfileLayer,
    context: ContextLayer, model_timeout_policy: TimeoutLayer, tool_deadline_policy: ToolDeadlineLayer,
    mcp_servers: BTreeMap<crate::runtime::identity::McpServerId, McpAuthoring>,
    mcp_tool_policies: BTreeMap<crate::runtime::identity::McpServerId, InvocationPolicyDocument>,
    native_tools: NativeToolsLayer, environment: BTreeMap<String,String>,
    subagents: SubagentsLayer
});
partial!(AgentProfileLayer {
    description: String, instructions: String, model: ModelLayer, timeout_ms: u64,
    tools: crate::capabilities::selection::ToolSelectionDocument,
    skills: Vec<String>, extensions: crate::extensions::NativeAgentExtensionsDocument,
    agents: Vec<crate::runtime::subagent::SubagentName>,
    workflows: Vec<crate::runtime::workflow::WorkflowId>,
    agents_md: super::config::SubagentAgentsMdDocument,
    worktree: super::config::SubagentWorktreeDocument
});
partial!(ModelLayer {
    model: ModelRef,
    reasoning_profile: ReasoningSelection,
    request_params_json: RequestParamsJson,
    max_output_tokens: ModelOutput,
    summary_model: SummaryAuthoring
});
partial!(ContextLayer {
    reserve_tokens: u64,
    keep_recent_tokens: u64,
    summary_output_cap: SummaryOutput
});
partial!(TimeoutLayer {
    response_start_timeout_ms: u64,
    stream_idle_timeout_ms: u64
});
partial!(ToolDeadlineLayer {
    hard_deadline_ms: u64,
    idle_liveness_ms: IdleLiveness
});
partial!(SubagentsLayer { max_concurrent: usize, workflow: Vec<crate::runtime::subagent::SubagentName> });

partial!(NativeToolsLayer {
    read: NativePolicyOverrideDocument,
    write: NativePolicyOverrideDocument,
    edit: NativePolicyOverrideDocument,
    glob: NativePolicyOverrideDocument,
    grep: NativePolicyOverrideDocument,
    bash: NativePolicyOverrideDocument
});
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ReasoningSelection {
    CatalogDefault {},
    Profile { name: ReasoningProfileId },
}
impl ReasoningSelection {
    pub fn resolve(self) -> Option<ReasoningProfileId> {
        match self {
            Self::CatalogDefault {} => None,
            Self::Profile { name } => Some(name),
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ModelOutput {
    CatalogDefault {},
    Limit { tokens: u32 },
}
impl ModelOutput {
    fn resolve(self) -> Option<u32> {
        match self {
            Self::CatalogDefault {} => None,
            Self::Limit { tokens } => Some(tokens),
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum SummaryOutput {
    ModelLimit {},
    Limit { tokens: u32 },
}
impl SummaryOutput {
    fn resolve(self) -> Option<u32> {
        match self {
            Self::ModelLimit {} => None,
            Self::Limit { tokens } => Some(tokens),
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum IdleLiveness {
    Disabled {},
    Window { milliseconds: u64 },
}
impl IdleLiveness {
    fn resolve(self) -> Option<u64> {
        match self {
            Self::Disabled {} => None,
            Self::Window { milliseconds } => Some(milliseconds),
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum SummaryAuthoring {
    Session {},
    Explicit {
        model: ModelRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_profile: Option<ReasoningSelection>,
        #[serde(default)]
        request_params_json: RequestParamsJson,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_output_tokens: Option<ModelOutput>,
    },
}
impl SummaryAuthoring {
    fn resolve(self) -> SummaryModelPolicy {
        match self {
            Self::Session {} => SummaryModelPolicy::Session,
            Self::Explicit {
                model,
                reasoning_profile,
                request_params_json,
                max_output_tokens,
            } => SummaryModelPolicy::Explicit {
                model,
                reasoning_profile: reasoning_profile.and_then(ReasoningSelection::resolve),
                request_params: request_params_json.0,
                max_output_tokens: max_output_tokens.and_then(ModelOutput::resolve),
            },
        }
    }
}

/// Secret-field presence is retained even for empty tables, so project authority
/// cannot be widened by replacing an authored empty table with a default map.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct McpAuthoring {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitive_env: Option<BTreeMap<String, crate::credentials::EnvironmentReference>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitive_headers: Option<BTreeMap<String, crate::credentials::EnvironmentReference>>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(rename = "type", default)]
    pub transport_type: Option<McpTransportType>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}
impl McpAuthoring {
    fn resolve(self) -> McpServerDocument {
        McpServerDocument {
            sensitive_env: self.sensitive_env.unwrap_or_default(),
            sensitive_headers: self.sensitive_headers.unwrap_or_default(),
            enabled: self.enabled,
            transport_type: self.transport_type,
            url: self.url,
            headers: self.headers,
            command: self.command,
            args: self.args,
            env: self.env,
            cwd: self.cwd,
        }
    }
}

macro_rules! merge_record {
    ($name:ident, [$($replace:ident),*], [$($record:ident),*], [$($map:ident),*]) => {
        impl $name {
            fn merge(&mut self, layer: Self, prefix: &str, origin: &Origin, origins: &mut Origins) {
                $(replace(&mut self.$replace, layer.$replace, &format!("{prefix}{}", stringify!($replace)), origin, origins);)*
                $(if let Some(child) = layer.$record { self.$record.get_or_insert_default().merge(child, &format!("{prefix}{}.", stringify!($record)), origin, origins); })*
                $(named(&mut self.$map, layer.$map, &format!("{prefix}{}", stringify!($map)), origin, origins);)*
            }
        }
    }
}
merge_record!(
    RuntimeLayer,
    [
        models,
        runtime_root,
        schema_version,
        agent_id,
        approval_mode
    ],
    [
        agent,
        context,
        model_timeout_policy,
        tool_deadline_policy,
        subagents,
        native_tools
    ],
    [mcp_servers, mcp_tool_policies, environment]
);
merge_record!(
    ModelLayer,
    [
        model,
        reasoning_profile,
        request_params_json,
        max_output_tokens,
        summary_model
    ],
    [],
    []
);
merge_record!(
    ContextLayer,
    [reserve_tokens, keep_recent_tokens, summary_output_cap],
    [],
    []
);
merge_record!(
    TimeoutLayer,
    [response_start_timeout_ms, stream_idle_timeout_ms],
    [],
    []
);
merge_record!(
    ToolDeadlineLayer,
    [hard_deadline_ms, idle_liveness_ms],
    [],
    []
);
merge_record!(SubagentsLayer, [max_concurrent, workflow], [], []);
merge_record!(
    AgentProfileLayer,
    [
        description,
        instructions,
        timeout_ms,
        tools,
        skills,
        extensions,
        agents,
        workflows,
        agents_md,
        worktree
    ],
    [model],
    []
);
impl Copy for NativeToolsLayer {}

impl NativeToolsLayer {
    fn merge(&mut self, layer: Self, prefix: &str, origin: &Origin, origins: &mut Origins) {
        if layer.read.is_none()
            && layer.write.is_none()
            && layer.edit.is_none()
            && layer.glob.is_none()
            && layer.grep.is_none()
            && layer.bash.is_none()
        {
            *self = Self::default();
            replace_origin(origins, prefix.trim_end_matches('.'), origin);
            return;
        }
        macro_rules! entry { ($($field:ident),*) => { $(replace(&mut self.$field, layer.$field, &format!("{prefix}{}",stringify!($field)),origin,origins);)* } }
        entry!(read, write, edit, glob, grep, bash);
    }
}

macro_rules! apply {
    ($layer:ident, $target:ident, $($field:ident),* $(,)?) => { $(if let Some(value) = $layer.$field { $target.$field = value; })* }
}
pub(super) fn serialize_profile_model<S: serde::Serializer>(
    model: &Option<SessionModelConfig>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    model
        .as_ref()
        .map(|model| ModelLayer {
            model: Some(model.model.clone()),
            reasoning_profile: model
                .reasoning_profile
                .clone()
                .map(|name| ReasoningSelection::Profile { name }),
            request_params_json: Some(RequestParamsJson(model.request_params.clone())),
            max_output_tokens: model
                .max_output_tokens
                .map(|tokens| ModelOutput::Limit { tokens }),
            summary_model: Some(match &model.summary_model {
                SummaryModelPolicy::Session => SummaryAuthoring::Session {},
                SummaryModelPolicy::Explicit {
                    model,
                    reasoning_profile,
                    request_params,
                    max_output_tokens,
                } => SummaryAuthoring::Explicit {
                    model: model.clone(),
                    reasoning_profile: reasoning_profile
                        .clone()
                        .map(|name| ReasoningSelection::Profile { name }),
                    request_params_json: RequestParamsJson(request_params.clone()),
                    max_output_tokens: max_output_tokens
                        .map(|tokens| ModelOutput::Limit { tokens }),
                },
            }),
        })
        .serialize(serializer)
}

pub(super) fn deserialize_profile_model<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SessionModelConfig>, D::Error> {
    ModelLayer::deserialize(deserializer)?
        .resolve()
        .map(Some)
        .map_err(serde::de::Error::custom)
}
impl ModelLayer {
    pub fn resolve(self) -> Result<SessionModelConfig, String> {
        let mut model = SessionModelConfig::of(self.model.ok_or("missing model.model")?);
        model.reasoning_profile = self.reasoning_profile.and_then(ReasoningSelection::resolve);
        model.max_output_tokens = self.max_output_tokens.and_then(ModelOutput::resolve);
        model.request_params = self.request_params_json.unwrap_or_default().0;
        if let Some(summary) = self.summary_model {
            model.summary_model = summary.resolve();
        }
        Ok(model)
    }
}
impl ContextLayer {
    fn resolve(self) -> ContextPolicyDocument {
        let mut context = ContextPolicyDocument::default();
        apply!(self, context, reserve_tokens, keep_recent_tokens);
        if let Some(cap) = self.summary_output_cap {
            context.summary_output_cap = cap.resolve();
        }
        context
    }
}
impl RuntimeLayer {
    /// Default provenance is native metadata, not a serialization/merge pass.
    pub fn record_default_origins(config: &CurrentRuntimeConfig, origins: &mut Origins) {
        fn map(
            origins: &mut Origins,
            prefix: &str,
            names: impl Iterator<Item = impl std::fmt::Display>,
        ) {
            let mut empty = true;
            for name in names {
                empty = false;
                origins
                    .entry(format!("{prefix}.{name}"))
                    .or_insert(Origin::Builtin);
            }
            if empty {
                origins.entry(prefix.into()).or_insert(Origin::Builtin);
            }
        }
        for path in [
            "schema_version",
            "agent_id",
            "approval_mode",
            "agent.tools",
            "agent.skills",
            "agent.model.model",
            "agent.model.reasoning_profile",
            "agent.model.request_params_json",
            "agent.model.max_output_tokens",
            "agent.model.summary_model",
            "context.reserve_tokens",
            "context.keep_recent_tokens",
            "context.summary_output_cap",
            "model_timeout_policy.response_start_timeout_ms",
            "model_timeout_policy.stream_idle_timeout_ms",
            "tool_deadline_policy.hard_deadline_ms",
            "tool_deadline_policy.idle_liveness_ms",
            "agent.extensions.agent_status.enabled",
            "agent.extensions.agent_status.time.enabled",
            "agent.extensions.agent_status.time.timezone",
            "agent.extensions.agent_status.background.enabled",
            "agent.extensions.todo",
            "agent.extensions.goal",
            "subagents.max_concurrent",
            "agent.agents",
            "subagents.workflow",
            "agent.workflows",
            "native_tools.read",
            "native_tools.write",
            "native_tools.edit",
            "native_tools.glob",
            "native_tools.grep",
            "native_tools.bash",
        ] {
            origins.entry(path.into()).or_insert(Origin::Builtin);
        }
        map(origins, "environment", config.environment.keys());
        map(origins, "mcp_servers", config.mcp_servers.keys());
        map(
            origins,
            "mcp_tool_policies",
            config.mcp_tool_policies.keys(),
        );
    }

    pub fn overlay(&mut self, layer: Self, origin: &Origin, origins: &mut Origins) {
        self.merge(layer, "", origin, origins);
    }
    pub fn model_sections(self) -> Result<(SessionModelConfig, ContextPolicyDocument), String> {
        Ok((
            self.agent
                .and_then(|agent| agent.model)
                .ok_or("missing agent.model")?
                .resolve()?,
            self.context.unwrap_or_default().resolve(),
        ))
    }
    pub fn resolve(self) -> Result<CurrentRuntimeConfig, String> {
        let mut config = CurrentRuntimeConfig::defaults(
            self.agent
                .as_ref()
                .and_then(|agent| agent.model.clone())
                .ok_or("missing agent.model.model")?
                .resolve()?,
        );
        apply!(
            self,
            config,
            schema_version,
            agent_id,
            approval_mode,
            mcp_tool_policies,
            environment
        );
        if let Some(layer) = self.agent {
            let mut profile = config.agent;
            apply!(
                layer,
                profile,
                description,
                instructions,
                tools,
                skills,
                extensions,
                agents,
                workflows,
                agents_md,
                worktree
            );
            if let Some(model) = layer.model {
                profile.model = Some(model.resolve()?);
            }
            if let Some(timeout) = layer.timeout_ms {
                profile.timeout_ms = Some(timeout);
            }
            config.agent = profile;
        }
        config.context = self.context.unwrap_or_default().resolve();
        if let Some(layer) = self.model_timeout_policy {
            let mut policy = config.model_timeout_policy;
            apply!(
                layer,
                policy,
                response_start_timeout_ms,
                stream_idle_timeout_ms
            );
            config.model_timeout_policy = policy;
        }
        if let Some(layer) = self.tool_deadline_policy {
            let mut policy = config.tool_deadline_policy;
            apply!(layer, policy, hard_deadline_ms);
            if let Some(idle) = layer.idle_liveness_ms {
                policy.idle_liveness_ms = idle.resolve();
            }
            config.tool_deadline_policy = policy;
        }
        if let Some(layer) = self.subagents {
            let mut subagents = config.subagents;
            apply!(layer, subagents, max_concurrent, workflow);
            config.subagents = subagents;
        }
        if let Some(layer) = self.native_tools {
            let mut policies = config.native_tools;
            apply!(layer, policies, read, write, edit, glob, grep, bash);
            config.native_tools = policies;
        }
        config.mcp_servers = self
            .mcp_servers
            .unwrap_or_default()
            .into_iter()
            .map(|(id, entry)| (id, entry.resolve()))
            .collect();
        Ok(config)
    }
    pub fn resources_only(&mut self) {
        self.models = None;
        self.runtime_root = None;
        self.schema_version = None;
        self.agent_id = None;
        if let Some(agent) = &mut self.agent {
            agent.model = None;
            agent.extensions = None;
        }
        self.approval_mode = None;

        self.context = None;
        self.model_timeout_policy = None;
        self.tool_deadline_policy = None;
    }
    pub fn copy_resources(config: &mut CurrentRuntimeConfig, resources: CurrentRuntimeConfig) {
        config.mcp_servers = resources.mcp_servers;
        config.mcp_tool_policies = resources.mcp_tool_policies;
        config.native_tools = resources.native_tools;
        config.environment = resources.environment;
        let extensions = config.agent.extensions.clone();
        config.agent = resources.agent;
        config.agent.extensions = extensions;
        config.subagents = resources.subagents;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layer(text: &str) -> RuntimeLayer {
        crate::toml_authoring::parse(text.as_bytes()).unwrap()
    }
    fn resolve(lower: &str, upper: &str) -> (CurrentRuntimeConfig, Origins) {
        let mut merged = layer(lower);
        let mut origins = Origins::new();
        merged.overlay(
            layer(upper),
            &Origin::Project {
                document: "rustx.toml".into(),
                base: "/workspace".into(),
            },
            &mut origins,
        );
        (merged.resolve().unwrap(), origins)
    }
    const LOWER: &str = r#"
[model]
model = "p/m"
reasoning_profile = { mode = "profile", name = "custom" }
max_output_tokens = { mode = "limit", tokens = 512 }
[context]
summary_output_cap = { mode = "limit", tokens = 256 }
[tool_deadline_policy]
idle_liveness_ms = { mode = "window", milliseconds = 100 }
[extensions.agent_status.time]
timezone = "Asia/Shanghai"
"#;
    #[test]
    fn omission_inherits_and_each_domain_can_explicitly_reset() {
        let (inherited, _) = resolve(LOWER, "");
        assert_eq!(
            inherited
                .initial_model()
                .clone()
                .reasoning_profile
                .unwrap()
                .as_str(),
            "custom"
        );
        assert_eq!(
            inherited.initial_model().clone().max_output_tokens,
            Some(512)
        );
        assert_eq!(inherited.context.summary_output_cap, Some(256));
        assert_eq!(inherited.tool_deadline_policy.idle_liveness_ms, Some(100));
        let (reset, origins) = resolve(
            LOWER,
            r#"
[model]
reasoning_profile = { mode = "catalog_default" }
max_output_tokens = { mode = "catalog_default" }
[context]
summary_output_cap = { mode = "model_limit" }
[tool_deadline_policy]
idle_liveness_ms = { mode = "disabled" }
[extensions.agent_status.time]
timezone = "UTC"
"#,
        );
        assert_eq!(reset.initial_model().clone().reasoning_profile, None);
        assert_eq!(reset.initial_model().clone().max_output_tokens, None);
        assert_eq!(reset.context.summary_output_cap, None);
        assert_eq!(reset.tool_deadline_policy.idle_liveness_ms, None);
        assert_eq!(
            reset
                .agent
                .extensions
                .agent_status
                .time
                .effective_timezone(),
            chrono_tz::UTC
        );
        for field in [
            "agent.model.reasoning_profile",
            "agent.model.max_output_tokens",
            "context.summary_output_cap",
            "tool_deadline_policy.idle_liveness_ms",
            "agent.extensions.agent_status.time.timezone",
        ] {
            assert!(matches!(origins[field], Origin::Project { .. }));
        }
    }
    #[test]
    fn concrete_replacements_and_profile_names_do_not_collide_with_reset_modes() {
        let (config, _) = resolve(
            LOWER,
            r#"
[model]
reasoning_profile = { mode = "profile", name = "catalog_default" }
max_output_tokens = { mode = "limit", tokens = 1024 }
[context]
summary_output_cap = { mode = "limit", tokens = 512 }
[tool_deadline_policy]
idle_liveness_ms = { mode = "window", milliseconds = 200 }
"#,
        );
        assert_eq!(
            config
                .initial_model()
                .clone()
                .reasoning_profile
                .unwrap()
                .as_str(),
            "catalog_default"
        );
        assert_eq!(config.initial_model().clone().max_output_tokens, Some(1024));
        assert_eq!(config.context.summary_output_cap, Some(512));
        assert_eq!(config.tool_deadline_policy.idle_liveness_ms, Some(200));
    }
    #[test]
    fn strict_unknown_and_malformed_layers_fail_completely() {
        for text in [
            "typo = true",
            "approvalMode = 'policy'",
            "[model]\nmodel = 'p/m'\nrequest_params = {}",
            "[model]\nmodel = 'p/m'\nreasoning_profile = { mode = 'catalog_default', name = 'hidden' }",
            "[model]\nmodel = 'p/m'\n[unterminated",
            "[model]\nmodel = 'p/m'\nmodel = 'p/other'",
        ] {
            assert!(
                crate::toml_authoring::parse::<RuntimeLayer>(text.as_bytes()).is_err(),
                "{text}"
            );
        }
    }
    #[test]
    fn project_cannot_author_host_policy_or_even_empty_secret_tables() {
        for text in [
            "models = 'models.toml'",
            "runtime_root = '/tmp/state'",
            "approval_mode = 'policy'",
            "native_tools = {}",
            "mcp_tool_policies = {}",
            "[mcp_servers.source]\nsensitive_env = {}",
            "[mcp_servers.source]\nsensitive_headers = {}",
        ] {
            assert!(
                super::super::launch::parse_layer(
                    std::path::Path::new("rustx.toml"),
                    text.as_bytes(),
                    true
                )
                .is_err(),
                "{text}"
            );
            assert!(
                super::super::launch::parse_layer(
                    std::path::Path::new("settings.toml"),
                    text.as_bytes(),
                    false
                )
                .is_ok(),
                "{text}"
            );
        }
    }
    #[test]
    fn opaque_json_string_is_the_only_overlay_form_including_explicit_summary() {
        let config = layer(r#"
[model]
model = "p/m"
request_params_json = '''{"future":{"nested":[1,null,{"new":true}]},"text":"x","flag":false,"temperature":0.7}'''
[model.summary_model]
mode = "explicit"
model = "p/s"
request_params_json = '''{"vendor":[null,[1,2],{"arbitrary":"yes"}]}'''
"#).resolve().unwrap();
        assert_eq!(
            config.initial_model().clone().request_params["future"]["nested"][1],
            serde_json::Value::Null
        );
        assert_eq!(
            config.initial_model().clone().request_params["future"]["nested"][2]["new"],
            true
        );
        assert_eq!(
            config
                .initial_model()
                .clone()
                .summary_selection()
                .unwrap()
                .request_params["vendor"][0],
            serde_json::Value::Null
        );
        for json in ["null", "[]", "[1]", "42", "true", "\"string\"", "{broken"] {
            let text = format!("[model]\nmodel = 'p/m'\nrequest_params_json = '{json}'");
            let error = crate::toml_authoring::parse::<RuntimeLayer>(text.as_bytes()).unwrap_err();
            assert!(error.contains("request_params_json must contain a valid JSON object"));
        }
    }
}

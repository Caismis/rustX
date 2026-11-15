use super::{CurrentRuntimeConfig, CurrentRuntimeConfigError, ModelTimeoutPolicyDocument};
use crate::model::deadline::{DEFAULT_RESPONSE_START_TIMEOUT, DEFAULT_STREAM_IDLE_TIMEOUT};

pub(super) const MINIMAL: &str = r#"agent_id = "agent-a"

[context]
reserve_tokens = 1024
keep_recent_tokens = 4096


[agent]
[agent.model]
model = "p/m"
"#;

/// The minimal configuration parses and derives its policy pieces.
#[test]
fn minimal_configuration_parses() {
    let config = CurrentRuntimeConfig::from_toml_slice(MINIMAL.as_bytes()).expect("valid");
    assert_eq!(config.approval_mode, crate::runtime::ApprovalMode::Plan);
    assert_eq!(config.context_policy().reserve_tokens, 1024);
    assert!(config.agent.extensions.agent_status.time.enabled);
    assert!(config.agent.extensions.agent_status.background.enabled);
    assert_eq!(config.agent.extensions.agent_status.time.timezone, None);
    assert_eq!(
        config.model_timeout_policy,
        ModelTimeoutPolicyDocument::default()
    );
    let timeout_policy = config.timeout_policy().expect("finite timeout policy");
    assert_eq!(
        timeout_policy.response_start_timeout,
        DEFAULT_RESPONSE_START_TIMEOUT
    );
    assert_eq!(
        timeout_policy.stream_idle_timeout,
        DEFAULT_STREAM_IDLE_TIMEOUT
    );
    assert!(config.mcp_bindings().expect("bindings").is_empty());
    assert_eq!(
        config
            .tool_environment()
            .expect("environment")
            .authorized_entries(),
        []
    );
}

/// Issue #462 B1/D02: Agent Git Worktree authoring is retired. Strict
/// parsing refuses the old Profile table and its nested fields for Root
/// and named Agents; no ignored or default-disabled legacy mode remains.
#[test]
fn retired_agent_worktree_authoring_is_refused() {
    for worktree in [
        r#"{"enabled": true}"#,
        r#"{"enabled": false}"#,
        r#"{"require_clean_parent": true}"#,
        "{}",
    ] {
        let mut document =
            serde_json::json!({"description":"worker", "instructions":"Worker instructions"});
        document["worktree"] = serde_json::from_str(worktree).unwrap();
        let error =
            crate::local_runtime::agent_resources::parse(&toml::to_string(&document).unwrap())
                .expect_err("a named Agent cannot author worktree");
        assert!(error.contains("worktree"), "{error}");
        let root = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            &format!(
                "agent_id = \"agent-a\"\n[agent.worktree]\n{}",
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(worktree)
                    .unwrap()
                    .iter()
                    .map(|(key, value)| format!("{key} = {value}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        );
        assert!(
            CurrentRuntimeConfig::from_toml_slice(root.as_bytes()).is_err(),
            "{root}"
        );
    }
}

/// The one shared timeout policy is current runtime state and accepts
/// finite millisecond values without entering Session model state.
#[test]
fn model_timeout_policy_is_configurable() {
    let json = MINIMAL.replace(
        r#"agent_id = "agent-a""#,
        r#"agent_id = "agent-a"
model_timeout_policy = { "response_start_timeout_ms" = 7, "stream_idle_timeout_ms" = 11 }"#,
    );
    let config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
    let policy = config.timeout_policy().expect("finite timeout policy");
    assert_eq!(
        policy.response_start_timeout,
        std::time::Duration::from_millis(7)
    );
    assert_eq!(
        policy.stream_idle_timeout,
        std::time::Duration::from_millis(11)
    );
}

fn worker_config(timeout: &str) -> Result<super::NamedAgentProfileDocument, String> {
    crate::local_runtime::agent_resources::parse(&format!(
        "description = \"worker\"\ninstructions = \"Worker\"\ntimeout_ms = {timeout}\n"
    ))
}

#[test]
fn named_subagent_execution_deadline_is_optional_and_typed_at_admission() {
    let absent = crate::local_runtime::agent_resources::parse(
        r#"description = "worker"
instructions = "Worker""#,
    )
    .unwrap();
    assert_eq!(absent.execution_deadline().unwrap(), None);
    assert_eq!(
        worker_config("30000")
            .unwrap()
            .execution_deadline()
            .unwrap()
            .unwrap()
            .as_millis(),
        30_000
    );
}

#[test]
fn named_subagent_execution_deadline_rejects_zero_and_values_above_maximum() {
    for (raw, expected) in [
        ("0", "must be positive"),
        ("86400001", "must not exceed 86400000 milliseconds"),
    ] {
        let error = worker_config(raw).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    assert!(worker_config("\"30s\"").is_err());
}

/// A zero deadline is rejected at the current-runtime composition
/// boundary rather than creating an ambiguous request.
#[test]
fn zero_model_timeout_is_rejected() {
    let json = MINIMAL.replace(
        r#"agent_id = "agent-a""#,
        r#"agent_id = "agent-a"
model_timeout_policy = { "response_start_timeout_ms" = 0, "stream_idle_timeout_ms" = 11 }"#,
    );
    let error = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail");
    assert!(matches!(error, CurrentRuntimeConfigError::Invalid { .. }));
    assert!(
        error
            .to_string()
            .contains("model_timeout_policy.response_start_timeout_ms")
    );
}

/// A policy read for one admission remains unchanged when current
/// configuration is edited; a later admission reads the new values.
#[test]
fn timeout_policy_changes_apply_only_to_later_admissions() {
    let json = MINIMAL.replace(
        r#"agent_id = "agent-a""#,
        r#"agent_id = "agent-a"
model_timeout_policy = { "response_start_timeout_ms" = 7, "stream_idle_timeout_ms" = 11 }"#,
    );
    let mut config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
    let admitted = config.timeout_policy().expect("initial policy");
    config.model_timeout_policy.response_start_timeout_ms = 13;
    config.model_timeout_policy.stream_idle_timeout_ms = 17;
    let later = config.timeout_policy().expect("later policy");

    assert_eq!(
        admitted.response_start_timeout,
        std::time::Duration::from_millis(7)
    );
    assert_eq!(
        admitted.stream_idle_timeout,
        std::time::Duration::from_millis(11)
    );
    assert_eq!(
        later.response_start_timeout,
        std::time::Duration::from_millis(13)
    );
    assert_eq!(
        later.stream_idle_timeout,
        std::time::Duration::from_millis(17)
    );
}

/// The tool execution-liveness deadline policy (Issue #204) is current
/// runtime state and accepts finite millisecond values without entering
/// executor-visible or durable state.
#[test]
fn tool_deadline_policy_is_configurable() {
    let json = MINIMAL.replace(
        r#"agent_id = "agent-a""#,
        r#"agent_id = "agent-a"
tool_deadline_policy = { "hard_deadline_ms" = 7000, "idle_liveness_ms" = { "mode" = "window", "milliseconds" = 1500 } }"#,
    );
    let config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
    let policy = config
        .tool_deadline_policy()
        .expect("finite deadline policy");
    assert_eq!(policy.hard_deadline, std::time::Duration::from_secs(7));
    assert_eq!(
        policy.idle_liveness,
        Some(std::time::Duration::from_millis(1500))
    );
}

/// An omitted tool deadline policy (Issue #204) resolves to the default
/// finite hard deadline without an idle watchdog.
#[test]
fn tool_deadline_policy_defaults_when_omitted() {
    let config = CurrentRuntimeConfig::from_toml_slice(MINIMAL.as_bytes()).expect("valid");
    let policy = config
        .tool_deadline_policy()
        .expect("default deadline policy");
    assert_eq!(
        policy,
        crate::tools::deadline::ToolExecutionDeadlinePolicy::default()
    );
    assert_eq!(
        policy.hard_deadline,
        crate::tools::deadline::DEFAULT_TOOL_HARD_DEADLINE
    );
    assert_eq!(policy.idle_liveness, None);
}

/// A zero hard deadline is rejected at the policy boundary (Issue #204)
/// rather than admitting an execution that can never make progress.
#[test]
fn zero_tool_hard_deadline_is_rejected() {
    let json = MINIMAL.replace(
        r#"agent_id = "agent-a""#,
        r#"agent_id = "agent-a"
tool_deadline_policy = { "hard_deadline_ms" = 0, "idle_liveness_ms" = { "mode" = "window", "milliseconds" = 1500 } }"#,
    );
    let error = CurrentRuntimeConfig::from_toml_slice(json.as_bytes())
        .expect_err("zero hard deadline must fail");
    assert!(matches!(error, CurrentRuntimeConfigError::Invalid { .. }));
    assert!(
        error
            .to_string()
            .contains("tool_deadline_policy.hard_deadline_ms")
    );
}

/// A zero idle-liveness window is rejected at the policy boundary
/// (Issue #204): it would fire at every observation.
#[test]
fn zero_tool_idle_liveness_is_rejected() {
    let json = MINIMAL.replace(
        r#"agent_id = "agent-a""#,
        r#"agent_id = "agent-a"
tool_deadline_policy = { "hard_deadline_ms" = 7000, "idle_liveness_ms" = { "mode" = "window", "milliseconds" = 0 } }"#,
    );
    let error = CurrentRuntimeConfig::from_toml_slice(json.as_bytes())
        .expect_err("zero idle liveness must fail");
    assert!(matches!(error, CurrentRuntimeConfigError::Invalid { .. }));
    assert!(
        error
            .to_string()
            .contains("tool_deadline_policy.idle_liveness_ms")
    );
}

/// A policy read for one admission stays frozen when current
/// configuration is edited (Issue #204); a later admission reads the new
/// values.
#[test]
fn tool_deadline_policy_changes_apply_only_to_later_admissions() {
    let json = MINIMAL.replace(
        r#"agent_id = "agent-a""#,
        r#"agent_id = "agent-a"
tool_deadline_policy = { "hard_deadline_ms" = 7000, "idle_liveness_ms" = { "mode" = "window", "milliseconds" = 1500 } }"#,
    );
    let mut config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
    let admitted = config.tool_deadline_policy().expect("initial policy");
    config.tool_deadline_policy.hard_deadline_ms = 13000;
    config.tool_deadline_policy.idle_liveness_ms = Some(2500);
    let later = config.tool_deadline_policy().expect("later policy");

    assert_eq!(admitted.hard_deadline, std::time::Duration::from_secs(7));
    assert_eq!(
        admitted.idle_liveness,
        Some(std::time::Duration::from_millis(1500))
    );
    assert_eq!(later.hard_deadline, std::time::Duration::from_secs(13));
    assert_eq!(
        later.idle_liveness,
        Some(std::time::Duration::from_millis(2500))
    );
}

/// `ApprovalMode` is current runtime configuration and accepts the
/// explicit `FullAccess` spelling without becoming Session state.
#[test]
fn approval_mode_is_current_configuration_with_policy_default() {
    let json = MINIMAL.replace(
        r#"agent_id = "agent-a""#,
        r#"agent_id = "agent-a"
approval_mode = "full_access""#,
    );
    let config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
    assert_eq!(
        config.approval_mode,
        crate::runtime::ApprovalMode::FullAccess
    );
}

/// Unknown fields fail rather than silently changing semantics.
#[test]
fn unknown_fields_are_rejected() {
    let json = r#"agent_id = "a"
future_knob = true

[context]
reserve_tokens = 0
keep_recent_tokens = 0


[agent]
[agent.model]
model = "p/m"
"#;
    json.parse::<toml_edit::DocumentMut>().expect("valid TOML");
    let error = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail");
    assert!(
        error.to_string().contains("unknown field `future_knob`"),
        "{error}"
    );
}

/// Schema v3 owns timezone under the Time status module; the obsolete
/// top-level field is rejected rather than silently ignored.
#[test]
fn top_level_timezone_is_rejected() {
    let json = MINIMAL.replace(
        r#"agent_id = "agent-a""#,
        r#"agent_id = "agent-a"
timezone = "UTC""#,
    );
    assert!(matches!(
        CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail"),
        CurrentRuntimeConfigError::Syntax { .. }
    ));
}

/// Issue #256 regression 3: the obsolete top-level `agentStatus`
/// contract is *rejected*, not silently accepted, quietly relocated, or
/// warned about. There is no alias, no fallback parse, and no
/// compatibility mode: the strict-field boundary is the whole mechanism.
#[test]
fn ext256_the_obsolete_top_level_agent_status_contract_is_rejected() {
    for obsolete in [
        "agent_status = {}",
        "agent_status = { time = { enabled = false } }",
        "agent_status = { time = { timezone = 'Asia/Shanghai' }, background = { enabled = true } }",
    ] {
        let text = format!("{obsolete}\n{MINIMAL}");
        let error = CurrentRuntimeConfig::from_toml_slice(text.as_bytes()).expect_err("must fail");
        assert!(
            matches!(error, CurrentRuntimeConfigError::Syntax { .. }),
            "the obsolete contract must fail loudly: {error}"
        );
        assert!(
            error.to_string().contains("agent_status"),
            "the failure names the offending obsolete field: {error}"
        );
    }
}

/// The closed extension surface keeps the global strict-field contract:
/// an unknown *extension name* and an unknown knob inside a known
/// extension both fail at launch rather than being ignored.
#[test]
fn ext256_unknown_extension_names_and_fields_are_rejected() {
    for (fragment, expected) in [
        ("future = true", "unknown field `future`"),
        (
            "[agent.plugins]\n[agent.plugins.future_goal]\nenabled = true\n",
            "unknown field `future_goal`",
        ),
        (
            "[agent.plugins]\n[agent.plugins.todo]\nenabled = true\nfuture = true\n",
            "unknown field `future`",
        ),
        (
            "[agent.plugins]\n[agent.plugins.todo]\nenabled = \"true\"\n",
            "expected a boolean",
        ),
        (
            "[agent.plugins]\n[agent.plugins.agent_status]\nfuture = true\n",
            "unknown field `future`",
        ),
        (
            "[agent.plugins]\n[agent.plugins.agent_status]\n[agent.plugins.agent_status.time]\nenabled = true\nfuture = true\n",
            "unknown field `future`",
        ),
        (
            "[agent.plugins]\n[agent.plugins.agent_status]\n[agent.plugins.agent_status.background]\nfuture = true\n",
            "unknown field `future`",
        ),
    ] {
        let text = if fragment.starts_with('[') {
            format!("{MINIMAL}\n{fragment}\n")
        } else {
            format!("{fragment}\n{MINIMAL}")
        };
        text.parse::<toml_edit::DocumentMut>()
            .expect("valid TOML must reach the typed schema");
        let error = CurrentRuntimeConfig::from_toml_slice(text.as_bytes())
            .expect_err("strict schema rejects the field or type");
        assert!(matches!(error, CurrentRuntimeConfigError::Syntax { .. }));
        assert!(error.to_string().contains(expected), "{fragment}: {error}");
    }
}

/// The closed extension surface composes exactly what it declares, and
/// `enabled: false` removes the extension from the frozen composition
/// rather than leaving a disabled one behind.
#[test]
fn ext256_the_extension_surface_freezes_the_declared_composition() {
    let enabled = MINIMAL.replace(
        r"[agent]",
        r#"[agent]
plugins = { "agent_status" = { "enabled" = true, "time" = { "timezone" = "Asia/Shanghai" }, "background" = { "enabled" = false } } }"#,
    );
    let config = CurrentRuntimeConfig::from_toml_slice(enabled.as_bytes()).expect("valid");
    let composition = config.extension_composition();
    let agent_status = composition
        .agent_status()
        .expect("the declared Agent Status extension is composed");
    assert_eq!(agent_status.time.timezone, Some(chrono_tz::Asia::Shanghai));
    assert!(!agent_status.background.enabled);

    let disabled = MINIMAL.replace(
        r"[agent]",
        r#"[agent]
plugins = { "agent_status" = { "enabled" = false } }"#,
    );
    let config = CurrentRuntimeConfig::from_toml_slice(disabled.as_bytes()).expect("valid");
    assert!(
        config.extension_composition().agent_status().is_none(),
        "a disabled extension leaves an empty composition, not a disabled one"
    );

    // Issue #259: the members are independent, and switching both off is
    // what empties the composition.
    let neither = MINIMAL.replace(
        r"[agent]",
        r#"[agent]
plugins = { "agent_status" = { "enabled" = false }, "todo" = { "enabled" = false } }"#,
    );
    assert!(
        CurrentRuntimeConfig::from_toml_slice(neither.as_bytes())
            .expect("valid")
            .extension_composition()
            .is_empty()
    );
}

/// An unsupported schema version fails.
#[test]
fn unsupported_schema_version_fails() {
    let json = r#"schema_version = 99
agent_id = "a"

[context]
reserve_tokens = 0
keep_recent_tokens = 0


[agent]
[agent.model]
model = "p/m"
"#;
    assert!(matches!(
        CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail"),
        CurrentRuntimeConfigError::UnsupportedSchemaVersion { .. }
    ));
}

/// The obsolete array-based `mcpServers` schema is not a valid document.
#[test]
fn array_based_mcp_servers_are_rejected() {
    let json = r#"agent_id = "a"
mcp_servers = [{ server_id = "s", transport = { type = "streamable_http", endpoint = "https://x" } }]

[context]
reserve_tokens = 0
keep_recent_tokens = 0


[agent]
[agent.model]
model = "p/m"
"#;
    assert!(matches!(
        CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail"),
        CurrentRuntimeConfigError::Syntax { .. }
    ));
}

/// A present zero summary cap is a configuration error, even when the
/// context would otherwise never need compaction.
#[test]
fn zero_summary_output_cap_is_rejected() {
    let json = MINIMAL.replace(
        "[context]",
        "[context]\nsummary_output_cap = { mode = 'limit', tokens = 0 }",
    );
    let error = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail");
    assert!(matches!(error, CurrentRuntimeConfigError::Invalid { .. }));
    assert!(
        error
            .to_string()
            .contains("summary_output_cap must be positive")
    );
}

#[test]
fn subagent_definition_and_admission_domains_are_independent() {
    let json = MINIMAL.replace(
        r#"agent_id = "agent-a""#,
        r#"agent_id = "agent-a"
subagents = {  "max_concurrent" = 4 }"#,
    );
    let config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
    assert_eq!(
        config.agent.agents,
        [] as [crate::runtime::subagent::catalog::SubagentName; 0]
    );
    assert_eq!(
        config.agent.agents,
        [] as [crate::runtime::subagent::catalog::SubagentName; 0]
    );

    let defined_but_unadmitted = MINIMAL.replace(
        r#"agent_id = "agent-a""#,
        r#"agent_id = "agent-a"
subagents = {  "max_concurrent" = 4 }"#,
    );
    assert!(CurrentRuntimeConfig::from_toml_slice(defined_but_unadmitted.as_bytes()).is_ok());
}

#[test]
fn selection_identity_resolution_is_deferred_but_duplicates_are_rejected() {
    let unknown = MINIMAL.replace(
        r"[agent]",
        r#"[agent]
agents = ["missing"]"#,
    );
    assert!(CurrentRuntimeConfig::from_toml_slice(unknown.as_bytes()).is_ok());

    let duplicate = MINIMAL.replace(
        r"[agent]",
        r#"[agent]
agents = ["worker", "worker"]"#,
    );
    let error = CurrentRuntimeConfig::from_toml_slice(duplicate.as_bytes()).expect_err("duplicate");
    assert!(error.to_string().contains("duplicate profile"));
}

#[test]
fn workflow_selection_resolves_after_discovery() {
    let valid = MINIMAL.replace(
        r"[agent]",
        r"[agent]
workflows = []",
    );
    let config = CurrentRuntimeConfig::from_toml_slice(valid.as_bytes()).expect("valid");
    assert_eq!(
        config.agent.workflows,
        [] as [crate::runtime::workflow::WorkflowId; 0]
    );
    assert_eq!(
        config.agent.workflows,
        [] as [crate::runtime::workflow::WorkflowId; 0]
    );

    let unknown = MINIMAL.replace(
        r"[agent]",
        r#"[agent]
workflows = ["investigate"]"#,
    );
    assert!(CurrentRuntimeConfig::from_toml_slice(unknown.as_bytes()).is_ok());
}

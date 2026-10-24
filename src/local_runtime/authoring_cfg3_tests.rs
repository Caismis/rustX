//! Contract tests for atomic semantic overlay, independent of source I/O.
use super::*;
use crate::local_runtime::config::{
    ConcurrencyPolicyDocument, McpStartupPolicyDocument, ModelTimeoutPolicyDocument,
    SubagentsDocument, ToolDeadlinePolicyDocument,
};

fn parse(text: &str) -> RuntimeLayer {
    crate::toml_authoring::parse(text.as_bytes()).unwrap()
}

fn workspace() -> Origin {
    Origin::Workspace {
        document: "/workspace/rustx.toml".into(),
        base: "/workspace".into(),
    }
}

fn overlay(lower: &str, upper: &str) -> (RuntimeLayer, Origins) {
    let mut target = RuntimeLayer::default();
    let mut origins = Origins::new();
    target.overlay(
        parse(lower),
        &Origin::User {
            document: "/home/user/rustx/rustx.toml".into(),
            base: "/home/user/rustx".into(),
        },
        &mut origins,
    );
    target.overlay(parse(upper), &workspace(), &mut origins);
    (target, origins)
}

fn effective(lower: &str, upper: &str) -> (CurrentRuntimeConfig, Origins) {
    let (layer, mut origins) = overlay(lower, upper);
    let config = layer.resolve().unwrap();
    RuntimeLayer::record_default_origins(&config, &mut origins);
    (config, origins)
}

const LOWER: &str = r#"
[agent.model]
model = "p/lower"
request_params = '{"vendor":{"flag":true}}'
max_output_tokens = { mode = "limit", tokens = 123 }
profile = "lower"
summary_model = { mode = "explicit", model = "p/summary" }
title_model = { mode = "explicit", model = "p/title" }
[context]
reserve_tokens = 321
keep_recent_tokens = 654
summary_output_cap = { mode = "limit", tokens = 111 }
[model_timeout_policy]
response_start_timeout_ms = 1234
stream_idle_timeout_ms = 5678
[tool_deadline_policy]
hard_deadline_ms = 4321
idle_liveness_ms = { mode = "window", milliseconds = 8765 }
[subagents]
max_concurrent = 17
[native_tools.bash]
execution = "foreground_only"
concurrency = "sequential"
approval = "always"
[native_tools.read]
approval = "always"
[environment]
A = "lower"
B = "retained"
"#;

#[test]
fn absent_atomic_dimensions_inherit() {
    let (config, _) = effective(LOWER, "");
    assert_eq!(config.initial_model().model.to_string(), "p/lower");
    assert_eq!(config.initial_model().max_output_tokens, Some(123));
    assert_eq!(
        config
            .initial_model()
            .title_selection()
            .unwrap()
            .model
            .as_str(),
        "p/title"
    );
    assert_eq!(config.context.keep_recent_tokens, 654);
    assert_eq!(config.model_timeout_policy.stream_idle_timeout_ms, 5678);
    assert_eq!(config.tool_deadline_policy.idle_liveness_ms, Some(8765));
    assert_eq!(config.subagents.max_concurrent, 17);
}

#[test]
fn model_selection_replacement_cannot_inherit_request_fields() {
    let (config, origins) = effective(LOWER, "[agent.model]\nmodel = 'p/upper'");
    assert_eq!(
        config.initial_model(),
        &SessionModelConfig::of(ModelRef::parse("p/upper").unwrap())
    );
    for field in [
        "model",
        "request_params",
        "profile",
        "max_output_tokens",
        "summary_model",
        "title_model",
    ] {
        assert_eq!(origins[&format!("agent.model.{field}")], workspace());
    }
}

#[test]
fn incomplete_higher_model_selection_does_not_borrow_lower_identity() {
    for text in ["[agent.model]", "[agent.model]\nrequest_params = '{}'"] {
        let (layer, _) = overlay(LOWER, text);
        // The replaced selection names no model; the composition is
        // incomplete, never completed with the lower model.
        let config = layer.resolve().unwrap();
        assert!(config.agent.model.is_none(), "{text}");
        assert!(config.validate().is_err(), "{text}");
    }
}

#[test]
fn context_replacement_uses_product_defaults_for_omitted_members() {
    let (config, origins) = effective(LOWER, "[context]\nreserve_tokens = 999");
    let defaults = ContextPolicyDocument::default();
    assert_eq!(config.context.reserve_tokens, 999);
    assert_eq!(
        config.context.keep_recent_tokens,
        defaults.keep_recent_tokens
    );
    assert_eq!(
        config.context.summary_output_cap,
        defaults.summary_output_cap
    );
    assert_eq!(origins["context.keep_recent_tokens"], workspace());
}

#[test]
fn timeout_and_deadline_objects_never_splice() {
    let (config, origins) = effective(
        LOWER,
        "[model_timeout_policy]\nresponse_start_timeout_ms = 888\n[tool_deadline_policy]\nhard_deadline_ms = 777",
    );
    assert_eq!(config.model_timeout_policy.response_start_timeout_ms, 888);
    assert_eq!(
        config.model_timeout_policy.stream_idle_timeout_ms,
        ModelTimeoutPolicyDocument::default().stream_idle_timeout_ms
    );
    assert_eq!(config.tool_deadline_policy.hard_deadline_ms, 777);
    assert_eq!(
        config.tool_deadline_policy.idle_liveness_ms,
        ToolDeadlinePolicyDocument::default().idle_liveness_ms
    );
    assert_eq!(
        origins["model_timeout_policy.stream_idle_timeout_ms"],
        workspace()
    );
    assert_eq!(
        origins["tool_deadline_policy.idle_liveness_ms"],
        workspace()
    );
}

#[test]
fn explicit_empty_policy_objects_reset_to_domain_defaults() {
    let (config, _) = effective(
        LOWER,
        "[context]\n[model_timeout_policy]\n[tool_deadline_policy]\n[subagents]",
    );
    assert_eq!(config.context, ContextPolicyDocument::default());
    assert_eq!(
        config.model_timeout_policy,
        ModelTimeoutPolicyDocument::default()
    );
    assert_eq!(
        config.tool_deadline_policy,
        ToolDeadlinePolicyDocument::default()
    );
    assert_eq!(config.subagents, SubagentsDocument::default());
}

#[test]
fn mcp_startup_policy_defaults_replaces_and_preserves_provenance() {
    let lower = "[agent.model]\nmodel = 'p/model'\n[mcp_startup_policy]\nwait_timeout_ms = 42000";
    let (defaults, origins) = effective("", "");
    assert_eq!(defaults.mcp_startup_policy.wait_timeout_ms, 15_000);
    assert_eq!(
        origins["mcp_startup_policy.wait_timeout_ms"],
        Origin::Builtin
    );
    let (inherited, origins) = effective(lower, "");
    assert_eq!(inherited.mcp_startup_policy.wait_timeout_ms, 42_000);
    assert!(matches!(
        origins["mcp_startup_policy.wait_timeout_ms"],
        Origin::User { .. }
    ));
    let (reset, origins) = effective(lower, "[mcp_startup_policy]");
    assert_eq!(
        reset.mcp_startup_policy,
        McpStartupPolicyDocument::default()
    );
    assert_eq!(origins["mcp_startup_policy.wait_timeout_ms"], workspace());
    let (overridden, origins) = effective(lower, "[mcp_startup_policy]\nwait_timeout_ms = 7000");
    assert_eq!(overridden.mcp_startup_policy.wait_timeout_ms, 7_000);
    assert_eq!(origins["mcp_startup_policy.wait_timeout_ms"], workspace());
}

#[test]
fn mcp_startup_policy_round_trips_through_authored_and_redacted_documents() {
    let authored = parse("[mcp_startup_policy]\nwait_timeout_ms = 23500");
    let encoded = toml::to_string(&authored).unwrap();
    assert_eq!(parse(&encoded), authored);
    let redacted = authored.clone().project(|_| (), |_| Vec::<String>::new());
    assert_eq!(redacted.mcp_startup_policy, authored.mcp_startup_policy);
    let wire = serde_json::to_value(redacted).unwrap();
    assert_eq!(wire["mcp_startup_policy"]["wait_timeout_ms"], 23_500);
}

#[test]
fn mcp_startup_policy_rejects_nonpositive_or_noninteger_values() {
    for value in ["-1", "1.5", "'15000'", "true"] {
        let text = format!("[mcp_startup_policy]\nwait_timeout_ms = {value}");
        assert!(
            crate::toml_authoring::parse::<RuntimeLayer>(text.as_bytes()).is_err(),
            "{value}"
        );
    }
    let (zero, _) = effective(
        "[agent.model]\nmodel = 'p/model'",
        "[mcp_startup_policy]\nwait_timeout_ms = 0",
    );
    let error = zero.validate_authored().unwrap_err();
    assert_eq!(error.field, "mcp_startup_policy");
    assert!(error.error.to_string().contains("must be positive"));
    let receipt = tokio::time::Instant::now();
    assert_eq!(
        McpStartupPolicyDocument::default()
            .deadline_from(receipt)
            .unwrap(),
        receipt + std::time::Duration::from_secs(15)
    );
}

#[test]
fn mcp_startup_policy_checks_the_absolute_clock_range() {
    let receipt = std::time::Instant::now();
    let (mut valid, mut invalid) = (0, u64::MAX);
    while invalid - valid > 1 {
        let seconds = valid + (invalid - valid) / 2;
        if receipt
            .checked_add(std::time::Duration::from_secs(seconds))
            .is_some()
        {
            valid = seconds;
        } else {
            invalid = seconds;
        }
    }
    let last = receipt
        .checked_add(std::time::Duration::from_secs(valid))
        .unwrap();
    assert!(
        McpStartupPolicyDocument::default()
            .deadline_from(tokio::time::Instant::from_std(last))
            .is_err()
    );
}

#[test]
fn plugin_omission_empty_and_explicit_enable_are_distinct_without_splicing() {
    let lower = "[agent.model]\nmodel = 'chosen'\n[agent.plugins.agent_status]\nenabled = true\n[agent.plugins.agent_status.background]\nenabled = false\n[agent.plugins.todo]\nenabled = true\n";
    let (inherited, _) = effective(lower, "");
    assert!(inherited.agent.extensions.resolve().todo().is_some());
    for higher in [
        "[agent.plugins.agent_status]\n[agent.plugins.todo]\n",
        "[agent.plugins.agent_status]\nenabled = false\n[agent.plugins.todo]\nenabled = false\n",
    ] {
        let (replaced, origins) = effective(lower, higher);
        assert!(replaced.agent.extensions.resolve().is_empty());
        assert!(matches!(
            origins["agent.plugins.todo"],
            Origin::Workspace { .. }
        ));
    }
    let (enabled, _) = effective(lower, "[agent.plugins.agent_status]\nenabled = true\n");
    assert!(
        enabled
            .agent
            .extensions
            .resolve()
            .agent_status()
            .unwrap()
            .background
            .enabled
    );
    let named: super::super::config::AgentProfileDocument =
        crate::toml_authoring::parse(b"[plugins.agent_status]\n[plugins.todo]\n").unwrap();
    assert!(named.extensions.resolve().is_empty());
}

#[test]
fn native_policy_is_atomic_per_tool() {
    let (config, origins) = effective(LOWER, "[native_tools.bash]\napproval = 'never'");
    assert_eq!(config.native_tools.bash.execution, None);
    assert_eq!(config.native_tools.bash.concurrency, None);
    assert!(config.native_tools.read.approval.is_some());
    assert_eq!(origins["native_tools.bash"], workspace());
    let (config, _) = effective(LOWER, "[native_tools.bash]");
    assert_eq!(
        config.native_tools.bash,
        NativePolicyOverrideDocument::default()
    );
    assert!(config.native_tools.read.approval.is_some());
}

#[test]
fn empty_identity_maps_do_not_erase_unmentioned_identities() {
    let (config, _) = effective(LOWER, "[environment]\n[native_tools]");
    assert_eq!(config.environment["A"], "lower");
    assert_eq!(config.environment["B"], "retained");
    assert_eq!(
        config.native_tools.bash.concurrency,
        Some(ConcurrencyPolicyDocument::Sequential)
    );
    let (config, _) = effective(LOWER, "[environment]\nA = ''");
    assert_eq!(config.environment["A"], "");
    assert_eq!(config.environment["B"], "retained");
}

#[test]
fn mcp_destination_replacement_drops_lower_secrets_and_headers() {
    // Selection is a different native dimension from definition ownership:
    // Workspace replaces the definition while inheriting User's Root selection.
    let (config, _) = effective(&format!("{LOWER}\n[agent.tools.sources]\ndocs = 'all'"), "");
    assert_eq!(
        serde_json::to_value(&config.agent.tools).unwrap()["sources"]["docs"],
        "all"
    );
    let root = tempfile::tempdir().unwrap();
    let user = root.path().join("user/.agents");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(workspace.join(".agents")).unwrap();
    std::fs::write(user.join("mcp.toml"), "[mcp_servers.docs]\nurl = 'https://lower.example/mcp'\nheaders = { Accept = 'lower' }\nsensitive_headers = { Authorization = '$LOWER_TOKEN' }").unwrap();
    std::fs::write(
        workspace.join(".agents/mcp.toml"),
        "[mcp_servers.docs]\nurl = 'https://upper.example/mcp'",
    )
    .unwrap();
    let catalog = crate::local_runtime::mcp_resources::load(&user, Some(&workspace));
    let server = catalog.definitions[&crate::runtime::identity::McpServerId::new("docs")]
        .as_ref()
        .unwrap()
        .clone()
        .resolve();
    assert_eq!(server.url.as_deref(), Some("https://upper.example/mcp"));
    assert!(server.headers.is_empty());
    assert!(server.sensitive_headers.is_empty());
}

//! CFG-01 filesystem and real native composition regressions (Linux and macOS CI).
#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)] // linear fixture scenarios
use super::launch::*;
use super::{LocalRuntimeDependencies, LocalSessionProduct};
use crate::capabilities::{CapabilitySourceId, CapabilitySourceState};
use serde_json::json;
use std::path::Path;

struct Fixture {
    root: tempfile::TempDir,
    host: HostEnvironment,
    request: LaunchRequest,
}

impl Fixture {
    fn new() -> Self {
        // macOS exposes /var through /private/var; fixture expectations use
        // physical paths just like the resolver, not the temporary-dir alias.
        let root =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let host =
            HostEnvironment::from_paths(workspace.clone(), root.path().join("home"), None, None)
                .unwrap();
        std::fs::create_dir_all(&host.config_directory).unwrap();
        std::fs::write(host.config_directory.join("models.jsonc"), serde_json::to_vec(&json!({
            "providers": { "host": { "baseUrl": "http://127.0.0.1:9/v1", "apiKey": "fixture", "models": [
                {"id":"one", "protocol":"openai_chat_completions", "contextWindow":128_000, "maxOutputTokens":4096,
                 "capabilities":{"inputModalities":["text"],"outputModalities":["text"],"toolCalls":true,"reasoning":false},"compat":{"chatReasoningReplay":"omit"}},
                {"id":"two", "protocol":"openai_chat_completions", "contextWindow":128_000, "maxOutputTokens":4096,
                 "capabilities":{"inputModalities":["text"],"outputModalities":["text"],"toolCalls":true,"reasoning":false},"compat":{"chatReasoningReplay":"omit"}}
            ]}}
        })).unwrap()).unwrap();
        let fixture = Self {
            root,
            host,
            request: LaunchRequest::default(),
        };
        fixture.user(json!({"model":{"model":"host/one"}}));
        fixture.trust(TrustAction::Grant);
        fixture
    }
    fn user(&self, value: serde_json::Value) {
        std::fs::write(
            self.host.config_directory.join("settings.jsonc"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
    fn project(&self, value: serde_json::Value) {
        std::fs::write(
            self.host.launch_directory.join("rustx.jsonc"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
    fn trust(&self, action: TrustAction) {
        change_trust(&self.request, &self.host, action).unwrap();
    }
    fn resolve(&self) -> ResolvedLaunch {
        resolve(&self.request, &self.host).unwrap()
    }
}

#[test]
fn cfg233_whole_source_replacement_never_rebinds_host_credentials() {
    let mut f = Fixture::new();
    f.host.credentials = crate::credentials::CredentialSnapshot::new([(
        "HOST_SECRET".into(),
        "CFG233_HOST_SECRET_SENTINEL".into(),
    )]);
    for (host, project) in [
        (
            json!({"enabled":true,"url":"https://host.invalid/mcp","sensitiveHeaders":{"Authorization":"$HOST_SECRET"}}),
            json!({"enabled":true,"url":"https://project.invalid/mcp"}),
        ),
        (
            json!({"enabled":true,"command":"host-server","sensitiveEnv":{"TOKEN":"$HOST_SECRET"}}),
            json!({"enabled":true,"command":"project-server"}),
        ),
    ] {
        f.user(json!({"model":{"model":"host/one"},"mcpServers":{"service":host}}));
        f.project(json!({"mcpServers":{"service":project}}));
        let launch = f.resolve();
        let bindings = super::composition::mcp_bindings_with_authority(
            launch.config(),
            &launch.workspace,
            launch.provenance(),
            &launch.credentials,
        )
        .unwrap();
        let binding = &bindings[&crate::runtime::identity::McpServerId::new("service")];
        assert!(binding.credentials.environment.is_empty());
        assert!(binding.credentials.headers.is_empty());
        assert!(!format!("{launch:?}").contains("CFG233_HOST_SECRET_SENTINEL"));
        for key in ["sensitiveEnv", "sensitiveHeaders"] {
            let mut forbidden = project.clone();
            forbidden[key] = json!({"TOKEN":"$HOST_SECRET"});
            f.project(json!({"mcpServers":{"service":forbidden}}));
            assert!(
                resolve(&f.request, &f.host)
                    .unwrap_err()
                    .contains("host-owned")
            );
        }
    }
}

#[tokio::test]
async fn cfg233_provider_binding_uses_launch_snapshot_and_ignores_unused_missing_keys() {
    let mut f = Fixture::new();
    let path = f.host.config_directory.join("models.jsonc");
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    document["providers"]["host"]["apiKey"] = json!("$CAPTURED_KEY");
    document["providers"]["unused"] = document["providers"]["host"].clone();
    document["providers"]["unused"]["apiKey"] = json!("$UNSET_UNUSED_KEY");
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    f.host.credentials = crate::credentials::CredentialSnapshot::new([(
        "CAPTURED_KEY".into(),
        "CFG233_PROVIDER_SENTINEL".into(),
    )]);
    let launch = f.resolve();
    f.host.credentials = crate::credentials::CredentialSnapshot::default();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    assert!(!format!("{launch:?}").contains("CFG233_PROVIDER_SENTINEL"));
    product.runtime().shutdown().await.unwrap();
    assert!(
        LocalSessionProduct::compose(&f.resolve(), &LocalRuntimeDependencies::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("CAPTURED_KEY")
    );
}

#[tokio::test]
async fn cfg233_enabled_missing_credentials_and_connection_failures_are_source_local() {
    let f = Fixture::new();
    f.user(json!({"model":{"model":"host/one"},"mcpServers":{
        "credential":{"enabled":true,"command":"/does/not/exist","sensitiveEnv":{"TOKEN":"$REQUIRED_KEY"}},
        "connection":{"enabled":true,"command":"/does/not/exist"},
        "disabled":{"enabled":false,"command":"/does/not/exist","sensitiveEnv":{"TOKEN":"$IGNORED_KEY"}}
    }}));
    let product = LocalSessionProduct::compose(&f.resolve(), &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let sources = product.runtime().capability().availability();
    let source = |id| CapabilitySourceId::Mcp(crate::runtime::identity::McpServerId::new(id));
    let CapabilitySourceState::Unavailable { reason } = &sources[&source("credential")] else {
        panic!("source failure")
    };
    assert!(reason.contains("credential") && reason.contains("env:REQUIRED_KEY"));
    assert!(matches!(
        sources[&source("connection")],
        CapabilitySourceState::Unavailable { .. }
    ));
    assert!(matches!(
        sources[&source("disabled")],
        CapabilitySourceState::Inactive { .. }
    ));
    assert!(!format!("{sources:?}").contains("IGNORED_KEY"));
    product.runtime().shutdown().await.unwrap();
}

#[tokio::test]
async fn cfg233_native_composition_keeps_disabled_and_discovered_resources_inert() {
    let mut f = Fixture::new();
    f.request.no_tools = true;
    let package = f.host.launch_directory.join(".agents/tools/discovered");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("server.py"),
        "raise Exception('must not import')",
    )
    .unwrap();
    // No requirements file: inert discovery cannot require package validity.
    f.user(json!({"model":{"model":"host/one"},"mcpServers":{
        "missing":{"enabled":false,"command":"/nonexistent/cfg233","sensitiveEnv":{"TOKEN":"$UNSET"}},
        "offline":{"enabled":false,"url":"http://127.0.0.1:1/mcp","sensitiveHeaders":{"Authorization":"$UNSET"}}
    }}));
    let launch = f.resolve();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    assert!(
        !launch
            .environment_store_root()
            .read_dir()
            .unwrap()
            .any(|entry| entry.unwrap().path().join("python-tools").exists())
    );
    assert_eq!(std::fs::read_dir(&package).unwrap().count(), 1);
    product.runtime().shutdown().await.unwrap();
    f.trust(TrustAction::Revoke);
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("not trusted")
    );
    assert_eq!(std::fs::read_dir(&package).unwrap().count(), 1);
}

#[tokio::test]
async fn cfg233_shared_connect_gate_rejects_all_inert_states_without_spawn_or_network() {
    use crate::capabilities::activation::SourceActivation;
    use crate::tools::mcp::{McpInvalidationState, McpServerRuntime};
    let f = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let marker = f.root.path().join("spawn-marker");
    f.user(json!({"model":{"model":"host/one"},"mcpServers":{
        "stdio":{"command":"/bin/sh","args":["-c",format!("touch {}", marker.display())],"sensitiveEnv":{"TOKEN":"$UNSET"}},
        "http":{"url":format!("http://{}/mcp", listener.local_addr().unwrap()),"sensitiveHeaders":{"Authorization":"$UNSET"}}
    }}));
    let launch = f.resolve();
    let workspace = crate::tools::Workspace::new(&launch.workspace).unwrap();
    for decision in [
        SourceActivation::Disabled,
        SourceActivation::Untrusted,
        SourceActivation::Unconfigured,
    ] {
        for (id, mut binding) in launch.config.mcp_bindings().unwrap() {
            binding.activation = decision;
            let error = McpServerRuntime::connect(
                &id,
                &binding,
                &workspace,
                std::sync::Arc::new(McpInvalidationState::default()),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains(decision.admit().unwrap_err()));
            assert!(
                !error.to_string().contains("UNSET"),
                "activation precedes credential resolution"
            );
        }
    }
    assert!(!marker.exists());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[tokio::test]
async fn cfg233_http_credential_failure_redacts_peer_echo_and_configuration() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const SENTINEL: &str = "CFG233_HTTP_SECRET_BEARER_62df";
    let mut f = Fixture::new();
    f.host.credentials =
        crate::credentials::CredentialSnapshot::new([("AUTH".into(), SENTINEL.into())]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        assert!(String::from_utf8(request).unwrap().contains(SENTINEL));
        let body = format!("credential rejected: {SENTINEL}");
        socket.write_all(format!("HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    });
    f.user(json!({"model":{"model":"host/one"},"mcpServers":{"authenticated":{"enabled":true,"url":format!("http://{endpoint}/mcp"),"sensitiveHeaders":{"Authorization":"$AUTH"}}}}));
    let launch = f.resolve();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    peer.await.unwrap();
    let sources = product.runtime().capability().availability();
    assert!(matches!(
        sources.values().next(),
        Some(CapabilitySourceState::Unavailable { .. })
    ));
    assert!(!format!("{sources:?} {launch:?}").contains(SENTINEL));
    assert!(
        !serde_json::to_string(launch.config())
            .unwrap()
            .contains(SENTINEL)
    );
    let response = product.endpoint().handle_request(
        crate::runtime_client::RuntimeClientRequest::Initialize {
            id: crate::runtime_client::RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        },
    );
    let payload = serde_json::to_string(&response).unwrap();
    assert!(payload.contains("authenticated") && payload.contains("unavailable"));
    assert!(!payload.contains(SENTINEL));
    product.runtime().shutdown().await.unwrap();
}

#[test]
fn precedence_absence_empty_and_whole_entries_keep_provenance() {
    let mut f = Fixture::new();
    let builtin = f.resolve();
    assert_eq!(builtin.config.agent_id.as_str(), "rustx");
    assert_eq!(builtin.config.context.reserve_tokens, 1024);
    assert_eq!(builtin.provenance["agentId"], Origin::Builtin);
    f.user(json!({"model":{"model":"host/one"}, "agentId":"user", "context":{"reserveTokens":2000,"keepRecentTokens":6000},
        "defaultTools":["read","bash"],"environment":{"USER_ENTRY":"one","REPLACED":"old"},
        "mcpServers":{"service":{"command":"old-command","args":["old"]},"retained":{"command":"retained"}},
        "subagents":{"definitions":{"role":{"description":"old","instructionsFile":"old.md","skills":["old"]}}}
    }));
    f.project(
        json!({"model":{"model":"host/two"},"context":{"reserveTokens":3000},"defaultTools":[],
            "environment":{"REPLACED":"new"}, "mcpServers":{"service":{"command":"new-command"}},
            "subagents":{"definitions":{"role":{"description":"new","instructionsFile":"new.md"}}}
        }),
    );
    let resolved = f.resolve();
    assert_eq!(resolved.config.agent_id.as_str(), "user");
    assert_eq!(resolved.config.context.reserve_tokens, 3000);
    assert_eq!(resolved.config.context.keep_recent_tokens, 6000);
    assert!(resolved.config.default_tools.is_empty());
    assert_eq!(resolved.config.environment.len(), 2);
    assert_eq!(resolved.config.environment["REPLACED"], "new");
    let service =
        &resolved.config.mcp_servers[&crate::runtime::identity::McpServerId::new("service")];
    assert!(service.args.is_empty(), "whole service replacement");
    let role = resolved
        .config
        .subagents
        .definitions
        .values()
        .next()
        .unwrap();
    assert!(role.skills.is_empty(), "whole role replacement");
    assert!(matches!(
        resolved.provenance["context.keepRecentTokens"],
        Origin::User { .. }
    ));
    assert!(matches!(
        resolved.provenance["context.reserveTokens"],
        Origin::Project { .. }
    ));
    f.request.model = Some("host/one".into());
    f.request.tools = Some(Vec::new());
    f.request.exclude_tools = Some(Vec::new());
    let cli = f.resolve();
    assert_eq!(cli.config.model.model.to_string(), "host/one");
    assert!(matches!(cli.provenance["model.model"], Origin::Cli { .. }));
    assert!(matches!(cli.provenance["excludeTools"], Origin::Cli { .. }));
    assert_eq!(cli.tools, Some(Vec::new()));
    f.project(json!({"environment":{},"mcpServers":{},"subagents":{"definitions":{}}}));
    let empty = f.resolve();
    assert!(empty.config.environment.is_empty());
    assert!(empty.config.mcp_servers.is_empty());
    assert!(empty.config.subagents.definitions.is_empty());
    assert_eq!(empty.config.default_tools, ["read", "bash"]);
}

#[test]
fn forbidden_authority_fails_through_discovery_and_relative_config_override() {
    let mut f = Fixture::new();
    for field in [
        "models",
        "providers",
        "credentials",
        "trust",
        "trusted",
        "trustStore",
        "stateDirectory",
        "runtimeRoot",
        "workspace",
    ] {
        f.project(json!({field:"../../host-authority"}));
        for explicit in [false, true] {
            f.request.config = explicit.then(|| "./rustx.jsonc".into());
            let error = resolve(&f.request, &f.host).unwrap_err();
            assert!(
                error.contains(field) && error.contains("forbidden"),
                "{error}"
            );
        }
    }
}

#[test]
fn optional_explicit_malformed_unknown_and_null_are_distinct() {
    let mut f = Fixture::new();
    assert_eq!(f.resolve().config.agent_id.as_str(), "rustx");
    f.request.config = Some("missing.jsonc".into());
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("cannot read")
    );
    f.request.config = None;
    for value in [
        "{",
        "{\"unknown\":1}",
        "{\"skills\":null}",
        "{\"context\":{\"typo\":1}}",
        "{\"defaultTools\":false}",
    ] {
        std::fs::write(f.host.launch_directory.join("rustx.jsonc"), value).unwrap();
        assert!(resolve(&f.request, &f.host).is_err(), "{value}");
    }
    f.project(json!({"context":{"summaryOutputCap":null},"skills":[]}));
    assert_eq!(f.resolve().config.context.summary_output_cap, None);
    std::fs::remove_file(f.host.config_directory.join("settings.jsonc")).unwrap();
    f.request.model = Some("host/one".into());
    assert!(
        resolve(&f.request, &f.host).is_ok(),
        "optional user settings"
    );
    std::fs::write(
        f.host.launch_directory.join("rustx.jsonc"),
        " ".repeat(1024 * 1024 + 1),
    )
    .unwrap();
    assert!(resolve(&f.request, &f.host).unwrap_err().contains("1 MiB"));
}

#[test]
fn relative_paths_keep_their_document_and_cli_bases() {
    let mut f = Fixture::new();
    f.user(json!({"model":{"model":"host/one"}, "skills":["user-skills"], "subagents":{"definitions":{"user":{"description":"user","instructionsFile":"user.md"}}}}));
    f.project(json!({"subagents":{"definitions":{"project":{"description":"project","instructionsFile":"project.md","agentsMd":{"files":["instructions.md"]}}}}}));
    let resolved = f.resolve();
    assert_eq!(
        resolved.config.skills,
        [f.host.config_directory.join("user-skills")]
    );
    f.request.skill_paths = vec!["cli-skill".into()];
    assert_eq!(
        f.resolve().config.skills,
        [f.host.launch_directory.join("cli-skill")]
    );
    for (name, role) in &resolved.config.subagents.definitions {
        let base = if name.as_str() == "user" {
            &f.host.config_directory
        } else {
            &f.host.launch_directory
        };
        assert_eq!(
            role.instructions_file,
            base.join(format!("{}.md", name.as_str()))
        );
    }
    f.request.config = Some("replacement.jsonc".into());
    std::fs::write(f.host.launch_directory.join("replacement.jsonc"), "{}").unwrap();
    assert_eq!(
        f.resolve().config.subagents.definitions.len(),
        1,
        "--config replaces the project slot"
    );
}

#[test]
fn workspace_boundaries_subdirectories_non_git_nested_and_explicit() {
    let mut f = Fixture::new();
    let original = f.resolve();
    f.project(json!({}));
    let sub = f.host.launch_directory.join("a/b");
    std::fs::create_dir_all(&sub).unwrap();
    f.host.launch_directory = sub.clone();
    assert_eq!(f.resolve().identity, original.identity);
    assert_eq!(f.resolve().runtime_root, original.runtime_root);
    std::fs::write(sub.join("rustx.jsonc"), "{}").unwrap();
    let (_, nested) = resolve_locations(&f.request, &f.host).unwrap();
    assert_ne!(nested, original.identity);
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("not trusted")
    );
    f.request.workspace = Some(original.workspace.clone());
    assert_eq!(f.resolve().identity, original.identity);
    f.request.workspace = Some(sub);
    f.trust(TrustAction::Grant);
    assert_eq!(f.resolve().identity, nested);
}

#[cfg(unix)]
#[test]
#[allow(clippy::items_after_statements)] // test-local Git assertion helper
fn canonical_symlink_and_real_git_worktree_identities_are_stable_and_separate() {
    let f = Fixture::new();
    let original = f.resolve();
    let alias = f.root.path().join("alias");
    std::os::unix::fs::symlink(&original.workspace, &alias).unwrap();
    let mut host = f.host.clone();
    host.launch_directory = alias;
    assert_eq!(
        resolve(&f.request, &host).unwrap().identity,
        original.identity
    );
    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    git(&original.workspace, &["init", "--quiet"]);
    git(
        &original.workspace,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
            "--quiet",
        ],
    );
    let worktree = f.root.path().join("worktree");
    git(
        &original.workspace,
        &[
            "worktree",
            "add",
            "--detach",
            worktree.to_str().unwrap(),
            "HEAD",
        ],
    );
    host.launch_directory = worktree.clone();
    let (locations, identity) = resolve_locations(&f.request, &host).unwrap();
    assert_ne!(identity, original.identity);
    assert_ne!(locations.runtime_root, original.runtime_root);
    assert!(
        resolve(&f.request, &host)
            .unwrap_err()
            .contains("not trusted")
    );
    change_trust(&f.request, &host, TrustAction::Grant).unwrap();
    assert_eq!(
        resolve(&f.request, &host).unwrap().workspace,
        std::fs::canonicalize(worktree).unwrap()
    );
    change_trust(&f.request, &host, TrustAction::Revoke).unwrap();
    assert!(resolve(&f.request, &host).is_err());
    assert!(resolve(&f.request, &f.host).is_ok(), "revocation is scoped");
    let outside = host.launch_directory.join("untrusted.md");
    std::fs::write(&outside, "other worktree").unwrap();
    f.project(json!({"subagents":{"definitions":{"other":{"description":"other","instructionsFile":outside}}}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("outside trusted workspace")
    );
}

#[tokio::test]
async fn minimal_native_composition_and_frozen_launch_ignore_later_config_edits() {
    let f = Fixture::new();
    let launch = f.resolve();
    f.user(json!({"model":{"model":"host/two"},"agentId":"edited"}));
    f.project(json!({"model":{"model":"missing/model"}}));
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    assert!(
        product
            .runtime()
            .runtime_resources()
            .subagents()
            .names()
            .is_empty()
    );
    assert_eq!(launch.config.model.model.to_string(), "host/one");
    assert_eq!(launch.config.agent_id.as_str(), "rustx");
    assert!(launch.config.mcp_servers.is_empty());
    assert!(launch.config.skills.is_empty());
    product.runtime().shutdown().await.unwrap();
    assert!(
        resolve(&f.request, &f.host).is_err(),
        "a new launch sees the edit"
    );
}

#[tokio::test]
async fn resolution_and_composition_failures_preserve_published_session_selection() {
    let f = Fixture::new();
    let launch = f.resolve();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    product.runtime().shutdown().await.unwrap();
    drop(product);
    let catalog = launch.runtime_root.join("sessions/catalog.json");
    let before = std::fs::read(&catalog).unwrap();
    f.trust(TrustAction::Revoke);
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("not trusted")
    );
    assert_eq!(before, std::fs::read(&catalog).unwrap());
    f.trust(TrustAction::Grant);
    f.project(json!({"agentId":""}));
    assert!(resolve(&f.request, &f.host).is_err());
    assert_eq!(before, std::fs::read(&catalog).unwrap());
    f.project(json!({"skills":["missing-skill"]}));
    let invalid_resources = f.resolve();
    assert!(
        LocalSessionProduct::compose(&invalid_resources, &LocalRuntimeDependencies::default())
            .await
            .is_err()
    );
    assert_eq!(before, std::fs::read(&catalog).unwrap());
}

#[test]
fn no_model_or_protocol_is_guessed_and_host_paths_are_validated() {
    let f = Fixture::new();
    f.user(json!({}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("unambiguous")
    );
    f.user(json!({"model":{"model":"one"}}));
    assert!(
        resolve(&f.request, &f.host).is_err(),
        "unqualified reference is ambiguous"
    );
    f.user(json!({"model":{"model":"host/one"}}));
    let model_path = f.host.config_directory.join("models.jsonc");
    let mut catalog: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&model_path).unwrap()).unwrap();
    catalog["providers"]["host"]["models"][0]
        .as_object_mut()
        .unwrap()
        .remove("protocol");
    std::fs::write(model_path, serde_json::to_vec(&catalog).unwrap()).unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("protocol")
    );
    assert!(
        HostEnvironment::from_paths(
            f.host.launch_directory.clone(),
            "/home/test".into(),
            Some("relative".into()),
            None
        )
        .is_err()
    );
}

#[test]
fn host_state_and_catalog_overrides_keep_document_and_cli_origins() {
    let mut f = Fixture::new();
    f.user(json!({"models":"models.jsonc","runtimeRoot":"private","model":{"model":"host/one"}}));
    let user = f.resolve();
    assert_eq!(user.runtime_root, f.host.config_directory.join("private"));
    assert_eq!(
        user.provenance["models"],
        Origin::User {
            document: f.host.config_directory.join("settings.jsonc"),
            base: f.host.config_directory.clone()
        }
    );
    assert!(matches!(
        user.provenance["runtimeRoot"],
        Origin::User { .. }
    ));
    f.request.runtime_root = Some("../cli-state".into());
    f.request.models = Some(f.host.config_directory.join("models.jsonc"));
    let cli = f.resolve();
    assert_eq!(cli.runtime_root, f.root.path().join("cli-state"));
    assert!(matches!(cli.provenance["runtimeRoot"], Origin::Cli { .. }));
    f.request.runtime_root = Some(f.host.state_directory.clone());
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("trust authority")
    );
}

#[test]
fn inspection_reads_only_the_host_state_reference_without_activation_or_writes() {
    let f = Fixture::new();
    f.trust(TrustAction::Revoke);
    f.user(json!({"runtimeRoot":"private","model":null,"context":false}));
    f.project(json!({"trust":true}));
    std::fs::remove_file(f.host.config_directory.join("models.jsonc")).unwrap();
    let locations = resolve_inspection_locations(&f.request, &f.host).unwrap();
    assert_eq!(
        locations.runtime_root,
        f.host.config_directory.join("private")
    );
    assert!(!locations.runtime_root.exists());
    assert!(resolve(&f.request, &f.host).is_err());
}

#[cfg(unix)]
#[test]
fn dangling_optional_files_and_project_redirected_trust_are_errors() {
    let f = Fixture::new();
    std::os::unix::fs::symlink("missing.jsonc", f.host.launch_directory.join("rustx.jsonc"))
        .unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("cannot read")
    );
    f.trust(TrustAction::Revoke);
    std::fs::remove_dir(f.host.state_directory.join("trust")).unwrap();
    std::os::unix::fs::symlink(
        &f.host.launch_directory,
        f.host.state_directory.join("trust"),
    )
    .unwrap();
    assert!(
        change_trust(&f.request, &f.host, TrustAction::Grant)
            .unwrap_err()
            .contains("outside")
    );
}

#[cfg(unix)]
#[test]
fn workspace_identity_has_exact_unix_byte_sha256_format() {
    use std::os::unix::ffi::OsStrExt;
    // Fixed canonical-path byte inputs at the pure hashing boundary. Golden
    // digests were independently calculated with sha256sum, not the helper.
    // Non-UTF-8 vectors also run on macOS without requiring its filesystem to
    // create filenames that it may reject.
    let vectors: &[(&[u8], &str)] = &[
        (
            b"/",
            "8a5edab282632443219e051e4ade2d1d5bbc671c781051bf1437897cbdfea0f1",
        ),
        (
            b"/rustx/workspace",
            "abe7db53f659b7fae2dfcc44469d33ab6b996505a4a32f09adbaef6d3525c1bd",
        ),
        (
            b"/rustx/project-\xff",
            "9617e4cce0d8db9dfec2c04014c1fc341a8479ed6812ecc04bc1cc749411cee8",
        ),
        (
            b"/rustx/project-\xfe",
            "59894cb99c81922900c2c3c7d55ee8fbb0c8e4ddff59a475915a1c597e4a8284",
        ),
    ];
    for (bytes, expected) in vectors {
        let path = Path::new(std::ffi::OsStr::from_bytes(bytes));
        assert_eq!(workspace_identity(path), *expected);
    }
    // Exercise the real resolution/canonicalization boundary too. Unix root
    // has known canonical bytes on both supported platforms; no tempdir,
    // configuration, trust, or state writes participate in this assertion.
    let host = HostEnvironment::from_paths("/".into(), "/unused-host".into(), None, None).unwrap();
    let request = LaunchRequest {
        workspace: Some("/".into()),
        ..Default::default()
    };
    let (locations, identity) = resolve_locations(&request, &host).unwrap();
    assert_eq!(locations.workspace, Path::new("/"));
    assert_eq!(identity, vectors[0].1);
}

// Linux filesystems accept arbitrary non-NUL bytes. macOS filesystem APIs may
// reject invalid UTF-8; its canonical alias/worktree contract is tested above.
#[cfg(target_os = "linux")]
#[test]
fn non_utf8_workspace_identity_is_not_lossy() {
    use std::os::unix::ffi::OsStringExt;
    let mut f = Fixture::new();
    let first = f
        .root
        .path()
        .join(std::ffi::OsString::from_vec(vec![b'a', 0xff]));
    let second = f
        .root
        .path()
        .join(std::ffi::OsString::from_vec(vec![b'a', 0xfe]));
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    f.request.workspace = Some(first);
    let (_, first_identity) = resolve_locations(&f.request, &f.host).unwrap();
    f.request.workspace = Some(second);
    let (_, second_identity) = resolve_locations(&f.request, &f.host).unwrap();
    assert_ne!(first_identity, second_identity);
}

#[test]
fn duplicate_role_definitions_and_invalid_lower_layer_cannot_be_hidden() {
    let f = Fixture::new();
    std::fs::write(f.host.launch_directory.join("rustx.jsonc"), r#"{"subagents":{"definitions":{"role":{"description":"one","instructionsFile":"one.md"},"role":{"description":"two","instructionsFile":"two.md"}}}}"#).unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("duplicate")
    );
    f.user(json!({"model":{"model":"host/one"},"context":{"unknown":true}}));
    f.project(json!({"context":{"reserveTokens":7}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("unknown field")
    );
}

#[test]
fn project_trust_never_grants_tool_approval_authority() {
    let mut f = Fixture::new();
    let user = json!({"model":{"model":"host/one"},"approvalMode":"full_access",
        "nativeTools":{"bash":{"approval":"always"}},
        "mcpServers":{"server":{"command":"fixture"}},
        "mcpToolPolicies":{"server":{"approval":"always"}}});
    f.user(user);
    let host = f.resolve();
    assert_eq!(
        host.config.approval_mode,
        crate::runtime::ApprovalMode::FullAccess
    );
    let serialized = serde_json::to_value(host.config()).unwrap();
    assert_eq!(serialized["nativeTools"]["bash"]["approval"], "always");
    assert_eq!(
        serialized["mcpToolPolicies"]["server"]["approval"],
        "always"
    );
    for document in [
        json!({"approvalMode":"full_access"}),
        json!({"nativeTools":{"bash":{"approval":"never"}}}),
        json!({"mcpToolPolicies":{"server":{"approval":"never"}}}),
        json!({"nativeTools":{}}),
        json!({"nativeTools":{"bash":{"execution":"background_only"}}}),
    ] {
        f.project(document);
        // Explicit CLI selection cannot mask a forbidden project declaration.
        f.request.model = Some("host/two".into());
        for explicit in [false, true] {
            f.request.config = explicit.then(|| "rustx.jsonc".into());
            let error = resolve(&f.request, &f.host).unwrap_err();
            assert!(
                error.contains("forbidden") && error.contains("approval authority"),
                "{error}"
            );
        }
    }
}

#[tokio::test]
async fn project_resource_authority_rejects_every_declared_escape_but_preserves_host_paths() {
    let mut f = Fixture::new();
    let other = f.root.path().join("B");
    std::fs::create_dir(&other).unwrap();
    let resource = other.join("resource");
    std::fs::write(&resource, "untrusted B bytes").unwrap();
    let role = |path: serde_json::Value| json!({"subagents":{"definitions":{"x":{"description":"x","instructionsFile":path}}}});
    for document in [
        role(json!("../B/resource")),
        role(json!(&resource)),
        json!({"skills":[&other]}),
        json!({"subagents":{"definitions":{"x":{"description":"x","instructionsFile":"inside.md","agentsMd":{"files":[&resource]}}}}}),
        json!({"mcpServers":{"x":{"command":&resource}}}),
        json!({"mcpServers":{"x":{"command":"fixture","cwd":&other}}}),
    ] {
        f.project(document);
        let error = resolve(&f.request, &f.host).unwrap_err();
        assert!(error.contains("outside trusted workspace"), "{error}");
    }
    // An external --config is inert input, not a grant for its neighboring files.
    std::fs::write(
        other.join("config.jsonc"),
        role(json!("resource")).to_string(),
    )
    .unwrap();
    f.request.config = Some(other.join("config.jsonc"));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("outside trusted workspace")
    );
    f.request.config = None;
    f.project(json!({}));
    f.user(json!({"model":{"model":"host/one"},"subagents":{"definitions":{"x":{"description":"host","instructionsFile":&resource}}}}));
    let host = f.resolve();
    assert!(matches!(
        host.provenance["subagents.definitions.x"],
        Origin::User { .. }
    ));
    let skill = other.join("outside");
    std::fs::create_dir(&skill).unwrap();
    f.request.skill_paths = vec![skill];
    std::fs::write(
        f.request.skill_paths[0].join("SKILL.md"),
        "---\nname: outside\ndescription: Host resource\n---\nHost instructions\n",
    )
    .unwrap();
    assert!(matches!(
        f.resolve().provenance["skills"],
        Origin::Cli { .. }
    ));
    let product = LocalSessionProduct::compose(&f.resolve(), &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let resources = product.runtime().runtime_resources();
    assert_eq!(
        resources
            .subagents()
            .get(&crate::runtime::subagent::SubagentName::parse("x").unwrap())
            .unwrap()
            .instructions(),
        "untrusted B bytes"
    );
    assert!(resources.skill_catalog().unwrap().contains("outside"));
    product.runtime().shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn project_symlink_escape_is_rechecked_before_composition_and_reload() {
    let f = Fixture::new();
    let other = f.root.path().join("B");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("instructions.md"), "B MUST NEVER BE ADMITTED").unwrap();
    let source = f.host.launch_directory.join("instructions.md");
    std::fs::write(&source, "trusted A").unwrap();
    f.project(json!({"subagents":{"definitions":{"x":{"description":"x","instructionsFile":"instructions.md"}}}}));
    let launch = f.resolve();
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink(other.join("instructions.md"), &source).unwrap();
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("outside trusted workspace")
    );
    assert!(
        LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
            .await
            .unwrap_err()
            .to_string()
            .contains("outside trusted workspace")
    );
    std::fs::remove_file(&source).unwrap();
    std::fs::write(&source, "trusted A").unwrap();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let before = product.runtime().runtime_resources();
    std::fs::remove_file(&source).unwrap();
    std::os::unix::fs::symlink(other.join("instructions.md"), &source).unwrap();
    assert!(
        product
            .runtime()
            .reload_resources()
            .await
            .unwrap_err()
            .to_string()
            .contains("outside trusted workspace")
    );
    let after = product.runtime().runtime_resources();
    assert_eq!(before.revision(), after.revision());
    assert_eq!(before.capability_revision(), after.capability_revision());
    assert!(std::sync::Arc::ptr_eq(&before, &after));
    assert_eq!(
        after
            .subagents()
            .get(&crate::runtime::subagent::SubagentName::parse("x").unwrap())
            .unwrap()
            .instructions(),
        "trusted A"
    );
    f.project(json!({"subagents":{"definitions":{"x":{"description":"escape","instructionsFile":other.join("instructions.md")}}}}));
    assert!(
        product
            .runtime()
            .reload_resources()
            .await
            .unwrap_err()
            .to_string()
            .contains("outside trusted workspace")
    );
    assert_eq!(
        product.runtime().runtime_resources().revision(),
        before.revision()
    );
    // Removing the offending declaration permits a new candidate: stale launch
    // paths must not become a second resource-generation authority.
    f.project(json!({"subagents":{"definitions":{}}}));
    product.runtime().reload_resources().await.unwrap();
    product.runtime().shutdown().await.unwrap();
}

#[cfg(unix)]
#[test]
fn project_directory_symlinks_cannot_authorize_builtin_or_declared_resources() {
    let f = Fixture::new();
    let other = f.root.path().join("B");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("file"), "B").unwrap();
    std::os::unix::fs::symlink(&other, f.host.launch_directory.join("link")).unwrap();
    for document in [
        json!({"skills":["link"]}),
        json!({"subagents":{"definitions":{"x":{"description":"x","instructionsFile":"link/file"}}}}),
        json!({"mcpServers":{"x":{"command":"link/file"}}}),
        json!({"mcpServers":{"x":{"command":"fixture","cwd":"link"}}}),
    ] {
        f.project(document);
        assert!(
            resolve(&f.request, &f.host)
                .unwrap_err()
                .contains("outside trusted workspace")
        );
    }
    f.project(json!({}));
    std::os::unix::fs::symlink(&other, f.host.launch_directory.join(".agents")).unwrap();
    assert!(
        f.resolve()
            .validate_resource_authority()
            .unwrap_err()
            .contains("outside trusted workspace")
    );
    std::os::unix::fs::symlink(
        other.join("file"),
        f.host.launch_directory.join("AGENTS.md"),
    )
    .unwrap();
    assert!(
        crate::runtime::load_project_context_files(&f.host.launch_directory)
            .unwrap_err()
            .to_string()
            .contains("outside trusted workspace")
    );
    std::fs::remove_file(f.host.launch_directory.join(".agents")).unwrap();
    std::fs::remove_file(f.host.launch_directory.join("AGENTS.md")).unwrap();
    let launch = f.resolve();
    std::fs::rename(&f.host.launch_directory, f.root.path().join("retired-A")).unwrap();
    std::os::unix::fs::symlink(&other, &f.host.launch_directory).unwrap();
    assert!(
        launch
            .validate_resource_authority()
            .unwrap_err()
            .contains("outside trusted workspace")
    );
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("not trusted")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_workflow_symlink_rejects_reload_without_reading_external_yaml() {
    let f = Fixture::new();
    let launch = f.resolve();
    let product = LocalSessionProduct::compose(&launch, &LocalRuntimeDependencies::default())
        .await
        .unwrap();
    let before = product.runtime().runtime_resources();
    let other = f.root.path().join("B.yaml");
    std::fs::write(&other, "THIS IS NOT YAML: [").unwrap();
    let directory = f.host.launch_directory.join(".agents/workflows");
    std::fs::create_dir_all(&directory).unwrap();
    std::os::unix::fs::symlink(&other, directory.join("escape.yaml")).unwrap();
    f.project(json!({"workflows":{"definitions":["escape"]}}));
    let error = product
        .runtime()
        .reload_resources()
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("outside trusted workspace"),
        "authority rejection must precede YAML parsing: {error}"
    );
    assert!(std::sync::Arc::ptr_eq(
        &before,
        &product.runtime().runtime_resources()
    ));
    product.runtime().shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn frozen_mcp_binding_rechecks_project_authority_on_every_connect() {
    use crate::tools::mcp::{McpInvalidationState, McpServerBinding, McpServerRuntime};
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let program = f.host.launch_directory.join("server");
    std::fs::write(&program, "initial project resource").unwrap();
    f.project(json!({"mcpServers":{"x":{"enabled":true,"command":"./server"}}}));
    let launch = f.resolve();
    let bindings = super::composition::mcp_bindings_with_authority(
        launch.config(),
        &launch.workspace,
        launch.provenance(),
        &launch.credentials,
    )
    .unwrap();
    let id = crate::runtime::identity::McpServerId::new("x");
    let binding = &bindings[&id];
    assert_eq!(binding.resource_workspace.as_ref(), Some(&launch.workspace));
    // The same binding is serialized into admitted child inputs and retained
    // by reconnect authority; no rediscovery is needed to enforce the root.
    let frozen: McpServerBinding =
        serde_json::from_slice(&serde_json::to_vec(binding).unwrap()).unwrap();
    assert_eq!(&frozen, binding);
    let other = f.root.path().join("B");
    std::fs::create_dir(&other).unwrap();
    let sentinel = other.join("started");
    std::fs::write(
        other.join("server"),
        format!("#!/bin/sh\ntouch '{}'\n", sentinel.display()),
    )
    .unwrap();
    std::fs::set_permissions(other.join("server"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_file(&program).unwrap();
    std::os::unix::fs::symlink(other.join("server"), &program).unwrap();
    let workspace = crate::tools::workspace::Workspace::new(&launch.workspace).unwrap();
    for _ in 0..2 {
        let error = McpServerRuntime::connect(
            &id,
            &frozen,
            &workspace,
            std::sync::Arc::new(McpInvalidationState::default()),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("outside trusted workspace"),
            "{error}"
        );
        assert!(!sentinel.exists());
    }
    f.project(json!({"mcpServers":{"x":{"command":"./server","resourceWorkspace":other}}}));
    assert!(
        resolve(&f.request, &f.host)
            .unwrap_err()
            .contains("unknown field")
    );
}

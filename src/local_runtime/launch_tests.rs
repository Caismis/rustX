//! CFG-01 filesystem and real native composition regressions (Linux and macOS CI).
#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)] // linear fixture scenarios
use super::launch::*;
use super::{LocalRuntimeDependencies, LocalSessionProduct};
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

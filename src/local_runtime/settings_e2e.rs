//! Native CFG3 source authoring and complete generation publication contracts.
use super::configuration::settings::{ConfigMutation, SettingsError, SourceMutation};
use super::launch::{HostEnvironment, LaunchRequest};
use std::collections::BTreeMap;

fn fixture() -> (tempfile::TempDir, HostEnvironment, LaunchRequest) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let home = root.path().join("home");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(home.join("rustx")).unwrap();
    std::fs::write(
        home.join("rustx/rustx.toml"),
        r#"
[providers.local]
base_url = "http://127.0.0.1:9/v1"
api_key = "SECRET_SENTINEL"
[models.a]
provider = "local"
id = "wire-a"
protocol = "openai_responses"
context_window = 128000
max_output_tokens = 8192
[models.a.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false
[agent.model]
model = "a"
"#,
    )
    .unwrap();
    let host = HostEnvironment::from_paths(workspace, home).unwrap();
    (root, host, LaunchRequest::default())
}

#[test]
fn root_identity_and_description_are_independent_cas_units() {
    let (_root, host, request) = fixture();
    let (manager, input) = request.session_input(&host).unwrap();
    let source = manager
        .read_source_settings(&rustx_target(&input.cwd))
        .unwrap();
    let identity = SourceMutation::Config {
        mutation: ConfigMutation::AgentIdentity {
            authored: Some(crate::runtime::identity::AgentId::new("reviewer")),
        },
    };
    let saved = manager
        .write_source_settings(
            &rustx_target(&input.cwd),
            &source.workspace.as_ref().unwrap().revision,
            identity.clone(),
        )
        .unwrap();
    assert_ne!(
        saved.workspace.as_ref().unwrap().revision,
        source.workspace.as_ref().unwrap().revision
    );
    assert!(matches!(
        manager.write_source_settings(
            &rustx_target(&input.cwd),
            &source.workspace.as_ref().unwrap().revision,
            identity
        ),
        Err(SettingsError::Conflict { .. })
    ));
    let saved = manager
        .write_source_settings(
            &rustx_target(&input.cwd),
            &saved.workspace.as_ref().unwrap().revision,
            SourceMutation::Config {
                mutation: ConfigMutation::Description {
                    authored: Some("Review changes".into()),
                },
            },
        )
        .unwrap();
    let document = saved.workspace.as_ref().unwrap().authored.clone().unwrap();
    assert_eq!(document.agent_id.unwrap().as_str(), "reviewer");
    assert_eq!(
        document.agent.unwrap().description.as_deref(),
        Some("Review changes")
    );
    let removed = manager
        .write_source_settings(
            &rustx_target(&input.cwd),
            &saved.workspace.as_ref().unwrap().revision,
            SourceMutation::Config {
                mutation: ConfigMutation::Description { authored: None },
            },
        )
        .unwrap();
    assert!(
        removed
            .workspace
            .as_ref()
            .unwrap()
            .authored
            .as_ref()
            .unwrap()
            .agent
            .as_ref()
            .unwrap()
            .description
            .is_none()
    );
}

#[test]
fn t08_capture_rejects_external_change_between_layers_and_resource_manifest() {
    let (_root, host, request) = fixture();
    let (manager, input) = request.session_input(&host).unwrap();
    let source = manager
        .read_source_settings(&rustx_target(&input.cwd))
        .unwrap();
    let path = source.user.path;
    let before = std::fs::read_to_string(&path).unwrap();
    // This hook is exactly after layer capture, before resource resolution and
    // the final manifest check. No thread scheduling or elapsed time is involved.
    let result = manager.capture_application_at_boundary(&input, || {
        std::fs::write(
            &path,
            format!("{before}\n[environment]\nCAPTURE_REVISION='new'\n"),
        )
        .unwrap();
    });
    assert!(matches!(result, Err(diagnostic) if diagnostic.contains("changed during capture")));
    let captured = manager.capture_application(&input).unwrap();
    assert!(captured.context.is_ok(), "a stable rescan succeeds");
}

fn rustx_target(
    directory: &std::path::Path,
) -> crate::local_runtime::configuration::settings::SourceTarget {
    crate::local_runtime::configuration::settings::SourceTarget::Workspace {
        directory: directory.to_path_buf(),
    }
}

/// A literal Tool environment value is a secret on the same terms as a Provider
/// credential: native authority keeps it, and no projection that leaves native
/// authority may carry it. The identity itself stays reachable so a higher
/// scope can discover it, label its provenance and author an override without
/// ever reading the lower-authority literal.
#[test]
fn s1_environment_literals_never_leave_native_authority() {
    const SENTINEL: &str = "S1-SECRET-SENTINEL";
    let (_root, host, request) = fixture();
    let (manager, input) = request.session_input(&host).unwrap();
    let user = crate::local_runtime::configuration::settings::SourceTarget::User;
    let before = manager.read_source_settings(&user).unwrap();
    let saved = manager
        .write_source_settings(
            &user,
            &before.user.revision,
            SourceMutation::Config {
                mutation: ConfigMutation::Environment {
                    name: "SECRET_ENV".into(),
                    authored: Some(SENTINEL.into()),
                },
            },
        )
        .unwrap();
    // Native authority really holds the literal: the authored document on disk
    // is unredacted, which is what the running Tool environment resolves from.
    let document = std::fs::read_to_string(&saved.user.path).unwrap();
    assert!(
        document.contains(SENTINEL),
        "native authoring keeps the value"
    );

    // Every projection of that document is identity-only. Both scopes are
    // checked, because a Workspace reads the same User document as `resolved`.
    for projection in [
        saved.clone(),
        manager.read_source_settings(&user).unwrap(),
        manager
            .read_source_settings(&rustx_target(&input.cwd))
            .unwrap(),
    ] {
        let wire = serde_json::to_string(&projection).unwrap();
        assert!(
            !wire.contains(SENTINEL),
            "environment literal reached the wire: {wire}"
        );
        assert!(
            !wire.contains("SECRET_SENTINEL"),
            "Provider literal credential reached the wire: {wire}"
        );
        assert!(
            wire.contains("SECRET_ENV"),
            "the environment identity must stay discoverable: {wire}"
        );
        assert_eq!(
            projection.user.authored.as_ref().unwrap().environment,
            Some(vec!["SECRET_ENV".to_string()]),
            "the authoring scope projects the identity it owns, never its value"
        );
        assert_eq!(
            projection.resolved.as_ref().unwrap().environment,
            Some(vec!["SECRET_ENV".to_string()]),
            "native resolution projects the effective identity, never its value"
        );
    }
}

/// The MCP projection redacts literal `env`/`headers` on the same contract, and
/// names the identities it retained. This is the pattern the environment
/// projection above follows; asserting it here keeps the two from diverging.
#[test]
fn s1_mcp_literal_environment_and_headers_stay_redacted() {
    const SENTINEL: &str = "S1-MCP-SENTINEL";
    let (_root, host, request) = fixture();
    let (manager, _input) = request.session_input(&host).unwrap();
    let user = crate::local_runtime::configuration::settings::SourceTarget::User;
    let before = manager.read_source_settings(&user).unwrap();
    let definition = super::authoring::McpAuthoring {
        sensitive_env: None,
        sensitive_headers: None,
        transport_type: Some(super::config::McpTransportType::Stdio),
        url: None,
        headers: BTreeMap::new(),
        command: Some("server".into()),
        args: Vec::new(),
        env: BTreeMap::from([("TOKEN".to_string(), SENTINEL.to_string())]),
        cwd: None,
    };
    let saved = manager
        .write_source_settings(
            &user,
            &before.user_mcp.revision,
            SourceMutation::Mcp {
                id: crate::runtime::identity::McpServerId::new("probe"),
                authored: Some(crate::local_runtime::configuration::settings::McpWrite {
                    definition,
                    retained_env: Vec::new(),
                    retained_headers: Vec::new(),
                }),
            },
        )
        .unwrap();
    let wire = serde_json::to_string(&saved).unwrap();
    assert!(
        !wire.contains(SENTINEL),
        "MCP literal environment reached the wire: {wire}"
    );
    let view = &saved.user_mcp.authored.as_ref().unwrap()
        [&crate::runtime::identity::McpServerId::new("probe")];
    assert!(view.definition.env.is_empty());
    assert_eq!(view.retained_env, vec!["TOKEN".to_string()]);
}

/// A diagnostic belongs to exactly the resource native attributes it to.
/// Two MCP definitions share one `mcp.toml`, so the file cannot say which one
/// failed: only the catalog entry keyed by the identity can, and a document
/// that fails as a whole is the document's diagnostic, not any identity's.
#[test]
fn resource_diagnostics_are_attributed_by_identity_never_by_shared_file() {
    use crate::runtime::capability_inspection::{ResourceDiagnosticSubject, ResourceFamily};
    let (root, host, request) = fixture();
    let (manager, _input) = request.session_input(&host).unwrap();
    let user = crate::local_runtime::configuration::settings::SourceTarget::User;
    let resources = root.path().join("home/rustx/.agents");
    std::fs::create_dir_all(resources.join("tools/analysis")).unwrap();
    std::fs::write(
        resources.join("mcp.toml"),
        "[mcp_servers.search]\ncommand = 'server'\n[mcp_servers.broken]\ncommand = 3\n",
    )
    .unwrap();
    let inventory = manager
        .read_source_settings(&user)
        .unwrap()
        .prospective_resources
        .unwrap();
    let subjects = |family: ResourceFamily| {
        inventory
            .resource_diagnostics
            .iter()
            .filter(|diagnostic| match &diagnostic.subject {
                ResourceDiagnosticSubject::Resource { family: owner, .. }
                | ResourceDiagnosticSubject::Collection { family: owner } => *owner == family,
            })
            .map(|diagnostic| diagnostic.subject.clone())
            .collect::<Vec<_>>()
    };
    // Native publishes resolved source paths; the temporary directory may be
    // reached through a symlink (macOS `/var` → `/private/var`).
    let mcp = std::fs::canonicalize(resources.join("mcp.toml")).unwrap();
    let definition = |name: &str| {
        inventory
            .definitions
            .iter()
            .find(|entry| entry.family == ResourceFamily::Mcp && entry.name == name)
            .unwrap()
    };
    // Both identities come from one document; only one of them is invalid.
    assert_eq!(definition("search").location.path, mcp);
    assert_eq!(definition("broken").location.path, mcp);
    assert!(definition("search").valid);
    assert!(!definition("broken").valid);
    assert_eq!(
        subjects(ResourceFamily::Mcp),
        [ResourceDiagnosticSubject::Resource {
            family: ResourceFamily::Mcp,
            name: "broken".into()
        }]
    );
    // A Managed Python package is one identity under its bare name, the same
    // name its definition is published under — not the `python:` source id.
    assert!(
        inventory
            .definitions
            .iter()
            .any(|entry| entry.family == ResourceFamily::ManagedPython && entry.name == "analysis")
    );
    assert_eq!(
        subjects(ResourceFamily::ManagedPython),
        [ResourceDiagnosticSubject::Resource {
            family: ResourceFamily::ManagedPython,
            name: "analysis".into()
        }]
    );

    // A document that does not parse names no identity at all: the failure is
    // the document's, and no identity diagnostic is fabricated from it.
    std::fs::write(&mcp, "invalid = [").unwrap();
    let inventory = manager
        .read_source_settings(&user)
        .unwrap()
        .prospective_resources
        .unwrap();
    let mcp_diagnostics: Vec<_> = inventory
        .resource_diagnostics
        .iter()
        .filter(|diagnostic| {
            matches!(
                diagnostic.subject,
                ResourceDiagnosticSubject::Resource {
                    family: ResourceFamily::Mcp,
                    ..
                } | ResourceDiagnosticSubject::Collection {
                    family: ResourceFamily::Mcp
                }
            )
        })
        .collect();
    assert_eq!(mcp_diagnostics.len(), 1);
    assert_eq!(
        mcp_diagnostics[0].subject,
        ResourceDiagnosticSubject::Collection {
            family: ResourceFamily::Mcp
        }
    );
    assert_eq!(mcp_diagnostics[0].file.as_deref(), Some(mcp.as_path()));
}

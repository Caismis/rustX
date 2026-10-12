//! Issue #464: deterministic repair planning, candidate validation, and the
//! source transaction, against real native parsing, Tool definitions and files.
use super::*;
use crate::local_runtime::configuration::UserConfigSources;
use std::fmt::Write;
use std::os::unix::fs::PermissionsExt;

/// A complete, valid User source: Provider, Model and default selection.
const USER: &str = r#"# User configuration — keep this comment.
[providers.local]
base_url = "http://127.0.0.1:9/v1"
api_key = "SECRET_SENTINEL"

[models."local/a"]
provider = "local"
id = "wire-a"
protocol = "openai_responses"
context_window = 128000
max_output_tokens = 8192
request_params = '{"vendor_x":{"secret_mode":true},"temperature":0.25}'

[models."local/a".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[agent.model]
model = "local/a"
"#;

struct Fixture {
    _root: tempfile::TempDir,
    manager: UserConfigManager,
    workspace: SourceTarget,
}

impl Fixture {
    fn new() -> Self {
        Self::with_config_path(|home| home.join("rustx/rustx.toml"))
    }

    fn with_config_path(config: impl FnOnce(&Path) -> PathBuf) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(home.join("rustx")).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        let config_path = config(&home);
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let manager = UserConfigManager::new(UserConfigSources {
            home_directory: home.clone(),
            config_path,
            runtime_root: root.path().join("state"),
        })
        .unwrap();
        Self {
            workspace: SourceTarget::Workspace {
                directory: workspace.canonicalize().unwrap(),
            },
            _root: root,
            manager,
        }
    }

    fn path(&self, target: &SourceTarget) -> PathBuf {
        self.manager.config_path(target)
    }

    fn write(&self, target: &SourceTarget, text: &str) {
        std::fs::write(self.path(target), text).unwrap();
    }

    fn read(&self, target: &SourceTarget) -> String {
        std::fs::read_to_string(self.path(target)).unwrap()
    }

    fn backups(&self) -> Vec<PathBuf> {
        let directory = self.manager.runtime_root().join(BACKUP_DIRECTORY);
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut paths: Vec<_> = entries.map(|entry| entry.unwrap().path()).collect();
        paths.sort();
        paths
    }

    fn receipt(&self, target: &SourceTarget) -> Option<ConfigRepairReceipt> {
        self.manager.repairs().latest(target)
    }

    fn sequence(&self) -> u64 {
        *self.manager.repairs().subscribe().borrow()
    }
}

fn changes(text: &str) -> Vec<ConfigRepairChange> {
    plan(text.as_bytes()).unwrap().changes
}

fn candidate(text: &str) -> String {
    String::from_utf8(plan(text.as_bytes()).unwrap().candidate).unwrap()
}

fn removed(path: &str) -> ConfigRepairChange {
    ConfigRepairChange::RemovedField {
        field_path: path.into(),
    }
}

fn tool(name: &str, reason: NativeToolRemoval) -> ConfigRepairChange {
    ConfigRepairChange::RemovedNativeTool {
        name: name.into(),
        reason,
    }
}

#[test]
fn the_issue_example_converges_to_its_documented_result() {
    let before = "schema_version = 8\n\n[agent.tools]\nbuiltin = [\"read\", \"old_tool\", \"grep\", \"read\"]\n\n[agent.plugins.todo]\nenabled = true\nunknown_permission = true\n\n[native_tools.old_tool]\napproval = \"always\"\n";
    let plan = plan(before.as_bytes()).unwrap();
    assert_eq!(
        String::from_utf8(plan.candidate.clone()).unwrap(),
        "schema_version = 10\n\n[agent.tools]\nbuiltin = [\"read\", \"grep\"]\n\n[agent.plugins.todo]\nenabled = true\n"
    );
    assert_eq!(
        plan.changes,
        vec![
            ConfigRepairChange::UpdatedSchemaVersion { from: 8, to: 10 },
            removed("agent.plugins.todo.unknown_permission"),
            removed("native_tools.old_tool"),
            tool("old_tool", NativeToolRemoval::Unknown),
            tool("read", NativeToolRemoval::Duplicate),
        ]
    );
    // One finite pass reaches the fixed point.
    assert!(changes(std::str::from_utf8(&plan.candidate).unwrap()).is_empty());
}

#[test]
fn the_rule_anchors_are_typed_source_schema_fields() {
    // The two fields repair edits by name are members of the generated schema.
    let schema = source_schema();
    assert!(schema["properties"][SCHEMA_VERSION_FIELD].is_object());
    let [agent, tools, builtin] = BUILTIN_SELECTION_PATH;
    assert!(schema["properties"][agent].is_object());
    assert!(schema["$defs"]["AgentProfileLayer"]["properties"][tools].is_object());
    assert!(schema["$defs"]["ToolsLayer"]["properties"][builtin].is_object());
}

#[test]
fn nested_unknown_fields_and_tables_go_in_one_pass_without_security_exemptions() {
    let text = r#"unknown_root = 1
[security]
sandbox = "off"
[security.network]
allow = ["*"]

[native_tools.bash]
approval = "always"
allow_unsandboxed = true

[agent.plugins.goal]
enabled = true
permissions = { escalate = true }

[agent.model]
model = "local/a"
[agent.model.max_output_tokens]
mode = "limit"
tokens = 512
bypass = true

[subagents]
max_concurrent = 2
isolation = "none"

[mcp_tool_policies.docs]
approval = "never"
trust_all = true
"#;
    let repaired = candidate(text);
    assert_eq!(
        changes(text),
        vec![
            removed("unknown_root"),
            removed("security"),
            removed("native_tools.bash.allow_unsandboxed"),
            removed("agent.plugins.goal.permissions"),
            removed("agent.model.max_output_tokens.bypass"),
            removed("subagents.isolation"),
            removed("mcp_tool_policies.docs.trust_all"),
        ]
    );
    for gone in [
        "unknown_root",
        "security",
        "allow_unsandboxed",
        "permissions",
        "bypass",
        "isolation",
        "trust_all",
    ] {
        assert!(!repaired.contains(gone), "{gone} survived:\n{repaired}");
    }
    // Known policy values stay exactly as authored.
    for kept in [
        "approval = \"always\"",
        "approval = \"never\"",
        "max_concurrent = 2",
        "tokens = 512",
    ] {
        assert!(repaired.contains(kept), "{kept} was lost:\n{repaired}");
    }
    let layer: RuntimeLayer = crate::toml_authoring::parse(repaired.as_bytes()).unwrap();
    assert!(layer.native_tools.unwrap().bash.is_some());
}

#[test]
fn dynamic_identities_and_opaque_request_params_are_never_fields() {
    let text = format!(
        r#"{USER}
[models."local/a".profiles."fast-🚀"]
reasoning_enabled = false
request_params = '{{"unknown_vendor_key":{{"nested":[1,2]}},"x-api-key":"SECRET_PARAM"}}'

[agent.tools.sources]
"mcp:docs" = ["search", "fetch"]

[environment]
CUSTOM_TOKEN = "SECRET_ENV"
"weird.name" = "v"

[mcp_tool_policies."docs"]
approval = "always"
"#
    );
    assert!(changes(&text).is_empty(), "{:?}", changes(&text));
    // Opaque JSON keys are data even beside a removed schema field.
    let with_unknown = format!("{text}\n[models.\"local/a\".compat]\nfuture_flag = true\n");
    assert_eq!(
        changes(&with_unknown),
        vec![removed("models.\"local/a\".compat.future_flag")]
    );
    let repaired = candidate(&with_unknown);
    assert!(repaired.starts_with(&text), "{repaired}");
    for exact in [
        r#"request_params = '{"vendor_x":{"secret_mode":true},"temperature":0.25}'"#,
        r#"request_params = '{"unknown_vendor_key":{"nested":[1,2]},"x-api-key":"SECRET_PARAM"}'"#,
        "CUSTOM_TOKEN = \"SECRET_ENV\"",
        "api_key = \"SECRET_SENTINEL\"",
        "# User configuration — keep this comment.",
    ] {
        assert!(repaired.contains(exact), "{exact}");
    }
}

#[test]
fn comments_and_unrelated_bytes_survive_deletions() {
    let text = "# head comment\nschema_version = 10 # trailing\n\n# before agent\n[agent]\ndescription = 'kept'   # spaced comment\nstale = 'x'\n\n[agent.tools]\n# list comment\nbuiltin = [\n  \"read\", # first\n  \"nope\",\n  \"grep\",\n]\n";
    let repaired = candidate(text);
    assert_eq!(
        repaired,
        "# head comment\nschema_version = 10 # trailing\n\n# before agent\n[agent]\ndescription = 'kept'   # spaced comment\n\n[agent.tools]\n# list comment\nbuiltin = [\n  \"read\", # first\n  \"grep\",\n]\n"
    );
}

#[test]
fn native_tool_entries_are_classified_by_the_native_domain() {
    use crate::capabilities::selection::{BuiltinSelectionClass, classify_builtin_selection};
    let ordinary: Vec<String> = crate::tools::native::definitions(
        crate::tools::native::NativeToolPolicies::default(),
        &crate::runtime::subagent::AgentCatalog::default(),
    )
    .into_iter()
    .map(|(definition, _)| definition.name)
    .collect();
    assert!(
        ordinary
            .iter()
            .all(|name| classify_builtin_selection(name) == BuiltinSelectionClass::Ordinary)
    );
    let controls = crate::tools::native::AGENT_CONTROL_TOOL_NAMES;
    let plugins: Vec<&str> = std::iter::once(crate::tools::native::TODO_TOOL_NAME)
        .chain(crate::tools::native::GOAL_TOOL_NAMES.iter().copied())
        .collect();
    let mut selection: Vec<String> = ordinary.clone();
    selection.extend(controls.iter().map(|name| (*name).to_owned()));
    selection.extend(plugins.iter().map(|name| (*name).to_owned()));
    selection.push(ordinary[0].clone());
    selection.push("not_a_native_tool".into());
    selection.push(String::new());
    let text = format!(
        "[agent.tools]\nbuiltin = [{}]\n",
        selection
            .iter()
            .map(|name| format!("{name:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut expected: Vec<_> = controls
        .iter()
        .map(|name| tool(name, NativeToolRemoval::AgentControl))
        .collect();
    expected.extend(
        plugins
            .iter()
            .map(|name| tool(name, NativeToolRemoval::PluginOwned)),
    );
    expected.push(tool(&ordinary[0], NativeToolRemoval::Duplicate));
    expected.push(tool("not_a_native_tool", NativeToolRemoval::Unknown));
    expected.push(tool("", NativeToolRemoval::Unknown));
    assert_eq!(changes(&text), expected);
    let repaired: RuntimeLayer = crate::toml_authoring::parse(candidate(&text).as_bytes()).unwrap();
    assert_eq!(
        repaired.agent.unwrap().tools.unwrap().builtin.unwrap(),
        ordinary
    );
    // A non-string element is a known-field type error, never a selection edit.
    assert!(changes("[agent.tools]\nbuiltin = [\"read\", 3, \"read\"]\n").is_empty());
}

#[test]
fn explicit_schema_versions_are_normalized_and_omission_is_preserved() {
    for from in [0_i64, 8, 9, 11, 99, -1] {
        let text = format!("schema_version = {from} # authored\n");
        assert_eq!(
            changes(&text),
            vec![ConfigRepairChange::UpdatedSchemaVersion { from, to: 10 }]
        );
        assert_eq!(candidate(&text), "schema_version = 10 # authored\n");
    }
    assert!(changes("schema_version = 10\n").is_empty());
    assert!(changes("[agent]\ndescription = 'x'\n").is_empty());
    assert!(!candidate("[agent]\nstale = 1\n").contains("schema_version"));
    // A non-integer version is a type error, not version metadata.
    for wrong in ["'10'", "10.0", "true"] {
        assert!(changes(&format!("schema_version = {wrong}\n")).is_empty());
    }
}

#[test]
fn tagged_unions_close_only_their_selected_variant() {
    let text = "[agent.model]\nmodel = 'local/a'\n[agent.model.summary_model]\nmode = 'explicit'\nmodel = 'local/a'\nlegacy = true\n[tool_deadline_policy.idle_liveness_ms]\nmode = 'window'\nmilliseconds = 5\nextra = 1\n";
    assert_eq!(
        changes(text),
        vec![
            removed("agent.model.summary_model.legacy"),
            removed("tool_deadline_policy.idle_liveness_ms.extra"),
        ]
    );
    // An unknown tag selects no variant: nothing under it is guessed.
    assert!(changes("[context.summary_output_cap]\nmode = 'future'\nextra = 1\n").is_empty());
}

#[test]
fn syntax_and_encoding_are_never_edited() {
    assert_eq!(
        plan(b"[agent\nmodel = 1").unwrap_err(),
        Uneditable::Syntax {
            line: Some(1),
            column: Some(7)
        }
    );
    assert_eq!(plan(b"\xff\xfe").unwrap_err(), Uneditable::NotUtf8);
}

#[test]
fn identical_rules_apply_to_user_and_workspace_sources() {
    let fixture = Fixture::new();
    let text = format!("legacy = true\n{USER}[agent.plugins.todo]\nenabled = true\nstale = 1\n");
    for target in [SourceTarget::User, fixture.workspace.clone()] {
        fixture.write(&target, &text);
        assert_eq!(
            fixture.manager.repair_source(&target, &target),
            RepairOutcome::Committed
        );
        let receipt = fixture.receipt(&target).unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
        assert_eq!(receipt.target, target);
        assert_eq!(
            receipt.changes,
            vec![removed("legacy"), removed("agent.plugins.todo.stale")]
        );
    }
    assert_eq!(
        fixture.read(&SourceTarget::User),
        fixture.read(&fixture.workspace)
    );
}

#[test]
fn a_process_bound_config_override_is_repaired_in_place() {
    let fixture = Fixture::with_config_path(|home| home.join("elsewhere/custom-name.toml"));
    fixture.write(&SourceTarget::User, &format!("{USER}\nlegacy = 1\n"));
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    assert_eq!(fixture.read(&SourceTarget::User), USER);
    assert!(
        fixture
            .path(&SourceTarget::User)
            .ends_with("elsewhere/custom-name.toml")
    );
    assert_eq!(fixture.backups().len(), 1);
}

#[test]
fn a_committed_repair_keeps_one_exact_private_backup_outside_the_workspace() {
    let fixture = Fixture::new();
    let original = format!("{USER}\n[agent.plugins.todo]\nenabled = true\nlegacy = 'SECRET_X'\n");
    fixture.write(&fixture.workspace, &original);
    fixture.write(&SourceTarget::User, USER);
    assert_eq!(
        fixture
            .manager
            .repair_source(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Committed
    );
    let backups = fixture.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read(&backups[0]).unwrap(), original.as_bytes());
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&backups[0]), 0o600);
    assert_eq!(mode(backups[0].parent().unwrap()), 0o700);
    let directory = fixture.workspace.workspace().unwrap();
    assert!(!backups[0].starts_with(directory));
    let name = backups[0].file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("workspace-"), "{name}");
    assert!(!name.contains("workspace/"), "{name}");
    let receipt = fixture.receipt(&fixture.workspace).unwrap();
    assert!(receipt.backup_available);
    assert_eq!(
        receipt.after_revision.as_deref(),
        Some(
            super::super::super::settings::revision(Some(
                fixture.read(&fixture.workspace).as_bytes()
            ))
            .as_str()
        )
    );
    // The receipt never carries the removed value.
    assert!(!serde_json::to_string(&receipt).unwrap().contains("SECRET"));
}

#[test]
fn healthy_and_repaired_sources_are_disk_and_ledger_no_ops() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, USER);
    let modified = std::fs::metadata(fixture.path(&SourceTarget::User))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Unchanged
    );
    assert_eq!(fixture.read(&SourceTarget::User), USER);
    assert_eq!(
        std::fs::metadata(fixture.path(&SourceTarget::User))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    assert!(fixture.backups().is_empty());
    assert!(fixture.receipt(&SourceTarget::User).is_none());
    // A healthy check takes no writer lock, so it creates no lock file.
    assert!(
        !fixture
            .path(&SourceTarget::User)
            .with_file_name(".rustx.toml.lock")
            .exists()
    );
    assert_eq!(fixture.sequence(), 0);

    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    let repaired = fixture.read(&SourceTarget::User);
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    let sequence = fixture.sequence();
    for _ in 0..3 {
        assert_eq!(
            fixture
                .manager
                .repair_source(&SourceTarget::User, &SourceTarget::User),
            RepairOutcome::Unchanged
        );
    }
    assert_eq!(fixture.read(&SourceTarget::User), repaired);
    assert_eq!(fixture.backups().len(), 1);
    assert_eq!(fixture.receipt(&SourceTarget::User).unwrap(), receipt);
    assert_eq!(fixture.sequence(), sequence);
}

#[test]
fn nonrepairable_sources_stay_unchanged_with_a_safe_unresolved_receipt() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, USER);
    let cases = [
        // Syntax is never edited.
        format!("{USER}legacy = 1\n[broken\n"),
        // A known field with the wrong type.
        format!("{USER}legacy = 1\n[subagents]\nmax_concurrent = 'SECRET_TYPE'\n"),
        // Invalid request_params JSON.
        USER.replace(
            r#"'{"vendor_x":{"secret_mode":true},"temperature":0.25}'"#,
            r#"'{"SECRET_JSON": }'"#,
        ) + "legacy = 1\n",
        // A broken Model reference.
        USER.replace("model = \"local/a\"", "model = \"local/missing\"") + "legacy = 1\n",
        // A missing required Model fact.
        USER.replace("context_window = 128000\n", "") + "legacy = 1\n",
        // A wrong-type version cannot be normalized.
        format!("schema_version = 'SECRET_VERSION'\n{USER}legacy = 1\n"),
    ];
    for text in &cases {
        fixture.write(&fixture.workspace, text);
        assert_eq!(
            fixture
                .manager
                .repair_source(&fixture.workspace, &fixture.workspace),
            RepairOutcome::Unchanged,
            "{text}"
        );
        assert_eq!(&fixture.read(&fixture.workspace), text);
        let receipt = fixture.receipt(&fixture.workspace).unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Unresolved, "{text}");
        assert!(receipt.after_revision.is_none());
        assert!(!receipt.backup_available);
        let wire = serde_json::to_string(&receipt).unwrap();
        assert!(!wire.contains("SECRET"), "{wire}");
        assert!(receipt.diagnostic.is_some());
    }
    assert!(fixture.backups().is_empty());
    // Workspace process policy is a source restriction, not an unknown field.
    fixture.write(
        &fixture.workspace,
        "[app_server]\nmax_connections = 4\n[agent]\nlegacy = 1\n",
    );
    assert_eq!(
        fixture
            .manager
            .repair_source(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Unchanged
    );
    let receipt = fixture.receipt(&fixture.workspace).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
    assert!(receipt.diagnostic.unwrap().contains("app_server"));
}

#[test]
fn a_partial_source_is_incomplete_not_invalid() {
    let fixture = Fixture::new();
    // A User source with a catalog but no default selection is complete enough.
    let user = USER.replace("[agent.model]\nmodel = \"local/a\"\n", "");
    fixture.write(&SourceTarget::User, &format!("{user}legacy = 1\n"));
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    // A Workspace selecting a User-defined Model validates over the User.
    fixture.write(
        &fixture.workspace,
        "[agent.model]\nmodel = \"local/a\"\nstale = true\n",
    );
    assert_eq!(
        fixture
            .manager
            .repair_source(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Committed
    );
    assert_eq!(
        fixture.read(&fixture.workspace),
        "[agent.model]\nmodel = \"local/a\"\n"
    );
}

#[test]
fn a_source_found_healthy_clears_its_unresolved_receipt() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, "[broken");
    fixture
        .manager
        .repair_source(&SourceTarget::User, &SourceTarget::User);
    assert_eq!(
        fixture.receipt(&SourceTarget::User).unwrap().status,
        ConfigRepairStatus::Unresolved
    );
    let sequence = fixture.sequence();
    // The same invalid bytes are the same observation.
    fixture
        .manager
        .repair_source(&SourceTarget::User, &SourceTarget::User);
    assert_eq!(fixture.sequence(), sequence);
    fixture.write(&SourceTarget::User, USER);
    fixture
        .manager
        .refresh_repair_observation(&SourceTarget::User);
    assert!(fixture.receipt(&SourceTarget::User).is_none());
    assert_eq!(fixture.sequence(), sequence + 1);
    assert_eq!(
        fixture
            .manager
            .repairs()
            .changed_after(&SourceKey::User, sequence),
        Some((sequence + 1, None))
    );
}

#[test]
fn user_and_workspace_repairs_are_independent_transactions() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    fixture.write(&fixture.workspace, "[agent\n");
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    assert_eq!(
        fixture
            .manager
            .repair_source(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Unchanged
    );
    assert_eq!(
        fixture.receipt(&SourceTarget::User).unwrap().status,
        ConfigRepairStatus::Repaired
    );
    assert_eq!(
        fixture.receipt(&fixture.workspace).unwrap().status,
        ConfigRepairStatus::Unresolved
    );
    assert_eq!(fixture.read(&fixture.workspace), "[agent\n");
}

#[test]
fn an_external_edit_before_the_writer_lock_supersedes_the_candidate() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    let path = fixture.path(&SourceTarget::User);
    let external = format!("{USER}# edited concurrently\n");
    let edit = external.clone();
    fixture
        .manager
        .test_hooks
        .insert("repair_before_lock", move || {
            std::fs::write(&path, edit).unwrap();
        });
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Superseded
    );
    assert_eq!(fixture.read(&SourceTarget::User), external);
    assert!(fixture.backups().is_empty());
    assert!(fixture.receipt(&SourceTarget::User).is_none());
}

#[test]
fn an_external_edit_before_publication_is_never_overwritten() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    let path = fixture.path(&SourceTarget::User);
    let external = format!("{USER}# edited concurrently\n");
    let edit = external.clone();
    fixture
        .manager
        .test_hooks
        .insert("before_publication", move || {
            std::fs::write(&path, edit).unwrap();
        });
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Superseded
    );
    assert_eq!(fixture.read(&SourceTarget::User), external);
    assert!(fixture.receipt(&SourceTarget::User).is_none());
    // The next observation plans from the external bytes; nothing is replayed.
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Unchanged
    );
    assert_eq!(fixture.read(&SourceTarget::User), external);
}

#[test]
fn failures_before_publication_leave_the_source_unchanged() {
    for point in ["backup", "stage_write", "stage_sync", "replace_refused"] {
        let fixture = Fixture::new();
        let original = format!("{USER}legacy = 1\n");
        fixture.write(&SourceTarget::User, &original);
        fixture.manager.test_hooks.fail_once(point);
        assert_eq!(
            fixture
                .manager
                .repair_source(&SourceTarget::User, &SourceTarget::User),
            RepairOutcome::Unchanged,
            "{point}"
        );
        assert_eq!(fixture.read(&SourceTarget::User), original, "{point}");
        let receipt = fixture.receipt(&SourceTarget::User).unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Unresolved, "{point}");
        assert_eq!(receipt.backup_available, point != "backup", "{point}");
        assert!(receipt.after_revision.is_none());
        if point == "backup" {
            assert!(fixture.backups().is_empty());
        }
    }
}

#[test]
fn an_uncertain_publication_is_reported_and_never_replayed() {
    for point in ["directory_sync", "replace"] {
        let fixture = Fixture::new();
        fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
        fixture.manager.test_hooks.fail_once(point);
        assert_eq!(
            fixture
                .manager
                .repair_source(&SourceTarget::User, &SourceTarget::User),
            RepairOutcome::Committed,
            "{point}"
        );
        let published = fixture.read(&SourceTarget::User);
        assert_eq!(published, USER, "{point}");
        let receipt = fixture.receipt(&SourceTarget::User).unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::CommitUncertain);
        assert_eq!(
            receipt.after_revision,
            Some(super::super::super::settings::revision(Some(
                published.as_bytes()
            )))
        );
        let sequence = fixture.sequence();
        // A later observation sees healthy bytes: no write, no new receipt.
        assert_eq!(
            fixture
                .manager
                .repair_source(&SourceTarget::User, &SourceTarget::User),
            RepairOutcome::Unchanged
        );
        assert_eq!(fixture.read(&SourceTarget::User), published);
        assert_eq!(fixture.backups().len(), 1);
        assert_eq!(fixture.sequence(), sequence);
    }
}

#[test]
fn receipts_are_bounded_and_sanitized() {
    let fixture = Fixture::new();
    let mut text = USER.to_owned();
    for index in 0..(MAX_RECEIPT_CHANGES + 6) {
        writeln!(text, "legacy_{index} = 1").unwrap();
    }
    writeln!(text, "\"{}\\u0007\" = 1", "k".repeat(300)).unwrap();
    fixture.write(&SourceTarget::User, &text);
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.changed_count as usize, MAX_RECEIPT_CHANGES + 7);
    assert_eq!(receipt.changes.len(), MAX_RECEIPT_CHANGES);
    assert!(receipt.truncated);
    let all = plan(text.as_bytes()).unwrap().changes;
    let ConfigRepairChange::RemovedField { field_path } = all.last().unwrap() else {
        panic!("expected a removed field");
    };
    let segment = field_path.strip_prefix("agent.model.").unwrap();
    assert_eq!(segment.chars().count(), MAX_SEGMENT_CHARS);
    assert!(segment.ends_with('…'));
    assert!(!field_path.chars().any(char::is_control));
    // A short key keeps its escaped control character visible but inert.
    assert_eq!(
        changes("\"a\\u0007b\" = 1\n"),
        vec![removed("\"a\\u0007b\"")]
    );
}

#[test]
fn the_ledger_forgets_the_least_recent_workspace_beyond_its_bound() {
    let ledger = RepairLedger::default();
    let observation = |index: usize| {
        receipt(
            &SourceTarget::Workspace {
                directory: PathBuf::from(format!("/w/{index}")),
            },
            "sha256:before",
            Observation {
                status: ConfigRepairStatus::Unresolved,
                after_revision: None,
                changes: Vec::new(),
                backup_available: false,
                diagnostic: None,
            },
        )
    };
    ledger.record(receipt(
        &SourceTarget::User,
        "sha256:user",
        Observation {
            status: ConfigRepairStatus::Repaired,
            after_revision: Some("sha256:after".into()),
            changes: Vec::new(),
            backup_available: true,
            diagnostic: None,
        },
    ));
    for index in 0..=MAX_LEDGER_WORKSPACES {
        ledger.record(observation(index));
    }
    assert!(ledger.latest(&SourceTarget::User).is_some());
    assert!(
        ledger
            .latest(&SourceTarget::Workspace {
                directory: "/w/0".into()
            })
            .is_none()
    );
    assert!(
        ledger
            .latest(&SourceTarget::Workspace {
                directory: format!("/w/{MAX_LEDGER_WORKSPACES}").into()
            })
            .is_some()
    );
}

#[test]
fn array_removals_keep_neighbouring_layout_and_comments() {
    for (before, after) in [
        (r#"["nope", "read", "grep"]"#, r#"["read", "grep"]"#),
        (r#"["read", "nope"]"#, r#"["read"]"#),
        (r#"[ "read" , "nope" , "grep" ]"#, r#"[ "read" , "grep" ]"#),
        (
            "[\n  \"nope\", # about nope\n  # about read\n  \"read\", # first\n  \"bad\", # about bad\n  \"grep\", # last ok\n  \"gone\", # about gone\n]",
            "[\n  # about read\n  \"read\", # first\n  \"grep\", # last ok\n]",
        ),
    ] {
        let text = format!("[agent.tools]\nbuiltin = {before}\n");
        assert_eq!(
            candidate(&text),
            format!("[agent.tools]\nbuiltin = {after}\n"),
            "{before}"
        );
    }
}

/// A Workspace that defines a Provider and Model the User selects.
const WORKSPACE_MODEL: &str = r#"[providers.workspace-provider]
base_url = "https://example.com/v1"
api_key = "$MODEL_API_KEY"

[models."workspace-provider/model-a"]
provider = "workspace-provider"
id = "model-a"
protocol = "openai_responses"
context_window = 128000
max_output_tokens = 8192

[models."workspace-provider/model-a".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false
"#;

const USER_SELECTING_WORKSPACE_MODEL: &str =
    "legacy_field = true\n\n# keep\n[agent.model]\nmodel = \"workspace-provider/model-a\"\n";

fn unresolved_diagnostic(fixture: &Fixture, target: &SourceTarget) -> String {
    let receipt = fixture.receipt(target).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
    assert!(receipt.after_revision.is_none());
    assert!(!receipt.backup_available);
    receipt.diagnostic.unwrap()
}

#[test]
fn semantic_diagnostics_never_echo_authored_values_or_identities() {
    let provider = |name: &str, url: &str| {
        format!("[providers.{name}]\nbase_url = \"{url}\"\napi_key = \"$MODEL_API_KEY\"\n")
    };
    let model = |key: &str, extra: &str| {
        format!(
            "[models.\"{key}\"]\nprovider = \"local\"\nid = \"wire\"\nprotocol = \"openai_responses\"\ncontext_window = 128000\nmax_output_tokens = 8192\n{extra}[models.\"{key}\".capabilities]\ninput_modalities = [\"text\"]\noutput_modalities = [\"text\"]\ntool_calls = true\nreasoning = false\n"
        )
    };
    let local = provider("local", "https://example.com/v1");
    // (source, expected safe location in the diagnostic)
    let cases = [
        // Provider URL carrying a credential-like query.
        (
            provider("local", "https://example.com/?token=SECRET_URL"),
            "providers.*.base_url",
        ),
        // A Provider identity is never quoted, valid or not.
        (
            provider("SECRET_PROVIDER", "ftp://SECRET_HOST/"),
            "providers.*.base_url",
        ),
        (
            provider("\"SECRET PROVIDER\"", "https://example.com/v1"),
            "providers",
        ),
        // Model identities and references.
        (
            format!("{local}{}", model("local/SECRET MODEL", "")),
            "models",
        ),
        (
            format!(
                "{local}{}",
                model("local/SECRET_MODEL", "")
                    .replace("max_output_tokens = 8192", "max_output_tokens = 999999")
            ),
            "models.*",
        ),
        (
            format!(
                "{local}{}",
                model(
                    "local/SECRET_PARAMS",
                    "request_params = '{\"model\":\"SECRET_VALUE\"}'\n"
                )
            ),
            "models.*.request_params",
        ),
        // A request_params parse failure under a Model identity.
        (
            format!(
                "{local}{}",
                model(
                    "local/SECRET_JSON",
                    "request_params = '{\"SECRET_KEY\": }'\n"
                )
            ),
            "models.*.request_params",
        ),
        // Invalid known-field values.
        (
            "[environment]\n\"SECRET=ENV\" = \"SECRET_LITERAL\"\n".to_owned(),
            "environment",
        ),
        (
            "[agent]\nagents = [\"sentinelagent\", \"sentinelagent\"]\n".to_owned(),
            "agent.agents",
        ),
    ];
    for (source, location) in &cases {
        let fixture = Fixture::new();
        let text = format!("legacy_field = true\n{source}");
        fixture.write(&SourceTarget::User, &text);
        assert_eq!(
            fixture
                .manager
                .repair_source(&SourceTarget::User, &SourceTarget::User),
            RepairOutcome::Unchanged,
            "{text}"
        );
        assert_eq!(fixture.read(&SourceTarget::User), text);
        let diagnostic = unresolved_diagnostic(&fixture, &SourceTarget::User);
        assert!(
            diagnostic.contains(&format!(" at {location}")),
            "{diagnostic} / {text}"
        );
        let wire = serde_json::to_string(&fixture.receipt(&SourceTarget::User)).unwrap();
        for sentinel in ["SECRET", "sentinelagent", "example.com", "MODEL_API_KEY"] {
            assert!(!wire.contains(sentinel), "{wire}");
        }
        assert!(fixture.backups().is_empty());
    }
}

#[test]
fn a_composition_reference_diagnostic_names_its_field_only() {
    // A Model reference resolved against the composition: the selected
    // reference is named by its field, never quoted.
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, USER);
    let text = "legacy_field = true\n[agent.model]\nmodel = \"local/SECRET_MISSING\"\n";
    fixture.write(&fixture.workspace, text);
    assert_eq!(
        fixture
            .manager
            .repair_source(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Unchanged
    );
    let diagnostic = unresolved_diagnostic(&fixture, &fixture.workspace);
    assert!(diagnostic.contains(" at agent.model"), "{diagnostic}");
    assert!(
        !serde_json::to_string(&fixture.receipt(&fixture.workspace))
            .unwrap()
            .contains("SECRET")
    );
}

#[test]
fn a_user_may_select_a_model_its_workspace_defines() {
    // At startup no Workspace is known: the reference is a missing external
    // fact, never an intrinsic failure.
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, USER_SELECTING_WORKSPACE_MODEL);
    fixture.write(&fixture.workspace, WORKSPACE_MODEL);
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    let expected = "\n# keep\n[agent.model]\nmodel = \"workspace-provider/model-a\"\n";
    assert_eq!(fixture.read(&SourceTarget::User), expected);
    assert_eq!(fixture.read(&fixture.workspace), WORKSPACE_MODEL);
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
    assert_eq!(receipt.changes, vec![removed("legacy_field")]);
    assert!(fixture.receipt(&fixture.workspace).is_none());
    // The composition Session resolution builds is complete and valid.
    let directory = fixture.workspace.workspace().unwrap().to_path_buf();
    let session = fixture
        .manager
        .resolve_session(&super::super::SessionConfigInput::new(directory))
        .unwrap();
    assert_eq!(
        session.config().initial_model().model.to_string(),
        "workspace-provider/model-a"
    );

    // With that Workspace selected, the composition is checked and passes.
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, USER_SELECTING_WORKSPACE_MODEL);
    fixture.write(&fixture.workspace, WORKSPACE_MODEL);
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &fixture.workspace),
        RepairOutcome::Committed
    );
    assert_eq!(fixture.read(&SourceTarget::User), expected);
}

#[test]
fn an_available_composition_is_checked_like_session_resolution() {
    // The User's context reserve cannot fit the Workspace Model's window:
    // decidable only once both scopes are known.
    let user = format!("{USER_SELECTING_WORKSPACE_MODEL}[context]\nreserve_tokens = 127000\n");
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, &user);
    fixture.write(&fixture.workspace, WORKSPACE_MODEL);
    let directory = fixture.workspace.workspace().unwrap().to_path_buf();
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &fixture.workspace),
        RepairOutcome::Unchanged
    );
    assert_eq!(fixture.read(&SourceTarget::User), user);
    let diagnostic = unresolved_diagnostic(&fixture, &SourceTarget::User);
    assert!(
        diagnostic.contains("context budgets cannot fit the selected models at context"),
        "{diagnostic}"
    );
    assert!(fixture.backups().is_empty());
    // Session resolution rejects the same composition for the same reason.
    std::fs::write(
        fixture.path(&SourceTarget::User),
        user.replace("legacy_field = true\n", ""),
    )
    .unwrap();
    let failure = fixture
        .manager
        .resolve_session(&super::super::SessionConfigInput::new(directory))
        .unwrap_err();
    assert_eq!(failure.diagnostic.path, "context");
    assert!(!failure.incomplete);
}

#[test]
fn independent_constraints_hold_without_a_default_model() {
    for source in [
        "legacy_field = true\n[subagents]\nmax_concurrent = 0\n",
        "legacy_field = true\n[tool_deadline_policy]\nhard_deadline_ms = 0\n",
    ] {
        for (target, scope) in [
            (SourceTarget::User, SourceTarget::User),
            (Fixture::new().workspace, SourceTarget::User),
        ] {
            let fixture = Fixture::new();
            let (target, scope) = match target {
                SourceTarget::User => (SourceTarget::User, scope),
                SourceTarget::Workspace { .. } => {
                    (fixture.workspace.clone(), fixture.workspace.clone())
                }
            };
            fixture.write(&target, source);
            let applications = fixture.sequence();
            assert_eq!(
                fixture.manager.repair_source(&target, &scope),
                RepairOutcome::Unchanged,
                "{source}"
            );
            assert_eq!(fixture.read(&target), source);
            let diagnostic = unresolved_diagnostic(&fixture, &target);
            assert!(
                diagnostic.contains("automatic repair never corrects an invalid value"),
                "{diagnostic}"
            );
            assert!(fixture.backups().is_empty());
            assert_eq!(fixture.sequence(), applications + 1);
        }
    }
    // Resolution agrees: invalid, never merely incomplete.
    let fixture = Fixture::new();
    fixture.write(&fixture.workspace, "[subagents]\nmax_concurrent = 0\n");
    let directory = fixture.workspace.workspace().unwrap().to_path_buf();
    let failure = fixture
        .manager
        .resolve_session(&super::super::SessionConfigInput::new(directory.clone()))
        .unwrap_err();
    assert!(!failure.incomplete);
    assert_eq!(failure.diagnostic.path, "subagents.max_concurrent");
    // And so does source inspection.
    let settings = fixture
        .manager
        .read_source_settings(&fixture.workspace)
        .unwrap();
    assert!(
        settings
            .prospective_diagnostic
            .unwrap()
            .contains("at subagents.max_concurrent")
    );
    // Without the invalid value the same partial composition is incomplete.
    fixture.write(&fixture.workspace, "[subagents]\nmax_concurrent = 2\n");
    let failure = fixture
        .manager
        .resolve_session(&super::super::SessionConfigInput::new(directory))
        .unwrap_err();
    assert!(failure.incomplete);
}

#[test]
fn a_partial_intrinsically_valid_source_is_repaired_without_inventing_facts() {
    let fixture = Fixture::new();
    let text = "legacy_field = true\n[agent]\ninstructions = \"Be brief.\"\n";
    fixture.write(&SourceTarget::User, text);
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    assert_eq!(
        fixture.read(&SourceTarget::User),
        "[agent]\ninstructions = \"Be brief.\"\n"
    );
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.changes, vec![removed("legacy_field")]);
}

#[test]
fn a_planted_backup_symlink_is_refused_before_publication() {
    let fixture = Fixture::new();
    let text = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &text);
    let root = fixture.manager.runtime_root().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    let workspace = fixture.workspace.workspace().unwrap().to_path_buf();
    std::os::unix::fs::symlink(&workspace, root.join(BACKUP_DIRECTORY)).unwrap();
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Unchanged
    );
    assert_eq!(fixture.read(&SourceTarget::User), text);
    assert!(
        unresolved_diagnostic(&fixture, &SourceTarget::User).contains("could not be backed up")
    );
    assert_eq!(std::fs::read_dir(&workspace).unwrap().count(), 0);
}

#[test]
fn a_backup_directory_replaced_after_opening_cannot_redirect_the_copy() {
    let fixture = Fixture::new();
    let text = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &text);
    let root = fixture.manager.runtime_root().to_path_buf();
    let workspace = fixture.workspace.workspace().unwrap().to_path_buf();
    let (directory, moved, target) = (
        root.join(BACKUP_DIRECTORY),
        root.join("moved-backups"),
        workspace.clone(),
    );
    // Between opening the directory and writing through it, the name is
    // renamed away and replaced by a symlink into the Workspace.
    fixture
        .manager
        .test_hooks
        .insert("backup_directory_opened", move || {
            std::fs::rename(&directory, &moved).unwrap();
            std::os::unix::fs::symlink(&target, &directory).unwrap();
        });
    assert_eq!(
        fixture
            .manager
            .repair_source(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    // Nothing reached the Workspace; the exact private copy is in the
    // directory that was opened.
    assert_eq!(std::fs::read_dir(&workspace).unwrap().count(), 0);
    let copies: Vec<_> = std::fs::read_dir(root.join("moved-backups"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(std::fs::read(&copies[0]).unwrap(), text.as_bytes());
    assert_eq!(
        std::fs::metadata(&copies[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        fixture
            .receipt(&SourceTarget::User)
            .unwrap()
            .backup_available
    );
}

#[test]
fn an_unsupported_user_version_is_invalid_in_projection_and_resolution_alike() {
    let fixture = Fixture::new();
    let text = format!("schema_version = 9\n{USER}");
    fixture.write(&SourceTarget::User, &text);
    let settings = fixture
        .manager
        .read_source_settings(&fixture.workspace)
        .unwrap();
    assert!(settings.user.authored.is_none());
    assert!(settings.user.diagnostic.unwrap().contains("schema_version"));
    assert!(settings.resolved.is_none());
    assert!(settings.prospective_diagnostic.is_some());
    assert!(
        fixture.receipt(&SourceTarget::User).is_none(),
        "reads are read-only"
    );
    assert_eq!(fixture.read(&SourceTarget::User), text);
    // Manual repair stays available; afterwards both views agree again.
    let repaired = fixture
        .manager
        .write_source_settings(
            &SourceTarget::User,
            &settings.user.revision,
            super::super::settings::SourceMutation::RepairConfig {
                document: USER.into(),
            },
        )
        .unwrap();
    assert!(repaired.user.authored.is_some());
    let settings = fixture
        .manager
        .read_source_settings(&fixture.workspace)
        .unwrap();
    assert!(settings.user.authored.is_some());
    assert!(settings.resolved.is_some());
    assert!(settings.prospective_diagnostic.is_none());
}

// Physical source authority (PHY-01..04): a source transaction publishes only
// into the directory it admitted. Each case replaces the Workspace pathname
// with a symlink to another Workspace holding identical bytes, so a
// content-only revision check would accept it.

struct Swap {
    directory: PathBuf,
    moved: PathBuf,
    other: PathBuf,
}

impl Swap {
    fn prepare(fixture: &Fixture, text: &str) -> Self {
        let directory = fixture.workspace.workspace().unwrap().to_path_buf();
        let swap = Self {
            moved: directory.with_file_name("moved"),
            other: directory.with_file_name("other"),
            directory,
        };
        std::fs::create_dir(&swap.other).unwrap();
        std::fs::write(swap.other.join("rustx.toml"), text).unwrap();
        swap
    }

    /// Rename the admitted Workspace away and put a symlink to the other
    /// Workspace at its pathname.
    fn hook(&self, fixture: &Fixture, point: &'static str) {
        let (directory, moved, other) = (
            self.directory.clone(),
            self.moved.clone(),
            self.other.clone(),
        );
        fixture.manager.test_hooks.insert(point, move || {
            std::fs::rename(&directory, &moved).unwrap();
            std::os::unix::fs::symlink(&other, &directory).unwrap();
        });
    }

    fn moved(&self) -> String {
        std::fs::read_to_string(self.moved.join("rustx.toml")).unwrap()
    }

    fn other(&self) -> String {
        std::fs::read_to_string(self.other.join("rustx.toml")).unwrap()
    }

    /// Only documents and lock files remain: no staged candidate leaked.
    fn entries(directory: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

const REPAIRABLE: &str = "# Workspace\n[agent]\nlegacy = 1\n";

#[test]
fn a_workspace_replaced_before_the_writer_lock_is_never_written() {
    let fixture = Fixture::new();
    fixture.write(&fixture.workspace, REPAIRABLE);
    let swap = Swap::prepare(&fixture, REPAIRABLE);
    swap.hook(&fixture, "repair_before_lock");
    assert_eq!(
        fixture
            .manager
            .repair_source(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Superseded
    );
    assert_eq!(swap.other(), REPAIRABLE);
    assert_eq!(swap.moved(), REPAIRABLE);
    assert_eq!(Swap::entries(&swap.other), ["rustx.toml"]);
    assert!(fixture.receipt(&fixture.workspace).is_none());
    assert!(fixture.backups().is_empty());
    assert_eq!(fixture.sequence(), 0);
}

#[test]
fn a_workspace_replaced_before_publication_cannot_redirect_it() {
    let fixture = Fixture::new();
    fixture.write(&fixture.workspace, REPAIRABLE);
    let swap = Swap::prepare(&fixture, REPAIRABLE);
    swap.hook(&fixture, "before_publication");
    assert_eq!(
        fixture
            .manager
            .repair_source(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Superseded
    );
    assert_eq!(swap.other(), REPAIRABLE);
    assert_eq!(swap.moved(), REPAIRABLE);
    assert_eq!(Swap::entries(&swap.other), ["rustx.toml"]);
    assert_eq!(
        Swap::entries(&swap.moved),
        [".rustx.toml.lock", "rustx.toml"]
    );
    assert!(fixture.receipt(&fixture.workspace).is_none());
    assert_eq!(fixture.sequence(), 0);
}

#[test]
fn a_workspace_renamed_after_replacement_is_uncertain_and_never_replayed() {
    let fixture = Fixture::new();
    fixture.write(&fixture.workspace, REPAIRABLE);
    let swap = Swap::prepare(&fixture, REPAIRABLE);
    swap.hook(&fixture, "after_publication");
    assert_eq!(
        fixture
            .manager
            .repair_source(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Committed
    );
    // The bytes are in the physical directory the transaction admitted; the
    // pathname now names another one, so success cannot be claimed for it.
    assert_eq!(swap.moved(), "# Workspace\n[agent]\n");
    assert_eq!(swap.other(), REPAIRABLE);
    let receipt = fixture.receipt(&fixture.workspace).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::CommitUncertain);
    assert_eq!(receipt.after_revision, None);
    // A redirected target is no source of this owner: nothing is replayed.
    let sequence = fixture.sequence();
    assert_eq!(
        fixture
            .manager
            .repair_source(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Unchanged
    );
    assert_eq!(swap.other(), REPAIRABLE);
    assert_eq!(fixture.sequence(), sequence);
}

fn capacity(value: usize) -> super::super::settings::SourceMutation {
    super::super::settings::SourceMutation::Config {
        mutation: super::super::settings::ConfigMutation::Capacity {
            authored: Some(crate::local_runtime::authoring::SubagentsLayer {
                max_concurrent: Some(value),
            }),
        },
    }
}

#[test]
fn ordinary_source_writes_obey_the_same_physical_authority() {
    let text = "[subagents]\nmax_concurrent = 2\n";
    for point in [
        "write_before_lock",
        "before_publication",
        "after_publication",
    ] {
        let fixture = Fixture::new();
        fixture.write(&fixture.workspace, text);
        let revision = fixture
            .manager
            .read_source_settings(&fixture.workspace)
            .unwrap()
            .workspace
            .unwrap()
            .revision;
        let swap = Swap::prepare(&fixture, text);
        swap.hook(&fixture, point);
        let result =
            fixture
                .manager
                .write_source_settings(&fixture.workspace, &revision, capacity(3));
        // The other Workspace is never written, whatever the outcome.
        assert_eq!(swap.other(), text, "{point}");
        assert_eq!(Swap::entries(&swap.other), ["rustx.toml"], "{point}");
        if point == "after_publication" {
            // Published into the admitted directory, unconfirmed for the
            // target pathname: uncertain, never success.
            assert!(
                matches!(result, Err(SettingsError::Committed)),
                "{point}: {result:?}"
            );
            assert_eq!(swap.moved(), "[subagents]\nmax_concurrent = 3\n");
        } else {
            // Definitely unpublished, and distinct from a revision conflict.
            assert!(
                matches!(result, Err(SettingsError::Invalid)),
                "{point}: {result:?}"
            );
            assert_eq!(swap.moved(), text, "{point}");
        }
    }
}

#[test]
fn a_valid_source_write_replaces_atomically_under_its_exact_revision() {
    use std::os::unix::fs::MetadataExt;
    let fixture = Fixture::new();
    let text = "[subagents]\nmax_concurrent = 2\n";
    fixture.write(&fixture.workspace, text);
    let path = fixture.path(&fixture.workspace);
    let stale = super::super::super::settings::revision(Some(b"stale"));
    assert!(matches!(
        fixture
            .manager
            .write_source_settings(&fixture.workspace, &stale, capacity(3)),
        Err(SettingsError::Conflict { .. })
    ));
    assert_eq!(fixture.read(&fixture.workspace), text);
    let before = std::fs::metadata(&path).unwrap().ino();
    let revision = super::super::super::settings::revision(Some(text.as_bytes()));
    let written = fixture
        .manager
        .write_source_settings(&fixture.workspace, &revision, capacity(3))
        .unwrap();
    let published = "[subagents]\nmax_concurrent = 3\n";
    assert_eq!(fixture.read(&fixture.workspace), published);
    assert_eq!(
        written.workspace.unwrap().revision,
        super::super::super::settings::revision(Some(published.as_bytes()))
    );
    // A new inode renamed into place; no staged file is left behind.
    assert_ne!(std::fs::metadata(&path).unwrap().ino(), before);
    assert_eq!(
        Swap::entries(path.parent().unwrap()),
        [".rustx.toml.lock", "rustx.toml"]
    );
    // An unconfirmed directory sync is uncertain, never success.
    fixture.manager.test_hooks.fail_once("directory_sync");
    let revision = super::super::super::settings::revision(Some(published.as_bytes()));
    assert!(matches!(
        fixture
            .manager
            .write_source_settings(&fixture.workspace, &revision, capacity(4)),
        Err(SettingsError::Committed)
    ));
}

// Source-intrinsic health (VAL-01..04).

const INTRINSICALLY_INVALID: &str = "[subagents]\nmax_concurrent = 0\n";

#[test]
fn nothing_eligible_is_not_proof_of_health() {
    let fixture = Fixture::new();
    for target in [SourceTarget::User, fixture.workspace.clone()] {
        fixture.write(&target, INTRINSICALLY_INVALID);
        assert_eq!(
            fixture.manager.repair_source(&target, &target),
            RepairOutcome::Unchanged
        );
        assert_eq!(fixture.read(&target), INTRINSICALLY_INVALID);
        let receipt = fixture.receipt(&target).unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
        assert!(receipt.changes.is_empty());
        assert!(!receipt.backup_available);
        let diagnostic = receipt.diagnostic.unwrap();
        assert!(
            diagnostic.contains("subagents.max_concurrent"),
            "{diagnostic}"
        );
        assert!(!diagnostic.contains("TOML"), "{diagnostic}");
        // No source transaction began: no writer lock, no backup.
        assert!(
            !fixture
                .path(&target)
                .with_file_name(".rustx.toml.lock")
                .exists()
        );
        let sequence = fixture.sequence();
        assert_eq!(
            fixture.manager.repair_source(&target, &target),
            RepairOutcome::Unchanged
        );
        assert_eq!(fixture.sequence(), sequence);
    }
    assert!(fixture.backups().is_empty());
}

#[test]
fn an_unresolved_receipt_is_withdrawn_only_on_proven_recovery() {
    use super::super::settings::SourceMutation;
    let fixture = Fixture::new();
    let target = fixture.workspace.clone();
    fixture.write(&target, "[agent\n");
    fixture.manager.repair_source(&target, &target);
    let malformed = fixture.receipt(&target).unwrap();
    assert_eq!(malformed.status, ConfigRepairStatus::Unresolved);

    // Manual repair to a parsable document that is still intrinsically
    // invalid: no success is claimed, the source stays unresolved, now with
    // its current diagnostic.
    let revision = super::super::super::settings::revision(Some(b"[agent\n"));
    let manual = fixture
        .manager
        .write_source_settings(
            &target,
            &revision,
            SourceMutation::RepairConfig {
                document: INTRINSICALLY_INVALID.into(),
            },
        )
        .unwrap();
    let still = fixture.receipt(&target).unwrap();
    assert_eq!(still.status, ConfigRepairStatus::Unresolved);
    assert_ne!(still.id, malformed.id);
    assert_eq!(
        still.before_revision,
        manual.workspace.as_ref().unwrap().revision
    );
    assert!(
        still
            .diagnostic
            .as_deref()
            .unwrap()
            .contains("subagents.max_concurrent")
    );
    // A semantically invalid but parsable source stays editable.
    let workspace = manual.workspace.unwrap();
    assert!(workspace.authored.is_some());
    assert!(workspace.diagnostic.is_none());

    // Ordinary editing corrects it: exactly one withdrawal.
    let sequence = fixture.sequence();
    fixture
        .manager
        .write_source_settings(&target, &workspace.revision, capacity(2))
        .unwrap();
    assert!(fixture.receipt(&target).is_none());
    assert_eq!(fixture.sequence(), sequence + 1);
    assert_eq!(
        fixture
            .manager
            .repairs()
            .changed_after(&SourceKey::from(&target), sequence),
        Some((sequence + 1, None))
    );
    // Later observations are no-ops.
    fixture.manager.refresh_repair_observation(&target);
    assert_eq!(
        fixture.manager.repair_source(&target, &target),
        RepairOutcome::Unchanged
    );
    assert_eq!(fixture.sequence(), sequence + 1);
}

#[test]
fn an_ordinary_edit_of_a_source_without_a_receipt_records_none() {
    let fixture = Fixture::new();
    let target = fixture.workspace.clone();
    fixture.write(&target, "[subagents]\nmax_concurrent = 2\n");
    let revision =
        super::super::super::settings::revision(Some(b"[subagents]\nmax_concurrent = 2\n"));
    fixture
        .manager
        .write_source_settings(&target, &revision, capacity(0))
        .unwrap();
    assert!(fixture.receipt(&target).is_none());
    assert_eq!(fixture.sequence(), 0);
}

#[test]
fn a_valid_partial_source_is_healthy_without_composition_facts() {
    let fixture = Fixture::new();
    // No Provider, no Model, no default selection: facts another scope may
    // supply. Nothing here is intrinsically invalid.
    let partial = "[subagents]\nmax_concurrent = 2\n[agent]\ndescription = \"partial\"\n";
    for target in [SourceTarget::User, fixture.workspace.clone()] {
        fixture.write(&target, partial);
        assert_eq!(
            fixture.manager.repair_source(&target, &target),
            RepairOutcome::Unchanged
        );
        assert!(fixture.receipt(&target).is_none());
    }
    assert_eq!(fixture.sequence(), 0);
}

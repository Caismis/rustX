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
[agent.model]
model = "local/a"
"#;

struct Fixture {
    root: tempfile::TempDir,
    manager: UserConfigManager,
    workspace: SourceTarget,
}

impl Fixture {
    fn new() -> Self {
        Self::with_config_path(|home| home.join("rustx/rustx.toml"))
    }

    fn with_config_path(config: impl FnOnce(&Path) -> PathBuf) -> Self {
        Self::with_paths(config, |root| root.join("state"))
    }

    fn with_paths(
        config: impl FnOnce(&Path) -> PathBuf,
        runtime: impl FnOnce(&Path) -> PathBuf,
    ) -> Self {
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
            runtime_root: runtime(root.path()),
        })
        .unwrap();
        Self {
            workspace: SourceTarget::Workspace {
                directory: workspace.canonicalize().unwrap(),
            },
            root,
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

    /// One repair transaction of `target` within the admitted `scope`.
    fn repair(&self, target: &SourceTarget, scope: &SourceTarget) -> RepairOutcome {
        let scope = self.manager.admit_repair_scope(scope).expect("admitted");
        self.manager.repair_source(target, &scope)
    }

    fn receipt(&self, target: &SourceTarget) -> Option<ConfigRepairReceipt> {
        self.manager.repairs().latest(target)
    }

    fn sequence(&self) -> u64 {
        *self.manager.repairs().subscribe().borrow()
    }
}

/// The writer lock beside `document`.
fn lock_of(document: &Path) -> PathBuf {
    document.with_file_name(super::super::super::settings::lock_name(
        document.file_name().unwrap(),
    ))
}

/// The directory entries of a written `rustx.toml`: the document and its
/// writer lock, no staged candidate.
fn document_entries() -> [String; 2] {
    [
        super::super::super::settings::lock_name(std::ffi::OsStr::new("rustx.toml")),
        "rustx.toml".into(),
    ]
}

fn changes(text: &str) -> Vec<ConfigRepairChange> {
    plan(text.as_bytes()).unwrap().changes
}

fn candidate(text: &str) -> String {
    String::from_utf8(plan(text.as_bytes()).unwrap().candidate).unwrap()
}

/// An unknown field removed from the closed object at `object_path`.
fn removed(object_path: &str) -> ConfigRepairChange {
    ConfigRepairChange::RemovedField {
        object_path: object_path.into(),
    }
}

/// A removed selection entry naming the native Tool `name`.
fn tool(position: u32, name: &str, reason: NativeToolRemoval) -> ConfigRepairChange {
    ConfigRepairChange::RemovedNativeTool {
        position,
        name: Some(name.into()),
        reason,
    }
}

/// A removed selection entry naming no native Tool: its name stays unsaid.
fn unknown_tool(position: u32) -> ConfigRepairChange {
    ConfigRepairChange::RemovedNativeTool {
        position,
        name: None,
        reason: NativeToolRemoval::Unknown,
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
            removed("agent.plugins.todo"),
            removed("native_tools"),
            unknown_tool(1),
            tool(3, "read", NativeToolRemoval::Duplicate),
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
            removed(""),
            removed(""),
            removed("native_tools.bash"),
            removed("agent.plugins.goal"),
            removed("agent.model.max_output_tokens"),
            removed("subagents"),
            removed("mcp_tool_policies.*"),
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
    assert_eq!(changes(&with_unknown), vec![removed("models.*.compat")]);
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
    let position = |name: &str| {
        u32::try_from(selection.iter().rposition(|entry| entry == name).unwrap()).unwrap()
    };
    let mut expected: Vec<_> = controls
        .iter()
        .map(|name| tool(position(name), name, NativeToolRemoval::AgentControl))
        .collect();
    expected.extend(
        plugins
            .iter()
            .map(|name| tool(position(name), name, NativeToolRemoval::PluginOwned)),
    );
    expected.push(tool(
        position(&ordinary[0]),
        &ordinary[0],
        NativeToolRemoval::Duplicate,
    ));
    expected.push(unknown_tool(position("not_a_native_tool")));
    expected.push(unknown_tool(position("")));
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
            removed("agent.model.summary_model"),
            removed("tool_deadline_policy.idle_liveness_ms"),
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
        assert_eq!(fixture.repair(&target, &target), RepairOutcome::Committed);
        let receipt = fixture.receipt(&target).unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
        assert_eq!(receipt.target, target);
        assert_eq!(
            receipt.changes,
            vec![removed(""), removed("agent.plugins.todo")]
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
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
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
        fixture.repair(&fixture.workspace, &fixture.workspace),
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
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
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
    assert!(!lock_of(&fixture.path(&SourceTarget::User)).exists());
    assert_eq!(fixture.sequence(), 0);

    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    let repaired = fixture.read(&SourceTarget::User);
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    let sequence = fixture.sequence();
    for _ in 0..3 {
        assert_eq!(
            fixture.repair(&SourceTarget::User, &SourceTarget::User),
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
            fixture.repair(&fixture.workspace, &fixture.workspace),
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
        fixture.repair(&fixture.workspace, &fixture.workspace),
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
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    // A Workspace selecting a User-defined Model validates over the User.
    fixture.write(
        &fixture.workspace,
        "[agent.model]\nmodel = \"local/a\"\nstale = true\n",
    );
    assert_eq!(
        fixture.repair(&fixture.workspace, &fixture.workspace),
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
    fixture.repair(&SourceTarget::User, &SourceTarget::User);
    assert_eq!(
        fixture.receipt(&SourceTarget::User).unwrap().status,
        ConfigRepairStatus::Unresolved
    );
    let sequence = fixture.sequence();
    // The same invalid bytes are the same observation.
    fixture.repair(&SourceTarget::User, &SourceTarget::User);
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
        Some(SourceChange::Entry(sequence + 1, None))
    );
}

#[test]
fn user_and_workspace_repairs_are_independent_transactions() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    fixture.write(&fixture.workspace, "[agent\n");
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    assert_eq!(
        fixture.repair(&fixture.workspace, &fixture.workspace),
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
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
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
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Superseded
    );
    assert_eq!(fixture.read(&SourceTarget::User), external);
    assert!(fixture.receipt(&SourceTarget::User).is_none());
    // The next observation plans from the external bytes; nothing is replayed.
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
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
            fixture.repair(&SourceTarget::User, &SourceTarget::User),
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
            fixture.repair(&SourceTarget::User, &SourceTarget::User),
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
            fixture.repair(&SourceTarget::User, &SourceTarget::User),
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
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.changed_count as usize, MAX_RECEIPT_CHANGES + 7);
    assert_eq!(receipt.changes.len(), MAX_RECEIPT_CHANGES);
    assert!(receipt.truncated);
    // The removed key itself, however long or strange, is never carried.
    let all = plan(text.as_bytes()).unwrap().changes;
    assert_eq!(all.last(), Some(&removed("agent.model")));
    let serialized = serde_json::to_string(&receipt).unwrap();
    assert!(!serialized.contains("legacy_"), "{serialized}");
    assert!(!serialized.contains("kkkk"), "{serialized}");
    assert!(!serialized.contains("\\u0007"), "{serialized}");
    assert_eq!(changes("\"a\\u0007b\" = 1\n"), vec![removed("")]);
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
            "[models.\"{key}\"]\nprovider = \"local\"\nid = \"wire\"\nprotocol = \"openai_responses\"\ncontext_window = 128000\nmax_output_tokens = 8192\n{extra}[models.\"{key}\".capabilities]\ninput_modalities = [\"text\"]\noutput_modalities = [\"text\"]\ntool_calls = true\n"
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
            fixture.repair(&SourceTarget::User, &SourceTarget::User),
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
        fixture.repair(&fixture.workspace, &fixture.workspace),
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
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    let expected = "\n# keep\n[agent.model]\nmodel = \"workspace-provider/model-a\"\n";
    assert_eq!(fixture.read(&SourceTarget::User), expected);
    assert_eq!(fixture.read(&fixture.workspace), WORKSPACE_MODEL);
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
    assert_eq!(receipt.changes, vec![removed("")]);
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
        fixture.repair(&SourceTarget::User, &fixture.workspace),
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
        fixture.repair(&SourceTarget::User, &fixture.workspace),
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
                fixture.repair(&target, &scope),
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
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    assert_eq!(
        fixture.read(&SourceTarget::User),
        "[agent]\ninstructions = \"Be brief.\"\n"
    );
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.changes, vec![removed("")]);
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
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
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
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Unchanged
    );
    // Nothing reached the Workspace: the exact private copy is in the
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
    // The bound backup path no longer reaches it, so the backup is not
    // confirmed: the source is unchanged and nothing claims a backup.
    assert_eq!(fixture.read(&SourceTarget::User), text);
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
    assert!(!receipt.backup_available);
    assert!(receipt.diagnostic.unwrap().contains("backup storage moved"));
}

// Backup root physical authority (BACKUP-01..06). The bound runtime root is
// `data/state/rustx`; the Workspace holds a `rustx` directory, so an
// ancestor redirected into the Workspace would resolve.

struct Redirect {
    fixture: Fixture,
    data: PathBuf,
    workspace: PathBuf,
}

impl Redirect {
    fn new() -> Self {
        let fixture = Fixture::with_paths(
            |home| home.join("rustx/rustx.toml"),
            |root| root.join("data/state/rustx"),
        );
        let data = fixture
            .manager
            .runtime_root()
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        std::fs::create_dir_all(data.join("state")).unwrap();
        let workspace = fixture.workspace.workspace().unwrap().to_path_buf();
        std::fs::create_dir(workspace.join("rustx")).unwrap();
        Self {
            fixture,
            data,
            workspace,
        }
    }

    /// Rename `data/state` away and put a symlink into the Workspace in its
    /// place.
    fn redirect(data: &Path, workspace: &Path) {
        std::fs::rename(data.join("state"), data.join("state.moved")).unwrap();
        std::os::unix::fs::symlink(workspace, data.join("state")).unwrap();
    }

    fn hook(&self, point: &'static str) {
        let (data, workspace) = (self.data.clone(), self.workspace.clone());
        self.fixture
            .manager
            .test_hooks
            .insert(point, move || Self::redirect(&data, &workspace));
    }

    /// Every file below the Workspace other than its source and that
    /// source's writer lock.
    fn workspace_files(&self) -> Vec<PathBuf> {
        fn walk(directory: &Path, files: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                let kind = std::fs::symlink_metadata(&path).unwrap().file_type();
                if kind.is_dir() {
                    walk(&path, files);
                } else {
                    files.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&self.workspace, &mut files);
        let source = self.workspace.join("rustx.toml");
        files.retain(|path| *path != source && *path != lock_of(&source));
        files
    }

    /// Nothing reached the Workspace: no file but its source and that
    /// source's writer lock, no backup directory.
    fn assert_workspace_untouched(&self) {
        assert_eq!(self.workspace_files(), Vec::<PathBuf>::new());
        assert!(!self.workspace.join("rustx").join(BACKUP_DIRECTORY).exists());
    }

    /// The repair of `target` within `scope` failed closed before any
    /// publication, with an unresolved receipt naming `diagnostic`.
    fn assert_refused(&self, case: &Case, diagnostic: &str) {
        let fixture = &self.fixture;
        assert_eq!(
            fixture.repair(&case.target, &case.scope),
            RepairOutcome::Unchanged
        );
        assert_eq!(fixture.read(&case.target), case.original);
        let receipt = fixture.receipt(&case.target).unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
        assert!(!receipt.backup_available);
        assert!(receipt.after_revision.is_none());
        let found = receipt.diagnostic.unwrap();
        assert!(found.contains(diagnostic), "{found}");
        self.assert_workspace_untouched();
    }

    /// `scope` is refused at admission: no transaction of it begins, so no
    /// source changes and no receipt or ledger change is recorded.
    fn assert_scope_refused(&self, case: &Case) {
        let fixture = &self.fixture;
        let sequence = fixture.sequence();
        assert!(fixture.manager.admit_repair_scope(&case.scope).is_err());
        assert_eq!(fixture.read(&case.target), case.original);
        assert!(fixture.receipt(&case.target).is_none());
        assert_eq!(fixture.sequence(), sequence);
        self.assert_workspace_untouched();
    }
}

/// One repair transaction: its source, its scope and the source's bytes.
#[derive(Clone)]
struct Case {
    target: SourceTarget,
    scope: SourceTarget,
    original: String,
}

/// The User alone, the User within the selected Workspace, and the
/// Workspace, each holding a repairable sensitive value.
fn redirect_cases(redirect: &Redirect) -> [Case; 3] {
    let user = format!("{USER}legacy = 'SECRET_X'\n");
    let workspace = "# Workspace\n[agent]\nlegacy = 'SECRET_X'\n".to_string();
    let selected = redirect.fixture.workspace.clone();
    redirect.fixture.write(&SourceTarget::User, &user);
    redirect.fixture.write(&selected, &workspace);
    [
        Case {
            target: SourceTarget::User,
            scope: SourceTarget::User,
            original: user.clone(),
        },
        Case {
            target: SourceTarget::User,
            scope: selected.clone(),
            original: user,
        },
        Case {
            target: selected.clone(),
            scope: selected,
            original: workspace,
        },
    ]
}

#[test]
fn an_intermediate_runtime_root_symlink_is_never_followed_for_backups() {
    let redirect = Redirect::new();
    let [user, within, workspace] = redirect_cases(&redirect);
    Redirect::redirect(&redirect.data, &redirect.workspace);
    // A selected Workspace is refused before either of its sources; the
    // User alone reaches its backup, which refuses the symlink.
    redirect.assert_scope_refused(&within);
    redirect.assert_scope_refused(&workspace);
    redirect.assert_refused(&user, "could not be backed up");
}

#[test]
fn an_ancestor_redirected_after_scope_admission_cannot_reach_the_workspace() {
    for index in 0..3 {
        let redirect = Redirect::new();
        let case = redirect_cases(&redirect)[index].clone();
        redirect.hook("before_backup");
        redirect.assert_refused(&case, "could not be backed up");
        // Admission observes runtime storage and never creates it; the backup
        // that would have created it refused the redirected ancestor first.
        let moved = redirect.data.join("state.moved/rustx");
        assert!(!moved.exists());
    }
}

#[test]
fn a_workspace_moved_onto_the_runtime_root_path_never_receives_a_backup() {
    // The User within the selected Workspace, and the Workspace itself.
    for index in 1..3 {
        let redirect = Redirect::new();
        let case = redirect_cases(&redirect)[index].clone();
        // No symlink anywhere: the selected Workspace itself is renamed onto
        // `data/state` after admission, so a no-follow walk reaches
        // `<workspace>/rustx`.
        let (data, workspace) = (redirect.data.clone(), redirect.workspace.clone());
        redirect
            .fixture
            .manager
            .test_hooks
            .insert("before_backup", move || {
                std::fs::rename(data.join("state"), data.join("state.moved")).unwrap();
                std::fs::rename(&workspace, data.join("state")).unwrap();
            });
        let fixture = &redirect.fixture;
        assert_eq!(
            fixture.repair(&case.target, &case.scope),
            RepairOutcome::Unchanged
        );
        let moved = redirect.data.join("state");
        let source = match case.target {
            SourceTarget::User => fixture.path(&SourceTarget::User),
            SourceTarget::Workspace { .. } => moved.join("rustx.toml"),
        };
        assert_eq!(std::fs::read_to_string(source).unwrap(), case.original);
        assert!(!moved.join("rustx").join(BACKUP_DIRECTORY).exists());
        let receipt = fixture.receipt(&case.target).unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
        assert!(!receipt.backup_available);
        assert!(
            receipt
                .diagnostic
                .unwrap()
                .contains("outside the Workspace")
        );
    }
}

#[test]
fn a_workspace_moved_into_the_runtime_root_after_admission_receives_no_backup() {
    for index in 1..3 {
        let redirect = Redirect::new();
        let case = redirect_cases(&redirect)[index].clone();
        let (root, workspace) = (
            redirect.fixture.manager.runtime_root().to_path_buf(),
            redirect.workspace.clone(),
        );
        redirect
            .fixture
            .manager
            .test_hooks
            .insert("before_backup", move || {
                std::fs::create_dir_all(&root).unwrap();
                std::fs::rename(&workspace, root.join("moved-workspace")).unwrap();
            });
        let fixture = &redirect.fixture;
        assert_eq!(
            fixture.repair(&case.target, &case.scope),
            RepairOutcome::Unchanged
        );
        let receipt = fixture.receipt(&case.target).unwrap();
        assert!(!receipt.backup_available);
        assert!(
            receipt
                .diagnostic
                .unwrap()
                .contains("outside the Workspace")
        );
        assert!(fixture.backups().is_empty());
        let moved = fixture.manager.runtime_root().join("moved-workspace");
        let source = match case.target {
            SourceTarget::User => fixture.path(&SourceTarget::User),
            SourceTarget::Workspace { .. } => moved.join("rustx.toml"),
        };
        assert_eq!(std::fs::read_to_string(source).unwrap(), case.original);
    }
}

#[test]
fn an_ancestor_redirected_after_admission_cannot_redirect_the_copy() {
    for index in 0..3 {
        let redirect = Redirect::new();
        let case = redirect_cases(&redirect)[index].clone();
        redirect.hook("backup_directory_opened");
        redirect.assert_refused(&case, "backup storage moved");
        // The exact copy is in the physical directory that was admitted, now
        // reachable only under its moved ancestor.
        let directory = redirect
            .data
            .join("state.moved/rustx")
            .join(BACKUP_DIRECTORY);
        let copies: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(copies.len(), 1);
        assert_eq!(std::fs::read(&copies[0]).unwrap(), case.original.as_bytes());
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&copies[0]), 0o600);
        assert_eq!(mode(&directory), 0o700);
    }
}

// Repair scope admission (ISO-01..06). A selected Workspace is admitted
// against the runtime root before either of its sources is touched.

/// A Fixture whose runtime root is placed relative to its Workspace, with a
/// repairable sensitive value in both sources.
fn overlapping(runtime: impl FnOnce(&Path) -> PathBuf) -> (Fixture, [String; 2]) {
    let fixture = Fixture::with_paths(|home| home.join("rustx/rustx.toml"), runtime);
    let user = format!("{USER}legacy = 'SECRET_X'\n");
    let workspace = "# Workspace\n[agent]\nlegacy = 'SECRET_X'\n".to_string();
    fixture.write(&SourceTarget::User, &user);
    fixture.write(&fixture.workspace, &workspace);
    (fixture, [user, workspace])
}

#[test]
fn an_overlapping_runtime_root_is_refused_before_anything_is_created() {
    for runtime in [
        // The runtime root inside the selected Workspace.
        (|root: &Path| root.join("workspace/.rustx/state")) as fn(&Path) -> PathBuf,
        // The selected Workspace inside the runtime root.
        |root: &Path| root.to_path_buf(),
    ] {
        let (fixture, [user, workspace]) = overlapping(runtime);
        let root = fixture.manager.runtime_root().to_path_buf();
        let existed = root.exists();
        let refused = fixture
            .manager
            .admit_repair_scope(&fixture.workspace)
            .err()
            .unwrap();
        assert_eq!(refused, "runtime_root must be disjoint from the workspace");
        assert_eq!(root.exists(), existed, "nothing is created");
        assert_eq!(fixture.read(&SourceTarget::User), user);
        assert_eq!(fixture.read(&fixture.workspace), workspace);
        assert!(fixture.backups().is_empty());
        assert_eq!(fixture.sequence(), 0);
        // Startup's User-only scope selects no Workspace, so it proves
        // nothing about directories a later boundary may select, and still
        // repairs the User alone.
        assert_eq!(
            fixture.repair(&SourceTarget::User, &SourceTarget::User),
            RepairOutcome::Committed
        );
        assert_eq!(fixture.read(&SourceTarget::User), USER);
        assert_eq!(fixture.backups().len(), 1);
    }
}

#[test]
fn a_workspace_target_is_repaired_only_through_its_own_admitted_scope() {
    let fixture = Fixture::new();
    fixture.write(&fixture.workspace, REPAIRABLE);
    let other = fixture
        .workspace
        .workspace()
        .unwrap()
        .with_file_name("other");
    std::fs::create_dir(&other).unwrap();
    let other = SourceTarget::Workspace {
        directory: other.canonicalize().unwrap(),
    };
    for scope in [SourceTarget::User, other] {
        let scope = fixture.manager.admit_repair_scope(&scope).unwrap();
        assert_eq!(
            fixture.manager.repair_source(&fixture.workspace, &scope),
            RepairOutcome::Unchanged
        );
    }
    assert_eq!(fixture.read(&fixture.workspace), REPAIRABLE);
    assert_eq!(fixture.sequence(), 0);
}

#[test]
fn a_workspace_replaced_after_admission_is_never_repaired_through_its_scope() {
    let fixture = Fixture::new();
    let user = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &user);
    fixture.write(&fixture.workspace, REPAIRABLE);
    let swap = Swap::prepare(&fixture, REPAIRABLE);
    swap.hook(&fixture, "repair_scope_admitted");
    let scope = fixture
        .manager
        .admit_repair_scope(&fixture.workspace)
        .unwrap();
    // The Workspace pathname now names another directory: the admitted
    // Workspace is never published through a pathname it no longer owns,
    // and the other directory is never written.
    assert_eq!(
        fixture.manager.repair_source(&fixture.workspace, &scope),
        RepairOutcome::Unchanged
    );
    assert_eq!(swap.moved(), REPAIRABLE);
    assert_eq!(swap.other(), REPAIRABLE);
    assert_eq!(Swap::entries(&swap.other), ["rustx.toml"]);
    assert!(fixture.receipt(&fixture.workspace).is_none());
    // The User is its own transaction; its backup is checked against the
    // admitted (moved) Workspace, which the runtime root is disjoint from.
    assert_eq!(
        fixture.manager.repair_source(&SourceTarget::User, &scope),
        RepairOutcome::Committed
    );
    assert_eq!(fixture.read(&SourceTarget::User), USER);
    assert_eq!(fixture.backups().len(), 1);
}

#[test]
fn an_existing_backup_is_reused_only_when_it_is_exactly_the_original() {
    let fixture = Fixture::new();
    let original = format!("{USER}legacy = 1\n");
    // An identical copy from an earlier attempt is kept, not duplicated.
    fixture.write(&SourceTarget::User, &original);
    fixture.manager.test_hooks.fail_once("replace_refused");
    fixture.repair(&SourceTarget::User, &SourceTarget::User);
    assert_eq!(fixture.read(&SourceTarget::User), original);
    let backups = fixture.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    assert_eq!(fixture.backups(), backups);
    assert_eq!(std::fs::read(&backups[0]).unwrap(), original.as_bytes());

    // A different file under that name is never trusted as the backup.
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, &original);
    fixture.manager.test_hooks.fail_once("replace_refused");
    fixture.repair(&SourceTarget::User, &SourceTarget::User);
    let backup = fixture.backups().remove(0);
    std::fs::write(&backup, "tampered").unwrap();
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Unchanged
    );
    assert_eq!(fixture.read(&SourceTarget::User), original);
    assert!(
        unresolved_diagnostic(&fixture, &SourceTarget::User).contains("could not be backed up")
    );
    assert_eq!(fixture.backups(), [backup]);
}

// Bounded internal filenames (NAME-01..05).

/// The longest component the filesystem holding `directory` accepts.
fn name_max(directory: &Path) -> usize {
    nix::unistd::pathconf(directory, nix::unistd::PathconfVar::NAME_MAX)
        .unwrap()
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap()
}

/// A source basename is repaired, backed up and authored like any other,
/// leaving only the document and its writer lock beside it.
fn assert_basename_writable(name: &std::ffi::OsStr) {
    let fixture = Fixture::with_config_path(|home| home.join("long").join(name));
    let path = fixture.path(&SourceTarget::User);
    let original = format!("{USER}legacy = 'SECRET_X'\n");
    fixture.write(&SourceTarget::User, &original);
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    assert_eq!(fixture.read(&SourceTarget::User), USER);
    let backups = fixture.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read(&backups[0]).unwrap(), original.as_bytes());
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
    let entries = || {
        let mut names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        names.sort();
        names
    };
    let mut expected = vec![lock_of(&path), path.clone()];
    expected.sort();
    assert_eq!(entries(), expected);
    let revision = super::super::super::settings::revision(Some(USER.as_bytes()));
    fixture
        .manager
        .write_source_settings(&SourceTarget::User, &revision, capacity(3))
        .unwrap();
    assert!(
        fixture
            .read(&SourceTarget::User)
            .contains("max_concurrent = 3")
    );
    assert_eq!(entries(), expected);
}

#[test]
fn a_long_utf8_source_basename_is_repaired_and_authored() {
    // 225 bytes: the old `.<name>.<uuid>.tmp` staging name was 263.
    let name = format!("{}{}.toml", "設定".repeat(36), "a".repeat(4));
    assert_eq!(name.len(), 225);
    assert_basename_writable(name.as_ref());
}

#[test]
fn a_source_basename_at_the_filesystem_limit_is_writable() {
    let probe = tempfile::tempdir().unwrap();
    let limit = name_max(probe.path());
    let name = format!("{}.toml", "n".repeat(limit - ".toml".len()));
    assert_eq!(name.len(), limit);
    assert_basename_writable(name.as_ref());
}

/// Basenames that are not UTF-8, exactly as Linux filesystems store them.
/// Both decode lossily to the same text, so only exact bytes tell them apart.
#[cfg(target_os = "linux")]
fn non_utf8_names() -> [std::ffi::OsString; 2] {
    use std::os::unix::ffi::OsStringExt;
    [
        std::ffi::OsString::from_vec(b"rustx-\xff.toml".to_vec()),
        std::ffi::OsString::from_vec(b"rustx-\xfe.toml".to_vec()),
    ]
}

#[cfg(target_os = "linux")]
#[test]
fn a_non_utf8_source_basename_is_repaired_and_authored() {
    let [name, other] = non_utf8_names();
    assert!(name.to_str().is_none());
    assert_eq!(name.to_string_lossy(), other.to_string_lossy());
    assert_basename_writable(&name);
}

/// PATH-01..03: a User source bound under a non-UTF-8 basename is read,
/// repaired, backed up, receipted, fenced and authored under exactly that
/// name; nothing appears under any other name.
#[cfg(target_os = "linux")]
#[test]
fn a_non_utf8_user_source_keeps_its_exact_name_through_every_transaction() {
    let [name, other] = non_utf8_names();
    let fixture = Fixture::with_config_path(|home| home.join("native").join(&name));
    let path = fixture.path(&SourceTarget::User);
    assert_eq!(path.file_name(), Some(name.as_os_str()));
    let directory = path.parent().unwrap().to_path_buf();
    // A neighbour whose name differs only in bytes a lossy decoding erases.
    let neighbour = format!("{USER}legacy = 'NEIGHBOUR'\n");
    std::fs::write(directory.join(&other), &neighbour).unwrap();
    let original = format!("{USER}legacy = 'SECRET_X'\n");
    fixture.write(&SourceTarget::User, &original);
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    assert_eq!(fixture.read(&SourceTarget::User), USER);
    assert_eq!(
        std::fs::read_to_string(directory.join(&other)).unwrap(),
        neighbour
    );
    let backups = fixture.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read(&backups[0]).unwrap(), original.as_bytes());
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    let revision = |text: &str| super::super::super::settings::revision(Some(text.as_bytes()));
    assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
    assert_eq!(receipt.before_revision, revision(&original));
    assert_eq!(receipt.after_revision, Some(revision(USER)));
    assert_eq!(receipt.changes, vec![removed("agent.model")]);
    assert!(receipt.backup_available);
    let entries = || {
        let mut names: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let mut expected = vec![
        name.clone(),
        other.clone(),
        lock_of(&path).file_name().unwrap().to_os_string(),
    ];
    expected.sort();
    assert_eq!(entries(), expected);
    // Ordinary authoring reads the exact revision and is fenced by it.
    let read = fixture
        .manager
        .read_source_settings(&SourceTarget::User)
        .unwrap();
    assert_eq!(read.user.path, path);
    assert_eq!(read.user.revision, revision(USER));
    assert!(matches!(
        fixture.manager.write_source_settings(
            &SourceTarget::User,
            &revision(&original),
            capacity(3)
        ),
        Err(super::super::settings::SettingsError::Conflict { .. })
    ));
    assert_eq!(fixture.read(&SourceTarget::User), USER);
    let written = fixture
        .manager
        .write_source_settings(&SourceTarget::User, &read.user.revision, capacity(3))
        .unwrap();
    let authored = fixture.read(&SourceTarget::User);
    assert!(authored.contains("max_concurrent = 3"));
    assert_eq!(written, revision(&authored));
    assert_eq!(
        std::fs::read_to_string(directory.join(&other)).unwrap(),
        neighbour
    );
    assert_eq!(entries(), expected);
}

#[test]
fn every_writer_takes_one_bounded_lock_per_document() {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap();
    let limit = name_max(&directory);
    let mut names: Vec<std::ffi::OsString> = ["rustx.toml", "rustx.toml.", "Rustx.toml"]
        .into_iter()
        .map(Into::into)
        .collect();
    names.push("x".repeat(limit).into());
    #[cfg(target_os = "linux")]
    names.extend(non_utf8_names());
    let source = SourceDirectory::open(&directory.join("rustx.toml"))
        .unwrap()
        .unwrap();
    let mut inodes = BTreeSet::new();
    let excluded = |lock: &Path| {
        matches!(
            std::fs::File::open(lock).unwrap().try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        )
    };
    for name in &names {
        // The handle-relative lock of source transactions and the path lock
        // of initialization are one bounded inode, which no reader creates.
        let lock = lock_of(&directory.join(name));
        assert!(source.share_lock(name).unwrap().is_none());
        assert!(!source.lock_exists(name).unwrap());
        assert!(!lock.exists());
        assert!(lock.file_name().unwrap().len() <= limit);
        let held = source.lock(name).unwrap();
        assert_eq!(
            std::fs::metadata(&lock).unwrap().ino(),
            held.metadata().unwrap().ino()
        );
        assert!(excluded(&lock));
        drop(held);
        assert!(!excluded(&lock));
        let held = super::super::super::settings::lock_document(&directory.join(name)).unwrap();
        assert_eq!(
            std::fs::metadata(&lock).unwrap().ino(),
            held.metadata().unwrap().ino()
        );
        assert!(excluded(&lock));
        drop(held);
        // A capture shares that inode without ever creating it, and excludes
        // writers while it holds it.
        assert!(source.lock_exists(name).unwrap());
        let shared = source.share_lock(name).unwrap().unwrap();
        assert_eq!(
            std::fs::metadata(&lock).unwrap().ino(),
            shared.metadata().unwrap().ino()
        );
        assert!(excluded(&lock));
        drop(shared);
        assert!(!excluded(&lock));
        // Distinct documents never share a lock.
        assert!(
            inodes.insert(std::fs::metadata(&lock).unwrap().ino()),
            "{name:?}"
        );
    }
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), names.len());
}

#[test]
fn a_failed_staging_leaves_no_candidate_and_the_source_unchanged() {
    for point in ["replace_refused", "stage_write"] {
        let fixture = Fixture::new();
        let original = format!("{USER}legacy = 1\n");
        fixture.write(&SourceTarget::User, &original);
        let path = fixture.path(&SourceTarget::User);
        fixture.manager.test_hooks.fail_once(point);
        assert_eq!(
            fixture.repair(&SourceTarget::User, &SourceTarget::User),
            RepairOutcome::Unchanged,
            "{point}"
        );
        assert_eq!(fixture.read(&SourceTarget::User), original, "{point}");
        assert_eq!(
            fixture.receipt(&SourceTarget::User).unwrap().status,
            ConfigRepairStatus::Unresolved,
            "{point}"
        );
        assert_eq!(
            Swap::entries(path.parent().unwrap()),
            document_entries(),
            "{point}"
        );
    }
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
    let committed = fixture.read(&SourceTarget::User);
    assert_eq!(
        repaired,
        crate::local_runtime::settings::revision(Some(committed.as_bytes()))
    );
    let authored: toml::Value = toml::from_str(&committed).unwrap();
    assert!(
        authored.get("schema_version").is_none(),
        "manual repair preserves the input's implicit current schema version"
    );
    let path = fixture.path(&SourceTarget::User);
    let original = super::super::capture::parse_layer(&path, USER.as_bytes(), false).unwrap();
    let repaired_layer =
        super::super::capture::parse_layer(&path, committed.as_bytes(), false).unwrap();
    assert_eq!(
        repaired_layer, original,
        "manual repair preserves every authored semantic field through native encoding"
    );
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
        fixture.repair(&fixture.workspace, &fixture.workspace),
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
        fixture.repair(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Superseded
    );
    assert_eq!(swap.other(), REPAIRABLE);
    assert_eq!(swap.moved(), REPAIRABLE);
    assert_eq!(Swap::entries(&swap.other), ["rustx.toml"]);
    assert_eq!(Swap::entries(&swap.moved), document_entries());
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
        fixture.repair(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Committed
    );
    // The bytes are in the physical directory the transaction admitted; the
    // pathname now names another one, so success cannot be claimed for it.
    assert_eq!(swap.moved(), "# Workspace\n[agent]\n");
    assert_eq!(swap.other(), REPAIRABLE);
    let receipt = fixture.receipt(&fixture.workspace).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::CommitUncertain);
    assert_eq!(receipt.after_revision, None);
    // A redirected target is no source of this owner: its scope is not
    // admitted, so nothing is replayed.
    let sequence = fixture.sequence();
    assert!(
        fixture
            .manager
            .admit_repair_scope(&fixture.workspace)
            .is_err()
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

/// An interrupted blocking lock continues before reading the CAS revision.
/// Both actual native writers still compete for the same persistent inode.
#[test]
fn interrupted_native_writer_locks_preserve_exactly_one_cas_winner() {
    use crate::local_runtime::settings::locks;
    let fixture = Fixture::new();
    let original = "[subagents]\nmax_concurrent = 2\n";
    fixture.write(&fixture.workspace, original);
    let expected = revision_of(original);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let writers: Vec<_> = [3, 4]
        .into_iter()
        .map(|value| {
            let manager = fixture.manager.clone();
            let target = fixture.workspace.clone();
            let expected = expected.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                locks::fail_next(libc::EINTR);
                barrier.wait();
                let result = manager.write_source_settings(&target, &expected, capacity(value));
                assert!(locks::attempts() >= 2, "the same lock must be retried");
                result
            })
        })
        .collect();
    let outcomes: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .collect();
    let committed = revision_of(&fixture.read(&fixture.workspace));
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
        1,
        "{outcomes:?}"
    );
    let conflict = outcomes
        .iter()
        .find_map(|outcome| match outcome {
            Err(SettingsError::Conflict {
                expected, actual, ..
            }) => Some((expected, actual)),
            _ => None,
        })
        .expect("the losing native writer must report Conflict");
    assert_eq!(conflict, (&expected, &committed));
    assert!(
        outcomes
            .iter()
            .any(|outcome| outcome.as_ref().ok() == Some(&committed))
    );
}

#[test]
fn noninterrupted_native_writer_lock_failure_does_not_retry_or_publish() {
    use crate::local_runtime::settings::locks;
    let fixture = Fixture::new();
    let original = "[subagents]\nmax_concurrent = 2\n";
    fixture.write(&fixture.workspace, original);
    locks::fail_next(libc::EACCES);
    let result = fixture.manager.write_source_settings(
        &fixture.workspace,
        &revision_of(original),
        capacity(3),
    );
    assert!(matches!(result, Err(SettingsError::Io)), "{result:?}");
    assert_eq!(locks::attempts(), 1);
    assert_eq!(fixture.read(&fixture.workspace), original);
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
        written,
        super::super::super::settings::revision(Some(published.as_bytes()))
    );
    // A new inode renamed into place; no staged file is left behind.
    assert_ne!(std::fs::metadata(&path).unwrap().ino(), before);
    assert_eq!(Swap::entries(path.parent().unwrap()), document_entries());
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
        assert_eq!(fixture.repair(&target, &target), RepairOutcome::Unchanged);
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
        assert!(!lock_of(&fixture.path(&target)).exists());
        let sequence = fixture.sequence();
        assert_eq!(fixture.repair(&target, &target), RepairOutcome::Unchanged);
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
    fixture.repair(&target, &target);
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
    assert_eq!(still.before_revision, manual);
    assert!(
        still
            .diagnostic
            .as_deref()
            .unwrap()
            .contains("subagents.max_concurrent")
    );
    // A semantically invalid but parsable source stays editable.
    let workspace = fixture
        .manager
        .read_source_settings(&target)
        .unwrap()
        .workspace
        .unwrap();
    assert_eq!(workspace.revision, manual);
    assert!(workspace.authored.is_some());
    assert!(workspace.diagnostic.is_none());

    // Ordinary editing corrects it: exactly one withdrawal.
    let sequence = fixture.sequence();
    fixture
        .manager
        .write_source_settings(&target, &manual, capacity(2))
        .unwrap();
    assert!(fixture.receipt(&target).is_none());
    assert_eq!(fixture.sequence(), sequence + 1);
    assert_eq!(
        fixture
            .manager
            .repairs()
            .changed_after(&SourceKey::from(&target), sequence),
        Some(SourceChange::Entry(sequence + 1, None))
    );
    // Later observations are no-ops.
    fixture.manager.refresh_repair_observation(&target);
    assert_eq!(fixture.repair(&target, &target), RepairOutcome::Unchanged);
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
        assert_eq!(fixture.repair(&target, &target), RepairOutcome::Unchanged);
        assert!(fixture.receipt(&target).is_none());
    }
    assert_eq!(fixture.sequence(), 0);
}

// Backup durability (DUR-RACE-01..10): before a source is replaced, the
// publishing transaction itself has confirmed the exact backup and every
// directory entry on the way to it, whether that entry was created, created
// concurrently by another writer, or merely visible. Creating a directory
// confirms nothing; the confirming walk does.

use super::super::super::settings::durability::{self, Step};

/// The durability steps of `operation` on this thread, the sync of `fail`
/// (if given) failing once.
fn traced<T>(fail: Option<&Path>, operation: impl FnOnce() -> T) -> (T, Vec<Step>) {
    durability::record(fail);
    let result = operation();
    (result, durability::take())
}

fn created(path: &Path) -> Step {
    Step::Created(path.into())
}

fn synced(path: &Path) -> Step {
    Step::Synced(path.into())
}

fn published(path: &Path) -> Step {
    Step::Published(path.into())
}

/// The steps of one confirmation of the directory `path`: from `/` down,
/// every component has its entry synced in its parent before the walk
/// descends, whoever created it.
fn confirmed(path: &Path) -> Vec<Step> {
    let mut components: Vec<&Path> = path.ancestors().collect();
    components.reverse();
    components
        .into_iter()
        .skip(1)
        .map(|component| synced(component.parent().unwrap()))
        .collect()
}

/// The steps of creating the `new` directories, in order.
fn creating(new: &[&Path]) -> Vec<Step> {
    new.iter().map(|path| created(path)).collect()
}

/// The steps of one confirmed publication of the source document `source`:
/// its directory's path confirmed, then — only then — the replacement and
/// its directory sync.
fn source_published(source: &Path) -> Vec<Step> {
    let directory = source.parent().unwrap();
    let mut steps = confirmed(directory);
    steps.extend([published(source), synced(directory)]);
    steps
}

/// The backup storage of `fixture`, below its runtime root.
fn storage(fixture: &Fixture) -> PathBuf {
    fixture.manager.runtime_root().join(BACKUP_DIRECTORY)
}

/// The exact steps of one committed User repair whose backup storage is
/// opened creating the `new` directories and whose copy is `backup`: the
/// storage path confirmed, the copy and its directory sync, then — only then
/// — the source directory's path confirmed, the source replacement and its
/// directory sync.
fn committed_user_repair(fixture: &Fixture, new: &[&Path], backup: &Path) -> Vec<Step> {
    let mut steps = creating(new);
    steps.extend(confirmed(&storage(fixture)));
    steps.extend([published(backup), synced(backup.parent().unwrap())]);
    steps.extend(source_published(&fixture.path(&SourceTarget::User)));
    steps
}

fn assert_backup_failed(fixture: &Fixture, original: &str, steps: &[Step]) {
    let source = fixture.path(&SourceTarget::User);
    assert_eq!(fixture.read(&SourceTarget::User), original);
    assert!(!steps.contains(&published(&source)), "{steps:?}");
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
    assert!(!receipt.backup_available);
    assert!(receipt.after_revision.is_none());
    assert!(
        receipt
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("could not be backed up")),
        "{receipt:?}"
    );
}

#[test]
fn a_first_repair_confirms_every_backup_entry_before_publication() {
    let fixture = Fixture::new();
    let original = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &original);
    let root = fixture.manager.runtime_root().to_path_buf();
    let storage = storage(&fixture);
    assert!(!root.exists());
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Committed);
    let backups = fixture.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        steps,
        committed_user_repair(&fixture, &[&root, &storage], &backups[0])
    );
    // An exact private copy, reported only once confirmed.
    assert_eq!(std::fs::read(&backups[0]).unwrap(), original.as_bytes());
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&backups[0]), 0o600);
    assert_eq!(mode(&storage), 0o700);
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
    assert!(receipt.backup_available);
}

#[test]
fn nested_runtime_root_ancestors_are_created_and_confirmed_in_order() {
    let fixture = Fixture::with_paths(
        |home| home.join("rustx/rustx.toml"),
        |root| root.join("a/b/state"),
    );
    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    let root = fixture.manager.runtime_root().to_path_buf();
    let b = root.parent().unwrap();
    let a = b.parent().unwrap();
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Committed);
    let storage = storage(&fixture);
    assert_eq!(
        steps,
        committed_user_repair(&fixture, &[a, b, &root, &storage], &fixture.backups()[0])
    );
}

#[test]
fn preexisting_backup_storage_is_confirmed_without_assuming_prior_persistence() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    // Created by someone else, never synced by any rustX transaction.
    std::fs::create_dir_all(storage(&fixture)).unwrap();
    std::fs::set_permissions(storage(&fixture), std::fs::Permissions::from_mode(0o755)).unwrap();
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Committed);
    assert_eq!(
        steps,
        committed_user_repair(&fixture, &[], &fixture.backups()[0])
    );
    let mode = std::fs::metadata(storage(&fixture))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn a_failed_confirmation_fails_the_backup_and_the_leftover_entry_is_confirmed_again() {
    let fixture = Fixture::new();
    let original = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &original);
    let root = fixture.manager.runtime_root().to_path_buf();
    let storage = storage(&fixture);
    // Syncing the runtime root to confirm the new backup directory fails.
    let (outcome, steps) = traced(Some(&root), || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Unchanged);
    let mut expected = creating(&[&root, &storage]);
    expected.extend(confirmed(&root));
    assert_eq!(steps, expected);
    assert_backup_failed(&fixture, &original, &steps);
    assert!(fixture.backups().is_empty());
    // Nothing is cleaned up; the unconfirmed entry is simply never trusted:
    // the next transaction confirms it before using it.
    assert!(storage.exists());
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Committed);
    assert_eq!(
        steps,
        committed_user_repair(&fixture, &[], &fixture.backups()[0])
    );
}

#[test]
fn an_unconfirmed_runtime_root_ancestor_fails_the_backup() {
    let fixture = Fixture::with_paths(
        |home| home.join("rustx/rustx.toml"),
        |root| root.join("a/state"),
    );
    let original = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &original);
    let root = fixture.manager.runtime_root().to_path_buf();
    let a = root.parent().unwrap().to_path_buf();
    let storage = storage(&fixture);
    let (outcome, steps) = traced(Some(a.parent().unwrap()), || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Unchanged);
    // Created, then never trusted: nothing is copied below an ancestor whose
    // entry could not be confirmed.
    let mut expected = creating(&[&a, &root, &storage]);
    expected.extend(confirmed(a.parent().unwrap()));
    assert_eq!(steps, expected);
    assert_backup_failed(&fixture, &original, &steps);
    assert!(fixture.backups().is_empty());
}

#[test]
fn an_unsynced_backup_copy_fails_the_backup() {
    let fixture = Fixture::new();
    let original = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &original);
    let storage = storage(&fixture);
    let (outcome, steps) = traced(Some(&storage), || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Unchanged);
    assert_eq!(steps.last(), Some(&published(&fixture.backups()[0])));
    assert_backup_failed(&fixture, &original, &steps);
}

#[test]
fn an_existing_exact_backup_is_reused_after_its_path_is_confirmed() {
    let fixture = Fixture::new();
    let original = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &original);
    fixture.manager.test_hooks.fail_once("replace_refused");
    fixture.repair(&SourceTarget::User, &SourceTarget::User);
    let backups = fixture.backups();
    assert_eq!(backups.len(), 1);
    // The copy exists, so nothing new is linked; the path to it and its
    // directory are confirmed again before the source is replaced.
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Committed);
    let mut expected = confirmed(&storage(&fixture));
    expected.push(synced(&storage(&fixture)));
    expected.extend(source_published(&fixture.path(&SourceTarget::User)));
    assert_eq!(steps, expected);
    assert_eq!(fixture.backups(), backups);
    assert_eq!(std::fs::read(&backups[0]).unwrap(), original.as_bytes());
}

#[test]
fn healthy_sources_and_their_scope_admission_perform_no_durability_io() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, USER);
    fixture.write(&fixture.workspace, "[subagents]\nmax_concurrent = 2\n");
    let workspace = fixture.workspace.clone();
    let ((), steps) = traced(None, || {
        for scope in [&SourceTarget::User, &workspace] {
            let admitted = fixture.manager.admit_repair_scope(scope).unwrap();
            for target in [&SourceTarget::User, &workspace] {
                assert_eq!(
                    fixture.manager.repair_source(target, &admitted),
                    RepairOutcome::Unchanged
                );
            }
        }
    });
    assert_eq!(steps, []);
    // No runtime storage, writer lock, staged file, backup or receipt.
    assert!(!fixture.manager.runtime_root().exists());
    assert_eq!(
        Swap::entries(fixture.path(&SourceTarget::User).parent().unwrap()),
        ["rustx.toml"]
    );
    assert_eq!(
        Swap::entries(fixture.path(&workspace).parent().unwrap()),
        ["rustx.toml"]
    );
    assert!(fixture.receipt(&SourceTarget::User).is_none());
    assert!(fixture.receipt(&workspace).is_none());
}

#[test]
fn an_ordinary_write_confirms_its_created_source_directory_before_publication() {
    let fixture = Fixture::with_config_path(|home| home.join("fresh/rustx.toml"));
    let source = fixture.path(&SourceTarget::User);
    let directory = source.parent().unwrap();
    std::fs::remove_dir(directory).unwrap();
    let (written, steps) = traced(None, || {
        fixture
            .manager
            .write_source_settings(&SourceTarget::User, "missing", capacity(2))
    });
    written.unwrap();
    let mut expected = creating(&[directory]);
    expected.extend(source_published(&source));
    assert_eq!(steps, expected);
}

/// A second writer over the same bindings — another process, as far as the
/// first one can tell: its own ledger, hooks and transactions.
fn second_writer(fixture: &Fixture) -> UserConfigManager {
    UserConfigManager::new(fixture.manager.source_bindings().clone()).unwrap()
}

/// Writer A, paused right after creating the backup directory, before
/// confirming it.
struct PausedWriter {
    /// Signalled once A has created the entry and paused.
    paused: std::sync::mpsc::Receiver<()>,
    /// Resumes A.
    release: std::sync::mpsc::Sender<()>,
    /// A's repair outcome and its latest Workspace receipt.
    a: std::thread::JoinHandle<(RepairOutcome, Option<ConfigRepairReceipt>)>,
}

/// Writer A repairs the Workspace on its own thread and pauses right after
/// creating the backup directory, before confirming it (syncing its parent).
/// When `fail` is given, A's own sync of it fails.
fn pause_after_creating_storage(fixture: &Fixture, fail: Option<PathBuf>) -> PausedWriter {
    let (paused_tx, paused) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    durability::after_creating(&storage(fixture), move || {
        paused_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    let writer = second_writer(fixture);
    let workspace = fixture.workspace.clone();
    let a = std::thread::spawn(move || {
        durability::record(fail.as_deref());
        let scope = writer.admit_repair_scope(&workspace).unwrap();
        let outcome = writer.repair_source(&workspace, &scope);
        (outcome, writer.repairs().latest(&workspace))
    });
    PausedWriter { paused, release, a }
}

#[test]
fn a_backup_directory_another_writer_has_not_confirmed_is_confirmed_before_publication() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    fixture.write(&fixture.workspace, REPAIRABLE);
    std::fs::create_dir_all(fixture.manager.runtime_root()).unwrap();
    let PausedWriter { paused, release, a } = pause_after_creating_storage(&fixture, None);
    paused.recv().unwrap();
    // A created the entry and has not synced its parent. B sees it, and must
    // establish the barrier itself before its source is replaced.
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Committed);
    let backup = fixture
        .backups()
        .into_iter()
        .find(|backup| steps.contains(&published(backup)))
        .unwrap();
    assert_eq!(steps, committed_user_repair(&fixture, &[], &backup));
    release.send(()).unwrap();
    let (outcome, receipt) = a.join().unwrap();
    assert_eq!(outcome, RepairOutcome::Committed);
    assert_eq!(receipt.unwrap().status, ConfigRepairStatus::Repaired);
    assert_eq!(fixture.backups().len(), 2);
}

#[test]
fn a_concurrent_creator_whose_confirmation_fails_cannot_affect_the_writer_using_the_entry() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    fixture.write(&fixture.workspace, REPAIRABLE);
    let root = fixture.manager.runtime_root().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    // A's own sync of the runtime root fails after it created the entry.
    let PausedWriter { paused, release, a } = pause_after_creating_storage(&fixture, Some(root));
    paused.recv().unwrap();
    // B admits the directory A created; then A resumes, fails and returns,
    // all while B holds its admitted handle.
    let a = std::sync::Mutex::new(Some(a));
    let joined = std::sync::Arc::new(std::sync::Mutex::new(None));
    let record = joined.clone();
    fixture
        .manager
        .test_hooks
        .insert("backup_directory_opened", move || {
            release.send(()).unwrap();
            let a = a.lock().unwrap().take().unwrap();
            *record.lock().unwrap() = Some(a.join().unwrap());
        });
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    let (a_outcome, a_receipt) = joined.lock().unwrap().take().unwrap();
    assert_eq!(a_outcome, RepairOutcome::Unchanged);
    let a_receipt = a_receipt.unwrap();
    assert_eq!(a_receipt.status, ConfigRepairStatus::Unresolved);
    assert!(!a_receipt.backup_available);
    assert_eq!(fixture.read(&fixture.workspace), REPAIRABLE);
    // B depended on neither A's sync nor any cleanup by A.
    assert_eq!(outcome, RepairOutcome::Committed);
    let backups = fixture.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(steps, committed_user_repair(&fixture, &[], &backups[0]));
    assert!(
        fixture
            .receipt(&SourceTarget::User)
            .unwrap()
            .backup_available
    );
}

// Source-path durability (SRC-DUR-01..10): a repair reports Repaired only
// once its own transaction confirmed every name leading to the published
// source, whoever created those directories.

/// A User source three directories below the home directory, none of whose
/// directories any rustX transaction ever synced.
fn nested_user_source() -> (Fixture, String) {
    let fixture = Fixture::with_config_path(|home| home.join("a/b/rustx.toml"));
    let original = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &original);
    (fixture, original)
}

/// The position of `step` in `steps`, which holds it exactly once.
fn only(steps: &[Step], step: &Step) -> usize {
    let found: Vec<usize> = (0..steps.len())
        .filter(|&index| steps[index] == *step)
        .collect();
    assert_eq!(found.len(), 1, "{step:?} in {steps:?}");
    found[0]
}

/// The repair of a source failed to confirm its path: nothing was published
/// and the source keeps `original`; the backup it made is reported, and no
/// staged candidate is left beside the document.
fn assert_unconfirmed_source(
    fixture: &Fixture,
    target: &SourceTarget,
    original: &str,
    steps: &[Step],
) {
    let source = fixture.path(target);
    assert!(!steps.contains(&published(&source)), "{steps:?}");
    assert_eq!(fixture.read(target), original);
    let receipt = fixture.receipt(target).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
    assert!(receipt.after_revision.is_none());
    assert!(receipt.backup_available);
    assert_eq!(fixture.backups().len(), 1);
    assert!(
        receipt
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("could not be durably published")),
        "{receipt:?}"
    );
    assert_eq!(Swap::entries(source.parent().unwrap()), document_entries());
}

#[test]
fn an_existing_source_directory_is_confirmed_before_its_single_publication() {
    // SRC-DUR-01, SRC-DUR-08: the User source directory and the admitted
    // Workspace exist, created outside rustX and never synced by it.
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    fixture.write(&fixture.workspace, REPAIRABLE);
    let root = fixture.manager.runtime_root().to_path_buf();
    let storage = storage(&fixture);
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Committed);
    let backup = fixture.backups()[0].clone();
    assert_eq!(
        steps,
        committed_user_repair(&fixture, &[&root, &storage], &backup)
    );
    let source = fixture.path(&SourceTarget::User);
    let publication = only(&steps, &published(&source));
    for confirmation in confirmed(source.parent().unwrap()) {
        assert!(steps[..publication].contains(&confirmation), "{steps:?}");
    }
    assert_eq!(
        fixture.receipt(&SourceTarget::User).unwrap().status,
        ConfigRepairStatus::Repaired
    );
    // The Workspace is published through the handle its scope observed, and
    // that handle's path is confirmed by the transaction all the same.
    let workspace = fixture.workspace.clone();
    let (outcome, steps) = traced(None, || fixture.repair(&workspace, &workspace));
    assert_eq!(outcome, RepairOutcome::Committed);
    let source = fixture.path(&workspace);
    let backup = fixture
        .backups()
        .into_iter()
        .find(|path| steps.contains(&published(path)))
        .unwrap();
    let mut expected = confirmed(&storage);
    expected.extend([published(&backup), synced(&storage)]);
    expected.extend(source_published(&source));
    assert_eq!(steps, expected);
    only(&steps, &published(&source));
    assert_eq!(
        fixture.receipt(&workspace).unwrap().status,
        ConfigRepairStatus::Repaired
    );
}

#[test]
fn source_ancestry_created_by_another_writer_is_confirmed_before_publication() {
    // SRC-DUR-02: another writer created the whole source ancestry after
    // this manager was bound, and synced none of it.
    let fixture = Fixture::with_config_path(|home| home.join("x/y/z/rustx.toml"));
    let home = fixture.manager.source_bindings().home_directory.clone();
    let source = fixture.path(&SourceTarget::User);
    std::fs::remove_dir_all(home.join("x")).unwrap();
    let writer = {
        let directory = source.parent().unwrap().to_path_buf();
        std::thread::spawn(move || std::fs::create_dir_all(directory).unwrap())
    };
    writer.join().unwrap();
    let original = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &original);
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Committed);
    let root = fixture.manager.runtime_root().to_path_buf();
    assert_eq!(
        steps,
        committed_user_repair(
            &fixture,
            &[&root, &storage(&fixture)],
            &fixture.backups()[0]
        )
    );
    let publication = only(&steps, &published(&source));
    for entry in [home.clone(), home.join("x"), home.join("x/y")] {
        assert!(steps[..publication].contains(&synced(&entry)), "{steps:?}");
    }
    assert_eq!(fixture.read(&SourceTarget::User), USER);
}

#[test]
fn a_failed_sync_of_an_intermediate_source_ancestor_publishes_nothing() {
    // SRC-DUR-03, SRC-DUR-07: syncing `home` to confirm its entry `a` fails.
    let (fixture, original) = nested_user_source();
    let home = fixture.manager.source_bindings().home_directory.clone();
    let (outcome, steps) = traced(Some(&home), || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Unchanged);
    let source = fixture.path(&SourceTarget::User);
    let root = fixture.manager.runtime_root().to_path_buf();
    let mut expected = committed_user_repair(
        &fixture,
        &[&root, &storage(&fixture)],
        &fixture.backups()[0],
    );
    expected.truncate(expected.len() - source_published(&source).len());
    expected.extend(confirmed(&home));
    assert_eq!(steps, expected);
    assert_unconfirmed_source(&fixture, &SourceTarget::User, &original, &steps);
}

#[test]
fn a_failed_sync_of_the_source_directory_parent_publishes_nothing() {
    // SRC-DUR-04, SRC-DUR-07: syncing `a` to confirm the source directory
    // `b` itself fails.
    let (fixture, original) = nested_user_source();
    let source = fixture.path(&SourceTarget::User);
    let directory = source.parent().unwrap();
    let parent = directory.parent().unwrap();
    let (outcome, steps) = traced(Some(parent), || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Unchanged);
    let mut expected = confirmed(directory);
    expected.pop();
    assert!(steps.ends_with(&expected), "{steps:?}");
    assert_unconfirmed_source(&fixture, &SourceTarget::User, &original, &steps);
    // Nothing is replayed: the next observation is a transaction of its own,
    // which confirms the path itself and reuses the exact backup.
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Committed);
    assert!(steps.ends_with(&source_published(&source)), "{steps:?}");
    assert_eq!(fixture.backups().len(), 1);
}

#[test]
fn a_workspace_replaced_while_its_path_is_confirmed_is_never_published() {
    // SRC-DUR-05: the admitted Workspace is renamed away, and another
    // directory with the same bytes put at its pathname, while the
    // transaction's confirming walk syncs the Workspace's grandparent —
    // before it reaches the Workspace component. Installed once staging is
    // done, so the backup's own confirmation never meets it.
    let fixture = Fixture::new();
    fixture.write(&fixture.workspace, REPAIRABLE);
    let workspace = fixture.workspace.clone();
    let directory = workspace.workspace().unwrap().to_path_buf();
    let moved = directory.with_file_name("moved");
    let grandparent = directory.parent().unwrap().parent().unwrap().to_path_buf();
    let (from, to) = (directory.clone(), moved.clone());
    fixture
        .manager
        .test_hooks
        .insert("before_publication", move || {
            durability::before_syncing(&grandparent, move || {
                std::fs::rename(&from, &to).unwrap();
                std::fs::create_dir(&from).unwrap();
                std::fs::write(from.join("rustx.toml"), REPAIRABLE).unwrap();
            });
        });
    let sequence = fixture.sequence();
    let (outcome, steps) = traced(None, || fixture.repair(&workspace, &workspace));
    assert_eq!(outcome, RepairOutcome::Superseded);
    // The walk reached the replacement, whose identity is not the admitted
    // Workspace's: neither directory was published to.
    assert!(steps.ends_with(&confirmed(&directory)), "{steps:?}");
    assert!(
        !steps
            .iter()
            .any(|step| matches!(step, Step::Published(path) if path.ends_with("rustx.toml")))
    );
    assert_eq!(
        std::fs::read_to_string(moved.join("rustx.toml")).unwrap(),
        REPAIRABLE
    );
    assert_eq!(fixture.read(&workspace), REPAIRABLE);
    assert_eq!(fixture.sequence(), sequence);
}

#[test]
fn an_observed_source_directory_whose_path_names_another_directory_is_never_published() {
    // SRC-DUR-06: every component of the User source path exists again, but
    // `home` was replaced after observation, so the confirming walk ends at
    // another physical directory with the very same bytes.
    let fixture = Fixture::new();
    let original = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &original);
    let home = fixture.manager.source_bindings().home_directory.clone();
    let moved = home.with_file_name("home-moved");
    let (from, to, text) = (home.clone(), moved.clone(), original.clone());
    fixture
        .manager
        .test_hooks
        .insert("before_publication", move || {
            std::fs::rename(&from, &to).unwrap();
            std::fs::create_dir_all(from.join("rustx")).unwrap();
            std::fs::write(from.join("rustx/rustx.toml"), &text).unwrap();
        });
    let source = fixture.path(&SourceTarget::User);
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Superseded);
    assert!(
        steps.ends_with(&confirmed(source.parent().unwrap())),
        "{steps:?}"
    );
    assert!(!steps.contains(&published(&source)));
    assert!(!steps.contains(&published(&moved.join("rustx/rustx.toml"))));
    assert_eq!(fixture.read(&SourceTarget::User), original);
    assert_eq!(
        std::fs::read_to_string(moved.join("rustx/rustx.toml")).unwrap(),
        original
    );
    // The staged candidate went through the observed handle and is gone.
    assert_eq!(Swap::entries(&moved.join("rustx")), document_entries());
}

#[test]
fn a_failed_sync_after_the_confirmed_publication_is_uncertain_and_never_replayed() {
    // SRC-DUR-09: the source directory is synced only after the rename.
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, &format!("{USER}legacy = 1\n"));
    let source = fixture.path(&SourceTarget::User);
    let directory = source.parent().unwrap();
    let (outcome, steps) = traced(Some(directory), || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Committed);
    let mut expected = confirmed(directory);
    expected.push(published(&source));
    assert!(steps.ends_with(&expected), "{steps:?}");
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::CommitUncertain);
    assert_eq!(
        receipt.after_revision,
        Some(super::super::super::settings::revision(Some(
            USER.as_bytes()
        )))
    );
    let sequence = fixture.sequence();
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Unchanged);
    assert_eq!(steps, []);
    assert_eq!(fixture.sequence(), sequence);
}

// The authored file bound (SIZE-01..08): no published candidate is larger
// than its reader accepts.

/// The authored file bound, in bytes.
fn bound() -> usize {
    usize::try_from(crate::bounded_file::MAX_RESOURCE_BYTES).unwrap()
}

/// `prefix` padded with one comment line to exactly `size` bytes.
fn sized(prefix: &str, size: usize) -> String {
    let padding = size - prefix.len() - "#\n".len();
    let text = format!("{prefix}#{}\n", "x".repeat(padding));
    assert_eq!(text.len(), size);
    text
}

#[test]
fn a_repair_candidate_exactly_at_the_bound_is_published_and_readable() {
    let fixture = Fixture::new();
    let original = sized(&format!("schema_version = 9\n{USER}"), bound() - 1);
    fixture.write(&SourceTarget::User, &original);
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    let published = std::fs::read(fixture.path(&SourceTarget::User)).unwrap();
    assert_eq!(published.len(), bound());
    assert!(published.starts_with(b"schema_version = 10\n"));
    // The normal native reader accepts exactly what was published.
    let read = fixture
        .manager
        .read_source_settings(&SourceTarget::User)
        .unwrap();
    assert!(read.user.diagnostic.is_none(), "{:?}", read.user.diagnostic);
    assert!(read.user.authored.is_some());
    assert_eq!(
        read.user.revision,
        super::super::super::settings::revision(Some(&published))
    );
}

#[test]
fn a_repair_growing_past_the_bound_is_unresolved_without_a_backup() {
    let fixture = Fixture::new();
    let original = sized(&format!("schema_version = 9\n{USER}"), bound());
    fixture.write(&SourceTarget::User, &original);
    // The original itself is readable at exactly the bound.
    assert!(crate::bounded_file::read_bounded(&fixture.path(&SourceTarget::User)).is_ok());
    let candidate = plan(original.as_bytes()).unwrap().candidate;
    assert_eq!(candidate.len(), bound() + 1);
    let sequence = fixture.sequence();
    let (outcome, steps) = traced(None, || {
        fixture.repair(&SourceTarget::User, &SourceTarget::User)
    });
    assert_eq!(outcome, RepairOutcome::Unchanged);
    // Nothing was created, linked, synced or published.
    assert_eq!(steps, []);
    assert_eq!(fixture.read(&SourceTarget::User), original);
    assert!(!fixture.manager.runtime_root().exists());
    assert!(fixture.backups().is_empty());
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(fixture.sequence(), sequence + 1);
    assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
    assert_eq!(
        receipt.changes,
        [ConfigRepairChange::UpdatedSchemaVersion { from: 9, to: 10 }]
    );
    assert!(!receipt.backup_available);
    assert!(receipt.after_revision.is_none());
    assert_eq!(
        receipt.diagnostic.as_deref(),
        Some(
            "the repaired document would exceed the authored file size limit; the source is unchanged"
        )
    );
    // No writer lock was taken either: the source directory is untouched.
    assert_eq!(
        Swap::entries(fixture.path(&SourceTarget::User).parent().unwrap()),
        ["rustx.toml"]
    );
}

#[test]
fn a_repair_shrinking_an_at_limit_source_is_published() {
    let fixture = Fixture::new();
    let original = sized(&format!("{USER}legacy = 1\n"), bound());
    fixture.write(&SourceTarget::User, &original);
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    let published = std::fs::read(fixture.path(&SourceTarget::User)).unwrap();
    assert_eq!(published.len(), bound() - "legacy = 1\n".len());
    assert_eq!(
        std::fs::read(&fixture.backups()[0]).unwrap(),
        original.as_bytes()
    );
    let read = fixture
        .manager
        .read_source_settings(&SourceTarget::User)
        .unwrap();
    assert!(read.user.diagnostic.is_none(), "{:?}", read.user.diagnostic);
}

#[test]
fn ordinary_writes_never_publish_beyond_the_bound() {
    let fixture = Fixture::new();
    let instructions = |length: usize| super::super::settings::SourceMutation::Config {
        mutation: super::super::settings::ConfigMutation::Instructions {
            authored: Some("i".repeat(length)),
        },
    };
    let current = || {
        super::super::super::settings::revision(
            std::fs::read(fixture.path(&SourceTarget::User))
                .ok()
                .as_deref(),
        )
    };
    // The encoded document grows by exactly one byte per instruction byte.
    fixture
        .manager
        .write_source_settings(&SourceTarget::User, &current(), instructions(1))
        .unwrap();
    let base = fixture.read(&SourceTarget::User).len() - 1;
    let at_bound = bound() - base;
    fixture
        .manager
        .write_source_settings(&SourceTarget::User, &current(), instructions(at_bound))
        .unwrap();
    assert_eq!(fixture.read(&SourceTarget::User).len(), bound());
    let before = fixture.read(&SourceTarget::User);
    assert!(matches!(
        fixture.manager.write_source_settings(
            &SourceTarget::User,
            &current(),
            instructions(at_bound + 1)
        ),
        Err(SettingsError::Invalid)
    ));
    assert_eq!(fixture.read(&SourceTarget::User), before);
    // Nothing was staged for the refused candidate.
    assert_eq!(
        Swap::entries(fixture.path(&SourceTarget::User).parent().unwrap()),
        document_entries()
    );
}

// Observation ordering (RACE-05): the ledger frontier a read reports is never
// lower than any change it has published, even for a source it forgot.

fn ledger_receipt(target: &SourceTarget, before: &str) -> ConfigRepairReceipt {
    receipt(
        target,
        before,
        Observation {
            status: ConfigRepairStatus::Unresolved,
            after_revision: None,
            changes: Vec::new(),
            backup_available: false,
            diagnostic: Some("invalid value".into()),
        },
    )
}

#[test]
fn an_evicted_workspace_reads_as_withdrawn_at_a_frontier_after_its_last_change() {
    let ledger = RepairLedger::default();
    let workspace = |index: usize| SourceTarget::Workspace {
        directory: PathBuf::from(format!("/work/w{index}")),
    };
    ledger.record(ledger_receipt(&workspace(0), "sha256:w0"));
    let (published, _, current) = ledger.snapshot(&workspace(0));
    assert!(current.is_some());
    // A notice of W0's receipt carries `published`; then 64 newer Workspaces
    // make the ledger forget W0.
    for index in 1..=MAX_LEDGER_WORKSPACES {
        ledger.record(ledger_receipt(
            &workspace(index),
            &format!("sha256:w{index}"),
        ));
    }
    let (frontier, _, forgotten) = ledger.snapshot(&workspace(0));
    assert!(forgotten.is_none());
    // The empty read is newer than the notice: a client holding the notice
    // replaces it, and the notice arriving late cannot override the read.
    assert!(frontier > published);
    assert_eq!(
        frontier,
        u64::try_from(MAX_LEDGER_WORKSPACES).unwrap() + published
    );
    // A later observation of W0 is newer than that frontier again.
    ledger.record(ledger_receipt(&workspace(0), "sha256:w0-again"));
    let (again, _, current) = ledger.snapshot(&workspace(0));
    assert!(again > frontier);
    assert!(current.is_some());
}

// INV-01: a receipt carries native vocabulary only. Every authored identity,
// key, Tool name and value below contains the test-only sentinel `zsecretz`.

const SENTINEL: &str = "zsecretz";

/// A valid User source whose every authored identity holds the sentinel, with
/// unknown keys (also holding it) under each kind of open-map identity, an
/// unknown root key and table, and unknown Native Tool selections.
const SENTINEL_SOURCE: &str = r#"zsecretz_root_key = "zsecretz-value"

[providers.zsecretz-provider]
base_url = "http://127.0.0.1:9/v1"
api_key = "zsecretz-credential"
zsecretz_provider_key = "zsecretz-value"

[models."zsecretz-provider/zsecretz-model"]
provider = "zsecretz-provider"
id = "zsecretz-wire"
protocol = "openai_responses"
context_window = 128000
max_output_tokens = 8192
default_profile = "zsecretz-profile"
zsecretz_model_key = 1

[models."zsecretz-provider/zsecretz-model".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
[models."zsecretz-provider/zsecretz-model".profiles."zsecretz-profile"]
request_params = '{"zsecretz_param":"zsecretz-value"}'
zsecretz_profile_key = true

[mcp_tool_policies."zsecretz-mcp"]
approval = "always"
trust_all = true

[environment]
ZSECRETZ_ENV = "zsecretz-value"

[agent.model]
model = "zsecretz-provider/zsecretz-model"

[agent.tools]
builtin = ["read", "zsecretz_tool", "read", "zsecretz-\u0007-tool"]

[agent.tools.sources]
"mcp:zsecretz-source" = ["zsecretz_search"]

[zsecretz_table]
zsecretz_nested = "zsecretz-value"
"#;

fn assert_no_sentinel(receipt: &ConfigRepairReceipt) {
    let serialized = serde_json::to_string(receipt).unwrap();
    assert!(
        !serialized.to_ascii_lowercase().contains(SENTINEL),
        "{serialized}"
    );
    let debug = format!("{receipt:?}");
    assert!(!debug.to_ascii_lowercase().contains(SENTINEL), "{debug}");
}

/// REDACT-01: open-map identities (Provider, Model, Profile, MCP policy, Tool
/// source), unknown keys and unsupported Tool names never leave native
/// authority, while the schema structure, categories, positions and counts
/// that explain the repair do.
#[test]
fn receipts_never_carry_authored_identities_keys_or_tool_names() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, SENTINEL_SOURCE);
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed,
        "{:?}",
        fixture.receipt(&SourceTarget::User)
    );
    let receipt = fixture.receipt(&SourceTarget::User).unwrap();
    assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
    assert_eq!(
        receipt.changes,
        vec![
            removed(""),
            removed(""),
            removed("providers.*"),
            removed("models.*"),
            removed("models.*.profiles.*"),
            removed("mcp_tool_policies.*"),
            unknown_tool(1),
            tool(2, "read", NativeToolRemoval::Duplicate),
            unknown_tool(3),
        ]
    );
    assert_eq!(receipt.changed_count, 9);
    assert_no_sentinel(&receipt);
    // The edit itself still used the exact authored keys: every known
    // identity, value and opaque request_params survives, every unknown key
    // and unsupported Tool selection is gone.
    let repaired = fixture.read(&SourceTarget::User);
    for kept in [
        "[providers.zsecretz-provider]",
        "api_key = \"zsecretz-credential\"",
        "[models.\"zsecretz-provider/zsecretz-model\".profiles.\"zsecretz-profile\"]",
        "request_params = '{\"zsecretz_param\":\"zsecretz-value\"}'",
        "[mcp_tool_policies.\"zsecretz-mcp\"]",
        "ZSECRETZ_ENV = \"zsecretz-value\"",
        "\"mcp:zsecretz-source\" = [\"zsecretz_search\"]",
        "builtin = [\"read\"]",
    ] {
        assert!(repaired.contains(kept), "{kept} was lost:\n{repaired}");
    }
    for gone in [
        "zsecretz_root_key",
        "zsecretz_provider_key",
        "zsecretz_model_key",
        "zsecretz_profile_key",
        "trust_all",
        "zsecretz_tool",
        "zsecretz_table",
    ] {
        assert!(!repaired.contains(gone), "{gone} survived:\n{repaired}");
    }
}

/// REDACT-02: an unresolved receipt lists the eligible changes it could not
/// apply, and its diagnostic, in the same native vocabulary.
#[test]
fn unresolved_receipts_never_carry_authored_identities_or_values() {
    let fixture = Fixture::new();
    for (text, reason) in [
        // Eligible removals beside an invalid known value: unresolved, with
        // the eligible changes listed.
        (
            format!("{SENTINEL_SOURCE}\n[subagents]\nmax_concurrent = \"zsecretz-not-a-number\"\n"),
            "invalid document shape",
        ),
        // A syntax error on a line full of authored text.
        (
            format!("{SENTINEL_SOURCE}\n[zsecretz-unclosed\nzsecretz = \"zsecretz\"\n"),
            "malformed TOML",
        ),
    ] {
        fixture.write(&SourceTarget::User, &text);
        assert_eq!(
            fixture.repair(&SourceTarget::User, &SourceTarget::User),
            RepairOutcome::Unchanged
        );
        assert_eq!(fixture.read(&SourceTarget::User), text);
        let receipt = fixture.receipt(&SourceTarget::User).unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
        assert!(
            receipt.diagnostic.as_deref().unwrap().contains(reason),
            "{receipt:?}"
        );
        assert_no_sentinel(&receipt);
    }
}

/// REDACT-03: a removed known Native Tool keeps its native name and reason;
/// only a name no native definition has is withheld.
#[test]
fn known_native_tool_removals_keep_their_native_identity() {
    let control = crate::tools::native::AGENT_CONTROL_TOOL_NAMES[0];
    let plugin = crate::tools::native::TODO_TOOL_NAME;
    let text = format!(
        "[agent.tools]\nbuiltin = [\"grep\", \"{control}\", \"{plugin}\", \"grep\", \"zsecretz\"]\n"
    );
    assert_eq!(
        changes(&text),
        vec![
            tool(1, control, NativeToolRemoval::AgentControl),
            tool(2, plugin, NativeToolRemoval::PluginOwned),
            tool(3, "grep", NativeToolRemoval::Duplicate),
            unknown_tool(4),
        ]
    );
    // A name differing from a native one only by case is not that Tool.
    assert_eq!(
        changes("[agent.tools]\nbuiltin = [\"GREP\"]\n"),
        vec![unknown_tool(0)]
    );
}

/// REDACT-04: rendered paths stay within their bound, whatever the depth of
/// arrays and open maps.
#[test]
fn schema_paths_are_native_and_bounded() {
    let mut text = String::new();
    for index in 0..(MAX_RECEIPT_CHANGES * 2) {
        writeln!(
            text,
            "[mcp_tool_policies.\"zsecretz-{index}\"]\nzsecretz_{index} = 1"
        )
        .unwrap();
    }
    let planned = changes(&text);
    assert_eq!(planned.len(), MAX_RECEIPT_CHANGES * 2);
    for change in &planned {
        let ConfigRepairChange::RemovedField { object_path } = change else {
            panic!("a removed field");
        };
        assert_eq!(object_path, "mcp_tool_policies.*");
        assert!(object_path.chars().count() <= MAX_PATH_CHARS);
    }
}

// INV-02: occurrence identity, distinct from content and from ordering.

fn observed(target: &SourceTarget, status: ConfigRepairStatus) -> ConfigRepairReceipt {
    receipt(
        target,
        "sha256:a",
        Observation {
            status,
            after_revision: (status != ConfigRepairStatus::Unresolved).then(|| "sha256:b".into()),
            changes: vec![removed("")],
            backup_available: status != ConfigRepairStatus::Unresolved,
            diagnostic: None,
        },
    )
}

/// OCC-01: every publication attempt is an occurrence even with identical
/// content; an unresolved observation equal to the current episode is not;
/// a withdrawal advances ordering without naming an occurrence.
#[test]
fn occurrences_are_distinct_from_content_and_from_ordering() {
    let ledger = RepairLedger::default();
    let sequence = || *ledger.subscribe().borrow();
    let latest = || ledger.latest(&SourceTarget::User).unwrap();
    let mut seen = BTreeSet::new();
    for status in [
        ConfigRepairStatus::Repaired,
        ConfigRepairStatus::Repaired,
        ConfigRepairStatus::CommitUncertain,
        ConfigRepairStatus::CommitUncertain,
    ] {
        let before = sequence();
        ledger.record(observed(&SourceTarget::User, status));
        assert_eq!(sequence(), before + 1, "{status:?}");
        assert!(seen.insert(latest().id), "{status:?} reused an id");
    }
    ledger.record(observed(
        &SourceTarget::User,
        ConfigRepairStatus::Unresolved,
    ));
    let episode = latest();
    assert!(seen.insert(episode.id.clone()));
    let before = sequence();
    for _ in 0..3 {
        ledger.record(observed(
            &SourceTarget::User,
            ConfigRepairStatus::Unresolved,
        ));
        ledger.replace_unresolved(observed(
            &SourceTarget::User,
            ConfigRepairStatus::Unresolved,
        ));
    }
    assert_eq!(sequence(), before);
    assert_eq!(latest(), episode);
    // Changed unresolved facts are a new observation and a new occurrence.
    let mut changed = observed(&SourceTarget::User, ConfigRepairStatus::Unresolved);
    changed.diagnostic = Some("another invalid value".into());
    ledger.record(changed);
    assert_eq!(sequence(), before + 1);
    assert!(seen.insert(latest().id));
    // A withdrawal orders, names nothing; the identical episode after it is
    // a new one.
    ledger.clear_unresolved(&SourceKey::User);
    assert_eq!(sequence(), before + 2);
    assert!(ledger.latest(&SourceTarget::User).is_none());
    ledger.record(observed(
        &SourceTarget::User,
        ConfigRepairStatus::Unresolved,
    ));
    assert_eq!(sequence(), before + 3);
    let again = latest();
    assert_eq!(
        ConfigRepairReceipt {
            id: episode.id.clone(),
            ..again.clone()
        },
        episode
    );
    assert!(seen.insert(again.id));
    // Sources share one occurrence space: a Workspace never reuses an id.
    let workspace = SourceTarget::Workspace {
        directory: "/w".into(),
    };
    ledger.record(observed(&workspace, ConfigRepairStatus::Repaired));
    assert!(seen.insert(ledger.latest(&workspace).unwrap().id));
}

/// OCC-02: A (invalid) → repair commits B (R1); an external edit restores
/// exactly A → repair commits B again (R2). R2 is a new occurrence, ordered
/// after R1, announced, at the same revision B — one transaction, one
/// committed outcome, each time.
#[test]
fn a_restored_original_repaired_again_is_a_new_occurrence() {
    let fixture = Fixture::new();
    let original = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &original);
    let mut changed = fixture.manager.repairs().subscribe();
    changed.mark_unchanged();
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    assert!(changed.has_changed().unwrap());
    changed.mark_unchanged();
    let first = fixture.receipt(&SourceTarget::User).unwrap();
    let first_sequence = fixture.sequence();
    let repaired = fixture.read(&SourceTarget::User);
    // Observations of R1 never become another occurrence.
    for _ in 0..3 {
        assert_eq!(
            fixture.repair(&SourceTarget::User, &SourceTarget::User),
            RepairOutcome::Unchanged
        );
        fixture
            .manager
            .read_source_settings(&SourceTarget::User)
            .unwrap();
        let (sequence, user, _) = fixture.manager.repairs().snapshot(&SourceTarget::User);
        assert_eq!(sequence, first_sequence);
        assert_eq!(user.as_ref(), Some(&first));
    }
    assert!(!changed.has_changed().unwrap());

    fixture.write(&SourceTarget::User, &original);
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    let second = fixture.receipt(&SourceTarget::User).unwrap();
    assert_ne!(second.id, first.id);
    assert_eq!(fixture.sequence(), first_sequence + 1);
    assert!(changed.has_changed().unwrap());
    assert_eq!(second.after_revision, first.after_revision);
    assert_eq!(fixture.read(&SourceTarget::User), repaired);
    // The same content in every other respect, and one exact backup.
    assert_eq!(
        ConfigRepairReceipt {
            id: first.id.clone(),
            ..second.clone()
        },
        first
    );
    assert_eq!(fixture.backups().len(), 1);
    assert_eq!(
        fixture
            .manager
            .repairs()
            .changed_after(&SourceKey::User, first_sequence),
        Some(SourceChange::Entry(first_sequence + 1, Some(second)))
    );
}

/// OCC-03: an invalid source observed again and again is one episode; once a
/// proven recovery withdraws it, the identical invalid bytes are a new one.
#[test]
fn an_identical_unresolved_episode_after_recovery_is_observable() {
    let fixture = Fixture::new();
    fixture.write(&fixture.workspace, "[agent\n");
    fixture.repair(&fixture.workspace, &fixture.workspace);
    let first = fixture.receipt(&fixture.workspace).unwrap();
    let sequence = fixture.sequence();
    for _ in 0..3 {
        fixture.repair(&fixture.workspace, &fixture.workspace);
        fixture
            .manager
            .refresh_repair_observation(&fixture.workspace);
    }
    assert_eq!(fixture.sequence(), sequence);
    assert_eq!(fixture.receipt(&fixture.workspace).unwrap(), first);
    fixture.write(&fixture.workspace, "");
    fixture.repair(&fixture.workspace, &fixture.workspace);
    assert!(fixture.receipt(&fixture.workspace).is_none());
    assert_eq!(fixture.sequence(), sequence + 1);
    fixture.write(&fixture.workspace, "[agent\n");
    fixture.repair(&fixture.workspace, &fixture.workspace);
    let second = fixture.receipt(&fixture.workspace).unwrap();
    assert_eq!(fixture.sequence(), sequence + 2);
    assert_ne!(second.id, first.id);
    assert_eq!(second.before_revision, first.before_revision);
    assert_eq!(second.diagnostic, first.diagnostic);
}

/// OCC-04: a superseded transaction published nothing, so it names no
/// occurrence and orders nothing; the next commit is the next occurrence.
#[test]
fn a_superseded_transaction_manufactures_no_occurrence() {
    let fixture = Fixture::new();
    let original = format!("{USER}legacy = 1\n");
    fixture.write(&SourceTarget::User, &original);
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Committed
    );
    let first = fixture.receipt(&SourceTarget::User).unwrap();
    let sequence = fixture.sequence();
    fixture.write(&SourceTarget::User, &original);
    let path = fixture.path(&SourceTarget::User);
    let edit = original.clone();
    fixture
        .manager
        .test_hooks
        .insert("repair_before_lock", move || {
            // An external edit between planning and the writer lock.
            std::fs::write(&path, format!("{edit}# concurrently\n")).unwrap();
        });
    assert_eq!(
        fixture.repair(&SourceTarget::User, &SourceTarget::User),
        RepairOutcome::Superseded
    );
    assert_eq!(fixture.sequence(), sequence);
    assert_eq!(fixture.receipt(&SourceTarget::User).unwrap(), first);
}

// INV-03: source inspection has no persistent filesystem side effects, and
// its snapshot stays coherent against rustX writers and explicit about
// everyone else.

/// Every entry below `root`, with the bytes of every regular file and the
/// target of every symlink.
fn namespace(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            let content = if metadata.is_dir() {
                pending.push(path.clone());
                None
            } else if metadata.is_symlink() {
                use std::os::unix::ffi::OsStringExt;
                Some(
                    std::fs::read_link(&path)
                        .unwrap()
                        .into_os_string()
                        .into_vec(),
                )
            } else if metadata.is_file() {
                Some(std::fs::read(&path).unwrap())
            } else {
                // A FIFO or other special entry, recorded without opening it.
                Some(format!("{:?}", metadata.file_type()).into_bytes())
            };
            entries.insert(path, content);
        }
    }
    entries
}

fn read_both(fixture: &Fixture) {
    for target in [SourceTarget::User, fixture.workspace.clone()] {
        fixture.manager.read_source_settings(&target).unwrap();
    }
}

/// READ-01: absent User and Workspace sources — even an absent User
/// directory — are read without creating any directory, lock or file.
#[test]
fn reading_absent_sources_creates_nothing() {
    let fixture = Fixture::with_config_path(|home| home.join(".config/rustx/rustx.toml"));
    let root = fixture.root.path().to_path_buf();
    std::fs::remove_dir_all(root.join("home/.config")).unwrap();
    let before = namespace(&root);
    for _ in 0..3 {
        for target in [SourceTarget::User, fixture.workspace.clone()] {
            let read = fixture.manager.read_source_settings(&target).unwrap();
            assert_eq!(read.user.revision, "missing");
        }
    }
    assert_eq!(namespace(&root), before);
    assert!(!root.join("home/.config").exists());
}

/// READ-02: valid sources, a repairable one and a malformed one are read
/// without any writer artifact, backup, repair, receipt or modification.
#[test]
fn reading_present_sources_writes_nothing_and_repairs_nothing() {
    let fixture = Fixture::new();
    let root = fixture.root.path().to_path_buf();
    for (user, workspace) in [
        (
            USER.to_owned(),
            "[subagents]\nmax_concurrent = 2\n".to_owned(),
        ),
        (
            format!("{USER}legacy = 1\n"),
            "[agent]\nfuture = 1\n".to_owned(),
        ),
        ("[agent\n".to_owned(), "[broken\n".to_owned()),
    ] {
        fixture.write(&SourceTarget::User, &user);
        fixture.write(&fixture.workspace, &workspace);
        let before = namespace(&root);
        for _ in 0..3 {
            read_both(&fixture);
        }
        assert_eq!(namespace(&root), before, "{user}");
        assert_eq!(fixture.sequence(), 0);
        assert!(fixture.receipt(&SourceTarget::User).is_none());
        assert!(fixture.backups().is_empty());
    }
    assert!(!lock_of(&fixture.path(&SourceTarget::User)).exists());
    assert!(!lock_of(&fixture.path(&fixture.workspace)).exists());
}

/// Whether a rustX writer could take the lock at `lock` now.
fn writer_excluded(lock: &Path) -> bool {
    matches!(
        std::fs::File::open(lock).unwrap().try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    )
}

/// READ-03: where rustX writers have locked a document before, a reader holds
/// that same lock shared for its whole capture: every rustX writer is
/// excluded, other readers are not.
#[test]
fn a_reader_excludes_rustx_writers_through_their_existing_locks() {
    let fixture = Fixture::new();
    for target in [SourceTarget::User, fixture.workspace.clone()] {
        let revision = fixture.manager.read_source_settings(&target).unwrap();
        let expected = match &target {
            SourceTarget::User => revision.user.revision,
            SourceTarget::Workspace { .. } => revision.workspace.unwrap().revision,
        };
        fixture
            .manager
            .write_source_settings(&target, &expected, capacity(2))
            .unwrap();
    }
    let locks = [
        lock_of(&fixture.path(&SourceTarget::User)),
        lock_of(&fixture.path(&fixture.workspace)),
    ];
    let checked = Arc::new(Mutex::new(false));
    let observed = checked.clone();
    let held = locks.clone();
    fixture
        .manager
        .test_hooks
        .insert("before_capture_verification", move || {
            for lock in &held {
                assert!(writer_excluded(lock), "{lock:?}");
                std::fs::File::open(lock)
                    .unwrap()
                    .try_lock_shared()
                    .unwrap();
            }
            *observed.lock().unwrap() = true;
        });
    fixture
        .manager
        .read_source_settings(&fixture.workspace)
        .unwrap();
    assert!(*checked.lock().unwrap());
    for lock in &locks {
        assert!(!writer_excluded(lock));
    }
}

/// READ-04: a rustX writer that creates a document's lock during a read ran
/// unexcluded, so that round never answers — not even when its bytes
/// changed back (A → B → A). The next round holds the new lock and answers
/// the coherent current snapshot.
#[test]
fn a_writer_creating_the_lock_during_a_read_restarts_it() {
    for restore in [false, true] {
        let fixture = Fixture::new();
        fixture.write(&SourceTarget::User, USER);
        fixture.write(&fixture.workspace, "[subagents]\nmax_concurrent = 2\n");
        let lock = lock_of(&fixture.path(&SourceTarget::User));
        assert!(!lock.exists());
        let writer = fixture.manager.clone();
        let hooks = fixture.manager.test_hooks.clone();
        let restarted = Arc::new(Mutex::new(false));
        let observed = restarted.clone();
        fixture
            .manager
            .test_hooks
            .insert("before_capture_verification", move || {
                let original = super::super::super::settings::revision(Some(USER.as_bytes()));
                let written = writer
                    .write_source_settings(&SourceTarget::User, &original, capacity(3))
                    .unwrap();
                assert_ne!(written, original);
                if restore {
                    std::fs::write(writer.config_path(&SourceTarget::User), USER).unwrap();
                }
                // Installed only now, so the writer's own capture cannot take it.
                hooks.insert("before_documents", move || {
                    *observed.lock().unwrap() = true;
                });
            });
        let read = fixture
            .manager
            .read_source_settings(&fixture.workspace)
            .unwrap();
        assert!(*restarted.lock().unwrap(), "restore = {restore}");
        assert!(lock.exists());
        let current = std::fs::read(fixture.path(&SourceTarget::User)).unwrap();
        assert_eq!(
            read.user.revision,
            super::super::super::settings::revision(Some(&current))
        );
        assert_eq!(
            read.workspace.unwrap().revision,
            super::super::super::settings::revision(Some(
                b"[subagents]\nmax_concurrent = 2\n".as_slice()
            ))
        );
    }
}

/// READ-05: a writer outside rustX, which no advisory lock excludes, makes
/// the capture an explicit revision conflict — never a mixed snapshot.
#[test]
fn an_external_edit_during_a_read_is_an_explicit_conflict() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, USER);
    fixture.write(&fixture.workspace, "[subagents]\nmax_concurrent = 2\n");
    let path = fixture.path(&fixture.workspace);
    fixture
        .manager
        .test_hooks
        .insert("before_capture_verification", move || {
            std::fs::write(&path, "[subagents]\nmax_concurrent = 3\n").unwrap();
        });
    assert!(matches!(
        fixture.manager.read_source_settings(&fixture.workspace),
        Err(super::super::settings::SettingsError::Conflict {
            scope: SourceScope::Workspace,
            ..
        })
    ));
    let read = fixture
        .manager
        .read_source_settings(&fixture.workspace)
        .unwrap();
    assert_eq!(
        read.workspace.unwrap().revision,
        super::super::super::settings::revision(Some(
            b"[subagents]\nmax_concurrent = 3\n".as_slice()
        ))
    );
}

/// READ-06: a redirected lock name or document is refused, never followed:
/// nothing is created or locked at its target.
#[test]
fn a_redirected_lock_or_document_is_never_followed_by_a_reader() {
    let fixture = Fixture::new();
    let elsewhere = fixture.root.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let lock = lock_of(&fixture.path(&fixture.workspace));
    std::os::unix::fs::symlink(elsewhere.join("planted.lock"), &lock).unwrap();
    assert!(
        fixture
            .manager
            .read_source_settings(&fixture.workspace)
            .is_err()
    );
    assert!(!elsewhere.join("planted.lock").exists());
    std::fs::remove_file(&lock).unwrap();
    std::fs::write(
        elsewhere.join("rustx.toml"),
        "[subagents]\nmax_concurrent = 2\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        elsewhere.join("rustx.toml"),
        fixture.path(&fixture.workspace),
    )
    .unwrap();
    let before = namespace(fixture.root.path());
    assert!(
        fixture
            .manager
            .read_source_settings(&fixture.workspace)
            .is_err()
    );
    assert_eq!(namespace(fixture.root.path()), before);
}

/// A valid document outside every source, which a reader must never answer.
const EXTERNAL: &str = "# not a source\n[subagents]\nmax_concurrent = 7\n";
const WORKSPACE_A: &str = "[subagents]\nmax_concurrent = 2\n";
const WORKSPACE_B: &str = "[subagents]\nmax_concurrent = 3\n";

fn revision_of(text: &str) -> String {
    super::super::super::settings::revision(Some(text.as_bytes()))
}

/// Whether anything opens a file from now on. Linux reads the kernel's own
/// record (`inotify` `IN_OPEN`); elsewhere the outcome assertions stand alone.
struct OpenWatch(#[cfg(target_os = "linux")] nix::sys::inotify::Inotify);

impl OpenWatch {
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
    fn new(path: &Path) -> Self {
        #[cfg(target_os = "linux")]
        {
            use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
            let inotify = Inotify::init(InitFlags::IN_NONBLOCK).unwrap();
            inotify.add_watch(path, AddWatchFlags::IN_OPEN).unwrap();
            Self(inotify)
        }
        #[cfg(not(target_os = "linux"))]
        Self()
    }

    fn opened(&self) -> bool {
        #[cfg(target_os = "linux")]
        match self.0.read_events() {
            Ok(events) => !events.is_empty(),
            Err(nix::errno::Errno::EAGAIN) => false,
            Err(error) => panic!("{error}"),
        }
        #[cfg(not(target_os = "linux"))]
        false
    }
}

/// What a test observed at the opening boundary: the namespace right after
/// its replacement, and a watch on the file the replacement redirects to.
struct Reached {
    namespace: BTreeMap<PathBuf, Option<Vec<u8>>>,
    redirect: OpenWatch,
}

/// Run `replace` right before the reader opens `document` through its
/// admitted directory: its first read, or — armed when verification begins
/// — its reread. `replace` returns the file it redirects to, which is
/// watched from then on. What was reached proves the boundary was reached,
/// lets a test prove the reader changed nothing afterwards, and that it never
/// opened the redirect.
fn at_opening(
    fixture: &Fixture,
    document: &Path,
    reread: bool,
    replace: impl FnOnce() -> PathBuf + Send + 'static,
) -> Arc<Mutex<Option<Reached>>> {
    let reached = Arc::new(Mutex::new(None));
    let observed = reached.clone();
    let root = fixture.root.path().to_path_buf();
    let document = document.to_path_buf();
    let arm = move || {
        super::super::super::settings::reads::before_opening(&document, move || {
            let redirect = replace();
            let namespace = namespace(&root);
            *observed.lock().unwrap() = Some(Reached {
                namespace,
                redirect: OpenWatch::new(&redirect),
            });
        });
    };
    if reread {
        fixture
            .manager
            .test_hooks
            .insert("before_capture_verification", arm);
    } else {
        arm();
    }
    reached
}

/// READ-07 (FS-RACE-01): a document replaced by a symlink at the exact
/// boundary where the reader opens it — its first read or its reread — is
/// refused, never followed: the external file it names never becomes the
/// source, and the reader changes nothing.
#[test]
fn a_document_replaced_by_a_symlink_while_it_is_opened_is_never_followed() {
    for reread in [false, true] {
        let fixture = Fixture::new();
        fixture.write(&SourceTarget::User, USER);
        fixture.write(&fixture.workspace, WORKSPACE_A);
        let external = fixture.root.path().join("external.toml");
        std::fs::write(&external, EXTERNAL).unwrap();
        let document = fixture.path(&fixture.workspace);
        let replaced = document.clone();
        let reached = at_opening(&fixture, &document, reread, move || {
            std::fs::remove_file(&replaced).unwrap();
            std::os::unix::fs::symlink(&external, &replaced).unwrap();
            external
        });
        let read = fixture.manager.read_source_settings(&fixture.workspace);
        let reached = reached
            .lock()
            .unwrap()
            .take()
            .expect("the opening boundary was reached");
        assert!(!reached.redirect.opened(), "reread = {reread}");
        assert!(
            matches!(read, Err(super::super::settings::SettingsError::Io)),
            "reread = {reread}: {read:?}"
        );
        assert_eq!(
            namespace(fixture.root.path()),
            reached.namespace,
            "reread = {reread}"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.root.path().join("external.toml")).unwrap(),
            EXTERNAL
        );
    }
}

/// READ-08 (FS-RACE-02): a source directory, or an ancestor of it, moved away
/// and replaced — by another directory or by a symlink to one — after the
/// reader admitted it cannot rebind the capture: the reader keeps reading the
/// admitted physical directory and, since the pathname no longer names it,
/// answers nothing (`Invalid`), never the replacement's document.
#[test]
fn a_source_directory_replaced_after_admission_never_rebinds_the_capture() {
    for (source, swapped) in [
        ("workspace", "workspace"),
        ("user", "home/rustx"),
        ("user", "home"),
    ] {
        for symlink in [false, true] {
            for reread in [false, true] {
                let case = format!("{swapped} symlink={symlink} reread={reread}");
                let fixture = Fixture::new();
                fixture.write(&SourceTarget::User, USER);
                fixture.write(&fixture.workspace, WORKSPACE_A);
                let target = if source == "user" {
                    SourceTarget::User
                } else {
                    fixture.workspace.clone()
                };
                let document = fixture.path(&target);
                let root = fixture.root.path().canonicalize().unwrap();
                let relative = document
                    .strip_prefix(root.join(swapped))
                    .unwrap()
                    .to_owned();
                let reached = at_opening(&fixture, &document, reread, move || {
                    let original = root.join(swapped);
                    std::fs::rename(&original, root.join("moved")).unwrap();
                    let replacement = if symlink {
                        root.join("elsewhere")
                    } else {
                        original.clone()
                    };
                    let planted = replacement.join(&relative);
                    std::fs::create_dir_all(planted.parent().unwrap()).unwrap();
                    std::fs::write(&planted, EXTERNAL).unwrap();
                    if symlink {
                        std::os::unix::fs::symlink(&replacement, &original).unwrap();
                    }
                    planted
                });
                let read = fixture.manager.read_source_settings(&target);
                let reached = reached
                    .lock()
                    .unwrap()
                    .take()
                    .expect("the opening boundary was reached");
                assert!(!reached.redirect.opened(), "{case}");
                assert!(
                    matches!(read, Err(super::super::settings::SettingsError::Invalid)),
                    "{case}: {read:?}"
                );
                assert_eq!(namespace(fixture.root.path()), reached.namespace, "{case}");
            }
        }
    }
}

/// READ-09 (FS-RACE-03): a regular document atomically replaced by another
/// regular file during a capture yields one coherent revision or an explicit
/// conflict: replaced before the reader opens it, the capture is the
/// replacement's; replaced before its reread, a conflict naming both exact
/// revisions; replaced and restored (A → B → A), the exact admitted bytes.
#[test]
fn a_regular_document_replaced_during_a_capture_is_coherent_or_a_conflict() {
    for (reread, restore) in [(false, false), (true, false), (true, true)] {
        let case = format!("reread={reread} restore={restore}");
        let fixture = Fixture::new();
        fixture.write(&SourceTarget::User, USER);
        fixture.write(&fixture.workspace, WORKSPACE_A);
        let document = fixture.path(&fixture.workspace);
        let replaced = document.clone();
        let reached = at_opening(&fixture, &document, reread, move || {
            let staged = replaced.with_file_name("staged.tmp");
            std::fs::write(&staged, WORKSPACE_B).unwrap();
            std::fs::rename(&staged, &replaced).unwrap();
            if restore {
                std::fs::write(&staged, WORKSPACE_A).unwrap();
                std::fs::rename(&staged, &replaced).unwrap();
            }
            replaced
        });
        let read = fixture.manager.read_source_settings(&fixture.workspace);
        assert!(reached.lock().unwrap().is_some(), "{case}");
        match (reread, restore) {
            (false, _) => assert_eq!(
                read.unwrap().workspace.unwrap().revision,
                revision_of(WORKSPACE_B),
                "{case}"
            ),
            (true, false) => match read {
                Err(super::super::settings::SettingsError::Conflict {
                    scope: SourceScope::Workspace,
                    expected,
                    actual,
                }) => {
                    assert_eq!(expected, revision_of(WORKSPACE_A));
                    assert_eq!(actual, revision_of(WORKSPACE_B));
                }
                other => panic!("{case}: {other:?}"),
            },
            (true, true) => assert_eq!(
                read.unwrap().workspace.unwrap().revision,
                revision_of(WORKSPACE_A),
                "{case}"
            ),
        }
    }
}

/// Wait until some process is blocked on the lock `lock` — a level, read
/// from the kernel's lock table, never a timing assumption.
#[cfg(target_os = "linux")]
fn await_blocked_waiter(lock: &Path) {
    use std::os::unix::fs::MetadataExt;
    let inode = format!(":{} ", std::fs::metadata(lock).unwrap().ino());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !std::fs::read_to_string("/proc/locks")
        .unwrap()
        .lines()
        .any(|line| line.contains("->") && line.contains(&inode))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the writer never waited for the reader's lock"
        );
        std::thread::yield_now();
    }
}

/// READ-10 (FS-RACE-04): a native source write that starts while a capture
/// holds the document's lock shared waits for it — the capture answers the
/// exact bytes before the write, never a partial publication — and then
/// commits whole; neither deadlocks.
#[test]
fn a_native_writer_interleaved_with_a_capture_waits_and_publishes_whole() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, USER);
    let initial = fixture
        .manager
        .read_source_settings(&fixture.workspace)
        .unwrap()
        .workspace
        .unwrap()
        .revision;
    // A first native write creates the lock inode the reader then shares.
    let base = fixture
        .manager
        .write_source_settings(&fixture.workspace, &initial, capacity(2))
        .unwrap();
    let lock = lock_of(&fixture.path(&fixture.workspace));
    let writer = fixture.manager.clone();
    let target = fixture.workspace.clone();
    let expected = base.clone();
    let spawned = Arc::new(Mutex::new(None));
    let slot = spawned.clone();
    fixture
        .manager
        .test_hooks
        .insert("before_capture_verification", move || {
            assert!(writer_excluded(&lock));
            let (arrived, arrival) = std::sync::mpsc::channel();
            writer.test_hooks.insert("write_before_lock", move || {
                arrived.send(()).unwrap();
            });
            let thread = std::thread::spawn(move || {
                writer.write_source_settings(&target, &expected, capacity(3))
            });
            arrival.recv().unwrap();
            #[cfg(target_os = "linux")]
            await_blocked_waiter(&lock);
            *slot.lock().unwrap() = Some(thread);
        });
    let read = fixture
        .manager
        .read_source_settings(&fixture.workspace)
        .unwrap();
    assert_eq!(read.workspace.unwrap().revision, base);
    let thread = spawned.lock().unwrap().take().expect("the writer started");
    let written = thread.join().unwrap().unwrap();
    assert_ne!(written, base);
    assert_eq!(written, revision_of(&fixture.read(&fixture.workspace)));
    assert!(
        fixture
            .read(&fixture.workspace)
            .contains("max_concurrent = 3")
    );
}

/// READ-11 (FS-RACE-05): a User source whose directory is missing is read as
/// absent; a native writer that creates the directory, its lock and the
/// document during the capture ran unexcluded, so that round never answers.
/// The next round admits the new directory, holds its lock and answers the
/// coherent current document.
#[test]
fn a_writer_creating_the_missing_directory_during_a_read_restarts_it() {
    let fixture = Fixture::with_config_path(|home| home.join(".config/rustx/rustx.toml"));
    let root = fixture.root.path().to_path_buf();
    std::fs::remove_dir_all(root.join("home/.config")).unwrap();
    let writer = fixture.manager.clone();
    let hooks = fixture.manager.test_hooks.clone();
    let restarted = Arc::new(Mutex::new(false));
    let observed = restarted.clone();
    fixture
        .manager
        .test_hooks
        .insert("before_capture_verification", move || {
            writer
                .write_source_settings(&SourceTarget::User, "missing", capacity(3))
                .unwrap();
            hooks.insert("before_documents", move || {
                *observed.lock().unwrap() = true;
            });
        });
    let read = fixture
        .manager
        .read_source_settings(&fixture.workspace)
        .unwrap();
    assert!(*restarted.lock().unwrap());
    assert_eq!(
        read.user.revision,
        revision_of(&fixture.read(&SourceTarget::User))
    );
    assert!(lock_of(&fixture.path(&SourceTarget::User)).exists());
}

/// A fixture whose User source directory `home/.config/rustx` is missing.
fn missing_user_directory() -> (Fixture, PathBuf) {
    let fixture = Fixture::with_config_path(|home| home.join(".config/rustx/rustx.toml"));
    let directory = fixture.root.path().join("home/.config/rustx");
    std::fs::remove_dir_all(directory.parent().unwrap()).unwrap();
    (fixture, directory)
}

/// F4-RACE-01: a User source directory missing at admission, created during
/// the capture outside rustX — no writer lock — and then moved away and
/// replaced, by another directory or by a symlink to one, right before the
/// reader opens the document through it: the directory admitted at
/// verification answers nothing (`Invalid`), never a "missing" User source,
/// and the replacement's document is never opened. The reader leaves
/// nothing behind and repairs nothing.
#[test]
fn a_missing_directory_created_then_replaced_during_a_read_never_answers() {
    for symlink in [false, true] {
        let case = format!("symlink={symlink}");
        let (fixture, directory) = missing_user_directory();
        let document = fixture.path(&SourceTarget::User);
        let root = fixture.root.path().to_path_buf();
        let replaced = directory.clone();
        // Armed now: a missing directory opens nothing on the first read, so
        // the document's first opening is the reread's, through directory A.
        let reached = at_opening(&fixture, &document, false, move || {
            std::fs::rename(&replaced, root.join("moved")).unwrap();
            let replacement = if symlink {
                root.join("elsewhere")
            } else {
                replaced.clone()
            };
            std::fs::create_dir(&replacement).unwrap();
            std::fs::write(replacement.join("rustx.toml"), USER).unwrap();
            if symlink {
                std::os::unix::fs::symlink(&replacement, &replaced).unwrap();
            }
            replacement.join("rustx.toml")
        });
        // The admission found nothing; directory A appears, empty, before
        // the reread admits it.
        let created = directory.clone();
        fixture
            .manager
            .test_hooks
            .insert("before_capture_verification", move || {
                assert!(!created.parent().unwrap().exists());
                std::fs::create_dir_all(&created).unwrap();
            });
        let read = fixture.manager.read_source_settings(&SourceTarget::User);
        let reached = reached
            .lock()
            .unwrap()
            .take()
            .expect("the opening boundary was reached");
        assert!(!reached.redirect.opened(), "{case}");
        assert!(
            matches!(read, Err(super::super::settings::SettingsError::Invalid)),
            "{case}: {read:?}"
        );
        assert_eq!(namespace(fixture.root.path()), reached.namespace, "{case}");
        let moved = fixture.root.path().join("moved");
        assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 0, "{case}");
        assert!(!lock_of(&document).exists(), "{case}");
        assert_eq!(fixture.read(&SourceTarget::User), USER, "{case}");
        assert_eq!(fixture.sequence(), 0, "{case}");
        assert!(fixture.receipt(&SourceTarget::User).is_none(), "{case}");
        assert!(fixture.backups().is_empty(), "{case}");
    }
}

/// F4-RACE-02: a User source directory missing at admission and created
/// during the capture outside rustX, then left in place: empty, the source
/// is still missing; holding a document, the capture is an explicit
/// conflict naming it, never a "missing" User source. The reader creates no
/// writer lock, and the next capture answers the document.
#[test]
fn a_missing_directory_created_during_a_read_is_compared_like_any_change() {
    for content in [None, Some(USER)] {
        let case = format!("document={}", content.is_some());
        let (fixture, directory) = missing_user_directory();
        let created = directory.clone();
        fixture
            .manager
            .test_hooks
            .insert("before_capture_verification", move || {
                std::fs::create_dir_all(&created).unwrap();
                if let Some(text) = content {
                    std::fs::write(created.join("rustx.toml"), text).unwrap();
                }
            });
        let read = fixture.manager.read_source_settings(&SourceTarget::User);
        match content {
            None => assert_eq!(read.unwrap().user.revision, "missing", "{case}"),
            Some(text) => match read {
                Err(super::super::settings::SettingsError::Conflict {
                    scope: SourceScope::User,
                    expected,
                    actual,
                }) => {
                    assert_eq!(expected, "missing");
                    assert_eq!(actual, revision_of(text));
                }
                other => panic!("{case}: {other:?}"),
            },
        }
        let document = fixture.path(&SourceTarget::User);
        assert!(!lock_of(&document).exists(), "{case}");
        let current = fixture.manager.read_source_settings(&SourceTarget::User);
        assert_eq!(
            current.unwrap().user.revision,
            content.map_or_else(|| "missing".to_owned(), revision_of),
            "{case}"
        );
        assert!(!lock_of(&document).exists(), "{case}");
        assert_eq!(fixture.sequence(), 0, "{case}");
    }
}

fn session_input(fixture: &Fixture) -> super::super::SessionConfigInput {
    super::super::SessionConfigInput::new(fixture.workspace.workspace().unwrap().to_path_buf())
}

/// Session resolution's source revisions, or its failure detail.
fn resolved(fixture: &Fixture) -> Result<BTreeMap<PathBuf, String>, String> {
    fixture
        .manager
        .resolve_session(&session_input(fixture))
        .map(|prospective| prospective.source_revisions)
        .map_err(|failure| failure.to_string())
}

/// Configuration application's capture, or its failure.
fn captured(fixture: &Fixture) -> Result<(), String> {
    fixture
        .manager
        .capture_application(&session_input(fixture))
        .map(drop)
}

/// FS-AUTH-01: a symlinked Workspace `rustx.toml` is refused by every
/// consumer alike — Session resolution, configuration application capture,
/// source capture and automatic repair — never followed by one while
/// refused by another; its target is never opened and nothing changes.
#[test]
fn a_symlinked_workspace_source_is_refused_by_every_consumer() {
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, USER);
    let external = fixture.root.path().join("external.toml");
    std::fs::write(&external, WORKSPACE_A).unwrap();
    std::os::unix::fs::symlink(&external, fixture.path(&fixture.workspace)).unwrap();
    let before = namespace(fixture.root.path());
    let watch = OpenWatch::new(&external);
    let failure = resolved(&fixture).unwrap_err();
    assert!(failure.contains("not a symlink"), "{failure}");
    let failure = captured(&fixture).unwrap_err();
    assert!(failure.contains("not a symlink"), "{failure}");
    assert!(matches!(
        fixture.manager.read_source_settings(&fixture.workspace),
        Err(super::super::settings::SettingsError::Io)
    ));
    assert_eq!(
        fixture.repair(&fixture.workspace, &fixture.workspace),
        RepairOutcome::Unchanged
    );
    // The User source alone is unaffected.
    fixture.manager.app_server_policy().unwrap();
    assert!(!watch.opened());
    assert_eq!(namespace(fixture.root.path()), before);
    assert!(fixture.receipt(&fixture.workspace).is_none());
    assert_eq!(fixture.sequence(), 0);
}

/// FS-AUTH-02: the User source is bound once, to the canonical pathname of
/// its binding — a symlink named by `--config` is resolved then, as the
/// process binding. Afterwards no consumer follows a symlink at the bound
/// pathname, at the document or at its directory: Session resolution,
/// application capture, the process policy and source capture all refuse
/// it, and the redirect's target is never opened.
#[test]
fn a_symlinked_user_source_cannot_bypass_its_bound_authority() {
    let fixture = Fixture::with_config_path(|home| {
        std::fs::write(home.join("bound.toml"), USER).unwrap();
        std::os::unix::fs::symlink(home.join("bound.toml"), home.join("link.toml")).unwrap();
        home.join("link.toml")
    });
    let home = fixture.root.path().canonicalize().unwrap().join("home");
    let bound = fixture.path(&SourceTarget::User);
    assert_eq!(bound, home.join("bound.toml"));
    // The binding is the pathname, not the symlink it was named through.
    std::fs::remove_file(home.join("link.toml")).unwrap();
    std::os::unix::fs::symlink(fixture.root.path().join("nothing"), home.join("link.toml"))
        .unwrap();
    assert_eq!(resolved(&fixture).unwrap()[&bound], revision_of(USER));

    for redirected in ["document", "directory"] {
        let fixture = Fixture::new();
        fixture.write(&SourceTarget::User, USER);
        let document = fixture.path(&SourceTarget::User);
        let root = fixture.root.path().canonicalize().unwrap();
        let external = if redirected == "document" {
            let external = root.join("external.toml");
            std::fs::write(&external, USER).unwrap();
            std::fs::remove_file(&document).unwrap();
            std::os::unix::fs::symlink(&external, &document).unwrap();
            external
        } else {
            let directory = document.parent().unwrap();
            std::fs::rename(directory, root.join("moved")).unwrap();
            std::fs::create_dir(root.join("elsewhere")).unwrap();
            std::fs::write(root.join("elsewhere/rustx.toml"), USER).unwrap();
            std::os::unix::fs::symlink(root.join("elsewhere"), directory).unwrap();
            root.join("elsewhere/rustx.toml")
        };
        let expected = if redirected == "document" {
            "not a symlink"
        } else {
            "without following a symlink"
        };
        let before = namespace(fixture.root.path());
        let watch = OpenWatch::new(&external);
        for failure in [
            resolved(&fixture).unwrap_err(),
            captured(&fixture).unwrap_err(),
            fixture.manager.app_server_policy().unwrap_err(),
        ] {
            assert!(failure.contains(expected), "{redirected}: {failure}");
        }
        assert!(
            fixture
                .manager
                .read_source_settings(&SourceTarget::User)
                .is_err(),
            "{redirected}"
        );
        assert!(!watch.opened(), "{redirected}");
        assert_eq!(namespace(fixture.root.path()), before, "{redirected}");
    }
}

/// FS-AUTH-03: a regular source replaced by a symlink right before Session
/// resolution opens it — its first read or the capture's reread — is
/// refused, never followed: the external file it names is never opened, and
/// resolution changes nothing.
#[test]
fn a_source_replaced_by_a_symlink_at_its_opening_is_never_followed_by_resolution() {
    for user in [false, true] {
        for reread in [false, true] {
            let case = format!("user={user} reread={reread}");
            let fixture = Fixture::new();
            fixture.write(&SourceTarget::User, USER);
            fixture.write(&fixture.workspace, WORKSPACE_A);
            let external = fixture.root.path().join("external.toml");
            std::fs::write(&external, EXTERNAL).unwrap();
            let target = if user {
                SourceTarget::User
            } else {
                fixture.workspace.clone()
            };
            let document = fixture.path(&target);
            let replaced = document.clone();
            let reached = at_opening(&fixture, &document, reread, move || {
                std::fs::remove_file(&replaced).unwrap();
                std::os::unix::fs::symlink(&external, &replaced).unwrap();
                external
            });
            let failure = resolved(&fixture).unwrap_err();
            let reached = reached
                .lock()
                .unwrap()
                .take()
                .expect("the opening boundary was reached");
            assert!(!reached.redirect.opened(), "{case}");
            assert!(failure.contains("not a symlink"), "{case}: {failure}");
            assert_eq!(namespace(fixture.root.path()), reached.namespace, "{case}");
        }
    }
}

/// FS-AUTH-04: a source directory, or an ancestor of it, moved away and
/// replaced — by another directory or a symlink to one, holding identical
/// bytes — after Session resolution admitted it, cannot rebind the capture:
/// resolution keeps reading the admitted physical directory and, since the
/// pathname no longer names it, answers nothing, never the replacement,
/// whose document is never opened.
#[test]
fn a_source_directory_replaced_during_resolution_never_rebinds_it() {
    for (source, swapped) in [
        ("workspace", "workspace"),
        ("user", "home/rustx"),
        ("user", "home"),
    ] {
        for symlink in [false, true] {
            for reread in [false, true] {
                let case = format!("{swapped} symlink={symlink} reread={reread}");
                let fixture = Fixture::new();
                fixture.write(&SourceTarget::User, USER);
                fixture.write(&fixture.workspace, WORKSPACE_A);
                let (target, text) = if source == "user" {
                    (SourceTarget::User, USER)
                } else {
                    (fixture.workspace.clone(), WORKSPACE_A)
                };
                let document = fixture.path(&target);
                let root = fixture.root.path().canonicalize().unwrap();
                let relative = document
                    .strip_prefix(root.join(swapped))
                    .unwrap()
                    .to_owned();
                let reached = at_opening(&fixture, &document, reread, move || {
                    let original = root.join(swapped);
                    std::fs::rename(&original, root.join("moved")).unwrap();
                    let replacement = if symlink {
                        root.join("elsewhere")
                    } else {
                        original.clone()
                    };
                    let planted = replacement.join(&relative);
                    std::fs::create_dir_all(planted.parent().unwrap()).unwrap();
                    std::fs::write(&planted, text).unwrap();
                    if symlink {
                        std::os::unix::fs::symlink(&replacement, &original).unwrap();
                    }
                    planted
                });
                let failure = resolved(&fixture).unwrap_err();
                let reached = reached
                    .lock()
                    .unwrap()
                    .take()
                    .expect("the opening boundary was reached");
                assert!(!reached.redirect.opened(), "{case}");
                assert!(
                    failure.contains("replaced or redirected"),
                    "{case}: {failure}"
                );
                assert_eq!(namespace(fixture.root.path()), reached.namespace, "{case}");
            }
        }
    }
}

/// FS-AUTH-05: Session capture holds no writer lock and needs none — rustX
/// writers publish only by atomic replacement — so a native write during
/// the capture, after both documents were read and before their reread,
/// runs unblocked; the capture is then an explicit "changed during capture"
/// failure, never a mix of both revisions. The next capture answers exactly
/// the revision the writer committed, as source capture reports it.
#[test]
fn a_native_writer_during_a_session_capture_is_never_mixed_into_it() {
    for application in [false, true] {
        let case = format!("application={application}");
        let fixture = Fixture::new();
        fixture.write(&SourceTarget::User, USER);
        let workspace = fixture.workspace.clone();
        let document = fixture.path(&workspace);
        let initial = fixture
            .manager
            .read_source_settings(&workspace)
            .unwrap()
            .workspace
            .unwrap()
            .revision;
        // A first native write creates the lock a reader could share.
        let base = fixture
            .manager
            .write_source_settings(&workspace, &initial, capacity(2))
            .unwrap();
        assert_eq!(resolved(&fixture).unwrap()[&document], base, "{case}");
        let lock = lock_of(&document);
        let writer = fixture.manager.clone();
        let committed = Arc::new(Mutex::new(None));
        let slot = committed.clone();
        let target = workspace.clone();
        let write = move || {
            assert!(!writer_excluded(&lock), "the capture holds no lock");
            let written = writer
                .write_source_settings(&target, &base, capacity(3))
                .unwrap();
            *slot.lock().unwrap() = Some(written);
        };
        let failure = if application {
            fixture
                .manager
                .capture_application_at_boundary(&session_input(&fixture), write)
                .map(drop)
                .unwrap_err()
        } else {
            fixture
                .manager
                .test_hooks
                .insert("before_capture_verification", write);
            resolved(&fixture).unwrap_err()
        };
        assert!(
            failure.contains("configuration changed during capture"),
            "{case}: {failure}"
        );
        let written = committed
            .lock()
            .unwrap()
            .take()
            .expect("the writer committed");
        assert_eq!(written, revision_of(&fixture.read(&workspace)), "{case}");
        assert_eq!(resolved(&fixture).unwrap()[&document], written, "{case}");
        captured(&fixture).unwrap();
        let read = fixture.manager.read_source_settings(&workspace).unwrap();
        assert_eq!(read.workspace.unwrap().revision, written, "{case}");
    }
}

/// FS-AUTH-07: Session resolution, source capture and automatic repair agree
/// on the identity of one valid source — the same bound pathname and the
/// revision of the same bytes — both for a source that is healthy as it is
/// and for one automatic repair has just committed.
#[test]
fn resolution_inspection_and_repair_agree_on_one_source_identity() {
    for repaired in [false, true] {
        let case = format!("repaired={repaired}");
        let fixture = Fixture::new();
        fixture.write(&SourceTarget::User, USER);
        let workspace = fixture.workspace.clone();
        let text = if repaired {
            format!("{WORKSPACE_A}legacy_field = true\n")
        } else {
            WORKSPACE_A.to_owned()
        };
        fixture.write(&workspace, &text);
        let outcome = fixture.repair(&workspace, &workspace);
        let current = fixture.read(&workspace);
        let receipt = fixture.receipt(&workspace);
        if repaired {
            assert_eq!(outcome, RepairOutcome::Committed, "{case}");
            let receipt = receipt.expect("a committed receipt");
            assert_eq!(receipt.target, workspace, "{case}");
            assert_eq!(receipt.before_revision, revision_of(&text), "{case}");
            assert_eq!(
                receipt.after_revision,
                Some(revision_of(&current)),
                "{case}"
            );
        } else {
            assert_eq!(outcome, RepairOutcome::Unchanged, "{case}");
            assert!(receipt.is_none(), "{case}");
            assert_eq!(current, text, "{case}");
        }
        let read = fixture.manager.read_source_settings(&workspace).unwrap();
        let view = read.workspace.unwrap();
        let resolution = resolved(&fixture).unwrap();
        for (path, revision) in [
            (&read.user.path, &read.user.revision),
            (&view.path, &view.revision),
        ] {
            assert_eq!(resolution.get(path), Some(revision), "{case}: {path:?}");
        }
        assert_eq!(view.path, fixture.path(&workspace), "{case}");
        assert_eq!(view.revision, revision_of(&current), "{case}");
        assert_eq!(read.user.revision, revision_of(USER), "{case}");
    }
}

/// Plants an entry at a path, given a redirect target and the exact-bound text.
type Plant = fn(&Path, &Path, &str);

/// READ-12 (FS-RACE-06/07): symlinks, FIFOs and directories at a source or
/// resource document are refused without blocking; the exact 1 MiB bound is
/// enforced on the opened descriptor. None of these reads, nor reading
/// missing, valid, repairable or malformed sources (READ-01, READ-02),
/// leaves anything behind.
#[test]
fn unsupported_entries_and_oversized_documents_are_refused_without_effects() {
    let bound = usize::try_from(crate::bounded_file::MAX_RESOURCE_BYTES).unwrap();
    let exact = format!("#{}\n", "x".repeat(bound - 2));
    let fixture = Fixture::new();
    fixture.write(&SourceTarget::User, USER);
    let document = fixture.path(&fixture.workspace);
    let resource = fixture.root.path().join("workspace/mcp.toml");
    let elsewhere = fixture.root.path().join("elsewhere.toml");
    std::fs::write(&elsewhere, EXTERNAL).unwrap();
    let cases: [(&str, Plant, bool); 5] = [
        (
            "symlink",
            |path, elsewhere, _| std::os::unix::fs::symlink(elsewhere, path).unwrap(),
            false,
        ),
        (
            "fifo",
            |path, _, _| nix::unistd::mkfifo(path, nix::sys::stat::Mode::S_IRWXU).unwrap(),
            false,
        ),
        (
            "directory",
            |path, _, _| std::fs::create_dir(path).unwrap(),
            false,
        ),
        (
            "exact bound",
            |path, _, exact| std::fs::write(path, exact).unwrap(),
            true,
        ),
        (
            "over bound",
            |path, _, exact| std::fs::write(path, format!("{exact} ")).unwrap(),
            false,
        ),
    ];
    for (case, plant, accepted) in cases {
        for path in [&document, &resource] {
            plant(path, &elsewhere, &exact);
        }
        let before = namespace(fixture.root.path());
        let read = fixture.manager.read_source_settings(&fixture.workspace);
        let resource_read = super::super::super::settings::read_document(&resource);
        assert_eq!(namespace(fixture.root.path()), before, "{case}");
        if accepted {
            assert_eq!(
                read.unwrap().workspace.unwrap().revision,
                revision_of(&exact)
            );
            assert_eq!(resource_read.unwrap().unwrap(), exact.as_bytes());
        } else {
            assert!(
                matches!(read, Err(super::super::settings::SettingsError::Io)),
                "{case}: {read:?}"
            );
            assert!(resource_read.is_err(), "{case}");
        }
        for path in [&document, &resource] {
            if std::fs::symlink_metadata(path).unwrap().is_dir() {
                std::fs::remove_dir(path).unwrap();
            } else {
                std::fs::remove_file(path).unwrap();
            }
        }
    }
    assert_eq!(
        super::super::super::settings::read_document(&resource).unwrap(),
        None
    );
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), EXTERNAL);
}

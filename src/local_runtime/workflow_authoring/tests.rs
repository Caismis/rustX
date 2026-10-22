//! Native Workflow file authoring contracts (Issue #491).
use super::super::configuration::UserConfigManager;
use super::super::configuration::settings::{SettingsError, SourceMutation, SourceTarget};
use super::super::launch::{HostEnvironment, LaunchRequest};
use super::*;
use crate::runtime::workflow::{
    WorkflowAdmissionDiagnostic, WorkflowDependencyFailure, WorkflowProgram,
};
use std::path::PathBuf;

struct Roots {
    _root: tempfile::TempDir,
    manager: UserConfigManager,
    home: PathBuf,
    a: PathBuf,
    b: PathBuf,
}

impl Roots {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let home = base.join("home");
        let a = base.join("workspace-a");
        let b = base.join("workspace-b");
        for directory in [&a, &b, &home.join("rustx/.agents/agents")] {
            std::fs::create_dir_all(directory).unwrap();
        }
        // Credentials are an environment reference that is never resolved:
        // offline checks must not need them.
        std::fs::write(
            home.join("rustx/rustx.toml"),
            r#"
[providers.local]
base_url = "http://127.0.0.1:9/v1"
api_key = "$RUSTX_ISSUE_491_UNSET"
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
        std::fs::write(
            home.join("rustx/.agents/agents/reviewer.toml"),
            "description = \"Review.\"\ninstructions = \"Review the input.\"\n\n[tools]\nbuiltin = []\n",
        )
        .unwrap();
        let request = LaunchRequest {
            workspace: Some(a.clone()),
            ..LaunchRequest::default()
        };
        let host = HostEnvironment::from_paths(a.clone(), home.clone()).unwrap();
        let (manager, _) = request.session_input(&host).unwrap();
        Self {
            _root: root,
            manager,
            home,
            a,
            b,
        }
    }

    fn user_directory(&self) -> PathBuf {
        self.home.join("rustx/.agents/workflows")
    }

    fn write(
        &self,
        target: &SourceTarget,
        id: &str,
        expected: &str,
        edit: WorkflowEdit,
    ) -> Result<String, SettingsError> {
        self.manager.write_source_settings(
            target,
            expected,
            SourceMutation::Workflow {
                id: WorkflowId::parse(id).unwrap(),
                edit,
            },
        )
    }

    fn create(&self, target: &SourceTarget, id: &str, description: &str) -> String {
        self.write(
            target,
            id,
            "missing",
            WorkflowEdit::Create {
                description: description.into(),
            },
        )
        .unwrap()
    }

    fn read(&self, target: &SourceTarget, id: &str) -> WorkflowSource {
        self.manager
            .workflow_source(target, &WorkflowId::parse(id).unwrap())
            .unwrap()
    }
}

fn workspace(directory: &Path) -> SourceTarget {
    SourceTarget::Workspace {
        directory: directory.to_path_buf(),
    }
}

fn id(value: &str) -> WorkflowId {
    WorkflowId::parse(value).unwrap()
}

fn definition(yaml: &str) -> WorkflowDefinition {
    serde_yaml::from_str(yaml).unwrap()
}

const AGENT_PROGRAM: &str = r"
description: Review one proposal.
block:
  input: {type: object, properties: {proposal: {type: string}}, required: [proposal], additionalProperties: false}
  output: {type: object, properties: {summary: {type: string}}, required: [summary], additionalProperties: false}
  entry: review
  nodes:
    review:
      type: agent
      profile: reviewer
      task: Review it.
      input:
        proposal: {type: reference, path: [args, proposal]}
      output: {type: object, properties: {summary: {type: string}}, required: [summary], additionalProperties: false}
    done:
      type: return
      output: {type: reference, path: [review]}
  edges:
  - {from: review, to: done}
";

#[test]
fn the_same_identity_is_independently_addressable_in_every_scope() {
    let roots = Roots::new();
    let (user, a, b) = (SourceTarget::User, workspace(&roots.a), workspace(&roots.b));
    for (target, description) in [(&user, "global"), (&a, "workspace a"), (&b, "workspace b")] {
        roots.create(target, "review", description);
    }
    // Each listing holds exactly the file its scope physically owns.
    for (target, directory, description, shadows) in [
        (&user, roots.user_directory(), "global", false),
        (&a, roots.a.join(".agents/workflows"), "workspace a", true),
        (&b, roots.b.join(".agents/workflows"), "workspace b", true),
    ] {
        let library = roots.manager.workflow_library(target).unwrap();
        assert_eq!(library.directory, directory);
        assert_eq!(library.files.len(), 1, "{library:?}");
        let file = &library.files[0];
        assert_eq!(file.id, Some(id("review")));
        assert_eq!(
            library.directory.join(&file.file_name),
            directory.join("review.yaml")
        );
        assert_eq!(file.relative_path, ".agents/workflows/review.yaml");
        assert_eq!(file.description.as_deref(), Some(description));
        assert_eq!(file.shadows_global, shadows);
        assert!(file.diagnostic.is_none());
        assert_eq!(roots.read(target, "review").revision, file.revision);
    }
    let user_bytes = std::fs::read(roots.user_directory().join("review.yaml")).unwrap();
    let b_bytes = std::fs::read(roots.b.join(".agents/workflows/review.yaml")).unwrap();

    // Save, copy and delete in Workspace A touch only Workspace A.
    let source = roots.read(&a, "review");
    let mut document = *source.document.unwrap();
    document.description = "edited in a".into();
    let saved = roots
        .write(
            &a,
            "review",
            &source.revision,
            WorkflowEdit::Replace {
                document: Box::new(document.clone()),
            },
        )
        .unwrap();
    assert_eq!(roots.read(&a, "review").revision, saved);
    assert_eq!(*roots.read(&a, "review").document.unwrap(), document);
    roots
        .write(
            &a,
            "review_copy",
            "missing",
            WorkflowEdit::Copy {
                source: id("review"),
                source_revision: saved.clone(),
            },
        )
        .unwrap();
    assert!(roots.read(&a, "review_copy").present);
    assert!(!roots.read(&user, "review_copy").present);
    assert!(!roots.read(&b, "review_copy").present);
    let effective = || {
        super::super::workflow_resources::load(Some(&roots.a), &roots.home.join("rustx/.agents"))
            .unwrap()
            .locations[&id("review")]
            .clone()
    };
    assert_eq!(
        effective().path,
        roots.a.join(".agents/workflows/review.yaml")
    );
    roots
        .write(&a, "review", &saved, WorkflowEdit::Delete)
        .unwrap();
    // Deleting the override exposes the Global file; it deletes nothing else.
    assert!(!roots.read(&a, "review").present);
    assert_eq!(effective().path, roots.user_directory().join("review.yaml"));
    assert_eq!(
        std::fs::read(roots.user_directory().join("review.yaml")).unwrap(),
        user_bytes
    );
    assert_eq!(
        std::fs::read(roots.b.join(".agents/workflows/review.yaml")).unwrap(),
        b_bytes
    );
}

#[test]
fn create_writes_the_native_minimal_program_and_never_overwrites() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    let revision = roots.create(&target, "fresh", "A fresh Workflow");
    let source = roots.read(&target, "fresh");
    assert_eq!(source.revision, revision);
    assert!(source.diagnostic.is_none());
    let document = source.document.unwrap();
    assert_eq!(document.block.entry, "done");
    WorkflowProgram::compile(id("fresh"), *document).unwrap();
    // An existing target is a conflict for the absent revision, and an
    // invalid create even with its exact revision.
    assert!(matches!(
        roots.write(
            &target,
            "fresh",
            "missing",
            WorkflowEdit::Create {
                description: "again".into()
            }
        ),
        Err(SettingsError::Conflict { .. })
    ));
    assert!(matches!(
        roots.write(
            &target,
            "fresh",
            &revision,
            WorkflowEdit::Create {
                description: "again".into()
            }
        ),
        Err(SettingsError::Invalid)
    ));
    // An empty description is a structural rejection with its native path.
    let Err(SettingsError::InvalidWorkflow(diagnostic)) = roots.write(
        &target,
        "empty",
        "missing",
        WorkflowEdit::Create {
            description: " ".into(),
        },
    ) else {
        panic!("empty description must be invalid");
    };
    assert_eq!(diagnostic.path, "description");
    assert!(!roots.read(&target, "empty").present);
    assert_eq!(roots.read(&target, "fresh").revision, revision);
}

#[test]
fn identities_are_native_and_reserved_names_are_refused() {
    for invalid in [
        "",
        "Upper",
        "1st",
        "has space",
        "dot.ted",
        "../escape",
        "workflow_output",
    ] {
        assert!(WorkflowId::parse(invalid).is_err(), "{invalid:?}");
        let mutation =
            serde_json::json!({"kind": "workflow", "id": invalid, "edit": {"operation": "delete"}});
        assert!(serde_json::from_value::<SourceMutation>(mutation).is_err());
    }
}

#[test]
fn invalid_candidates_leave_the_original_file_untouched() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    let revision = roots.create(&target, "flow", "Valid");
    let path = roots.a.join(".agents/workflows/flow.yaml");
    let before = std::fs::read(&path).unwrap();
    let mut broken = *roots.read(&target, "flow").document.unwrap();
    broken.block.entry = "absent".into();
    let Err(SettingsError::InvalidWorkflow(diagnostic)) = roots.write(
        &target,
        "flow",
        &revision,
        WorkflowEdit::Replace {
            document: Box::new(broken),
        },
    ) else {
        panic!("dangling entry must be refused");
    };
    assert_eq!(diagnostic.path, "block.entry");
    assert_eq!(diagnostic.category, "workflow_language");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // A nested rejection maps to its exact lexical block and field.
    let mut nested = definition(include_str!(
        "../../../examples/local-runtime/.agents/workflows/parallel_review.yaml"
    ));
    let crate::runtime::workflow::WorkflowNodeDefinition::Parallel { branches } =
        nested.block.nodes.get_mut("reviews").unwrap()
    else {
        panic!("parallel")
    };
    branches.get_mut("quality").unwrap().block.edges[0].to = "missing".into();
    let Err(SettingsError::InvalidWorkflow(diagnostic)) = roots.write(
        &target,
        "flow",
        &revision,
        WorkflowEdit::Replace {
            document: Box::new(nested),
        },
    ) else {
        panic!("dangling nested edge must be refused");
    };
    assert_eq!(
        diagnostic.path,
        "block.nodes.reviews.branches.quality.block.edges.0.to"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn malformed_files_are_reported_and_never_replaced_by_empty_programs() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    let directory = roots.a.join(".agents/workflows");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("broken.yaml"), "description: [").unwrap();
    std::fs::write(directory.join("Bad Name.yaml"), "description: x").unwrap();
    std::fs::write(
        directory.join("unsafe.yaml"),
        AGENT_PROGRAM
            .replace("Review it.", "Review it.\n      # 1")
            .replace(
                "proposal: {type: reference, path: [args, proposal]}\n      output",
                "proposal: {type: literal, value: 9007199254740993}\n      output",
            ),
    )
    .unwrap();
    std::fs::write(directory.join("notes.txt"), "ignored").unwrap();
    let library = roots.manager.workflow_library(&target).unwrap();
    let names: Vec<_> = library
        .files
        .iter()
        .map(|file| file.file_name.as_str())
        .collect();
    assert_eq!(names, ["Bad Name.yaml", "broken.yaml", "unsafe.yaml"]);
    assert_eq!(library.files[0].id, None);
    assert_eq!(
        library.files[0].diagnostic.as_ref().unwrap().category,
        "identity"
    );
    let broken = &library.files[1];
    let diagnostic = broken.diagnostic.as_ref().unwrap();
    assert_eq!(diagnostic.category, "workflow_language");
    assert!(diagnostic.line.is_some());
    assert!(broken.description.is_none());
    let unsafe_number = &library.files[2];
    assert_eq!(
        unsafe_number.diagnostic.as_ref().unwrap().category,
        "unsupported_number"
    );

    // Reading keeps the bytes and their revision and offers no document.
    let source = roots.read(&target, "broken");
    assert!(source.present);
    assert!(source.document.is_none());
    assert_eq!(source.revision, broken.revision);
    assert_eq!(
        std::fs::read(directory.join("broken.yaml")).unwrap(),
        b"description: ["
    );
    assert!(roots.read(&target, "unsafe").document.is_none());

    // An explicit whole-file replacement against the exact revision repairs it.
    let repaired = definition(AGENT_PROGRAM);
    roots
        .write(
            &target,
            "broken",
            &source.revision,
            WorkflowEdit::Replace {
                document: Box::new(repaired.clone()),
            },
        )
        .unwrap();
    assert_eq!(*roots.read(&target, "broken").document.unwrap(), repaired);
}

#[test]
fn revisions_guard_external_edits_and_competing_editors() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    let base = roots.create(&target, "flow", "Base");
    let path = roots.a.join(".agents/workflows/flow.yaml");
    let document = *roots.read(&target, "flow").document.unwrap();
    let edit = |description: &str| {
        let mut document = document.clone();
        document.description = description.into();
        WorkflowEdit::Replace {
            document: Box::new(document),
        }
    };
    // Two editors from the same base: the second cannot overwrite the first.
    let first = roots.write(&target, "flow", &base, edit("first")).unwrap();
    assert!(matches!(
        roots.write(&target, "flow", &base, edit("second")),
        Err(SettingsError::Conflict { expected, actual, .. }) if expected == base && actual == first
    ));
    // An external modification is observed, never silently overwritten.
    let external = format!("{}\n# external\n", std::fs::read_to_string(&path).unwrap());
    std::fs::write(&path, &external).unwrap();
    assert!(matches!(
        roots.write(&target, "flow", &first, edit("third")),
        Err(SettingsError::Conflict { .. })
    ));
    assert!(matches!(
        roots.write(&target, "flow", &first, WorkflowEdit::Delete),
        Err(SettingsError::Conflict { .. })
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
    // External deletion is a conflict too.
    let current = roots.read(&target, "flow").revision;
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        roots.write(&target, "flow", &current, edit("fourth")),
        Err(SettingsError::Conflict { .. })
    ));
    assert!(!path.exists());
}

#[test]
fn copy_guards_its_source_revision_and_destination_absence() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    let source = roots.create(&target, "origin", "Origin");
    roots.create(&target, "taken", "Taken");
    let copy = |destination: &str, expected: &str, revision: &str| {
        roots.write(
            &target,
            destination,
            expected,
            WorkflowEdit::Copy {
                source: id("origin"),
                source_revision: revision.into(),
            },
        )
    };
    assert!(matches!(
        copy("copied", "missing", "sha256:stale"),
        Err(SettingsError::Conflict { expected, actual, .. }) if expected == "sha256:stale" && actual == source
    ));
    assert!(!roots.read(&target, "copied").present);
    assert!(matches!(
        copy("taken", "missing", &source),
        Err(SettingsError::Conflict { .. })
    ));
    assert!(matches!(
        roots.write(
            &target,
            "origin",
            "missing",
            WorkflowEdit::Copy {
                source: id("origin"),
                source_revision: source.clone()
            }
        ),
        Err(SettingsError::Conflict { .. })
    ));
    copy("copied", "missing", &source).unwrap();
    let copied = roots.read(&target, "copied");
    assert_eq!(copied.document, roots.read(&target, "origin").document);
    assert_eq!(roots.read(&target, "origin").revision, source);
    // A copy never crosses scopes: the source is this scope's file.
    assert!(matches!(
        roots.write(
            &workspace(&roots.b),
            "copied",
            "missing",
            WorkflowEdit::Copy {
                source: id("origin"),
                source_revision: source,
            }
        ),
        Err(SettingsError::Conflict { actual, .. }) if actual == "missing"
    ));
    // A malformed source is never copied.
    std::fs::write(roots.a.join(".agents/workflows/bad.yaml"), "description: [").unwrap();
    let bad = roots.read(&target, "bad").revision;
    assert!(matches!(
        roots.write(
            &target,
            "bad_copy",
            "missing",
            WorkflowEdit::Copy {
                source: id("bad"),
                source_revision: bad,
            }
        ),
        Err(SettingsError::InvalidWorkflow(_))
    ));
    assert!(!roots.read(&target, "bad_copy").present);
}

#[test]
fn redirected_and_oversized_workflow_files_are_refused_without_outside_writes() {
    let roots = Roots::new();
    let outside = roots.a.parent().unwrap().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("leak.yaml"), AGENT_PROGRAM).unwrap();
    let outside_bytes = || {
        let mut names: Vec<_> = std::fs::read_dir(&outside)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        (names, std::fs::read(outside.join("leak.yaml")).unwrap())
    };
    let before = outside_bytes();

    // A redirected Workflow directory.
    let a = workspace(&roots.a);
    std::fs::create_dir_all(roots.a.join(".agents")).unwrap();
    std::os::unix::fs::symlink(&outside, roots.a.join(".agents/workflows")).unwrap();
    let library = roots.manager.workflow_library(&a).unwrap();
    assert!(library.files.is_empty());
    assert!(library.diagnostic.is_some());
    assert!(roots.manager.workflow_source(&a, &id("leak")).is_err());
    assert!(
        roots
            .write(
                &a,
                "leak",
                "missing",
                WorkflowEdit::Create {
                    description: "escape".into()
                }
            )
            .is_err()
    );
    assert!(
        roots
            .write(&a, "fresh", "missing", WorkflowEdit::Delete)
            .is_err()
    );

    // A redirected Workflow file.
    let b = workspace(&roots.b);
    let directory = roots.b.join(".agents/workflows");
    std::fs::create_dir_all(&directory).unwrap();
    std::os::unix::fs::symlink(outside.join("leak.yaml"), directory.join("leak.yaml")).unwrap();
    let library = roots.manager.workflow_library(&b).unwrap();
    assert_eq!(library.files.len(), 1);
    assert_eq!(
        library.files[0].diagnostic.as_ref().unwrap().category,
        "resource"
    );
    assert!(roots.manager.workflow_source(&b, &id("leak")).is_err());
    let replace = WorkflowEdit::Replace {
        document: Box::new(definition(AGENT_PROGRAM)),
    };
    assert!(roots.write(&b, "leak", "missing", replace.clone()).is_err());
    assert_eq!(outside_bytes(), before);

    // An aliased or relative Workspace target is not this Workspace.
    let alias = roots.a.parent().unwrap().join("alias");
    std::os::unix::fs::symlink(&roots.b, &alias).unwrap();
    for target in [workspace(&alias), workspace(Path::new("relative"))] {
        assert!(matches!(
            roots.manager.workflow_library(&target),
            Err(SettingsError::Invalid)
        ));
        assert!(
            roots
                .write(&target, "x", "missing", replace.clone())
                .is_err()
        );
    }

    // An oversized file is listed with its bound diagnostic.
    std::fs::remove_file(directory.join("leak.yaml")).unwrap();
    std::fs::write(
        directory.join("huge.yaml"),
        format!("{AGENT_PROGRAM}# {}\n", "x".repeat(MAX_WORKFLOW_BYTES)),
    )
    .unwrap();
    let library = roots.manager.workflow_library(&b).unwrap();
    assert!(library.files[0].diagnostic.is_some());
    assert!(roots.read(&b, "huge").document.is_none());
}

#[test]
fn committed_but_unconfirmed_publication_is_reported_without_replay() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    let base = roots.create(&target, "flow", "Base");
    let mut document = *roots.read(&target, "flow").document.unwrap();
    document.description = "committed".into();
    roots.manager.test_hooks.fail_once("directory_sync");
    let outcome = roots.write(
        &target,
        "flow",
        &base,
        WorkflowEdit::Replace {
            document: Box::new(document.clone()),
        },
    );
    assert!(
        matches!(outcome, Err(SettingsError::Committed)),
        "{outcome:?}"
    );
    // The bytes were published exactly once: a reread shows the commit, and
    // the stale base can no longer write.
    let reread = roots.read(&target, "flow");
    assert_eq!(*reread.document.unwrap(), document);
    assert_ne!(reread.revision, base);
    assert!(matches!(
        roots.write(
            &target,
            "flow",
            &base,
            WorkflowEdit::Replace {
                document: Box::new(document)
            }
        ),
        Err(SettingsError::Conflict { .. })
    ));
}

#[test]
fn offline_checks_are_side_effect_free_and_keep_structure_and_dependencies_apart() {
    let roots = Roots::new();
    let a = workspace(&roots.a);
    let valid = definition(AGENT_PROGRAM);
    let (report, effects) = super::super::static_effects::measure(|| {
        roots
            .manager
            .check_workflow(&a, &id("review"), valid.clone(), None, true)
            .unwrap()
    });
    assert_eq!(effects, [0; 11]);
    let WorkflowStructure::Valid { outputs, .. } = &report.structure else {
        panic!("valid structure: {report:?}");
    };
    assert!(outputs.contains_key("block.nodes.review"));
    assert!(
        matches!(&report.dependencies, WorkflowDependencies::Admitted { context } if *context == roots.a)
    );
    assert!(report.explanation.is_some());

    // A missing profile is a dependency fact of a structurally valid file.
    let missing = definition(&AGENT_PROGRAM.replace("profile: reviewer", "profile: absent"));
    let report = roots
        .manager
        .check_workflow(&a, &id("review"), missing.clone(), None, false)
        .unwrap();
    assert!(matches!(report.structure, WorkflowStructure::Valid { .. }));
    let WorkflowDependencies::Blocked { diagnostics, .. } = &report.dependencies else {
        panic!("blocked: {report:?}");
    };
    assert!(matches!(
        diagnostics.as_slice(),
        [WorkflowAdmissionDiagnostic { path, reason: WorkflowDependencyFailure::Agent(_) }] if path == "block.nodes.review.profile"
    ));
    assert!(report.explanation.is_none());

    // A Global file has no dependency context unless one is named, and is
    // never checked in another Workspace's place implicitly.
    let user = roots
        .manager
        .check_workflow(
            &SourceTarget::User,
            &id("review"),
            missing.clone(),
            None,
            false,
        )
        .unwrap();
    assert_eq!(user.dependencies, WorkflowDependencies::NotChecked);
    let user = roots
        .manager
        .check_workflow(
            &SourceTarget::User,
            &id("review"),
            missing,
            Some(&roots.b),
            false,
        )
        .unwrap();
    assert!(
        matches!(&user.dependencies, WorkflowDependencies::Blocked { context, .. } if *context == roots.b)
    );
    assert!(matches!(
        roots
            .manager
            .check_workflow(&a, &id("review"), valid.clone(), Some(&roots.b), false),
        Err(SettingsError::Invalid)
    ));

    // An invalid structure reports its path and checks no dependencies.
    let mut broken = valid;
    broken.block.edges.clear();
    let report = roots
        .manager
        .check_workflow(&a, &id("review"), broken, None, true)
        .unwrap();
    let WorkflowStructure::Invalid { diagnostic } = &report.structure else {
        panic!("invalid: {report:?}");
    };
    assert!(diagnostic.path.starts_with("block"), "{diagnostic:?}");
    assert_eq!(report.dependencies, WorkflowDependencies::NotChecked);
    assert!(report.explanation.is_none());
    // Nothing was written by any check.
    assert!(!roots.a.join(".agents").exists());
}

/// Every native example survives YAML → definition → wire JSON → definition
/// → canonical YAML with the same meaning and compiles under its identity.
#[test]
fn native_examples_round_trip_semantically_through_canonical_serialization() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local-runtime");
    for (directory, name) in [
        ("workflow-templates/.agents/workflows", "typed_agent"),
        ("workflow-templates/.agents/workflows", "specialized_agent"),
        ("workflow-templates/.agents/workflows", "parallel_checks"),
        ("workflow-templates/.agents/workflows", "human_plan"),
        (".agents/workflows", "parallel_review"),
        (".agents/workflows", "implement_and_review"),
    ] {
        let path = examples.join(directory).join(format!("{name}.yaml"));
        let original = definition(&std::fs::read_to_string(&path).unwrap());
        let wire: WorkflowDefinition =
            serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(wire, original, "{name}");
        let bytes = candidate::encode(&id(name), &path, &wire).unwrap();
        let canonical: WorkflowDefinition = serde_yaml::from_slice(&bytes).unwrap();
        assert_eq!(canonical, original, "{name}");
        assert!(
            !String::from_utf8(bytes).unwrap().contains("port: null"),
            "{name}"
        );
        assert_eq!(
            WorkflowProgram::compile(id(name), canonical)
                .unwrap()
                .tool_identity(),
            WorkflowProgram::compile(id(name), original)
                .unwrap()
                .tool_identity(),
            "{name}"
        );
    }
}

/// Untouched advanced fields and explicit-empty selections survive a save.
#[test]
fn overrides_and_explicit_empty_selections_survive_a_whole_file_save() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/local-runtime/workflow-templates/.agents/workflows/specialized_agent.yaml");
    let original = definition(&std::fs::read_to_string(path).unwrap());
    let text = serde_json::to_string(&original).unwrap();
    assert!(text.contains("\"override\""), "fixture carries an override");
    roots.create(&target, "specialized", "seed");
    let base = roots.read(&target, "specialized").revision;
    roots
        .write(
            &target,
            "specialized",
            &base,
            WorkflowEdit::Replace {
                document: Box::new(original.clone()),
            },
        )
        .unwrap();
    assert_eq!(
        *roots.read(&target, "specialized").document.unwrap(),
        original
    );
}

/// A program whose single node identity is `width` bytes of an invalid
/// identity: below the native file bound, structurally invalid, and copied
/// by the parser into both the diagnostic path and its detail.
fn oversized_identity(width: usize) -> String {
    let node = "N".repeat(width);
    format!(
        "description: Oversized.\nblock:\n  input: {{type: object, properties: {{}}, additionalProperties: false}}\n  output: {{type: object, properties: {{}}, additionalProperties: false}}\n  entry: {node}\n  nodes:\n    {node}:\n      type: return\n      output: {{type: literal, value: {{}}}}\n"
    )
}

#[test]
fn diagnostics_and_listings_stay_bounded_for_oversized_invalid_identities() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    roots.create(&target, "valid", "Still listed in full.");
    let directory = roots.a.join(".agents/workflows");
    let yaml = oversized_identity(100_000);
    assert!(yaml.len() > 100_000 && yaml.len() < crate::runtime::workflow::MAX_WORKFLOW_BYTES);
    for index in 0..6 {
        std::fs::write(directory.join(format!("big{index}.yaml")), &yaml).unwrap();
    }

    let source = roots.read(&target, "big0");
    let diagnostic = source.diagnostic.as_ref().expect("structurally invalid");
    assert!(source.document.is_none());
    assert!(
        diagnostic.path.len() <= MAX_DIAGNOSTIC_PATH_BYTES,
        "{}",
        diagnostic.path.len()
    );
    assert!(
        diagnostic
            .detail
            .as_ref()
            .is_none_or(|detail| detail.len() <= MAX_WORKFLOW_DIAGNOSTIC_BYTES)
    );
    // The bounded path still locates the owning block.
    assert!(
        "block.nodes".starts_with(&diagnostic.path) || diagnostic.path.starts_with("block"),
        "{}",
        diagnostic.path
    );
    assert!(serde_json::to_vec(&source).unwrap().len() < 16 * 1024);

    let library = roots.manager.workflow_library(&target).unwrap();
    let encoded = serde_json::to_vec(&library).unwrap();
    assert!(encoded.len() < 64 * 1024, "{}", encoded.len());
    assert_eq!(library.files.len(), 7);
    for file in &library.files[..6] {
        assert!(
            file.diagnostic.is_some(),
            "an invalid file stays discoverable"
        );
    }
    let valid = &library.files[6];
    assert_eq!(valid.description.as_deref(), Some("Still listed in full."));
    assert!(valid.diagnostic.is_none());

    // At the 128-file capacity every entry keeps its share, so the listing
    // fits the App Server message bound however its entries are authored.
    for index in 6..127 {
        std::fs::write(
            directory.join(format!("big{index}.yaml")),
            oversized_identity(4_000),
        )
        .unwrap();
    }
    let library = roots.manager.workflow_library(&target).unwrap();
    assert_eq!(library.files.len(), 128);
    assert!(
        serde_json::to_vec(&library).unwrap().len()
            < crate::app_server::transport::MAX_MESSAGE_BYTES - 64 * 1024
    );
}

#[test]
fn one_listing_entry_is_reduced_to_its_share_without_losing_its_identity() {
    // Control characters expand sixfold when escaped as JSON.
    let escaped = |bytes: usize| "\u{1}".repeat(bytes);
    let summary = WorkflowFileSummary {
        file_name: format!("{}.yaml", "x".repeat(200)),
        id: None,
        relative_path: format!(".agents/workflows/{}.yaml", "x".repeat(200)),
        revision: "sha256:0".into(),
        byte_length: 1,
        modified_ms: None,
        description: Some(escaped(MAX_LISTED_DESCRIPTION_BYTES)),
        nodes: None,
        diagnostic: Some(WorkflowDiagnostic {
            detail: Some(escaped(MAX_WORKFLOW_DIAGNOSTIC_BYTES)),
            ..WorkflowDiagnostic::new(
                escaped(MAX_DIAGNOSTIC_PATH_BYTES),
                "workflow_language",
                "invalid",
            )
        }),
        shadows_global: false,
    };
    assert!(serde_json::to_vec(&summary).unwrap().len() > MAX_LISTED_ENTRY_BYTES);
    let fitted = fit_listing_share(summary);
    assert!(serde_json::to_vec(&fitted).unwrap().len() <= MAX_LISTED_ENTRY_BYTES);
    let diagnostic = fitted.diagnostic.unwrap();
    assert_eq!(diagnostic.category, "workflow_language");
    assert!(diagnostic.detail.is_none());
    assert_eq!(fitted.revision, "sha256:0");
}

/// `AGENT_PROGRAM` returning `{values: [...]}` constructed from the given
/// numeric literals, under a `number` element contract.
fn returning(literals: &[&str]) -> String {
    let items = literals
        .iter()
        .map(|literal| format!("{{type: literal, value: {literal}}}"))
        .collect::<Vec<_>>()
        .join(", ");
    AGENT_PROGRAM
        .replace(
            "output: {type: object, properties: {summary: {type: string}}, required: [summary], additionalProperties: false}\n  entry",
            "output: {type: object, properties: {values: {type: array, items: {type: number}}}, required: [values], additionalProperties: false}\n  entry",
        )
        .replace(
            "output: {type: reference, path: [review]}",
            &format!("output: {{type: object, fields: {{values: {{type: array, items: [{items}]}}}}}}"),
        )
}

#[test]
fn the_browser_number_contract_holds_for_reading_checking_and_replacing() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    let directory = roots.a.join(".agents/workflows");
    // A valid native file the browser would change: it prints `1.0` as `1`,
    // after which the constructed array no longer has one element type.
    let floats = returning(&["1.0", "1.5"]);
    if let Err(error) = crate::local_runtime::workflow_resources::compile_definition(
        &id("floats"),
        &directory.join("floats.yaml"),
        definition(&floats),
    ) {
        panic!("natively valid: {error:?}\n{floats}");
    }
    roots.create(&target, "floats", "seed");
    std::fs::write(directory.join("floats.yaml"), &floats).unwrap();
    let source = roots.read(&target, "floats");
    assert!(
        source.document.is_none(),
        "read-only rather than silently retyped"
    );
    let diagnostic = source.diagnostic.unwrap();
    assert_eq!(diagnostic.category, "unsupported_number");
    assert_eq!(
        diagnostic.path,
        "block.nodes.done.output.fields.values.items.0.value"
    );
    let library = roots.manager.workflow_library(&target).unwrap();
    assert_eq!(
        library.files[0].diagnostic.as_ref().unwrap().category,
        "unsupported_number"
    );

    // What a browser can hold is editable: safe integers and non-integral
    // floats, and integral floats from 1e21, which it prints in exponent form.
    let editable = returning(&["2.5", "1.5", "-0.25", "1.0e21"]);
    std::fs::write(directory.join("floats.yaml"), &editable).unwrap();
    let source = roots.read(&target, "floats");
    assert!(source.diagnostic.is_none(), "{:?}", source.diagnostic);
    let document = source.document.unwrap();

    // A candidate carrying a number the browser already changed is refused
    // by check and by replace alike, and the file is untouched.
    for literal in ["9007199254740992", "1.0", "-0.0"] {
        let candidate = definition(&returning(&[literal, "1.5"]));
        let report = roots
            .manager
            .check_workflow(&target, &id("floats"), candidate.clone(), None, false)
            .unwrap();
        let WorkflowStructure::Invalid { diagnostic } = report.structure else {
            panic!("{literal} must be refused: {report:?}");
        };
        assert_eq!(diagnostic.category, "unsupported_number");
        let refused = roots.write(
            &target,
            "floats",
            &source.revision,
            WorkflowEdit::Replace {
                document: Box::new(candidate),
            },
        );
        assert!(
            matches!(refused, Err(SettingsError::InvalidWorkflow(ref diagnostic)) if diagnostic.category == "unsupported_number"),
            "{refused:?}"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("floats.yaml")).unwrap(),
            editable
        );
    }
    roots
        .write(
            &target,
            "floats",
            &source.revision,
            WorkflowEdit::Replace { document },
        )
        .expect("a browser-representable document saves");
}

#[test]
fn a_checked_candidate_names_the_revision_its_normalized_commit_takes() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    let base = roots.create(&target, "review", "seed");
    // As a browser authors it: an ordinary edge without `port`, and no
    // `timeout_ms`. Native writes both normalized.
    let mut candidate: serde_json::Value = serde_json::to_value(definition(AGENT_PROGRAM)).unwrap();
    candidate.as_object_mut().unwrap().remove("timeout_ms");
    for edge in candidate["block"]["edges"].as_array_mut().unwrap() {
        edge.as_object_mut().unwrap().remove("port");
    }
    let candidate: WorkflowDefinition = serde_json::from_value(candidate).unwrap();
    let report = roots
        .manager
        .check_workflow(&target, &id("review"), candidate.clone(), None, false)
        .unwrap();
    let WorkflowStructure::Valid { revision, .. } = report.structure else {
        panic!("valid: {report:?}");
    };
    let committed = roots
        .write(
            &target,
            "review",
            &base,
            WorkflowEdit::Replace {
                document: Box::new(candidate),
            },
        )
        .unwrap();
    assert_eq!(committed, revision);
    let reread = roots.read(&target, "review");
    assert_eq!(reread.revision, revision);
    assert!(
        std::fs::read_to_string(roots.a.join(".agents/workflows/review.yaml"))
            .unwrap()
            .contains("timeout_ms")
    );

    // A meaningful distinction is a different identity: an explicitly empty
    // override is not an omitted one.
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/local-runtime/workflow-templates/.agents/workflows/specialized_agent.yaml");
    let original: serde_json::Value =
        serde_json::to_value(definition(&std::fs::read_to_string(path).unwrap())).unwrap();
    let mut omitted = original.clone();
    let mut empty = original;
    for value in [&mut omitted, &mut empty] {
        let nodes = value["block"]["nodes"].as_object_mut().unwrap();
        let (_, agent) = nodes
            .iter_mut()
            .find(|(_, node)| node.get("override").is_some())
            .expect("an override");
        agent.as_object_mut().unwrap().remove("override");
    }
    let nodes = empty["block"]["nodes"].as_object_mut().unwrap();
    let agent = nodes
        .values_mut()
        .find(|node| node["type"] == "agent")
        .unwrap();
    agent
        .as_object_mut()
        .unwrap()
        .insert("override".into(), serde_json::json!({}));
    let identity = |value: serde_json::Value| {
        let report = roots
            .manager
            .check_workflow(
                &target,
                &id("specialized"),
                serde_json::from_value(value).unwrap(),
                None,
                false,
            )
            .unwrap();
        match report.structure {
            WorkflowStructure::Valid { revision, .. } => revision,
            WorkflowStructure::Invalid { diagnostic } => panic!("{diagnostic:?}"),
        }
    };
    assert_ne!(identity(omitted), identity(empty));
}

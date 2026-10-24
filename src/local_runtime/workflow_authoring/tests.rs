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
        let manager = manager_at(&home, &a);
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

/// The configuration manager of a session in `workspace` for `home`, as
/// another process opens it.
fn manager_at(home: &Path, workspace: &Path) -> UserConfigManager {
    let request = LaunchRequest {
        workspace: Some(workspace.to_path_buf()),
        ..LaunchRequest::default()
    };
    let host = HostEnvironment::from_paths(workspace.to_path_buf(), home.to_path_buf()).unwrap();
    request.session_input(&host).unwrap().0
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
fn model_authoring_round_trips_and_checks_without_model_resources() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    let templates = [
        (
            "model_text",
            include_str!(
                "../../../examples/local-runtime/workflow-templates/.agents/workflows/model_text.yaml"
            ),
        ),
        (
            "model_json",
            include_str!(
                "../../../examples/local-runtime/workflow-templates/.agents/workflows/model_json.yaml"
            ),
        ),
    ];
    for (name, yaml) in templates {
        let yaml = yaml.replace("example/demo-model", "a");
        let document = definition(&yaml);
        let base = roots.create(&target, name, "Model authoring test");
        roots
            .write(
                &target,
                name,
                &base,
                WorkflowEdit::Replace {
                    document: Box::new(document.clone()),
                },
            )
            .unwrap();
        let source = roots.read(&target, name);
        assert_eq!(source.document.as_deref(), Some(&document));
        assert!(
            matches!(roots.manager.workflow_model_choices(&target, WorkflowModelChoicesQuery::Models { offset: 0 }, None).unwrap(),
            WorkflowModelChoices::Models { models, .. } if models.iter().any(|model| model.to_string() == "a"))
        );
        let (report, effects) = super::super::static_effects::measure(|| {
            roots
                .manager
                .check_workflow(&target, &id(name), document.clone(), None, true)
                .unwrap()
        });
        assert_eq!(effects, [0; 11]);
        assert!(matches!(report.structure, WorkflowStructure::Valid { .. }));
        if name == "model_text" {
            assert!(
                matches!(report.dependencies, WorkflowDependencies::RequiresAttempt { model_nodes, .. }
                if model_nodes == ["block.nodes.rewrite.model"])
            );
        } else {
            assert!(matches!(
                report.dependencies,
                WorkflowDependencies::Admitted { .. }
            ));
            let absent = definition(&yaml.replace("model: a", "model: absent"));
            let report = roots
                .manager
                .check_workflow(&target, &id(name), absent, None, false)
                .unwrap();
            assert!(matches!(report.structure, WorkflowStructure::Valid { .. }));
            assert!(
                matches!(report.dependencies, WorkflowDependencies::Blocked { diagnostics, .. }
                if matches!(diagnostics.as_slice(), [WorkflowAdmissionDiagnostic {
                    path, reason: WorkflowDependencyFailure::Model { .. }
                }] if path == "block.nodes.classify.model"))
            );
        }
    }
}

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

/// Seed `directory` with Workflow files written as an external editor would:
/// `valid` files named `<prefix><n>` holding `content`, and `invalid_names`
/// files whose names are no Workflow identity at all.
fn seed(directory: &Path, prefix: &str, valid: usize, invalid_names: usize, content: &[u8]) {
    std::fs::create_dir_all(directory).unwrap();
    for index in 0..valid {
        std::fs::write(directory.join(format!("{prefix}{index:03}.yaml")), content).unwrap();
    }
    for index in 0..invalid_names {
        std::fs::write(
            directory.join(format!("Not-An-Id-{index:03}.yaml")),
            content,
        )
        .unwrap();
    }
}

fn capacity_refusal(result: Result<String, SettingsError>) -> WorkflowDiagnostic {
    match result {
        Err(SettingsError::InvalidWorkflow(diagnostic)) if diagnostic.category == "capacity" => {
            *diagnostic
        }
        other => panic!("expected a capacity refusal, got {other:?}"),
    }
}

impl Roots {
    fn workspace_directory(&self) -> PathBuf {
        self.a.join(".agents/workflows")
    }

    /// Canonical bytes of a valid minimal Workflow, written by native code.
    fn minimal_bytes(&self) -> Vec<u8> {
        self.create(&SourceTarget::User, "template_source", "Template.");
        let path = self.user_directory().join("template_source.yaml");
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        bytes
    }

    /// What discovery composes for `workspace` (none: User only).
    fn discover(&self, workspace: Option<&Path>) -> crate::runtime::workflow::WorkflowCatalog {
        crate::local_runtime::workflow_resources::load(workspace, &self.home.join("rustx/.agents"))
            .unwrap()
    }

    fn copy(&self, target: &SourceTarget, source: &str, id: &str) -> Result<String, SettingsError> {
        let source_revision = self.read(target, source).revision;
        self.write(
            target,
            id,
            "missing",
            WorkflowEdit::Copy {
                source: WorkflowId::parse(source).unwrap(),
                source_revision,
            },
        )
    }
}

#[test]
fn a_global_addition_is_admitted_up_to_64_identities_and_refused_before_publication_after() {
    let roots = Roots::new();
    let bytes = roots.minimal_bytes();
    // 62 valid files and one valid identity with invalid content: an
    // identity is occupied before anything is compiled.
    seed(&roots.user_directory(), "g", 62, 0, &bytes);
    std::fs::write(roots.user_directory().join("broken.yaml"), "description: [").unwrap();
    roots.create(&SourceTarget::User, "sixty_four", "The 64th identity.");
    assert_eq!(roots.discover(None).locations.len(), 64);

    let refused = capacity_refusal(roots.write(
        &SourceTarget::User,
        "sixty_five",
        "missing",
        WorkflowEdit::Create {
            description: "One too many.".into(),
        },
    ));
    assert_eq!(
        refused.reason,
        "Global already defines the maximum of 64 Workflow IDs"
    );
    capacity_refusal(roots.copy(&SourceTarget::User, "g000", "sixty_five"));
    assert!(!roots.user_directory().join("sixty_five.yaml").exists());
    // Discovery of the existing files is untouched.
    let catalog = roots.discover(None);
    assert!(catalog.get(&id("sixty_four")).is_some());
    assert!(catalog.discovery_diagnostics.is_empty());
}

#[test]
fn a_workspace_addition_counts_its_union_with_global_and_an_override_adds_no_identity() {
    let roots = Roots::new();
    let bytes = roots.minimal_bytes();
    let target = workspace(&roots.a);
    seed(&roots.user_directory(), "u", 40, 0, &bytes);
    seed(&roots.workspace_directory(), "w", 23, 0, &bytes);
    roots.create(&target, "workspace_last", "The 64th composed identity.");
    let refused = capacity_refusal(roots.write(
        &target,
        "workspace_more",
        "missing",
        WorkflowEdit::Create {
            description: "x".into(),
        },
    ));
    assert!(
        refused
            .reason
            .contains("Global and this Workspace together"),
        "{}",
        refused.reason
    );
    capacity_refusal(roots.copy(&target, "w000", "workspace_more"));
    assert!(
        !roots
            .workspace_directory()
            .join("workspace_more.yaml")
            .exists()
    );
    // Overriding a Global ID keeps the union at 64.
    roots.create(&target, "u000", "A Workspace override.");
    let catalog = roots.discover(Some(&roots.a));
    assert_eq!(catalog.locations.len(), 64);
    assert!(catalog.discovery_diagnostics.is_empty());
    assert!(catalog.get(&id("workspace_last")).is_some());
    // A Global addition counts the User-only catalog it is authorized for:
    // it cannot know every Workspace a later session composes with.
    roots.create(
        &SourceTarget::User,
        "global_more",
        "Within Global's own bound.",
    );
    assert_eq!(roots.discover(None).locations.len(), 41);
}

#[test]
fn a_directory_admits_up_to_128_yaml_entries_and_refuses_the_next() {
    let roots = Roots::new();
    let bytes = roots.minimal_bytes();
    // Entries without a Workflow identity count toward the physical bound
    // only, isolating it from the 64-identity bound.
    seed(&roots.user_directory(), "p", 2, 125, &bytes);
    roots.create(&SourceTarget::User, "entry_128", "The 128th entry.");
    let refused = capacity_refusal(roots.write(
        &SourceTarget::User,
        "entry_129",
        "missing",
        WorkflowEdit::Create {
            description: "x".into(),
        },
    ));
    assert!(
        refused.reason.contains("128 YAML files"),
        "{}",
        refused.reason
    );
    assert!(!roots.user_directory().join("entry_129.yaml").exists());
    let library = roots.manager.workflow_library(&SourceTarget::User).unwrap();
    assert!(library.diagnostic.is_none());
    assert_eq!(library.files.len(), 128);
    assert!(roots.discover(None).get(&id("entry_128")).is_some());
}

#[test]
fn an_externally_over_full_scope_still_allows_guarded_replace_and_delete() {
    let roots = Roots::new();
    let bytes = roots.minimal_bytes();
    seed(&roots.user_directory(), "o", 65, 0, &bytes);
    capacity_refusal(roots.write(
        &SourceTarget::User,
        "more",
        "missing",
        WorkflowEdit::Create {
            description: "x".into(),
        },
    ));
    let replaced = roots.read(&SourceTarget::User, "o000");
    let mut document = replaced.document.clone().unwrap();
    document.description = "Repaired in place.".into();
    roots
        .write(
            &SourceTarget::User,
            "o000",
            &replaced.revision,
            WorkflowEdit::Replace { document },
        )
        .unwrap();
    let doomed = roots.read(&SourceTarget::User, "o001").revision;
    // A delete keeps its revision guard.
    assert!(matches!(
        roots.write(
            &SourceTarget::User,
            "o001",
            "sha256:other",
            WorkflowEdit::Delete
        ),
        Err(SettingsError::Conflict { .. })
    ));
    roots
        .write(&SourceTarget::User, "o001", &doomed, WorkflowEdit::Delete)
        .unwrap();
    let catalog = roots.discover(None);
    assert_eq!(catalog.locations.len(), 64);
    assert!(catalog.discovery_diagnostics.is_empty());
    assert!(catalog.get(&id("o000")).is_some());
    assert_eq!(
        roots
            .read(&SourceTarget::User, "o000")
            .document
            .unwrap()
            .description,
        "Repaired in place."
    );
}

/// Wait until some writer blocks on the lock file `lock` (Linux `/proc/locks`).
/// Reading `/proc/locks` takes the kernel's lock-list locks every other lock
/// operation on the machine contends with, so it is polled at an interval,
/// never spun on while a writer (or its process) is still starting.
#[cfg(target_os = "linux")]
fn await_waiter(lock: &Path) {
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
            "the second writer never waited for the capacity lock"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Run `first` until it holds its capacity locks, start `second`, wait until
/// it blocks on the User capacity lock, then let both finish.
fn race(
    roots: &Roots,
    first: impl FnOnce(UserConfigManager) -> Result<String, SettingsError> + Send + 'static,
    second: impl FnOnce(UserConfigManager) -> Result<String, SettingsError> + Send + 'static,
) -> (Result<String, SettingsError>, Result<String, SettingsError>) {
    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    roots
        .manager
        .test_hooks
        .insert("workflow_capacity_locked", move || {
            held.send(()).unwrap();
            released.recv().unwrap();
        });
    let manager = roots.manager.clone();
    let first = std::thread::spawn(move || first(manager));
    holding.recv().unwrap();
    let manager = roots.manager.clone();
    let second = std::thread::spawn(move || second(manager));
    #[cfg(target_os = "linux")]
    await_waiter(&crate::local_runtime::settings::lock_path_of(
        &roots.user_directory().join("workflow-capacity"),
    ));
    release.send(()).unwrap();
    (first.join().unwrap(), second.join().unwrap())
}

fn creating(
    target: SourceTarget,
    name: &'static str,
) -> impl FnOnce(UserConfigManager) -> Result<String, SettingsError> + Send + 'static {
    move |manager| {
        manager.write_source_settings(
            &target,
            "missing",
            SourceMutation::Workflow {
                id: id(name),
                edit: WorkflowEdit::Create {
                    description: "Racing.".into(),
                },
            },
        )
    }
}

#[test]
fn two_additions_competing_for_the_last_slot_admit_at_most_one() {
    let roots = Roots::new();
    let bytes = roots.minimal_bytes();
    seed(&roots.user_directory(), "s", 63, 0, &bytes);
    let (first, second) = race(
        &roots,
        creating(SourceTarget::User, "first_racer"),
        creating(SourceTarget::User, "second_racer"),
    );
    assert!(first.is_ok());
    capacity_refusal(second);
    assert!(!roots.user_directory().join("second_racer.yaml").exists());
    assert_eq!(roots.discover(None).locations.len(), 64);
}

#[test]
fn a_workspace_addition_waits_for_and_counts_a_concurrent_global_addition() {
    let roots = Roots::new();
    let bytes = roots.minimal_bytes();
    seed(&roots.user_directory(), "u", 40, 0, &bytes);
    seed(&roots.workspace_directory(), "w", 23, 0, &bytes);
    let (global, local) = race(
        &roots,
        creating(SourceTarget::User, "global_last"),
        creating(workspace(&roots.a), "workspace_late"),
    );
    assert!(global.is_ok());
    capacity_refusal(local);
    let catalog = roots.discover(Some(&roots.a));
    assert_eq!(catalog.locations.len(), 64);
    assert!(catalog.discovery_diagnostics.is_empty());
}

/// Run `operation` on its own thread and require it to finish in time: a
/// writer waiting for a lock it already holds fails the test instead of
/// hanging the suite.
fn bounded<T: Send + 'static>(operation: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || done.send(operation()).unwrap());
    finished
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the write never returned: it waits for a lock it holds")
}

#[test]
fn a_workspace_whose_workflow_directory_is_global_holds_its_one_capacity_lock_once() {
    let roots = Roots::new();
    // A Workspace at `~/rustx`: its Workflow directory is the User one.
    let coincident = roots.home.join("rustx");
    let target = workspace(&coincident);
    let manager = roots.manager.clone();
    let created = {
        let target = target.clone();
        bounded(move || {
            manager.write_source_settings(
                &target,
                "missing",
                SourceMutation::Workflow {
                    id: id("shared_scope"),
                    edit: WorkflowEdit::Create {
                        description: "Created through the Workspace.".into(),
                    },
                },
            )
        })
    };
    created.unwrap();
    let source_revision = roots.read(&target, "shared_scope").revision;
    let manager = roots.manager.clone();
    let copied = {
        let target = target.clone();
        bounded(move || {
            manager.write_source_settings(
                &target,
                "missing",
                SourceMutation::Workflow {
                    id: id("shared_copy"),
                    edit: WorkflowEdit::Copy {
                        source: id("shared_scope"),
                        source_revision,
                    },
                },
            )
        })
    };
    copied.unwrap();
    // Both are listed and read through either scope at once.
    for scope in [&target, &SourceTarget::User] {
        let library = roots.manager.workflow_library(scope).unwrap();
        assert!(library.diagnostic.is_none());
        let ids: Vec<_> = library.files.iter().map(|file| file.id.clone()).collect();
        assert!(ids.contains(&Some(id("shared_scope"))), "{ids:?}");
        assert!(ids.contains(&Some(id("shared_copy"))), "{ids:?}");
        assert_eq!(
            roots
                .read(scope, "shared_copy")
                .document
                .unwrap()
                .description,
            "Created through the Workspace."
        );
    }
    assert!(
        roots
            .discover(Some(&coincident))
            .get(&id("shared_copy"))
            .is_some()
    );
}

/// The child half of the cross-process race below: open the session of
/// `RUSTX_491_CAPACITY_HOME`, add `RUSTX_491_CAPACITY_ID` to its target
/// (`user`, or the Workspace directory given), and print the outcome. With
/// `RUSTX_491_CAPACITY_HOLD`, it reports holding its capacity locks and
/// waits for a byte on stdin before going on.
#[test]
fn capacity_process_writer_gate() {
    use std::io::{Read, Write};
    let Ok(home) = std::env::var("RUSTX_491_CAPACITY_HOME") else {
        return;
    };
    let home = PathBuf::from(home);
    let session = PathBuf::from(std::env::var("RUSTX_491_CAPACITY_SESSION").unwrap());
    let manager = manager_at(&home, &session);
    if std::env::var_os("RUSTX_491_CAPACITY_HOLD").is_some() {
        manager.test_hooks.insert("workflow_capacity_locked", || {
            println!("CAPACITY_LOCKED");
            std::io::stdout().flush().unwrap();
            std::io::stdin().read_exact(&mut [0]).unwrap();
        });
    }
    let target = match std::env::var("RUSTX_491_CAPACITY_TARGET").unwrap().as_str() {
        "user" => SourceTarget::User,
        directory => workspace(Path::new(directory)),
    };
    let result = manager.write_source_settings(
        &target,
        "missing",
        SourceMutation::Workflow {
            id: id(&std::env::var("RUSTX_491_CAPACITY_ID").unwrap()),
            edit: WorkflowEdit::Create {
                description: "Racing from another process.".into(),
            },
        },
    );
    let outcome = match result {
        Ok(_) => "admitted".to_owned(),
        Err(SettingsError::InvalidWorkflow(diagnostic)) if diagnostic.category == "capacity" => {
            "capacity".to_owned()
        }
        Err(error) => format!("{error:?}"),
    };
    println!("OUTCOME {outcome}");
}

/// One child writer process; it is killed and reaped whatever happens.
struct Writer {
    process: std::process::Child,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Writer {
    fn spawn(roots: &Roots, target: &str, name: &str, hold: bool) -> Self {
        use std::io::BufRead;
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "local_runtime::workflow_authoring::tests::capacity_process_writer_gate",
                "--nocapture",
                "--format=terse",
                "--test-threads=1",
            ])
            .env("RUSTX_491_CAPACITY_HOME", &roots.home)
            .env("RUSTX_491_CAPACITY_SESSION", &roots.a)
            .env("RUSTX_491_CAPACITY_TARGET", target)
            .env("RUSTX_491_CAPACITY_ID", name)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        if hold {
            command.env("RUSTX_491_CAPACITY_HOLD", "1");
        }
        let mut process = command.spawn().unwrap();
        let output = std::io::BufReader::new(process.stdout.take().unwrap());
        let (sent, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in output.lines().map_while(Result::ok) {
                if sent.send(line).is_err() {
                    break;
                }
            }
        });
        Self { process, lines }
    }

    /// The first line starting with `prefix`, within a bounded wait.
    fn expect(&self, prefix: &str) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("the writer never printed {prefix}"));
            if let Some(rest) = line.strip_prefix(prefix) {
                return rest.trim().to_owned();
            }
        }
    }

    fn release(&mut self) {
        use std::io::Write;
        let stdin = self.process.stdin.as_mut().unwrap();
        stdin.write_all(&[0]).unwrap();
        stdin.flush().unwrap();
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[test]
fn writers_in_two_processes_competing_for_the_last_slot_of_one_shared_directory_admit_one() {
    let roots = Roots::new();
    let bytes = roots.minimal_bytes();
    seed(&roots.user_directory(), "s", 63, 0, &bytes);
    // One writer adds through a Workspace at `~/rustx`, whose Workflow
    // directory is the User one; the other adds to Global directly.
    let coincident = roots.home.join("rustx");
    let mut first = Writer::spawn(&roots, coincident.to_str().unwrap(), "first_racer", true);
    first.expect("CAPACITY_LOCKED");
    let second = Writer::spawn(&roots, "user", "second_racer", false);
    #[cfg(target_os = "linux")]
    await_waiter(&crate::local_runtime::settings::lock_path_of(
        &roots.user_directory().join("workflow-capacity"),
    ));
    first.release();
    assert_eq!(first.expect("OUTCOME"), "admitted");
    assert_eq!(second.expect("OUTCOME"), "capacity");
    drop((first, second));
    assert!(roots.user_directory().join("first_racer.yaml").exists());
    assert!(!roots.user_directory().join("second_racer.yaml").exists());
    let catalog = roots.discover(None);
    assert_eq!(catalog.locations.len(), 64);
    assert!(catalog.discovery_diagnostics.is_empty());
}

/// The names directly in `directory`.
fn names(directory: &Path) -> std::collections::BTreeSet<String> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}

#[test]
fn writer_lock_history_never_counts_toward_discovery_of_a_directory() {
    let roots = Roots::new();
    let directory = roots.user_directory();
    // Real create/delete cycles leave each document's lock behind, all of
    // them inside the one lock directory.
    for round in 0..3 {
        let name = format!("cycle_{round}");
        let revision = roots.create(&SourceTarget::User, &name, "Short-lived.");
        roots
            .write(&SourceTarget::User, &name, &revision, WorkflowEdit::Delete)
            .unwrap();
        assert!(
            crate::local_runtime::settings::lock_path_of(&directory.join(format!("{name}.yaml")))
                .exists()
        );
    }
    assert_eq!(names(&directory), [".rustx-locks".to_owned()].into());
    // Representative history: far more locks than discovery reads entries.
    let locks = directory.join(".rustx-locks");
    for index in 0..1100 {
        std::fs::write(locks.join(format!("{index:064x}.lock")), b"").unwrap();
    }
    roots.create(
        &SourceTarget::User,
        "after_history",
        "Created after much history.",
    );
    roots
        .copy(&SourceTarget::User, "after_history", "copied_after")
        .unwrap();
    let library = roots.manager.workflow_library(&SourceTarget::User).unwrap();
    assert!(library.diagnostic.is_none());
    assert_eq!(library.files.len(), 2);
    assert_eq!(
        roots
            .read(&SourceTarget::User, "copied_after")
            .document
            .unwrap()
            .description,
        "Created after much history."
    );
    let catalog = roots.discover(None);
    assert!(catalog.discovery_diagnostics.is_empty());
    assert!(catalog.get(&id("copied_after")).is_some());
}

#[test]
fn refused_additions_leave_no_entry_that_discovery_counts() {
    let roots = Roots::new();
    let bytes = roots.minimal_bytes();
    seed(&roots.user_directory(), "f", 64, 0, &bytes);
    let before = names(&roots.user_directory());
    for index in 0..40 {
        let name = format!("refused_{index}");
        capacity_refusal(roots.write(
            &SourceTarget::User,
            &name,
            "missing",
            WorkflowEdit::Create {
                description: "x".into(),
            },
        ));
        capacity_refusal(roots.copy(&SourceTarget::User, "f000", &format!("copy_{index}")));
    }
    assert_eq!(names(&roots.user_directory()), before);
    let library = roots.manager.workflow_library(&SourceTarget::User).unwrap();
    assert!(library.diagnostic.is_none());
    assert_eq!(library.files.len(), 64);
    assert!(roots.read(&SourceTarget::User, "f063").document.is_some());
    assert!(roots.discover(None).discovery_diagnostics.is_empty());
}

#[test]
fn an_addition_is_refused_when_its_publication_would_pass_the_entry_bound() {
    let roots = Roots::new();
    let directory = roots.user_directory();
    roots.create(&SourceTarget::User, "kept", "Present before the clutter.");
    // External files that are no Workflows: with `kept`, 1022 authored
    // entries beside the uncounted lock directory. A publication briefly
    // holds its staged candidate beside the published file, so 1022 + 2 is
    // the last fit.
    for index in 0..1021 {
        std::fs::write(directory.join(format!("note-{index:04}.txt")), b"").unwrap();
    }
    assert_eq!(names(&directory).len(), 1023);
    roots.create(&SourceTarget::User, "last_fit", "Fits the entry bound.");
    let refused = capacity_refusal(roots.write(
        &SourceTarget::User,
        "past_bound",
        "missing",
        WorkflowEdit::Create {
            description: "x".into(),
        },
    ));
    assert!(
        refused.reason.contains("1024 entries"),
        "{}",
        refused.reason
    );
    capacity_refusal(roots.copy(&SourceTarget::User, "kept", "past_copy"));
    assert_eq!(names(&directory).len(), 1024);
    let library = roots.manager.workflow_library(&SourceTarget::User).unwrap();
    assert!(library.diagnostic.is_none());
    assert_eq!(library.files.len(), 2);
    assert!(roots.discover(None).get(&id("last_fit")).is_some());
}

/// Canonical bytes of a valid minimal Workflow, written by native code in a
/// separate fixture: any write of this one would take its User capacity lock
/// and initialize the lock metadata the first-use tests must not have.
fn external_bytes(_roots: &Roots) -> Vec<u8> {
    Roots::new().minimal_bytes()
}

/// The reported first-use boundary, prepared outside rustX: `kept.yaml` and
/// 1023 ordinary files, 1024 entries, and no lock metadata yet.
fn seed_first_use(directory: &Path, kept: &[u8]) {
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(directory.join("kept.yaml"), kept).unwrap();
    for index in 0..1023 {
        std::fs::write(directory.join(format!("note-{index:04}.txt")), b"").unwrap();
    }
    assert_eq!(names(directory).len(), 1024);
    assert!(!directory.join(".rustx-locks").exists());
}

/// List, read and discover `kept` in `target`, exactly as seeded.
fn assert_kept(roots: &Roots, target: &SourceTarget, kept: &[u8]) {
    let library = roots.manager.workflow_library(target).unwrap();
    assert!(library.diagnostic.is_none(), "{:?}", library.diagnostic);
    assert!(library.files.iter().any(|file| file.id == Some(id("kept"))));
    let source = roots.read(target, "kept");
    assert!(source.document.is_some());
    assert_eq!(std::fs::read(&source.path).unwrap(), kept);
    assert!(roots.discover(None).get(&id("kept")).is_some());
}

#[test]
fn a_first_write_into_an_externally_full_directory_keeps_its_catalog_readable() {
    let roots = Roots::new();
    let kept = external_bytes(&roots);
    seed_first_use(&roots.user_directory(), &kept);
    assert_kept(&roots, &SourceTarget::User, &kept);
    let refused = capacity_refusal(roots.write(
        &SourceTarget::User,
        "fresh",
        "missing",
        WorkflowEdit::Create {
            description: "No room.".into(),
        },
    ));
    assert!(
        refused.reason.contains("1024 entries"),
        "{}",
        refused.reason
    );
    capacity_refusal(roots.copy(&SourceTarget::User, "kept", "kept_copy"));
    // The refusals initialized the lock metadata; it is not counted, and the
    // catalog and the file are exactly what they were.
    assert!(roots.user_directory().join(".rustx-locks").is_dir());
    assert!(!roots.user_directory().join("fresh.yaml").exists());
    assert!(!roots.user_directory().join("kept_copy.yaml").exists());
    assert_eq!(
        std::fs::read(roots.user_directory().join("kept.yaml")).unwrap(),
        kept
    );
    assert_kept(&roots, &SourceTarget::User, &kept);
    assert!(roots.discover(None).discovery_diagnostics.is_empty());
}

#[test]
fn a_first_save_into_an_externally_full_directory_stays_listed_and_readable() {
    let roots = Roots::new();
    let kept = external_bytes(&roots);
    seed_first_use(&roots.user_directory(), &kept);
    assert_kept(&roots, &SourceTarget::User, &kept);
    let before = roots.read(&SourceTarget::User, "kept");
    let mut document = before.document.clone().unwrap();
    document.description = "Saved at the boundary.".into();
    let revision = roots
        .write(
            &SourceTarget::User,
            "kept",
            &before.revision,
            WorkflowEdit::Replace { document },
        )
        .unwrap();
    assert_ne!(revision, before.revision);
    let after = roots.read(&SourceTarget::User, "kept");
    assert_eq!(after.revision, revision);
    assert_eq!(
        after.document.unwrap().description,
        "Saved at the boundary."
    );
    assert!(
        std::fs::read_to_string(roots.user_directory().join("kept.yaml"))
            .unwrap()
            .contains("Saved at the boundary.")
    );
    let library = roots.manager.workflow_library(&SourceTarget::User).unwrap();
    assert!(library.diagnostic.is_none(), "{:?}", library.diagnostic);
    assert_eq!(
        library
            .files
            .iter()
            .find(|file| file.id == Some(id("kept")))
            .unwrap()
            .revision,
        revision
    );
    assert!(roots.discover(None).get(&id("kept")).is_some());
}

#[test]
fn a_workspace_addition_never_makes_a_full_global_directory_unreadable() {
    let roots = Roots::new();
    let kept = external_bytes(&roots);
    seed_first_use(&roots.user_directory(), &kept);
    assert_kept(&roots, &SourceTarget::User, &kept);
    // The addition takes the User capacity lock, initializing User's lock
    // metadata, and counts the composition: it fits, and Global stays whole.
    let target = workspace(&roots.a);
    roots.create(&target, "local", "Added beside a full Global.");
    roots.copy(&target, "local", "local_copy").unwrap();
    assert!(roots.user_directory().join(".rustx-locks").is_dir());
    assert_kept(&roots, &SourceTarget::User, &kept);
    let composed = roots.discover(Some(&roots.a));
    assert!(composed.discovery_diagnostics.is_empty());
    for name in ["kept", "local", "local_copy"] {
        assert!(composed.get(&id(name)).is_some(), "{name}");
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One source cut proves finite pages, complete traversal and stale refusal.
fn model_choices_are_paged_revision_fenced_and_do_not_enlarge_workflow_reads() {
    let roots = Roots::new();
    let target = workspace(&roots.a);
    let path = roots.home.join("rustx/rustx.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    let mut config: serde_json::Value =
        serde_json::to_value(toml::from_str::<toml::Value>(&original).unwrap()).unwrap();
    let mut model = config["models"]["a"].clone();
    let plain_model = model.clone();
    let mut profiles = serde_json::Map::new();
    for index in 0..73 {
        profiles.insert(
            format!("profile-{index:03}"),
            serde_json::json!({"reasoning_enabled":false}),
        );
    }
    // Both source documents stay below their 1 MiB file bound. Their composed
    // catalog exceeds a single response through two distinct Model owners.
    // General catalog identities this long cannot be Workflow selectors.
    profiles.insert(
        "z".repeat(800_000),
        serde_json::json!({"reasoning_enabled":false}),
    );
    model["profiles"] = profiles.into();
    model["default_profile"] = "profile-000".into();
    let models = config["models"].as_object_mut().unwrap();
    models.clear();
    models.insert("model-000".into(), model.clone());
    for index in 1..72 {
        models.insert(format!("model-{index:03}"), plain_model.clone());
    }
    config["agent"]["model"]["model"] = "model-000".into();
    std::fs::write(&path, toml::to_string_pretty(&config).unwrap()).unwrap();
    std::fs::write(
        roots.a.join("rustx.toml"),
        toml::to_string_pretty(&serde_json::json!({
            "models":{"model-072":model}
        }))
        .unwrap(),
    )
    .unwrap();
    roots.create(&target, "small", "A small file with a large model catalog");
    let source = roots.read(&target, "small");
    assert!(serde_json::to_vec(&source).unwrap().len() < 16_384);
    let mut all_models = Vec::new();
    let mut revision = None;
    let mut offset = 0;
    loop {
        let (choices, effects) = super::super::static_effects::measure(|| {
            roots
                .manager
                .workflow_model_choices(
                    &target,
                    WorkflowModelChoicesQuery::Models { offset },
                    revision.as_deref(),
                )
                .unwrap()
        });
        assert_eq!(effects, [0; 11]);
        assert!(serde_json::to_vec(&choices).unwrap().len() < 16_384);
        let WorkflowModelChoices::Models {
            revision: actual,
            models,
            next_offset,
        } = choices
        else {
            panic!("models page");
        };
        assert!(models.len() <= 32);
        assert_eq!(revision.get_or_insert(actual.clone()), &actual);
        all_models.extend(models.into_iter().map(|model| model.to_string()));
        let Some(next) = next_offset else {
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(
        all_models,
        (0..73)
            .map(|index| format!("model-{index:03}"))
            .collect::<Vec<_>>()
    );
    let mut all_profiles = Vec::new();
    let mut offset = 0;
    loop {
        let choices = roots
            .manager
            .workflow_model_choices(
                &target,
                WorkflowModelChoicesQuery::Profiles {
                    model: crate::model::ModelRef::parse("model-000").unwrap(),
                    offset,
                },
                revision.as_deref(),
            )
            .unwrap();
        assert!(serde_json::to_vec(&choices).unwrap().len() < 16_384);
        let WorkflowModelChoices::Profiles {
            profiles,
            next_offset,
            ..
        } = choices
        else {
            panic!("profiles page");
        };
        assert!(profiles.len() <= 32);
        all_profiles.extend(profiles.into_iter().map(|profile| profile.to_string()));
        let Some(next) = next_offset else {
            break;
        };
        offset = next;
    }
    assert_eq!(
        all_profiles,
        (0..73)
            .map(|index| format!("profile-{index:03}"))
            .collect::<Vec<_>>()
    );
    std::fs::write(&path, original).unwrap();
    assert!(matches!(
        roots.manager.workflow_model_choices(
            &target,
            WorkflowModelChoicesQuery::Models { offset: 32 },
            revision.as_deref()
        ),
        Err(SettingsError::Conflict { .. })
    ));
}

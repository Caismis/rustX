//! Native Workflow file authoring over the real protocol boundary
//! (Issue #491): exact scoped list/read/check, whole-file writes through the
//! source commit, typed rejections, and unchanged admission freezes.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;
use crate::local_runtime::configuration::settings::{SourceMutation, SourceTarget};
use crate::local_runtime::workflow_authoring::{
    WorkflowDependencies, WorkflowEdit, WorkflowStructure,
};
use crate::runtime::workflow::WorkflowId;

fn workflow(id: &str, edit: WorkflowEdit) -> SourceMutation {
    SourceMutation::Workflow {
        id: WorkflowId::parse(id).unwrap(),
        edit,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue491_workflow_files_are_authored_over_the_protocol_without_execution() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let workspace = std::fs::canonicalize(&f.workspaces[0]).unwrap();
        let target = SourceTarget::Workspace {
            directory: workspace.clone(),
        };
        let MethodResult::WorkflowLibrary { library } = call(
            &connection,
            1,
            Method::WorkflowList {
                target: target.clone(),
            },
        )
        .await
        else {
            panic!("library")
        };
        assert!(library.files.is_empty());
        assert_eq!(library.directory, workspace.join(".agents/workflows"));

        let MethodResult::SourcesWritten { revision, .. } = call(
            &connection,
            2,
            Method::SourcesWrite {
                target: target.clone(),
                expected_revision: library.absent_revision.clone(),
                mutation: workflow(
                    "review",
                    WorkflowEdit::Create {
                        description: "Review a proposal".into(),
                        display: None,
                    },
                ),
            },
        )
        .await
        else {
            panic!("created")
        };
        let MethodResult::WorkflowSource { source } = call(
            &connection,
            3,
            Method::WorkflowRead {
                target: target.clone(),
                id: WorkflowId::parse("review").unwrap(),
            },
        )
        .await
        else {
            panic!("source")
        };
        assert_eq!(source.revision, revision);
        assert_eq!(source.relative_path, ".agents/workflows/review.yaml");
        let document = source.document.clone().unwrap();

        let MethodResult::WorkflowCheck { report } = call(
            &connection,
            4,
            Method::WorkflowCheck {
                target: target.clone(),
                id: WorkflowId::parse("review").unwrap(),
                document: document.clone(),
                context: None,
                explain: true,
            },
        )
        .await
        else {
            panic!("check")
        };
        assert!(matches!(report.structure, WorkflowStructure::Valid { .. }));
        assert!(matches!(
            report.dependencies,
            WorkflowDependencies::Admitted { .. } | WorkflowDependencies::Unavailable { .. }
        ));
        assert!(report.explanation.is_some());

        // A structurally invalid save is a typed rejection with its path.
        let mut broken = document.clone();
        let start = broken
            .block
            .nodes
            .iter()
            .find_map(|(id, node)| {
                matches!(
                    node,
                    crate::runtime::workflow::WorkflowNodeDefinition::Start {}
                )
                .then_some(id.clone())
            })
            .unwrap();
        broken.block.edges.retain(|edge| edge.from != start);
        broken.block.edges.insert(
            0,
            crate::runtime::workflow::WorkflowEdgeDefinition {
                from: start,
                to: "absent".into(),
                port: None,
            },
        );
        let ErrorData::WorkflowInvalid { diagnostic } = rejected(
            &connection,
            Method::SourcesWrite {
                target: target.clone(),
                expected_revision: revision.clone(),
                mutation: workflow(
                    "review",
                    WorkflowEdit::Replace {
                        document: Box::new(*broken),
                    },
                ),
            },
        )
        .await
        else {
            panic!("invalid Workflow")
        };
        assert_eq!(diagnostic.path, "block.edges.0.to");

        let MethodResult::SourcesWritten {
            revision: copied, ..
        } = call(
            &connection,
            5,
            Method::SourcesWrite {
                target: target.clone(),
                expected_revision: library.absent_revision.clone(),
                mutation: workflow(
                    "review_copy",
                    WorkflowEdit::Copy {
                        source: WorkflowId::parse("review").unwrap(),
                        source_revision: revision.clone(),
                    },
                ),
            },
        )
        .await
        else {
            panic!("copied")
        };
        // A stale delete is a conflict; an exact one removes only its file.
        assert!(matches!(
            rejected(
                &connection,
                Method::SourcesWrite {
                    target: target.clone(),
                    expected_revision: "sha256:stale".into(),
                    mutation: workflow("review_copy", WorkflowEdit::Delete),
                },
            )
            .await,
            ErrorData::SourceConflict { .. }
        ));
        call(
            &connection,
            6,
            Method::SourcesWrite {
                target: target.clone(),
                expected_revision: copied,
                mutation: workflow("review_copy", WorkflowEdit::Delete),
            },
        )
        .await;
        let MethodResult::WorkflowLibrary { library } =
            call(&connection, 7, Method::WorkflowList { target }).await
        else {
            panic!("library")
        };
        assert_eq!(
            library
                .files
                .iter()
                .map(|file| file.file_name.as_str())
                .collect::<Vec<_>>(),
            ["review.yaml"]
        );
        // Authoring executed nothing and created no Session.
        assert!(f.provider.request_bodies().is_empty());
        assert_eq!(
            f.manager
                .sessions
                .list_sessions(None, 0, 32)
                .await
                .unwrap()
                .sessions
                .len(),
            1
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue491_saved_workflow_files_change_only_future_adoption() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let target = SourceTarget::Workspace {
            directory: std::fs::canonicalize(&f.workspaces[0]).unwrap(),
        };
        // The root Agent selects the Workflow, so its program is demand.
        std::fs::write(
            f.workspaces[0].join("rustx.toml"),
            "[environment]\nRESIDENCY_SESSION = \"A\"\n[agent]\nworkflows = [\"fresh\"]\n",
        )
        .unwrap();
        let created = f
            .manager
            .write_source_settings(
                &target,
                "missing".into(),
                workflow(
                    "fresh",
                    WorkflowEdit::Create {
                        description: "First description".into(),
                        display: None,
                    },
                ),
            )
            .await
            .unwrap();
        let id = f.sessions[0].id.clone();
        f.manager.load(&id, None).await.unwrap();
        let runtime = f.manager.configuration_runtime(&id).unwrap();
        // The adopted generation's own compiled program, which an admitted
        // Attempt freezes.
        let description = || {
            runtime
                .configuration_cut()
                .resources
                .workflows()
                .get(&WorkflowId::parse("fresh").unwrap())
                .map(|program| program.description().to_owned())
        };
        assert_eq!(description().as_deref(), Some("First description"));
        let source = f
            .manager
            .workflow_source(target.clone(), WorkflowId::parse("fresh").unwrap())
            .await
            .unwrap();
        assert_eq!(source.revision, created.revision);
        let mut document = *source.document.unwrap();
        document.description = "Second description".into();
        let receipt = f
            .manager
            .write_source_settings(
                &target,
                created.revision,
                workflow(
                    "fresh",
                    WorkflowEdit::Replace {
                        document: Box::new(document),
                    },
                ),
            )
            .await
            .unwrap();
        // File commit and publication are separate facts from adoption: the
        // Session keeps its admitted program until it adopts the candidate.
        assert!(receipt.configuration_revision.is_some());
        let application = super::super::configuration::settled(&f, 0).await;
        assert_eq!(description().as_deref(), Some("First description"));
        let candidate = application.candidate.expect("a newer prepared candidate");
        f.manager
            .adopt_configuration(&id, &candidate.identity, candidate.expected_binding)
            .unwrap();
        assert_eq!(description().as_deref(), Some("Second description"));
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

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

fn workflow<D>(id: &str, edit: WorkflowEdit<D>) -> SourceMutation<D> {
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
        let report = read_report(&connection, &target, &report).await;
        assert!(matches!(report.structure, WorkflowStructure::Valid { .. }));
        assert!(matches!(
            report.dependencies,
            WorkflowDependencies::Admitted { .. } | WorkflowDependencies::Unavailable { .. }
        ));
        assert!(report.explanation.is_some());

        // As a Dify draft, an unfinished save commits; reading it back
        // keeps the document and names its first problem by path.
        let mut broken: Box<crate::workflow_file::WorkflowFileDocument> =
            Box::new(read_document(&connection, &target, &document).await);
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
        let MethodResult::SourcesWritten {
            revision: unfinished,
            ..
        } = call(
            &connection,
            41,
            Method::SourcesWrite {
                target: target.clone(),
                expected_revision: revision.clone(),
                mutation: workflow(
                    "review",
                    WorkflowEdit::Replace {
                        document: Box::new(stage_document(&connection, &target, &broken).await),
                    },
                ),
            },
        )
        .await
        else {
            panic!("unfinished save")
        };
        let MethodResult::WorkflowSource { source } = call(
            &connection,
            42,
            Method::WorkflowRead {
                target: target.clone(),
                id: WorkflowId::parse("review").unwrap(),
            },
        )
        .await
        else {
            panic!("source")
        };
        assert!(source.document.is_some());
        assert_eq!(source.diagnostic.unwrap().path, "block.edges.0.to");

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
                        source_revision: unfinished,
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

async fn stage_document(
    connection: &AppServerConnection,
    target: &SourceTarget,
    document: &crate::workflow_file::WorkflowFileDocument,
) -> crate::app_server::workflow_documents::WorkflowDocumentRef {
    use base64::Engine;
    use sha2::Digest;
    let bytes = crate::app_server::wire::workflow_document_to_bytes(document).unwrap();
    let MethodResult::WorkflowDocument { reference } = call(
        connection,
        1000,
        Method::WorkflowDocumentBegin {
            target: target.clone(),
            byte_length: u32::try_from(bytes.len()).unwrap(),
            sha256: format!("{:x}", sha2::Sha256::digest(&bytes)),
        },
    )
    .await
    else {
        panic!("document begin")
    };
    for (index, chunk) in bytes.chunks(65_536).enumerate() {
        call(
            connection,
            1001,
            Method::WorkflowDocumentWrite {
                target: target.clone(),
                reference: reference.clone(),
                offset: u32::try_from(index * 65_536).unwrap(),
                data: base64::engine::general_purpose::STANDARD.encode(chunk),
            },
        )
        .await;
    }
    call(
        connection,
        1002,
        Method::WorkflowDocumentFinish {
            target: target.clone(),
            reference: reference.clone(),
        },
    )
    .await;
    reference
}

async fn read_document(
    connection: &AppServerConnection,
    target: &SourceTarget,
    reference: &crate::app_server::workflow_documents::WorkflowDocumentRef,
) -> crate::workflow_file::WorkflowFileDocument {
    use base64::Engine;
    let mut bytes = Vec::new();
    loop {
        let MethodResult::WorkflowDocumentChunk { chunk } = call(
            connection,
            1003,
            Method::WorkflowDocumentRead {
                target: target.clone(),
                reference: reference.clone(),
                offset: u32::try_from(bytes.len()).unwrap(),
                max_bytes: 65_536,
            },
        )
        .await
        else {
            panic!("document read")
        };
        bytes.extend(
            base64::engine::general_purpose::STANDARD
                .decode(chunk.data)
                .unwrap(),
        );
        if chunk.next_offset.is_none() {
            break;
        }
    }
    crate::app_server::wire::workflow_document_from_json(&String::from_utf8(bytes).unwrap())
        .unwrap()
}

async fn read_report(
    connection: &AppServerConnection,
    target: &SourceTarget,
    reference: &crate::app_server::workflow_documents::WorkflowCheckReportRef,
) -> crate::local_runtime::workflow_authoring::WorkflowCheckReport {
    use base64::Engine as _;
    let mut bytes = Vec::new();
    loop {
        let MethodResult::WorkflowCheckReportChunk { chunk } = call(
            connection,
            1004,
            Method::WorkflowCheckReportRead {
                target: target.clone(),
                reference: reference.clone(),
                offset: bytes.len() as u64,
                max_bytes: 65_536,
            },
        )
        .await
        else {
            panic!("report chunk")
        };
        bytes.extend(
            base64::engine::general_purpose::STANDARD
                .decode(chunk.data)
                .unwrap(),
        );
        if chunk.next_offset.is_none() {
            break;
        }
    }
    call(
        connection,
        1005,
        Method::WorkflowCheckReportRelease {
            target: target.clone(),
            reference: reference.clone(),
        },
    )
    .await;
    crate::app_server::wire::workflow_check_report_from_json(&String::from_utf8(bytes).unwrap())
        .unwrap()
}

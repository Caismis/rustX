//! Issue #464: automatic `rustx.toml` repair at the native App Server
//! boundaries, its source-scoped receipts and notices, and their separation
//! from runtime application, canonical history and execution.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::protocol::{call, initialize};
use super::*;
use crate::app_server::connection::AppServerConnection;
use crate::app_server::protocol::{
    JsonRpcVersion, Method, MethodResult, NotificationMethod, Request, RequestId, Response,
};
use crate::local_runtime::configuration::observation::{BoundedSourceSettings, SourceFieldName};
use crate::local_runtime::configuration::repair::{
    ConfigRepairChange, ConfigRepairReceipt, ConfigRepairStatus,
};
use crate::local_runtime::configuration::settings::{
    SourceDocumentView, SourceMutation, SourceTarget, SourceView,
};

/// Only source fields this repair contract asserts, from one authoritative read cut.
struct RepairSourceFields {
    user_repair: Option<ConfigRepairReceipt>,
    workspace_repair: Option<ConfigRepairReceipt>,
    repair_sequence: u64,
    workspace: Option<SourceView<SourceDocumentView>>,
    prospective_diagnostic: Option<String>,
}

async fn source_field<T: serde::de::DeserializeOwned + schemars::JsonSchema>(
    connection: &AppServerConnection,
    target: &SourceTarget,
    view: &BoundedSourceSettings,
    name: SourceFieldName,
) -> T {
    use base64::Engine as _;
    static NEXT_READ_ID: std::sync::atomic::AtomicI64 =
        std::sync::atomic::AtomicI64::new(100_000_000);
    let field = view
        .fields
        .iter()
        .find(|field| field.name == name)
        .expect("advertised source field");
    let value = if let Some(reference) = &field.reference {
        assert_eq!(reference.revision, view.revision);
        assert_eq!(reference.field, name);
        assert!(reference.item_index.is_none());
        assert!(
            reference.byte_length <= 16 * 1024 * 1024,
            "explicit fixture field byte-cache bound"
        );
        let mut bytes = Vec::new();
        let mut offset = 0;
        loop {
            let id = NEXT_READ_ID.fetch_add(1, Ordering::Relaxed);
            let MethodResult::SourceValueChunk { chunk } = call(
                connection,
                id,
                Method::SourcesValueRead {
                    target: target.clone(),
                    reference: reference.clone(),
                    offset,
                    max_bytes: 65_536,
                },
            )
            .await
            else {
                panic!("source value chunk")
            };
            assert_eq!(chunk.reference, *reference);
            assert_eq!(chunk.offset, offset);
            assert!(chunk.data_base64.len() <= 87_384);
            let part = base64::engine::general_purpose::STANDARD
                .decode(chunk.data_base64)
                .unwrap();
            assert!(part.len() <= 65_536);
            let end = offset + u64::try_from(part.len()).unwrap();
            assert_eq!(chunk.end_offset, end);
            assert!(end <= reference.byte_length);
            assert!(end > offset || end == reference.byte_length);
            assert_eq!(
                chunk.next_offset,
                (end < reference.byte_length).then_some(end)
            );
            bytes.extend_from_slice(&part);
            offset = end;
            if offset == reference.byte_length {
                break;
            }
        }
        serde_json::from_slice(&bytes).unwrap()
    } else {
        field.value.clone().expect("inline source field")
    };
    crate::observation::structural_json::decode(value).unwrap()
}

fn workspace(f: &Fixture, index: usize) -> SourceTarget {
    SourceTarget::Workspace {
        directory: std::fs::canonicalize(&f.workspaces[index]).unwrap(),
    }
}

fn user_path(f: &Fixture) -> std::path::PathBuf {
    f.manager
        .configuration
        .source_bindings()
        .config_path
        .clone()
}

fn prepend(path: &std::path::Path, text: &str) -> String {
    let original = std::fs::read_to_string(path).unwrap();
    let edited = format!("{text}{original}");
    std::fs::write(path, &edited).unwrap();
    edited
}

async fn sources(
    connection: &AppServerConnection,
    id: i64,
    target: &SourceTarget,
) -> RepairSourceFields {
    let MethodResult::SourceSettings { projection } = call(
        connection,
        id,
        Method::SourcesRead {
            target: target.clone(),
        },
    )
    .await
    else {
        panic!("source settings")
    };
    assert_eq!(projection.target, *target);
    if let Some(source) = projection.inline {
        RepairSourceFields {
            user_repair: source.user_repair,
            workspace_repair: source.workspace_repair,
            repair_sequence: source.repair_sequence,
            workspace: source.workspace,
            prospective_diagnostic: source.prospective_diagnostic,
        }
    } else {
        RepairSourceFields {
            user_repair: source_field(connection, target, &projection, SourceFieldName::UserRepair)
                .await,
            workspace_repair: source_field(
                connection,
                target,
                &projection,
                SourceFieldName::WorkspaceRepair,
            )
            .await,
            repair_sequence: source_field(
                connection,
                target,
                &projection,
                SourceFieldName::RepairSequence,
            )
            .await,
            workspace: source_field(connection, target, &projection, SourceFieldName::Workspace)
                .await,
            prospective_diagnostic: source_field(
                connection,
                target,
                &projection,
                SourceFieldName::ProspectiveDiagnostic,
            )
            .await,
        }
    }
}

/// The next repair notice of `target` with its sequence, skipping
/// application notices, which are a different owner and scope. Another
/// source's notice before it may only be the empty level of a source this
/// connection admitted healthy.
async fn sequenced_repair_notice(
    connection: &AppServerConnection,
    target: &SourceTarget,
) -> (Option<ConfigRepairReceipt>, u64) {
    loop {
        match connection.next_notification().await.notification {
            NotificationMethod::SourceRepairChanged {
                target: announced,
                receipt,
                sequence,
            } => {
                if announced == *target {
                    return (receipt, sequence);
                }
                assert!(receipt.is_none(), "{receipt:?}");
            }
            NotificationMethod::ConfigurationChanged { application } => {
                // Application scopes are Session or source application keys,
                // never repair facts.
                assert!(!application.scope.is_empty());
            }
            _ => {}
        }
    }
}

/// The next repair notice of `target`.
async fn repair_notice(
    connection: &AppServerConnection,
    target: &SourceTarget,
) -> Option<ConfigRepairReceipt> {
    sequenced_repair_notice(connection, target).await.0
}

/// Every repair notice this connection owes now, by source: the levels of
/// sources it newly admitted. Each is empty — admitting a healthy source
/// presents nothing — and no source is delivered twice.
fn empty_levels(connection: &AppServerConnection) -> std::collections::BTreeSet<String> {
    let mut levels = std::collections::BTreeSet::new();
    while let Some(notice) = connection.next_source_repair() {
        let NotificationMethod::SourceRepairChanged {
            target, receipt, ..
        } = notice
        else {
            panic!("a source repair notice")
        };
        assert!(receipt.is_none(), "{receipt:?}");
        assert!(levels.insert(key(&target)), "a level was delivered twice");
    }
    levels
}

/// Collect exactly `count` repair notices, keyed by source.
async fn repair_notices(
    connection: &AppServerConnection,
    count: usize,
) -> std::collections::BTreeMap<String, Option<ConfigRepairReceipt>> {
    let mut notices = std::collections::BTreeMap::new();
    while notices.len() < count {
        let (target, receipt) = loop {
            if let NotificationMethod::SourceRepairChanged {
                target, receipt, ..
            } = connection.next_notification().await.notification
            {
                break (target, receipt);
            }
        };
        assert!(
            notices
                .insert(serde_json::to_string(&target).unwrap(), receipt)
                .is_none(),
            "a source was announced twice"
        );
    }
    notices
}

fn key(target: &SourceTarget) -> String {
    serde_json::to_string(target).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_creation_repairs_both_sources_before_resolution() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let target = workspace(&f, 0);
        prepend(&user_path(&f), "legacy_user = true\n");
        let workspace_file = f.workspaces[0].join("rustx.toml");
        std::fs::write(
            &workspace_file,
            "[environment]\nRESIDENCY_SESSION = \"A\"\n[agent.plugins.todo]\nenabled = true\nlegacy_grant = \"all\"\n",
        )
        .unwrap();
        let creator = AppServerConnection::new(f.host.clone());
        let observer = AppServerConnection::new(f.host.clone());
        initialize(&creator).await;
        initialize(&observer).await;
        let requests = f.provider.attempt_count();
        // Strictly, both documents would reject this Session's resolution.
        let MethodResult::SessionTransition { .. } = call(
            &creator,
            1,
            Method::SessionCreate { submission: None,
                settings: SessionPersistentState::from_input(&SessionConfigInput::new(
                    f.workspaces[0].clone(),
                )),
            },
        )
        .await
        else {
            panic!("Session created")
        };
        assert!(!std::fs::read_to_string(user_path(&f)).unwrap().contains("legacy_user"));
        assert_eq!(
            std::fs::read_to_string(&workspace_file).unwrap(),
            "[environment]\nRESIDENCY_SESSION = \"A\"\n[agent.plugins.todo]\nenabled = true\n"
        );
        let notices = repair_notices(&creator, 2).await;
        let user = notices[&key(&SourceTarget::User)].clone().unwrap();
        let repaired = notices[&key(&target)].clone().unwrap();
        assert_eq!(user.status, ConfigRepairStatus::Repaired);
        assert_eq!(
            user.changes,
            vec![ConfigRepairChange::RemovedField {
                object_path: String::new()
            }]
        );
        assert_eq!(repaired.status, ConfigRepairStatus::Repaired);
        assert_eq!(repaired.target, target);
        assert!(creator.next_source_repair().is_none());

        // Another connection observes the User source, but no Workspace it
        // never addressed.
        let receipt = repair_notice(&observer, &SourceTarget::User).await;
        assert_eq!(receipt.unwrap().id, user.id);
        assert!(observer.next_source_repair().is_none());
        // Addressing the Workspace makes its receipt readable, and announces
        // the same repair exactly once.
        let read = sources(&observer, 2, &target).await;
        assert_eq!(read.workspace_repair.as_ref(), Some(&repaired));
        assert_eq!(read.user_repair.as_ref(), Some(&user));
        let receipt = repair_notice(&observer, &target).await;
        assert_eq!(receipt.unwrap().id, repaired.id);
        for id in 3..6 {
            sources(&observer, id, &target).await;
            sources(&creator, id, &target).await;
        }
        assert!(observer.next_source_repair().is_none());
        assert!(creator.next_source_repair().is_none());
        // A reconnecting client learns each latest repair once, never more.
        let reconnected = AppServerConnection::new(f.host.clone());
        initialize(&reconnected).await;
        sources(&reconnected, 1, &target).await;
        let replayed = repair_notices(&reconnected, 2).await;
        assert_eq!(replayed[&key(&SourceTarget::User)].as_ref().unwrap().id, user.id);
        assert_eq!(replayed[&key(&target)].as_ref().unwrap().id, repaired.id);
        assert!(reconnected.next_source_repair().is_none());
        // Repair needs no model request.
        assert_eq!(f.provider.attempt_count(), requests);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconcile_repairs_an_external_edit_once_and_reads_stay_read_only() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let target = workspace(&f, 0);
        let runtime = f.load(0).await.unwrap().unwrap();
        let live = runtime.inspect_runtime().unwrap();
        let journal = events(&live).len();
        let requests = f.provider.attempt_count();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        // A healthy reconcile establishes the Session's application baseline
        // and repairs nothing.
        call(
            &connection,
            100,
            Method::ConfigurationReconcile {
                target: target.clone(),
            },
        )
        .await;
        assert!(f.manager.source_repairs().latest(&target).is_none());
        let workspace_file = f.workspaces[0].join("rustx.toml");
        let edited = format!(
            "{}[agent]\ninstructions = \"Reconciled instructions\"\nfuture_trust = true\n",
            std::fs::read_to_string(&workspace_file).unwrap()
        );
        std::fs::write(&workspace_file, &edited).unwrap();
        // Reading Settings is never a repair trigger.
        for id in 1..4 {
            let read = sources(&connection, id, &target).await;
            assert!(read.workspace_repair.is_none());
        }
        assert_eq!(std::fs::read_to_string(&workspace_file).unwrap(), edited);
        // Only the empty levels this connection's admissions owe it.
        assert_eq!(
            empty_levels(&connection),
            [key(&SourceTarget::User), key(&target)].into()
        );
        assert!(connection.next_source_repair().is_none());
        let probe = f.manager.probe(&f.id(0).await);
        let baseline = super::configuration::settled(&f, 0).await;
        let before = probe.configuration_preparations.load(Ordering::SeqCst);
        let MethodResult::ConfigurationReceipt { .. } = call(
            &connection,
            4,
            Method::ConfigurationReconcile {
                target: target.clone(),
            },
        )
        .await
        else {
            panic!("reconciled")
        };
        let repaired = std::fs::read_to_string(&workspace_file).unwrap();
        assert!(!repaired.contains("future_trust"));
        let receipt = repair_notice(&connection, &target).await.unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
        assert_eq!(
            receipt.changes,
            vec![ConfigRepairChange::RemovedField {
                object_path: "agent".into()
            }]
        );
        // One committed repair is one prepared application for its consumers,
        // however many captures observe it.
        let applied = super::configuration::settled(&f, 0).await;
        assert_ne!(
            applied.desired.input_revision,
            baseline.desired.input_revision
        );
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            before + 1
        );
        // Reconciling again observes healthy bytes: nothing is written,
        // announced or attempted.
        call(
            &connection,
            5,
            Method::ConfigurationReconcile {
                target: target.clone(),
            },
        )
        .await;
        assert_eq!(std::fs::read_to_string(&workspace_file).unwrap(), repaired);
        assert!(connection.next_source_repair().is_none());
        let settled = super::configuration::settled(&f, 0).await;
        assert_eq!(settled.desired, applied.desired);
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            before + 1
        );
        // Repair facts never enter canonical history or reach a provider.
        assert_eq!(events(&live).len(), journal);
        assert_eq!(f.provider.attempt_count(), requests);
        drop(live);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unrepairable_workspace_is_announced_and_manual_repair_clears_it() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let target = workspace(&f, 1);
        let workspace_file = f.workspaces[1].join("rustx.toml");
        let broken = "[agent\nlegacy = 1\n";
        std::fs::write(&workspace_file, broken).unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let Response::Failure(_) = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(1),
                call: Method::SessionCreate {
                    submission: None,
                    settings: SessionPersistentState::from_input(&SessionConfigInput::new(
                        f.workspaces[1].clone(),
                    )),
                },
            })
            .await
        else {
            panic!("an unrepairable Workspace still fails Session creation")
        };
        assert_eq!(std::fs::read_to_string(&workspace_file).unwrap(), broken);
        let receipt = repair_notice(&connection, &target).await.unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
        assert!(receipt.after_revision.is_none());
        assert!(receipt.diagnostic.unwrap().contains("malformed TOML"));
        let read = sources(&connection, 2, &target).await;
        assert_eq!(
            read.workspace_repair.as_ref().map(|receipt| receipt.status),
            Some(ConfigRepairStatus::Unresolved)
        );
        // The existing manual repair stays available; a committed valid
        // document withdraws the unresolved observation.
        let MethodResult::SourcesWritten { revision, .. } = call(
            &connection,
            3,
            Method::SourcesWrite {
                target: target.clone(),
                expected_revision: read.workspace.unwrap().revision,
                mutation: SourceMutation::RepairConfig {
                    document: "[environment]\nRESIDENCY_SESSION = \"B\"\n".into(),
                },
            },
        )
        .await
        else {
            panic!("manual repair")
        };
        let observed = sources(&connection, 4, &target).await;
        assert_eq!(observed.workspace.as_ref().unwrap().revision, revision);
        assert!(observed.workspace_repair.is_none());
        assert!(repair_notice(&connection, &target).await.is_none());
        assert!(connection.next_source_repair().is_none());
    })
    .await;
}

/// A waker that records whether it was woken.
struct WakeFlag(std::sync::atomic::AtomicBool);

impl std::task::Wake for WakeFlag {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Drain whatever is deliverable now, with no wait, and return the parked
/// reader. Nothing drained may concern `excluded`.
fn park<'a>(
    connection: &'a AppServerConnection,
    excluded: &SourceTarget,
) -> std::pin::Pin<
    Box<impl std::future::Future<Output = crate::app_server::protocol::Notification> + 'a>,
> {
    use futures_util::FutureExt;
    loop {
        let mut next = Box::pin(connection.next_notification());
        match next.as_mut().now_or_never() {
            Some(notification) => {
                if let NotificationMethod::SourceRepairChanged { target, .. } =
                    notification.notification
                {
                    assert_ne!(&target, excluded, "an unaddressed Workspace was announced");
                }
            }
            None => return next,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_workspace_interest_wakes_a_parked_reader_once() {
    use std::future::Future;
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let target = workspace(&f, 0);
        // Workspace A already holds a committed repair receipt.
        std::fs::write(
            f.workspaces[0].join("rustx.toml"),
            "[agent]\nfuture_trust = true\n",
        )
        .unwrap();
        assert_eq!(
            f.manager.configuration.repair_source(
                &target,
                &f.manager.configuration.admit_repair_scope(&target).unwrap()
            ),
            crate::local_runtime::configuration::repair::RepairOutcome::Committed
        );
        let receipt = f.manager.source_repairs().latest(&target).unwrap();
        let reader = AppServerConnection::new(f.host.clone());
        let bystander = AppServerConnection::new(f.host.clone());
        initialize(&reader).await;
        initialize(&bystander).await;
        // The reader is parked: every level check ran and found nothing.
        let mut next = park(&reader, &target);
        let flag = std::sync::Arc::new(WakeFlag(std::sync::atomic::AtomicBool::new(false)));
        let waker = std::task::Waker::from(flag.clone());
        let mut context = std::task::Context::from_waker(&waker);
        assert!(next.as_mut().poll(&mut context).is_pending());
        let sequence = *f.manager.source_repairs().subscribe().borrow();
        // Its first operation addressing A, and nothing else happens: no
        // source, Session or ledger event.
        sources(&reader, 1, &target).await;
        assert_eq!(*f.manager.source_repairs().subscribe().borrow(), sequence);
        assert!(
            flag.0.load(Ordering::SeqCst),
            "admitting A woke the parked reader"
        );
        let std::task::Poll::Ready(notification) = next.as_mut().poll(&mut context) else {
            panic!("A's existing receipt is deliverable")
        };
        let NotificationMethod::SourceRepairChanged {
            target: announced,
            receipt: Some(announced_receipt),
            ..
        } = notification.notification
        else {
            panic!("a source repair notice")
        };
        assert_eq!(announced, target);
        assert_eq!(announced_receipt.id, receipt.id);
        drop(next);
        // Addressing A again neither repeats the notice nor wakes anything.
        for id in 2..4 {
            sources(&reader, id, &target).await;
        }
        drop(park(&reader, &target));
        assert!(reader.next_source_repair().is_none());
        // A connection that never addressed A never hears of it.
        drop(park(&bystander, &target));
        assert!(bystander.next_source_repair().is_none());
    })
    .await;
}

/// PHY-03: ordinary authoring publishes under the same physical source
/// authority as automatic repair. A Workspace pathname replaced by a symlink
/// to another Workspace with identical bytes, after the write admitted its
/// directory, is refused as a redirected target: nothing is written anywhere
/// and nothing reaches the configuration coordinator.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_source_write_into_a_replaced_workspace_is_refused_before_application() {
    use crate::app_server::protocol::{ErrorData, Failure, RpcError};
    use crate::local_runtime::authoring::SubagentsLayer;
    use crate::local_runtime::configuration::settings::ConfigMutation;
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let target = workspace(&f, 0);
        let runtime = f.load(0).await.unwrap().unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        call(
            &connection,
            1,
            Method::ConfigurationReconcile {
                target: target.clone(),
            },
        )
        .await;
        let probe = f.manager.probe(&f.id(0).await);
        let baseline = super::configuration::settled(&f, 0).await;
        let before = probe.configuration_preparations.load(Ordering::SeqCst);
        let read = sources(&connection, 2, &target).await;
        let directory = target.workspace().unwrap().to_path_buf();
        let text = std::fs::read_to_string(directory.join("rustx.toml")).unwrap();
        let moved = directory.with_file_name("moved-workspace");
        let other = directory.with_file_name("other-workspace");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("rustx.toml"), &text).unwrap();
        let (from, to, link) = (directory.clone(), moved.clone(), other.clone());
        f.manager
            .configuration
            .test_hooks
            .insert("before_publication", move || {
                std::fs::rename(&from, &to).unwrap();
                std::os::unix::fs::symlink(&link, &from).unwrap();
            });
        let Response::Failure(Failure {
            error: RpcError {
                data: Some(data), ..
            },
            ..
        }) = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(3),
                call: Method::SourcesWrite {
                    target: target.clone(),
                    expected_revision: read.workspace.unwrap().revision,
                    mutation: SourceMutation::Config {
                        mutation: ConfigMutation::Capacity {
                            authored: Some(SubagentsLayer {
                                max_concurrent: Some(3),
                            }),
                        },
                    },
                },
            })
            .await
        else {
            panic!("a redirected source write is refused")
        };
        assert!(matches!(data, ErrorData::InvalidParams), "{data:?}");
        assert_eq!(
            std::fs::read_to_string(other.join("rustx.toml")).unwrap(),
            text
        );
        assert_eq!(
            std::fs::read_to_string(moved.join("rustx.toml")).unwrap(),
            text
        );
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            before
        );
        // Put the admitted Workspace back for the live Session's teardown.
        std::fs::remove_file(&directory).unwrap();
        std::fs::rename(&moved, &directory).unwrap();
        assert_eq!(
            super::configuration::settled(&f, 0).await.desired,
            baseline.desired
        );
        assert_eq!(
            empty_levels(&connection),
            [key(&SourceTarget::User), key(&target)].into()
        );
        drop(runtime);
        f.close().await;
    })
    .await;
}

/// Every file below `directory` whose bytes contain `needle`.
fn files_containing(directory: &std::path::Path, needle: &[u8]) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        let kind = std::fs::symlink_metadata(&path).unwrap().file_type();
        if kind.is_dir() {
            found.extend(files_containing(&path, needle));
        } else if kind.is_file()
            && std::fs::read(&path)
                .unwrap()
                .windows(needle.len())
                .any(|window| window == needle)
        {
            found.push(path);
        }
    }
    found
}

/// An external User edit no live Session has captured: new instructions,
/// which a captured commit prepares for every Session composed over the User,
/// beside a repairable field holding `secret`.
fn edit_user(f: &Fixture, secret: &str) -> String {
    let mut document: toml_edit::DocumentMut = std::fs::read_to_string(user_path(f))
        .unwrap()
        .parse()
        .unwrap();
    document["agent"]["instructions"] = toml_edit::value("User instructions");
    document["agent"]["legacy_trust"] = toml_edit::value(secret);
    let user = document.to_string();
    std::fs::write(user_path(f), &user).unwrap();
    user
}

/// ISO-01..04 at the real native Session creation boundary: a selected
/// Workspace that is not isolated from the runtime root — the runtime root
/// inside it, or it inside the runtime root — is refused before the User
/// source is touched. Neither source changes, no credential-bearing backup is
/// written, no receipt is recorded and no repair commit reaches the
/// configuration coordinator of the live Session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_creation_admits_the_selected_workspace_before_any_repair() {
    const SENTINEL: &str = "SECRET_SENTINEL_464";
    for overlap in [
        "runtime root inside the Workspace",
        "Workspace inside the runtime root",
    ] {
        bounded(async {
            let f = Fixture::with_session_count(None, 1).await;
            let runtime = f.load(0).await.unwrap().unwrap();
            let connection = AppServerConnection::new(f.host.clone());
            initialize(&connection).await;
            call(
                &connection,
                1,
                Method::ConfigurationReconcile {
                    target: workspace(&f, 0),
                },
            )
            .await;
            let probe = f.manager.probe(&f.id(0).await);
            let baseline = super::configuration::settled(&f, 0).await;
            let before = probe.configuration_preparations.load(Ordering::SeqCst);
            let root = f.archive_root.clone();
            let selected = if overlap.starts_with("runtime root") {
                // HOME: the runtime root is `HOME/rustx/runtime`.
                root.ancestors().nth(2).unwrap().to_path_buf()
            } else {
                let inside = root.join("project");
                std::fs::create_dir(&inside).unwrap();
                inside
            };
            let workspace_file = selected.join("rustx.toml");
            let workspace_text = format!("[agent]\nlegacy = \"{SENTINEL}\"\n");
            std::fs::write(&workspace_file, &workspace_text).unwrap();
            // A Workspace enclosing HOME also encloses the User source.
            let mut sentinel_files = vec![workspace_file.clone()];
            if user_path(&f).starts_with(&selected) {
                sentinel_files.push(user_path(&f));
            }
            sentinel_files.sort();
            let user = edit_user(&f, SENTINEL);
            let sequence = *f.manager.source_repairs().subscribe().borrow();
            let Response::Failure(_) = connection
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(2),
                    call: Method::SessionCreate {
                        submission: None,
                        settings: SessionPersistentState::from_input(&SessionConfigInput::new(
                            selected.clone(),
                        )),
                    },
                })
                .await
            else {
                panic!("{overlap}: Session admission fails")
            };
            assert_eq!(
                std::fs::read_to_string(user_path(&f)).unwrap(),
                user,
                "{overlap}"
            );
            assert_eq!(
                std::fs::read_to_string(&workspace_file).unwrap(),
                workspace_text,
                "{overlap}"
            );
            assert!(!root.join("configuration-backups").exists(), "{overlap}");
            // Only the sources themselves carry the sentinel there.
            let mut found = files_containing(&selected, SENTINEL.as_bytes());
            found.sort();
            assert_eq!(found, sentinel_files, "{overlap}");
            assert!(
                f.manager
                    .source_repairs()
                    .latest(&SourceTarget::User)
                    .is_none()
            );
            assert_eq!(
                *f.manager.source_repairs().subscribe().borrow(),
                sequence,
                "{overlap}"
            );
            // The refused Workspace was addressed, so its level is owed; it is
            // as empty as everything else here.
            assert_eq!(
                empty_levels(&connection),
                [
                    key(&SourceTarget::User),
                    key(&workspace(&f, 0)),
                    key(&SourceTarget::Workspace {
                        directory: std::fs::canonicalize(&selected).unwrap()
                    })
                ]
                .into(),
                "{overlap}"
            );
            assert_eq!(
                super::configuration::settled(&f, 0).await.desired,
                baseline.desired,
                "{overlap}"
            );
            assert_eq!(
                probe.configuration_preparations.load(Ordering::SeqCst),
                before,
                "{overlap}"
            );
            std::fs::remove_file(&workspace_file).unwrap();
            drop(runtime);
            f.close().await;
        })
        .await;
    }
}

/// ISO-06 with a valid, disjoint selected Workspace: both sources are
/// repaired as independent transactions with their own revisions and
/// receipts, both backups are private and outside the Workspace, and each
/// commit reaches the coordinator exactly as a source write would: the User
/// commit once for the live Session composed over it, the other Workspace's
/// commit not at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_valid_selected_workspace_repairs_both_sources_independently() {
    use std::os::unix::fs::PermissionsExt;
    bounded(async {
        let f = Fixture::new().await;
        let runtime = f.load(0).await.unwrap().unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        call(
            &connection,
            1,
            Method::ConfigurationReconcile {
                target: workspace(&f, 0),
            },
        )
        .await;
        let probe = f.manager.probe(&f.id(0).await);
        let baseline = super::configuration::settled(&f, 0).await;
        let before = probe.configuration_preparations.load(Ordering::SeqCst);
        let user = edit_user(&f, "SECRET_SENTINEL_464");
        let target = workspace(&f, 1);
        let workspace_file = f.workspaces[1].join("rustx.toml");
        let workspace_text = format!(
            "{}[agent]\nlegacy = \"SECRET_SENTINEL_464\"\n",
            std::fs::read_to_string(&workspace_file).unwrap()
        );
        std::fs::write(&workspace_file, &workspace_text).unwrap();
        let MethodResult::SessionTransition { .. } = call(
            &connection,
            2,
            Method::SessionCreate {
                submission: None,
                settings: SessionPersistentState::from_input(&SessionConfigInput::new(
                    f.workspaces[1].clone(),
                )),
            },
        )
        .await
        else {
            panic!("Session created")
        };
        let user_receipt = f
            .manager
            .source_repairs()
            .latest(&SourceTarget::User)
            .unwrap();
        let workspace_receipt = f.manager.source_repairs().latest(&target).unwrap();
        let revision = |bytes: &[u8]| {
            use sha2::Digest;
            format!("sha256:{:x}", sha2::Sha256::digest(bytes))
        };
        for (receipt, path, original) in [
            (&user_receipt, user_path(&f), &user),
            (&workspace_receipt, workspace_file.clone(), &workspace_text),
        ] {
            let repaired = std::fs::read(&path).unwrap();
            assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
            assert!(receipt.backup_available);
            assert_eq!(receipt.before_revision, revision(original.as_bytes()));
            assert_eq!(receipt.after_revision, Some(revision(&repaired)));
        }
        assert_ne!(user_receipt.id, workspace_receipt.id);
        let directory = f.archive_root.join("configuration-backups");
        let mut copies: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect();
        copies.sort();
        let mut originals = vec![user, workspace_text];
        originals.sort();
        assert_eq!(copies, originals);
        let mode = std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        for selected in &f.workspaces {
            assert!(
                files_containing(selected, b"SECRET_SENTINEL_464").is_empty(),
                "{}",
                selected.display()
            );
        }
        // The User commit was captured once for the live Session in A; B's
        // Workspace commit is not one of its sources.
        let applied = super::configuration::settled(&f, 0).await;
        assert_ne!(
            applied.desired.input_revision,
            baseline.desired.input_revision
        );
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            before + 1
        );
        drop(runtime);
        f.close().await;
    })
    .await;
}

/// ISO-04: a Session request whose Workspace does not resolve selects none,
/// so its boundary repairs nothing — never the User alone — before the
/// resolution rejects it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unresolvable_session_workspace_repairs_no_source() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let user = edit_user(&f, "SECRET_SENTINEL_464");
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let missing = f.workspaces[0].with_file_name("missing");
        let Response::Failure(_) = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(1),
                call: Method::SessionCreate {
                    submission: None,
                    settings: SessionPersistentState::from_input(&SessionConfigInput::new(missing)),
                },
            })
            .await
        else {
            panic!("Session admission fails")
        };
        assert_eq!(std::fs::read_to_string(user_path(&f)).unwrap(), user);
        assert!(!f.archive_root.join("configuration-backups").exists());
        assert!(
            f.manager
                .source_repairs()
                .latest(&SourceTarget::User)
                .is_none()
        );
        // A Workspace that does not resolve is never admitted.
        assert_eq!(empty_levels(&connection), [key(&SourceTarget::User)].into());
    })
    .await;
}

/// SIZE-08 at the real native Session creation boundary: a User repair whose
/// candidate would outgrow the authored file bound is unresolved before any
/// backup, so nothing is published, nothing reaches the configuration
/// coordinator, and the live Session prepares nothing — although the edited
/// User source carries instructions a committed repair would prepare.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repair_refused_by_the_size_bound_reaches_no_application() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let runtime = f.load(0).await.unwrap().unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        call(
            &connection,
            1,
            Method::ConfigurationReconcile {
                target: workspace(&f, 0),
            },
        )
        .await;
        let probe = f.manager.probe(&f.id(0).await);
        let baseline = super::configuration::settled(&f, 0).await;
        let before = probe.configuration_preparations.load(Ordering::SeqCst);
        // An exactly at-bound User source whose only eligible repair,
        // `schema_version = 9` → `10`, adds one byte.
        let mut document: toml_edit::DocumentMut = std::fs::read_to_string(user_path(&f))
            .unwrap()
            .parse()
            .unwrap();
        document["schema_version"] = toml_edit::value(9);
        document["agent"]["instructions"] = toml_edit::value("User instructions");
        let prefix = document.to_string();
        let bound = usize::try_from(crate::bounded_file::MAX_RESOURCE_BYTES).unwrap();
        let user = format!("{prefix}#{}\n", "x".repeat(bound - prefix.len() - 2));
        assert_eq!(user.len(), bound);
        std::fs::write(user_path(&f), &user).unwrap();
        // Session creation may or may not admit the unrepaired source; either
        // way the repair boundary has run.
        let _ = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(2),
                call: Method::SessionCreate {
                    submission: None,
                    settings: SessionPersistentState::from_input(&SessionConfigInput::new(
                        f.workspaces[1].clone(),
                    )),
                },
            })
            .await;
        assert_eq!(std::fs::read_to_string(user_path(&f)).unwrap(), user);
        assert!(!f.archive_root.join("configuration-backups").exists());
        let receipt = f
            .manager
            .source_repairs()
            .latest(&SourceTarget::User)
            .unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
        assert_eq!(
            receipt.changes,
            [ConfigRepairChange::UpdatedSchemaVersion { from: 9, to: 10 }]
        );
        assert!(!receipt.backup_available);
        assert!(receipt.after_revision.is_none());
        assert!(
            receipt
                .diagnostic
                .as_deref()
                .is_some_and(|diagnostic| diagnostic.contains("authored file size limit"))
        );
        assert_eq!(
            super::configuration::settled(&f, 0).await.desired,
            baseline.desired
        );
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            before
        );
        drop(runtime);
        f.close().await;
    })
    .await;
}

/// Every repair notice this connection owes now, in delivery order.
fn owed(connection: &AppServerConnection) -> Vec<(SourceTarget, Option<ConfigRepairReceipt>, u64)> {
    std::iter::from_fn(|| connection.next_source_repair())
        .map(|notice| {
            let NotificationMethod::SourceRepairChanged {
                target,
                receipt,
                sequence,
            } = notice
            else {
                panic!("a source repair notice")
            };
            (target, receipt, sequence)
        })
        .collect()
}

/// `count` further Workspaces beside the fixture's, named `{prefix}-{index}`,
/// each holding `document`.
fn more_workspaces(f: &Fixture, prefix: &str, count: usize, document: &str) -> Vec<SourceTarget> {
    let root = f.workspaces[0].parent().unwrap();
    (0..count)
        .map(|index| {
            let directory = root.join(format!("{prefix}-{index}"));
            std::fs::create_dir(&directory).unwrap();
            std::fs::write(directory.join("rustx.toml"), document).unwrap();
            SourceTarget::Workspace {
                directory: std::fs::canonicalize(&directory).unwrap(),
            }
        })
        .collect()
}

/// Record an unresolved receipt for `target` by observing it malformed.
fn record_unresolved(f: &Fixture, target: &SourceTarget) {
    std::fs::write(
        target.workspace().unwrap().join("rustx.toml"),
        "[agent\nlegacy = 1\n",
    )
    .unwrap();
    let configuration = &f.manager.configuration;
    assert_eq!(
        configuration.repair_source(target, &configuration.admit_repair_scope(target).unwrap()),
        crate::local_runtime::configuration::repair::RepairOutcome::Unchanged
    );
    assert_eq!(
        f.manager.source_repairs().latest(target).unwrap().status,
        ConfigRepairStatus::Unresolved
    );
}

/// Commit an automatic repair of `target`.
fn commit_repair(f: &Fixture, target: &SourceTarget) -> ConfigRepairReceipt {
    std::fs::write(
        target.workspace().unwrap().join("rustx.toml"),
        "[agent]\nfuture_trust = true\n",
    )
    .unwrap();
    let configuration = &f.manager.configuration;
    assert_eq!(
        configuration.repair_source(target, &configuration.admit_repair_scope(target).unwrap()),
        crate::local_runtime::configuration::repair::RepairOutcome::Committed
    );
    f.manager.source_repairs().latest(target).unwrap()
}

/// Withdraw `target`'s unresolved receipt through the existing manual repair
/// on `connection`; return the sequence from the explicit post-commit read.
async fn withdraw(connection: &AppServerConnection, id: i64, target: &SourceTarget) -> u64 {
    let read = sources(connection, id, target).await;
    let MethodResult::SourcesWritten { revision, .. } = call(
        connection,
        id + 1,
        Method::SourcesWrite {
            target: target.clone(),
            expected_revision: read.workspace.unwrap().revision,
            mutation: SourceMutation::RepairConfig {
                document: "[environment]\nRESIDENCY_SESSION = \"B\"\n".into(),
            },
        },
    )
    .await
    else {
        panic!("manual repair")
    };
    let observed = sources(connection, id + 2, target).await;
    assert_eq!(observed.workspace.as_ref().unwrap().revision, revision);
    assert!(observed.workspace_repair.is_none());
    observed.repair_sequence
}

fn sequence(f: &Fixture) -> u64 {
    *f.manager.source_repairs().subscribe().borrow()
}

/// NATIVE-LEVEL-01: admitting a healthy Workspace owes its empty level once,
/// at the ledger sequence it was read at; addressing it again owes nothing.
/// The level is a state, not a repair: nothing is recorded, written, backed
/// up, prepared, journaled or requested. No notice precedes `initialize`, and
/// a Workspace this connection never addressed — even one the ledger holds a
/// receipt for — never reaches it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_healthy_admission_owes_its_empty_level_exactly_once() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let target = workspace(&f, 0);
        let runtime = f.load(0).await.unwrap().unwrap();
        let live = runtime.inspect_runtime().unwrap();
        let journal = events(&live).len();
        let requests = f.provider.attempt_count();
        // Another Workspace holds a receipt this connection never addresses.
        let [unaddressed] = more_workspaces(&f, "unaddressed", 1, "")
            .try_into()
            .unwrap();
        record_unresolved(&f, &unaddressed);
        let current = sequence(&f);
        assert!(current > 0);
        let connection = AppServerConnection::new(f.host.clone());
        assert!(owed(&connection).is_empty(), "nothing before initialize");
        initialize(&connection).await;
        assert_eq!(owed(&connection), [(SourceTarget::User, None, current)]);
        let probe = f.manager.probe(&f.id(0).await);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let text = std::fs::read_to_string(f.workspaces[0].join("rustx.toml")).unwrap();
        for id in 1..4 {
            sources(&connection, id, &target).await;
            if id == 1 {
                assert_eq!(owed(&connection), [(target.clone(), None, current)]);
            }
            assert!(
                owed(&connection).is_empty(),
                "an admitted source is owed once"
            );
        }
        assert_eq!(sequence(&f), current, "a level records nothing");
        assert_eq!(
            std::fs::read_to_string(f.workspaces[0].join("rustx.toml")).unwrap(),
            text
        );
        assert!(!f.archive_root.join("configuration-backups").exists());
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        assert_eq!(events(&live).len(), journal);
        assert_eq!(f.provider.attempt_count(), requests);
        drop(live);
        drop(runtime);
        f.close().await;
    })
    .await;
}

/// Connection A holds W0's unresolved receipt, then addresses 64 other
/// Workspaces, so its interest forgets W0 while a client of A may still
/// present the receipt (a client orders by observation, not addressing).
/// Another connection withdraws the receipt; A, no longer interested, hears
/// nothing. Addressing W0 again is a new admission, and its level is the
/// withdrawal — and, when `ledger_forgets`, the ledger has meanwhile
/// forgotten W0 entirely.
async fn readmission_after_interest_eviction(ledger_forgets: bool) {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let w0 = workspace(&f, 1);
        let a = AppServerConnection::new(f.host.clone());
        let other = AppServerConnection::new(f.host.clone());
        initialize(&a).await;
        initialize(&other).await;
        owed(&a);
        record_unresolved(&f, &w0);
        sources(&a, 1, &w0).await;
        let kept = match owed(&a).as_slice() {
            [(announced, Some(receipt), kept)] => {
                assert_eq!(*announced, w0);
                assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
                *kept
            }
            owed => panic!("W0's receipt: {owed:?}"),
        };
        // 64 more Workspaces evict W0 from A's interest.
        let more = more_workspaces(&f, "more", 64, "");
        for (index, target) in (2..).zip(&more) {
            sources(&a, index, target).await;
        }
        for (target, receipt, _) in owed(&a) {
            assert!(more.contains(&target) && receipt.is_none());
        }
        let withdrawn = withdraw(&other, 1, &w0).await;
        assert!(withdrawn > kept);
        assert!(f.manager.source_repairs().holds(&w0));
        if ledger_forgets {
            // 64 newer Workspace entries make the ledger forget W0.
            for target in &more {
                record_unresolved(&f, target);
            }
            assert!(!f.manager.source_repairs().holds(&w0));
        }
        // A is not interested in W0 any more: nothing of it reaches A.
        for (target, _, _) in owed(&a) {
            assert_ne!(target, w0);
        }
        let current = sequence(&f);
        // Re-admission: W0's current level is the withdrawal, at a
        // sequence that orders it after every receipt A ever saw of W0.
        sources(&a, 100, &w0).await;
        assert_eq!(owed(&a), [(w0.clone(), None, current)], "{ledger_forgets}");
        assert!(current >= withdrawn);
        sources(&a, 101, &w0).await;
        assert!(owed(&a).is_empty());
        f.close().await;
    })
    .await;
}

/// NATIVE-LEVEL-02: re-admission delivers a withdrawal the ledger retains.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_readmitted_workspace_delivers_its_retained_withdrawal() {
    readmission_after_interest_eviction(false).await;
}

/// NATIVE-LEVEL-03: re-admission delivers an empty level for a Workspace the
/// ledger has forgotten: a missing entry is no retained receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_readmitted_workspace_forgotten_by_the_ledger_delivers_an_empty_level() {
    readmission_after_interest_eviction(true).await;
}

/// NATIVE-LEVEL-04: with the ledger at sequence zero, the User's and a
/// Workspace's empty levels are delivered once each at zero — never again —
/// and the first repair recorded afterwards is delivered at its own, later
/// sequence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn empty_levels_at_sequence_zero_are_delivered_once() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let target = workspace(&f, 0);
        assert_eq!(sequence(&f), 0);
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        sources(&connection, 1, &target).await;
        assert_eq!(
            owed(&connection),
            [(SourceTarget::User, None, 0), (target.clone(), None, 0)]
        );
        for id in 2..4 {
            sources(&connection, id, &target).await;
            assert!(owed(&connection).is_empty());
        }
        let receipt = commit_repair(&f, &target);
        assert_eq!(owed(&connection), [(target.clone(), Some(receipt), 1)]);
        assert!(owed(&connection).is_empty());
        f.close().await;
    })
    .await;
}

/// NATIVE-LEVEL-05: a ledger change interleaved with a new admission is never
/// lost and never delivered stale.
///
/// 1. Admission, then a commit, then delivery: the one level delivered is the
///    commit, never an empty level before it.
/// 2. Admission, delivery of the empty level, then a commit: the commit
///    follows at a higher sequence.
/// 3. A parked reader, woken by the admission, then a withdrawal committed
///    before it runs: it delivers the latest level once, and nothing after.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_change_racing_a_new_admission_is_neither_lost_nor_stale() {
    use std::future::Future;
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let [first, second, third] = more_workspaces(&f, "race", 3, "").try_into().unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        owed(&connection);
        // 1. Admitted, changed, then delivered: one coalesced level.
        sources(&connection, 1, &first).await;
        let receipt = commit_repair(&f, &first);
        assert_eq!(
            owed(&connection),
            [(first.clone(), Some(receipt), sequence(&f))]
        );
        // 2. Delivered empty, then changed: the change follows.
        sources(&connection, 2, &second).await;
        let level = sequence(&f);
        assert_eq!(owed(&connection), [(second.clone(), None, level)]);
        let receipt = commit_repair(&f, &second);
        assert_eq!(
            owed(&connection),
            [(second.clone(), Some(receipt), sequence(&f))]
        );
        assert!(sequence(&f) > level);
        // 3. A parked reader is woken by the admission; before it runs, the
        //    source's receipt is recorded and then withdrawn.
        record_unresolved(&f, &third);
        let mut next = park(&connection, &third);
        let flag = std::sync::Arc::new(WakeFlag(std::sync::atomic::AtomicBool::new(false)));
        let waker = std::task::Waker::from(flag.clone());
        let mut context = std::task::Context::from_waker(&waker);
        assert!(next.as_mut().poll(&mut context).is_pending());
        sources(&connection, 3, &third).await;
        assert!(flag.0.load(Ordering::SeqCst), "admission wakes the reader");
        let other = AppServerConnection::new(f.host.clone());
        initialize(&other).await;
        withdraw(&other, 1, &third).await;
        let std::task::Poll::Ready(notification) = next.as_mut().poll(&mut context) else {
            panic!("the admitted level is deliverable")
        };
        drop(next);
        // The withdrawal's application notice is another owner's and may come
        // first; of the source, only its latest level is delivered.
        let mut levels = Vec::new();
        match notification.notification {
            NotificationMethod::SourceRepairChanged {
                target,
                receipt,
                sequence,
            } => levels.push((target, receipt, sequence)),
            NotificationMethod::ConfigurationChanged { .. } => {}
            other => panic!("{other:?}"),
        }
        levels.extend(owed(&connection));
        assert_eq!(levels, [(third.clone(), None, sequence(&f))]);
        assert!(owed(&connection).is_empty(), "nothing older follows");
        f.close().await;
    })
    .await;
}

/// NATIVE-LEVEL-06: a connection's interest is the User and its 64 most
/// recently addressed Workspaces. Re-addressing an admitted Workspace only
/// refreshes it; addressing a forgotten one is a new admission with a new
/// level; the User is never forgotten. Another connection's interest is its
/// own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repair_interest_is_bounded_and_readmission_is_distinct_from_access() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let more = more_workspaces(&f, "bound", 65, "");
        let connection = AppServerConnection::new(f.host.clone());
        let bystander = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        initialize(&bystander).await;
        let mut id = 0;
        let mut address = async |target: &SourceTarget| {
            id += 1;
            sources(&connection, id, target).await;
            owed(&connection)
                .into_iter()
                .map(|(target, receipt, _)| {
                    assert!(receipt.is_none());
                    target
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            address(&more[0]).await,
            [SourceTarget::User, more[0].clone()]
        );
        for target in &more[1..] {
            assert_eq!(address(target).await, std::slice::from_ref(target));
        }
        // 65 addressed: the first was forgotten, the other 64 are admitted.
        for target in &more[1..] {
            assert!(address(target).await.is_empty(), "access is not admission");
        }
        assert_eq!(address(&more[0]).await, [more[0].clone()], "re-admission");
        // That re-admission forgot the least recently addressed, more[1].
        assert_eq!(address(&more[1]).await, [more[1].clone()]);
        // The User has its own slot, never forgotten.
        prepend(&user_path(&f), "legacy_user = true\n");
        let configuration = &f.manager.configuration;
        assert_eq!(
            configuration.repair_source(
                &SourceTarget::User,
                &configuration
                    .admit_repair_scope(&SourceTarget::User)
                    .unwrap()
            ),
            crate::local_runtime::configuration::repair::RepairOutcome::Committed
        );
        let [(SourceTarget::User, Some(_), _)] = owed(&connection).as_slice() else {
            panic!("the User repair")
        };
        // The bystander observes the User and no Workspace.
        let seen = owed(&bystander);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, SourceTarget::User);
        f.close().await;
    })
    .await;
}

/// Connection `b` reconciles 64 further Workspaces, each holding a malformed
/// document, so the native reconcile boundary records an unresolved receipt
/// for each: 64 Workspace entries newer than any before them, which the
/// finite ledger keeps in place of every older Workspace entry.
async fn evict_through(
    f: &Fixture,
    b: &AppServerConnection,
    prefix: &str,
    first_id: i64,
) -> Vec<SourceTarget> {
    let more = more_workspaces(f, prefix, 64, "[agent\nlegacy = 1\n");
    for (id, target) in (first_id..).zip(&more) {
        call(
            b,
            id,
            Method::ConfigurationReconcile {
                target: target.clone(),
            },
        )
        .await;
        assert_eq!(
            f.manager.source_repairs().latest(target).unwrap().status,
            ConfigRepairStatus::Unresolved
        );
    }
    more
}

/// Connection A admits W0, a Workspace with an unresolved receipt R0, and is
/// delivered R0; connection B is another client of the same App Server.
/// Returns A, B, W0, R0 and the sequence R0 was delivered at.
async fn delivered_unresolved(
    f: &Fixture,
    w0: &SourceTarget,
) -> (
    AppServerConnection,
    AppServerConnection,
    ConfigRepairReceipt,
    u64,
) {
    let a = AppServerConnection::new(f.host.clone());
    let b = AppServerConnection::new(f.host.clone());
    initialize(&a).await;
    initialize(&b).await;
    owed(&a);
    owed(&b);
    record_unresolved(f, w0);
    sources(&a, 1, w0).await;
    match owed(&a).as_slice() {
        [(announced, Some(receipt), delivered)] => {
            assert_eq!(announced, w0);
            assert_eq!(receipt.status, ConfigRepairStatus::Unresolved);
            (a, b, receipt.clone(), *delivered)
        }
        owed => panic!("R0: {owed:?}"),
    }
}

/// LEDGER-EVICT-01: A keeps observing W0 — it addresses nothing else — while
/// B's 64 newer receipts make the finite ledger forget W0's entry. Without
/// any request of A, A is owed W0's absence once, at a sequence newer than
/// R0. Absence is not health: W0's document is still as malformed as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retained_source_the_ledger_forgets_delivers_its_absence_once() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let w0 = workspace(&f, 1);
        let (a, b, _, delivered) = delivered_unresolved(&f, &w0).await;
        evict_through(&f, &b, "evict", 1).await;
        assert!(
            !f.manager.source_repairs().holds(&w0),
            "the ledger forgot W0"
        );
        assert!(a.observes_repairs_of(&w0), "A still observes W0");
        let forgotten = sequence(&f);
        assert!(forgotten > delivered);
        assert_eq!(owed(&a), [(w0.clone(), None, forgotten)]);
        assert!(owed(&a).is_empty(), "absence is delivered once");
        assert!(a.observes_repairs_of(&w0));
        // The empty level reports the ledger, not the document.
        let read = sources(&b, 100, &w0).await;
        assert!(read.workspace_repair.is_none());
        assert_eq!(read.repair_sequence, forgotten);
        assert!(
            read.workspace.unwrap().diagnostic.is_some(),
            "still malformed"
        );
        f.close().await;
    })
    .await;
}

/// LEDGER-EVICT-02: after A was delivered W0's absence, B repairs W0 by the
/// manual source write. W0 is then valid by its authoritative read; A, which
/// re-addresses W0 without a new admission, is owed nothing — R0 never
/// returns — and every sequence A can now read orders after R0, so a delayed
/// R0 notice is older than anything A holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manual_repair_after_ledger_eviction_never_restores_the_forgotten_receipt() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let w0 = workspace(&f, 1);
        let (a, b, r0, delivered) = delivered_unresolved(&f, &w0).await;
        evict_through(&f, &b, "evict", 1).await;
        let forgotten = sequence(&f);
        assert_eq!(owed(&a), [(w0.clone(), None, forgotten)]);
        // The manual repair is a source write; with no unresolved receipt
        // retained, it withdraws nothing and records nothing.
        let written = withdraw(&b, 100, &w0).await;
        assert_eq!(written, forgotten);
        let read = sources(&b, 102, &w0).await;
        let view = read.workspace.unwrap();
        assert!(view.diagnostic.is_none() && view.authored.is_some());
        assert!(read.prospective_diagnostic.is_none(), "W0 is valid now");
        assert!(owed(&a).is_empty());
        // Re-addressing is access, not admission: nothing is owed.
        let reread = sources(&a, 2, &w0).await;
        assert!(a.observes_repairs_of(&w0));
        assert!(owed(&a).is_empty());
        assert!(reread.workspace_repair.is_none());
        assert_eq!(reread.repair_sequence, forgotten);
        assert!(
            delivered < forgotten,
            "R0 orders before every level A holds"
        );
        assert_ne!(reread.workspace_repair.as_ref(), Some(&r0));
        f.close().await;
    })
    .await;
}

/// LEDGER-EVICT-03: W0's entry is forgotten and, before A runs, W0 records a
/// distinct receipt R1. A is delivered R1 at its own sequence — no transient
/// absence — and nothing of R0 or the eviction follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_receipt_recorded_after_eviction_supersedes_the_forgotten_one() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let w0 = workspace(&f, 1);
        let (a, b, r0, delivered) = delivered_unresolved(&f, &w0).await;
        evict_through(&f, &b, "evict", 1).await;
        let forgotten = sequence(&f);
        assert!(!f.manager.source_repairs().holds(&w0));
        let r1 = commit_repair(&f, &w0);
        let recorded = sequence(&f);
        assert_ne!(r1.id, r0.id);
        assert!(delivered < forgotten && forgotten < recorded);
        assert_eq!(owed(&a), [(w0.clone(), Some(r1), recorded)]);
        assert!(owed(&a).is_empty());
        f.close().await;
    })
    .await;
}

/// LEDGER-EVICT-04: once A was delivered W0's absence, further receipts of
/// other Workspaces — which advance the ledger and forget more entries —
/// owe A nothing of W0, until W0's own state changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivered_absence_is_not_repeated() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let w0 = workspace(&f, 1);
        let (a, b, _, _) = delivered_unresolved(&f, &w0).await;
        let evicting = evict_through(&f, &b, "evict", 1).await;
        let forgotten = sequence(&f);
        assert_eq!(owed(&a), [(w0.clone(), None, forgotten)]);
        for target in more_workspaces(&f, "unrelated", 8, "") {
            commit_repair(&f, &target);
            assert!(owed(&a).is_empty(), "nothing of W0");
        }
        for target in &evicting[..4] {
            commit_repair(&f, target);
            assert!(owed(&a).is_empty(), "nothing of W0");
        }
        assert!(sequence(&f) > forgotten);
        record_unresolved(&f, &w0);
        let current = f.manager.source_repairs().latest(&w0).unwrap();
        assert_eq!(owed(&a), [(w0.clone(), Some(current), sequence(&f))]);
        f.close().await;
    })
    .await;
}

/// LEDGER-EVICT-05: a healthy W0 the ledger never recorded is delivered its
/// empty level once, at sequence zero; 64 and more receipts of other
/// Workspaces afterwards owe A nothing of W0 or the User, and A still
/// observes W0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_never_recorded_source_is_not_repeated() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let w0 = workspace(&f, 1);
        assert_eq!(sequence(&f), 0);
        let a = AppServerConnection::new(f.host.clone());
        let b = AppServerConnection::new(f.host.clone());
        initialize(&a).await;
        initialize(&b).await;
        sources(&a, 1, &w0).await;
        assert_eq!(
            owed(&a),
            [(SourceTarget::User, None, 0), (w0.clone(), None, 0)]
        );
        evict_through(&f, &b, "evict", 1).await;
        for target in more_workspaces(&f, "unrelated", 8, "") {
            commit_repair(&f, &target);
        }
        assert!(sequence(&f) > 64);
        assert!(owed(&a).is_empty(), "no empty level is repeated");
        assert!(a.observes_repairs_of(&w0));
        assert!(!f.manager.source_repairs().holds(&w0));
        f.close().await;
    })
    .await;
}

/// LEDGER-EVICT-06: two connections are delivered R0. `kept` keeps observing
/// W0; `moved` addresses 64 other Workspaces, so its interest forgets W0.
/// The ledger then forgets W0 too. `kept` converges by its retained cursor,
/// with no request; `moved` hears nothing until it addresses W0 again, and
/// that new admission delivers the current level.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_and_readmitted_sources_converge_through_their_own_contracts() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let w0 = workspace(&f, 1);
        let (kept, b, r0, _) = delivered_unresolved(&f, &w0).await;
        let moved = AppServerConnection::new(f.host.clone());
        initialize(&moved).await;
        sources(&moved, 1, &w0).await;
        assert_eq!(
            owed(&moved),
            [
                (SourceTarget::User, None, sequence(&f)),
                (w0.clone(), Some(r0.clone()), sequence(&f))
            ]
        );
        for (id, target) in (2..).zip(&more_workspaces(&f, "moved", 64, "")) {
            sources(&moved, id, target).await;
        }
        owed(&moved);
        assert!(kept.observes_repairs_of(&w0));
        assert!(
            !moved.observes_repairs_of(&w0),
            "moved's interest forgot W0"
        );
        evict_through(&f, &b, "evict", 1).await;
        assert!(!f.manager.source_repairs().holds(&w0));
        let forgotten = sequence(&f);
        // Retained interest: the absence, with no request of `kept`.
        assert_eq!(owed(&kept), [(w0.clone(), None, forgotten)]);
        assert!(kept.observes_repairs_of(&w0));
        // Forgotten interest: nothing until re-admission, then the level.
        assert!(owed(&moved).iter().all(|(target, _, _)| *target != w0));
        sources(&moved, 100, &w0).await;
        assert!(moved.observes_repairs_of(&w0));
        assert_eq!(owed(&moved), [(w0.clone(), None, forgotten)]);
        assert!(owed(&kept).is_empty() && owed(&moved).is_empty());
        f.close().await;
    })
    .await;
}

/// LEDGER-EVICT-07: a parked reader of A, holding R0, is woken by the
/// recording that forgets W0.
///
/// 1. It runs before W0 records again: it delivers the absence; a later
///    receipt R1 wakes it again and follows at a higher sequence.
/// 2. W0 records R2 before it runs: it delivers R2 directly, never the
///    absence or anything older.
///
/// Each step is forced by polling the reader by hand. `record` takes only the
/// ledger lock and the reader takes the interest lock, then the ledger lock,
/// so the two never wait on each other in reverse.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_parked_across_ledger_eviction_never_loses_the_latest_level() {
    use std::future::Future;
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let w0 = workspace(&f, 1);
        let (a, b, _, delivered) = delivered_unresolved(&f, &w0).await;
        let poll_after = |record: &dyn Fn()| {
            let mut next = park(&a, &SourceTarget::User);
            let flag = std::sync::Arc::new(WakeFlag(std::sync::atomic::AtomicBool::new(false)));
            let waker = std::task::Waker::from(flag.clone());
            let mut context = std::task::Context::from_waker(&waker);
            assert!(next.as_mut().poll(&mut context).is_pending());
            record();
            assert!(flag.0.load(Ordering::SeqCst), "the ledger wakes the reader");
            let std::task::Poll::Ready(notification) = next.as_mut().poll(&mut context) else {
                panic!("a level is deliverable")
            };
            let NotificationMethod::SourceRepairChanged {
                target,
                receipt,
                sequence,
            } = notification.notification
            else {
                panic!("a source repair notice")
            };
            (target, receipt, sequence)
        };
        // 1. The forgetting recording, then the reader, then R1.
        let evicting = more_workspaces(&f, "evict", 64, "[agent\nlegacy = 1\n");
        let (last, first) = evicting.split_last().unwrap();
        for target in first {
            record_unresolved(&f, target);
        }
        assert!(f.manager.source_repairs().holds(&w0));
        let absence = poll_after(&|| record_unresolved(&f, last));
        assert!(!f.manager.source_repairs().holds(&w0));
        let forgotten = sequence(&f);
        assert!(forgotten > delivered);
        assert_eq!(absence, (w0.clone(), None, forgotten));
        let r1 = poll_after(&|| {
            commit_repair(&f, &w0);
        });
        assert_eq!(r1.0, w0);
        assert!(r1.1.is_some() && r1.2 > forgotten);
        assert!(owed(&a).is_empty());
        // 2. The forgetting recording and R2 both before the reader runs.
        let more = more_workspaces(&f, "again", 64, "[agent\nlegacy = 1\n");
        let (last, first) = more.split_last().unwrap();
        for target in first {
            record_unresolved(&f, target);
        }
        assert!(f.manager.source_repairs().holds(&w0));
        let r2 = poll_after(&|| {
            record_unresolved(&f, last);
            assert!(!f.manager.source_repairs().holds(&w0));
            record_unresolved(&f, &w0);
        });
        let latest = f.manager.source_repairs().latest(&w0).unwrap();
        assert_eq!(r2, (w0.clone(), Some(latest), sequence(&f)));
        assert!(owed(&a).is_empty(), "nothing older follows");
        drop(b);
        f.close().await;
    })
    .await;
}

/// LEDGER-EVICT-09: ledger eviction and its convergence touch nothing but
/// observations: no provider request, Tool call, canonical message or Event
/// Journal entry, no source write or backup, no prepared application, and
/// the Session's settled configuration is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ledger_eviction_convergence_is_isolated_from_runtime_and_sources() {
    bounded(async {
        let f = Fixture::new().await;
        let runtime = f.load(0).await.unwrap().unwrap();
        let live = runtime.inspect_runtime().unwrap();
        let w0 = workspace(&f, 1);
        let (a, b, _, _) = delivered_unresolved(&f, &w0).await;
        // A healthy reconcile establishes the Session's application baseline.
        call(
            &b,
            200,
            Method::ConfigurationReconcile {
                target: workspace(&f, 0),
            },
        )
        .await;
        owed(&a);
        let journal = events(&live).len();
        let requests = f.provider.attempt_count();
        let probe = f.manager.probe(&f.id(0).await);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let settled = super::configuration::settled(&f, 0).await;
        let w0_file = w0.workspace().unwrap().join("rustx.toml");
        let bytes = std::fs::read(&w0_file).unwrap();
        let user_bytes = std::fs::read(user_path(&f)).unwrap();
        evict_through(&f, &b, "evict", 1).await;
        assert_eq!(owed(&a), [(w0.clone(), None, sequence(&f))]);
        assert_eq!(events(&live).len(), journal);
        assert_eq!(f.provider.attempt_count(), requests);
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        assert_eq!(
            super::configuration::settled(&f, 0).await.desired,
            settled.desired
        );
        assert_eq!(std::fs::read(&w0_file).unwrap(), bytes);
        assert_eq!(std::fs::read(user_path(&f)).unwrap(), user_bytes);
        assert!(!f.archive_root.join("configuration-backups").exists());
        drop(live);
        drop(runtime);
        f.close().await;
    })
    .await;
}

/// Reconcile workspace 0 and return its announced receipt and sequence.
async fn reconcile_announced(
    f: &Fixture,
    connection: &AppServerConnection,
    id: i64,
) -> (ConfigRepairReceipt, u64) {
    let target = workspace(f, 0);
    let MethodResult::ConfigurationReceipt { .. } = call(
        connection,
        id,
        Method::ConfigurationReconcile {
            target: target.clone(),
        },
    )
    .await
    else {
        panic!("reconciled")
    };
    let (receipt, sequence) = sequenced_repair_notice(connection, &target).await;
    (receipt.unwrap(), sequence)
}

/// OCC-INT (INV-02): A (repairable) → reconcile commits B (R1); an external
/// edit restores exactly A → reconcile commits B again (R2). R2 is announced
/// as a distinct occurrence after R1 at the same revision B; rereading
/// either never repeats it; and the second commit of bytes the coordinator
/// already applied prepares nothing more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restored_original_repaired_again_is_announced_as_a_new_occurrence() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let target = workspace(&f, 0);
        let _runtime = f.load(0).await.unwrap().unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        call(
            &connection,
            100,
            Method::ConfigurationReconcile {
                target: target.clone(),
            },
        )
        .await;
        let probe = f.manager.probe(&f.id(0).await);
        let baseline = super::configuration::settled(&f, 0).await;
        let workspace_file = f.workspaces[0].join("rustx.toml");
        let original = format!(
            "{}[agent]\ninstructions = \"Occurrence instructions\"\nfuture_trust = true\n",
            std::fs::read_to_string(&workspace_file).unwrap()
        );
        std::fs::write(&workspace_file, &original).unwrap();
        let before = probe.configuration_preparations.load(Ordering::SeqCst);
        let (first, first_sequence) = reconcile_announced(&f, &connection, 1).await;
        let repaired = std::fs::read_to_string(&workspace_file).unwrap();
        let applied = super::configuration::settled(&f, 0).await;
        assert_ne!(
            applied.desired.input_revision,
            baseline.desired.input_revision
        );
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            before + 1
        );
        // Reading R1 again and again repeats nothing.
        for id in 2..5 {
            let read = sources(&connection, id, &target).await;
            assert_eq!(read.workspace_repair.as_ref(), Some(&first));
            assert_eq!(read.repair_sequence, first_sequence);
        }
        assert!(connection.next_source_repair().is_none());

        std::fs::write(&workspace_file, &original).unwrap();
        let (second, second_sequence) = reconcile_announced(&f, &connection, 5).await;
        assert_ne!(second.id, first.id);
        assert!(second_sequence > first_sequence);
        assert_eq!(second.status, ConfigRepairStatus::Repaired);
        assert_eq!(second.after_revision, first.after_revision);
        assert_eq!(std::fs::read_to_string(&workspace_file).unwrap(), repaired);
        let read = sources(&connection, 6, &target).await;
        assert_eq!(read.workspace_repair.as_ref(), Some(&second));
        assert_eq!(read.repair_sequence, second_sequence);
        // B is what the coordinator already applied: no further attempt.
        let settled = super::configuration::settled(&f, 0).await;
        assert_eq!(settled.desired, applied.desired);
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            before + 1
        );
        // A fresh connection to the same authority is told the current
        // occurrence once, never R1.
        let fresh = AppServerConnection::new(f.host.clone());
        initialize(&fresh).await;
        sources(&fresh, 1, &target).await;
        let mut announced = Vec::new();
        while let Some(NotificationMethod::SourceRepairChanged {
            target: noticed,
            receipt,
            ..
        }) = fresh.next_source_repair()
        {
            if noticed == target {
                announced.push(receipt.unwrap().id);
            }
        }
        assert_eq!(announced, std::slice::from_ref(&second.id));
        f.close().await;
    })
    .await;
}

/// REDACT-INT (INV-01): the notification and the authoritative read carry
/// one receipt, and neither carries an authored identity, unknown key or
/// unsupported Tool name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notices_and_reads_carry_the_same_redacted_receipt() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let target = workspace(&f, 0);
        let _runtime = f.load(0).await.unwrap().unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let workspace_file = f.workspaces[0].join("rustx.toml");
        std::fs::write(
            &workspace_file,
            format!(
                "zsecretz_root = \"zsecretz-value\"\n{}[agent]\nzsecretz_agent_key = 1\n[agent.tools]\nbuiltin = [\"read\", \"zsecretz_tool\"]\n[mcp_tool_policies.\"zsecretz-mcp\"]\napproval = \"always\"\ntrust_all = \"zsecretz-value\"\n",
                std::fs::read_to_string(&workspace_file).unwrap()
            ),
        )
        .unwrap();
        let MethodResult::ConfigurationReceipt { .. } = call(
            &connection,
            1,
            Method::ConfigurationReconcile {
                target: target.clone(),
            },
        )
        .await
        else {
            panic!("reconciled")
        };
        let notice = loop {
            let notification = connection.next_notification().await.notification;
            if let NotificationMethod::SourceRepairChanged {
                target: announced,
                receipt,
                ..
            } = &notification
            {
                if *announced == target {
                    break notification;
                }
                assert!(receipt.is_none(), "{receipt:?}");
            }
        };
        let serialized = serde_json::to_string(&notice).unwrap();
        assert!(!serialized.to_ascii_lowercase().contains("zsecretz"), "{serialized}");
        let NotificationMethod::SourceRepairChanged { receipt, .. } = notice else {
            unreachable!()
        };
        let receipt = receipt.unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Repaired, "{receipt:?}");
        assert_eq!(
            receipt.changes,
            vec![
                ConfigRepairChange::RemovedField {
                    object_path: String::new()
                },
                ConfigRepairChange::RemovedField {
                    object_path: "agent".into()
                },
                ConfigRepairChange::RemovedField {
                    object_path: "mcp_tool_policies.*".into()
                },
                ConfigRepairChange::RemovedNativeTool {
                    position: 1,
                    name: None,
                    reason: crate::local_runtime::configuration::repair::NativeToolRemoval::Unknown,
                },
            ]
        );
        let read = sources(&connection, 2, &target).await;
        assert_eq!(read.workspace_repair.as_ref(), Some(&receipt));
        let serialized = serde_json::to_string(&read.workspace_repair).unwrap();
        assert!(!serialized.to_ascii_lowercase().contains("zsecretz"), "{serialized}");
        f.close().await;
    })
    .await;
}

/// Every entry below `root`, with the bytes of every regular file.
fn namespace(root: &std::path::Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut entries = std::collections::BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                pending.push(path.clone());
            }
            let bytes = if metadata.is_file() {
                std::fs::read(&path).unwrap()
            } else {
                Vec::new()
            };
            entries.insert(path, bytes);
        }
    }
    entries
}

/// READ-INT (INV-03): `configuration/sourcesRead` — of the User, a Workspace
/// without a document, and a repairable or malformed Workspace document —
/// creates no directory, lock, staged file or backup, repairs nothing and
/// records no receipt, through any path the App Server takes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn source_reads_leave_the_filesystem_unchanged() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let user_directory = user_path(&f).parent().unwrap().to_path_buf();
        let runtime_root = f
            .manager
            .configuration
            .source_bindings()
            .runtime_root
            .clone();
        let roots = [
            user_directory,
            f.workspaces[0].clone(),
            f.workspaces[1].clone(),
        ];
        let observe = || {
            (
                roots.iter().map(|root| namespace(root)).collect::<Vec<_>>(),
                runtime_root.join("configuration-backups").exists(),
            )
        };
        let mut id = 1;
        for document in [
            None,
            Some("[agent]\nfuture_trust = true\n"),
            Some("[agent\nlegacy = 1\n"),
        ] {
            let workspace_file = f.workspaces[1].join("rustx.toml");
            match document {
                None => {
                    let _ = std::fs::remove_file(&workspace_file);
                }
                Some(text) => std::fs::write(&workspace_file, text).unwrap(),
            }
            let before = observe();
            for target in [SourceTarget::User, workspace(&f, 1), workspace(&f, 1)] {
                let read = sources(&connection, id, &target).await;
                id += 1;
                assert!(read.workspace_repair.is_none());
            }
            assert_eq!(observe(), before, "{document:?}");
            assert!(
                f.manager
                    .source_repairs()
                    .latest(&workspace(&f, 1))
                    .is_none()
            );
        }
        f.close().await;
    })
    .await;
}

/// FS-RACE-08: `configuration/sourcesRead` is a physically bound, read-only
/// capture. A document redirected by a symlink at the exact boundary where
/// the reader opens it is refused without its target ever answering; plain
/// reads of a repairable source repair nothing, create nothing, announce
/// nothing and prepare no application; nothing reaches the journal or a
/// provider. Repair stays reconciliation's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn source_reads_are_physically_bound_and_trigger_nothing() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let target = workspace(&f, 0);
        let runtime = f.load(0).await.unwrap().unwrap();
        let live = runtime.inspect_runtime().unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        call(
            &connection,
            100,
            Method::ConfigurationReconcile {
                target: target.clone(),
            },
        )
        .await;
        let baseline = super::configuration::settled(&f, 0).await;
        let probe = f.manager.probe(&f.id(0).await);
        let preparations = probe.configuration_preparations.load(Ordering::SeqCst);
        let journal = events(&live).len();
        let requests = f.provider.attempt_count();
        let workspace_file = std::fs::canonicalize(&f.workspaces[0])
            .unwrap()
            .join("rustx.toml");
        let edited = format!(
            "{}[agent]\nfuture_trust = true\n",
            std::fs::read_to_string(&workspace_file).unwrap()
        );
        std::fs::write(&workspace_file, &edited).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let external = outside.path().join("external.toml");
        std::fs::write(&external, "[subagents]\nmax_concurrent = 7\n").unwrap();
        let entries = |directory: &std::path::Path| {
            let mut names: Vec<_> = std::fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            names.sort();
            names
        };
        let user_directory = user_path(&f).parent().unwrap().to_path_buf();
        let namespace = (
            entries(workspace_file.parent().unwrap()),
            entries(&user_directory),
        );

        // The leaf redirected while the reader opens it: refused, and the
        // redirect's revision answers nothing.
        let (reached, arrival) = std::sync::mpsc::channel();
        let (document, redirect) = (workspace_file.clone(), external.clone());
        crate::local_runtime::settings::reads::before_opening(&workspace_file, move || {
            std::fs::remove_file(&document).unwrap();
            std::os::unix::fs::symlink(&redirect, &document).unwrap();
            reached.send(()).unwrap();
        });
        let response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(1),
                call: Method::SourcesRead {
                    target: target.clone(),
                },
            })
            .await;
        arrival
            .try_recv()
            .expect("the opening boundary was reached");
        assert!(matches!(response, Response::Failure(_)), "{response:?}");
        let external_revision = crate::local_runtime::settings::revision(Some(
            std::fs::read(&external).unwrap().as_slice(),
        ));
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains(&external_revision)
        );
        std::fs::remove_file(&workspace_file).unwrap();
        std::fs::write(&workspace_file, &edited).unwrap();

        // Plain reads of the repairable source are read-only.
        for id in 2..5 {
            let read = sources(&connection, id, &target).await;
            let workspace = read.workspace.unwrap();
            assert_eq!(
                workspace.revision,
                crate::local_runtime::settings::revision(Some(edited.as_bytes()))
            );
            assert!(read.workspace_repair.is_none());
        }
        assert_eq!(std::fs::read_to_string(&workspace_file).unwrap(), edited);
        assert_eq!(
            (
                entries(workspace_file.parent().unwrap()),
                entries(&user_directory)
            ),
            namespace
        );
        assert_eq!(
            empty_levels(&connection),
            [key(&SourceTarget::User), key(&target)].into()
        );
        assert!(f.manager.source_repairs().latest(&target).is_none());
        assert_eq!(
            probe.configuration_preparations.load(Ordering::SeqCst),
            preparations
        );
        assert_eq!(
            super::configuration::settled(&f, 0).await.desired,
            baseline.desired
        );
        assert_eq!(events(&live).len(), journal);
        assert_eq!(f.provider.attempt_count(), requests);
        drop(live);
        f.close().await;
    })
    .await;
}

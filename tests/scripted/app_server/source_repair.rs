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
use crate::local_runtime::configuration::repair::{
    ConfigRepairChange, ConfigRepairReceipt, ConfigRepairStatus,
};
use crate::local_runtime::configuration::settings::{SourceMutation, SourceSettings, SourceTarget};

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
) -> SourceSettings {
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
    *projection
}

/// The next source repair notice, skipping application notices, which are a
/// different owner and scope.
async fn repair_notice(
    connection: &AppServerConnection,
) -> (SourceTarget, Option<ConfigRepairReceipt>) {
    loop {
        match connection.next_notification().await.notification {
            NotificationMethod::SourceRepairChanged {
                target, receipt, ..
            } => {
                return (target, receipt);
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

/// Collect exactly `count` repair notices, keyed by source.
async fn repair_notices(
    connection: &AppServerConnection,
    count: usize,
) -> std::collections::BTreeMap<String, Option<ConfigRepairReceipt>> {
    let mut notices = std::collections::BTreeMap::new();
    while notices.len() < count {
        let (target, receipt) = repair_notice(connection).await;
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
            Method::SessionCreate {
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
                field_path: "legacy_user".into()
            }]
        );
        assert_eq!(repaired.status, ConfigRepairStatus::Repaired);
        assert_eq!(repaired.target, target);
        assert!(creator.next_source_repair().is_none());

        // Another connection observes the User source, but no Workspace it
        // never addressed.
        let (announced, receipt) = repair_notice(&observer).await;
        assert_eq!(announced, SourceTarget::User);
        assert_eq!(receipt.unwrap().id, user.id);
        assert!(observer.next_source_repair().is_none());
        // Addressing the Workspace makes its receipt readable, and announces
        // the same repair exactly once.
        let read = sources(&observer, 2, &target).await;
        assert_eq!(read.workspace_repair.as_ref(), Some(&repaired));
        assert_eq!(read.user_repair.as_ref(), Some(&user));
        let (announced, receipt) = repair_notice(&observer).await;
        assert_eq!(announced, target);
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
        let f = Fixture::new().await;
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
        assert!(connection.next_source_repair().is_none());
        let probe = f.manager.probe(&f.id(0).await);
        let baseline = super::configuration::settled(&f, 0).await;
        let before = probe.configuration_preparations.load(Ordering::SeqCst);
        let MethodResult::ConfigurationApplication { .. } = call(
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
        let (announced, receipt) = repair_notice(&connection).await;
        assert_eq!(announced, target);
        let receipt = receipt.unwrap();
        assert_eq!(receipt.status, ConfigRepairStatus::Repaired);
        assert_eq!(
            receipt.changes,
            vec![ConfigRepairChange::RemovedField {
                field_path: "agent.future_trust".into()
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
        let (announced, receipt) = repair_notice(&connection).await;
        assert_eq!(announced, target);
        let receipt = receipt.unwrap();
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
        let MethodResult::SourceSettings { projection } = call(
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
        assert!(projection.workspace_repair.is_none());
        let (announced, receipt) = repair_notice(&connection).await;
        assert_eq!(announced, target);
        assert!(receipt.is_none());
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
        let f = Fixture::new().await;
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
        assert!(connection.next_source_repair().is_none());
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
            assert!(connection.next_source_repair().is_none(), "{overlap}");
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
        assert!(connection.next_source_repair().is_none());
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

/// The next source repair notice with its native ledger sequence.
async fn sequenced_repair_notice(
    connection: &AppServerConnection,
) -> (SourceTarget, Option<ConfigRepairReceipt>, u64) {
    loop {
        if let NotificationMethod::SourceRepairChanged {
            target,
            receipt,
            sequence,
        } = connection.next_notification().await.notification
        {
            return (target, receipt, sequence);
        }
    }
}

/// REC-INT: a withdrawal while a client is away is never announced to its
/// next connection — a connection's first observation of a source without a
/// receipt is suppressed — so only the authoritative read can retire the
/// warning that client kept, and its `repair_sequence` orders that read
/// after every notice the client saw before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_withdrawal_while_away_reaches_a_fresh_connection_only_through_its_read() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let target = workspace(&f, 1);
        std::fs::write(f.workspaces[1].join("rustx.toml"), "[agent\nlegacy = 1\n").unwrap();
        // The first connection sees the unresolved receipt, then goes away.
        let away = AppServerConnection::new(f.host.clone());
        initialize(&away).await;
        let Response::Failure(_) = away
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(1),
                call: Method::SessionCreate {
                    settings: SessionPersistentState::from_input(&SessionConfigInput::new(
                        f.workspaces[1].clone(),
                    )),
                },
            })
            .await
        else {
            panic!("an unrepairable Workspace still fails Session creation")
        };
        let (announced, receipt, kept) = sequenced_repair_notice(&away).await;
        assert_eq!(announced, target);
        assert_eq!(receipt.unwrap().status, ConfigRepairStatus::Unresolved);
        drop(away);
        // Another client repairs the Workspace while the first is away.
        let other = AppServerConnection::new(f.host.clone());
        initialize(&other).await;
        let read = sources(&other, 1, &target).await;
        assert_eq!(read.repair_sequence, kept);
        // Addressing the Workspace announces its receipt at the same sequence.
        let (_, receipt, sequence) = sequenced_repair_notice(&other).await;
        assert!(receipt.is_some());
        assert_eq!(sequence, kept);
        let MethodResult::SourceSettings { projection } = call(
            &other,
            2,
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
        assert!(projection.workspace_repair.is_none());
        let withdrawn = projection.repair_sequence;
        assert!(withdrawn > kept);
        // The connection that saw the receipt hears the withdrawal, at the
        // sequence its write reported.
        let (_, receipt, sequence) = sequenced_repair_notice(&other).await;
        assert!(receipt.is_none());
        assert_eq!(sequence, withdrawn);
        // The returning client's fresh connection is told nothing about the
        // Workspace, even once it addresses it ...
        let returning = AppServerConnection::new(f.host.clone());
        initialize(&returning).await;
        let reread = sources(&returning, 1, &target).await;
        while let Some(notice) = returning.next_source_repair() {
            let NotificationMethod::SourceRepairChanged {
                target: noticed, ..
            } = notice
            else {
                panic!("a source repair notice")
            };
            assert_ne!(noticed, target, "a first withdrawal is never announced");
        }
        // ... and only its read says the receipt is gone, at a sequence
        // later than the notice it kept.
        assert!(reread.workspace_repair.is_none());
        assert!(reread.repair_sequence >= withdrawn);
        assert!(reread.repair_sequence > kept);
    })
    .await;
}

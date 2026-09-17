//! Deterministic native Session product lifecycle regressions (Issue #88).
//!
//! The test drives the real Rust product boundary, not a TUI transcript cache.
//! Protocol responses are the synchronization points: no readiness sleeps or
//! timing assumptions are involved.

use crate::launch_fixture::LaunchFixture;
use std::collections::BTreeMap;
use std::sync::Arc;

use rustx::durable::ConversationStore;
use rustx::local_runtime::composition::LocalRuntimeDependencies;
use rustx::local_runtime::{SessionCatalog, StartupSession};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::runtime::identity::MessageId;

const MODELS: &str = r#"[providers.local]
base_url = "http://127.0.0.1:9/v1"
api_key = "$RUSTX_ISSUE88_KEY"

[models."local/test-model"]
provider = "local"
id = "test-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 512

[models."local/test-model".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[models."local/test-model".compat]
chat_reasoning_replay = "omit"

[models."local/second-model"]
provider = "local"
id = "second-model"
protocol = "openai_chat_completions"
context_window = 32000
max_output_tokens = 256

[models."local/second-model".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[models."local/second-model".compat]
chat_reasoning_replay = "omit"
"#;

const BOOTSTRAP: &str = r#"agent_id = "agent-issue88"

[context]
reserve_tokens = 1024
keep_recent_tokens = 4096


[agent]
[agent.model]
model = "local/test-model"
"#;

fn paths(root: &std::path::Path) -> LaunchFixture {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(root.join("rustx.toml"), format!("{BOOTSTRAP}\n{MODELS}")).expect("rustx.toml");
    LaunchFixture {
        config: root.join("rustx.toml"),
        startup_session: StartupSession::Empty,
        session_name: None,
        workspace,
        runtime_root: root.join("runtime"),
    }
}

fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Some(Arc::new(MapCredentialEnvironment::new([(
            "RUSTX_ISSUE88_KEY".to_owned(),
            "test-only-secret".to_owned(),
        )]))),
        ..LocalRuntimeDependencies::default()
    }
}

fn workspace_snapshot(root: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(
        root: &std::path::Path,
        directory: &std::path::Path,
        snapshot: &mut BTreeMap<String, Vec<u8>>,
    ) {
        for entry in std::fs::read_dir(directory).expect("read workspace directory") {
            let entry = entry.expect("workspace entry");
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("workspace path is beneath root")
                .to_string_lossy()
                .into_owned();
            let file_type = entry.file_type().expect("workspace entry type");
            if file_type.is_dir() {
                snapshot.insert(format!("{relative}/"), Vec::new());
                visit(root, &path, snapshot);
            } else if file_type.is_file() {
                snapshot.insert(relative, std::fs::read(&path).expect("read workspace file"));
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

#[tokio::test]
async fn native_create_name_and_read_leave_the_client_attachment_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let product = paths.compose(&dependencies()).await.unwrap();
    let before = product.supervisor().current().await.unwrap();
    let workspace_before = workspace_snapshot(&paths.workspace);
    let created = product.supervisor().new_session().await.unwrap().session;
    assert_ne!(created.id, before.id);
    assert_eq!(product.supervisor().current().await.unwrap(), before);
    assert_eq!(
        product.runtime().conversation_id(),
        &before.active_conversation_id
    );
    product.supervisor().rename("named A".into()).await.unwrap();
    let catalog = SessionCatalog::open_existing(&paths.runtime_root)
        .unwrap()
        .unwrap();
    assert_eq!(catalog.snapshot(&created.id).unwrap(), created);
    assert_eq!(catalog.list_page(None, 0, 32).unwrap().sessions.len(), 2);
    assert_eq!(workspace_snapshot(&paths.workspace), workspace_before);
}
#[tokio::test]
async fn startup_creates_independent_sessions_unless_an_identity_is_supplied() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let first = paths.compose(&dependencies()).await.unwrap();
    let a = first.supervisor().current().await.unwrap();
    drop(first);
    let second = paths.compose(&dependencies()).await.unwrap();
    let b = second.supervisor().current().await.unwrap();
    drop(second);
    assert_ne!(a.id, b.id);
    let before = std::fs::read(paths.runtime_root.join("sessions/catalog.json")).unwrap();
    let resumed = selecting(&paths, &a.id, None)
        .compose(&dependencies())
        .await
        .unwrap();
    assert_eq!(resumed.supervisor().current().await.unwrap(), a);
    assert_eq!(
        std::fs::read(paths.runtime_root.join("sessions/catalog.json")).unwrap(),
        before
    );
    assert_eq!(
        SessionCatalog::open_existing(&paths.runtime_root)
            .unwrap()
            .unwrap()
            .snapshot(&b.id)
            .unwrap(),
        b
    );
}
#[tokio::test]
async fn restart_lists_unused_sessions_without_manufacturing_focus() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let product = paths.compose(&dependencies()).await.unwrap();
    let a = product.supervisor().current().await.unwrap();
    let b = product.supervisor().new_session().await.unwrap().session;
    drop(product);
    let controller =
        rustx::local_runtime::session_controller::SessionController::open(&paths.runtime_root)
            .unwrap();
    let page = controller.list_sessions(None, 0, 32).await.unwrap();
    assert_eq!(
        page.sessions.iter().map(|s| &s.id).collect::<Vec<_>>(),
        vec![&a.id, &b.id]
    );
    let json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(paths.runtime_root.join("sessions/catalog.json")).unwrap(),
    )
    .unwrap();
    assert!(json.get("active_session").is_none());
}
#[tokio::test]
async fn new_after_accepted_work_does_not_replace_the_source() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let first = paths.compose(&dependencies()).await.unwrap();
    let a = first.supervisor().current().await.unwrap();
    drop(first);
    use_session(
        &paths.runtime_root,
        &a.id,
        &a.active_conversation_id,
        "accepted",
    );
    let resumed = selecting(&paths, &a.id, None)
        .compose(&dependencies())
        .await
        .unwrap();
    let b = resumed.supervisor().new_session().await.unwrap().session;
    assert_ne!(a.id, b.id);
    assert_eq!(
        resumed.runtime().conversation_id(),
        &a.active_conversation_id
    );
    assert_eq!(resumed.supervisor().current().await.unwrap(), a);
}
#[tokio::test]
async fn named_cold_resume_only_renames_the_explicit_identity() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let first = paths.compose(&dependencies()).await.unwrap();
    let a = first.supervisor().current().await.unwrap();
    let b = first.supervisor().new_session().await.unwrap().session;
    drop(first);
    let resumed = named(&selecting(&paths, &a.id, None), "A")
        .compose(&dependencies())
        .await
        .unwrap();
    assert_eq!(
        resumed
            .supervisor()
            .current()
            .await
            .unwrap()
            .name
            .as_deref(),
        Some("A")
    );
    assert_eq!(
        SessionCatalog::open_existing(&paths.runtime_root)
            .unwrap()
            .unwrap()
            .snapshot(&b.id)
            .unwrap(),
        b
    );
}
#[tokio::test]
async fn a_launch_name_labels_only_the_new_session() {
    let root = tempfile::tempdir().unwrap();
    let paths = paths(root.path());
    let first = named(&paths, "first")
        .compose(&dependencies())
        .await
        .unwrap();
    let a = first.supervisor().current().await.unwrap();
    drop(first);
    let second = named(&paths, "second")
        .compose(&dependencies())
        .await
        .unwrap();
    let b = second.supervisor().current().await.unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(b.name.as_deref(), Some("second"));
    assert_eq!(
        SessionCatalog::open_existing(&paths.runtime_root)
            .unwrap()
            .unwrap()
            .snapshot(&a.id)
            .unwrap(),
        a
    );
}
fn named(paths: &LaunchFixture, name: &str) -> LaunchFixture {
    LaunchFixture {
        session_name: Some(name.to_owned()),
        ..paths.clone()
    }
}

/// Every persisted Session identity, including unused internal shells that
/// `/resume` does not list.
fn persisted_ids(runtime_root: &std::path::Path) -> Vec<rustx::local_runtime::SessionId> {
    SessionCatalog::open_existing(runtime_root)
        .expect("open catalog")
        .expect("catalog exists")
        .persisted_session_ids()
}

/// The startup arguments of a launch that names where it starts.
fn selecting(
    paths: &LaunchFixture,
    session: &rustx::local_runtime::SessionId,
    node: Option<&rustx::local_runtime::SessionNodeId>,
) -> LaunchFixture {
    LaunchFixture {
        startup_session: StartupSession::Select {
            session: session.clone(),
            node: node.cloned(),
        },
        ..paths.clone()
    }
}

/// Appends one canonical user message, which is the whole difference between
/// an unused Session and durable history.
fn use_session(
    runtime_root: &std::path::Path,
    session: &rustx::local_runtime::SessionId,
    conversation: &rustx::runtime::identity::ConversationId,
    message_id: &str,
) {
    let path = runtime_root
        .join("sessions")
        .join(session.as_str())
        .join("conversations")
        .join(conversation.as_str())
        .join("conversation.sqlite");
    let store = rustx::durable::SqliteConversationStore::open(conversation.clone(), path.as_path())
        .expect("open the conversation");
    store
        .append_canonical(&MessageBlock::User(UserMessageBlock {
            id: MessageId::new(message_id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "history".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }))
        .expect("append canonical history");
}

/// A launch that cannot compose its destination changes no durable catalog
/// state at all.
///
/// The failure is the realistic one: a persisted Session records a
/// Session-local model, and that model is later removed from
/// `rustx.toml`. Selecting that Session is metadata-valid — the catalog
/// knows the Session and the node — and only composition discovers the
/// model is gone. Publishing the selection before composing would leave a
/// process that never started having moved the active selection, so the
/// *next* launch would open a Session the user never chose and would fail
/// the same way again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_launch_leaves_the_catalog_and_the_active_selection_untouched() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = paths(root.path());
    let dependencies = dependencies();
    let runtime_root = root.path().join("runtime");
    let catalog_path = runtime_root.join("sessions").join("catalog.json");

    // A used Session, so a later launch treats it as history.
    let first = (paths).compose(&dependencies).await.expect("first launch");
    let doomed_conversation = first.runtime().conversation_id().clone();
    drop(first);
    let doomed_session = SessionCatalog::open_existing(&runtime_root)
        .unwrap()
        .unwrap()
        .persisted_session_ids()[0]
        .clone();
    use_session(
        &runtime_root,
        &doomed_session,
        &doomed_conversation,
        "issue88-doomed-user",
    );

    // A second Session becomes the active one; the first is history.
    let second = (paths).compose(&dependencies).await.expect("second launch");
    drop(second);
    let active_before = SessionCatalog::open_existing(&runtime_root)
        .unwrap()
        .unwrap()
        .persisted_session_ids()[1]
        .clone();
    assert_ne!(active_before, doomed_session);

    // The history Session records a model that `rustx.toml` no longer
    // offers. Nothing about the catalog is invalid; only composition can
    // discover this.
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&catalog_path).expect("read catalog"))
            .expect("catalog json");
    document["sessions"][doomed_session.as_str()]["state"]["model"] =
        serde_json::to_value(paths.resolve().config().initial_model()).unwrap();
    document["sessions"][doomed_session.as_str()]["state"]["model"]["model"] =
        serde_json::Value::String("local/retired-model".to_owned());
    std::fs::write(
        &catalog_path,
        serde_json::to_vec_pretty(&document).expect("encode catalog"),
    )
    .expect("write catalog");
    let catalog_before = std::fs::read(&catalog_path).expect("read catalog");

    // Selecting it, and naming it in the same launch, must both be undone
    // by the composition failure — because neither was ever done.
    let doomed = LaunchFixture {
        startup_session: StartupSession::Select {
            session: doomed_session.clone(),
            node: None,
        },
        session_name: Some("a name this launch never earned".to_owned()),
        ..paths.clone()
    };
    let _failure = (doomed)
        .compose(&dependencies)
        .await
        .expect_err("a Session whose model no longer exists cannot be composed");

    assert_eq!(
        std::fs::read(&catalog_path).expect("read catalog"),
        catalog_before,
        "a failed launch rewrote the catalog"
    );
    assert_eq!(
        SessionCatalog::open_existing(&runtime_root)
            .unwrap()
            .unwrap()
            .snapshot(&active_before)
            .unwrap()
            .id,
        active_before,
        "a failed launch moved the active selection"
    );
    assert_eq!(
        SessionCatalog::open_existing(&runtime_root)
            .expect("open catalog")
            .expect("catalog exists")
            .snapshot(&doomed_session)
            .expect("the history Session is still there")
            .name,
        None,
        "a failed launch named a Session it never bound"
    );

    // The launch the user can still make is unaffected: the catalog is
    // exactly what it was, so continuing works.
    let recovered = (selecting(&paths, &active_before, None))
        .compose(&dependencies)
        .await
        .expect("the untouched active selection still composes");
    assert_eq!(
        SessionCatalog::open_existing(&runtime_root)
            .unwrap()
            .unwrap()
            .snapshot(&active_before)
            .unwrap()
            .id,
        active_before
    );
    drop(recovered);
}

/// A launch that begins on a fresh empty Session and then fails to compose
/// publishes no Session at all: the seeded destination database is named by
/// nothing, so `/resume` never grows a row for a launch that did not start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_empty_launch_publishes_no_session() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = paths(root.path());
    let dependencies = dependencies();
    let runtime_root = root.path().join("runtime");

    let first = (paths).compose(&dependencies).await.expect("first launch");
    let used_conversation = first.runtime().conversation_id().clone();
    drop(first);
    let used_session = SessionCatalog::open_existing(&runtime_root)
        .unwrap()
        .unwrap()
        .persisted_session_ids()[0]
        .clone();
    use_session(
        &runtime_root,
        &used_session,
        &used_conversation,
        "issue88-empty-user",
    );
    let sessions_before = persisted_ids(&runtime_root);
    let catalog_before =
        std::fs::read(runtime_root.join("sessions").join("catalog.json")).expect("read catalog");

    // The active Session has history, so this launch must publish a new
    // empty one. It plans that publication, seeds its destination database,
    // and then fails to compose: the Workspace it was pointed at is a
    // regular file, which only the conversation tool runtime discovers.
    let broken_workspace = root.path().join("workspace-is-a-file");
    std::fs::write(&broken_workspace, b"not a directory").expect("workspace file");
    let doomed = LaunchFixture {
        workspace: broken_workspace,
        ..paths.clone()
    };
    let _failure = doomed
        .try_resolve()
        .expect_err("a Workspace that is not a directory cannot be resolved");

    assert_eq!(
        persisted_ids(&runtime_root),
        sessions_before,
        "a failed launch published a Session"
    );
    assert_eq!(
        std::fs::read(runtime_root.join("sessions").join("catalog.json")).expect("read catalog"),
        catalog_before,
        "a failed launch rewrote the catalog"
    );
}

/// A **first** launch that fails to compose publishes no catalog at all.
///
/// This is the one startup that has nothing to preserve — there is no
/// catalog, no Session, no selection — and it is the one that most easily
/// leaks a lie. Creating the root Session eagerly writes `catalog.json`
/// before the workspace, the capability composition, the recovery pass, and
/// the host binding have run; when one of those fails, the runtime root is
/// left with a visible, resumable Session belonging to a process that never
/// started, and the next launch continues into it.
///
/// So the first catalog document is a plan like any other, committed in the
/// same startup transaction. A failed first launch leaves no published
/// catalog state behind — only the inert seeded database, which nothing
/// names and nothing can reach.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_first_launch_publishes_no_catalog() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = paths(root.path());
    let dependencies = dependencies();
    let runtime_root = root.path().join("runtime");
    let catalog_path = runtime_root.join("sessions").join("catalog.json");

    // Nothing has ever launched here: the failure below is the first thing
    // this runtime root sees.
    assert!(!catalog_path.exists(), "the runtime root starts empty");
    let broken_workspace = root.path().join("workspace-is-a-file");
    std::fs::write(&broken_workspace, b"not a directory").expect("workspace file");
    let doomed = LaunchFixture {
        workspace: broken_workspace,
        ..paths.clone()
    };
    let _failure = doomed
        .try_resolve()
        .expect_err("a Workspace that is not a directory cannot be resolved");

    assert!(
        !catalog_path.exists(),
        "a first launch that never started published a catalog"
    );
    assert!(
        SessionCatalog::open_existing(&runtime_root)
            .expect("open catalog")
            .is_none(),
        "a first launch that never started left a resumable Session"
    );

    // The runtime root is still fresh, so the next launch is a first launch
    // and starts on the root Session it publishes itself.
    let recovered = (paths)
        .compose(&dependencies)
        .await
        .expect("the untouched runtime root still composes");
    assert_eq!(
        persisted_ids(&runtime_root).len(),
        1,
        "the successful launch published exactly one Session"
    );
    assert!(
        persisted_ids(&runtime_root).len() == 1,
        "the published root shell is durable, not resume history"
    );
    drop(recovered);
}

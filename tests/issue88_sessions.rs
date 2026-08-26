//! Deterministic native Session product lifecycle regressions (Issue #88).
//!
//! The test drives the real Rust product boundary, not a TUI transcript cache.
//! Protocol responses are the synchronization points: no readiness sleeps or
//! timing assumptions are involved.

use std::collections::BTreeMap;
use std::sync::Arc;

use rustx::durable::ConversationStore;
use rustx::local_runtime::composition::{
    LocalRuntimeDependencies, LocalRuntimePaths, LocalSessionProduct,
};
use rustx::local_runtime::{SessionCatalog, StartupSession};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::catalog::MapCredentialEnvironment;
use rustx::runtime::identity::MessageId;
use rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1;
use rustx::runtime_client::types::{
    RequestId, RuntimeClientError, RuntimeClientRequest, RuntimeClientResult,
};

const MODELS: &str = r#"{
  "providers": {
    "local": {
      "baseUrl": "http://127.0.0.1:9/v1",
      "apiKey": "$RUSTX_ISSUE88_KEY",
      "models": [{
        "id": "test-model",
        "protocol": "openai_chat_completions",
        "contextWindow": 128000,
        "maxOutputTokens": 512,
        "capabilities": {
          "inputModalities": ["text"],
          "outputModalities": ["text"],
          "toolCalls": true,
          "reasoning": false
        },
        "compat": {"chatReasoningReplay": "omit"}
      }, {
        "id": "second-model",
        "protocol": "openai_chat_completions",
        "contextWindow": 32000,
        "maxOutputTokens": 256,
        "capabilities": {
          "inputModalities": ["text"],
          "outputModalities": ["text"],
          "toolCalls": true,
          "reasoning": false
        },
        "compat": {"chatReasoningReplay": "omit"}
      }]
    }
  }
}"#;

const BOOTSTRAP: &str = r#"{
  "agentId": "agent-issue88",
  "model": {"model": "local/test-model"},
  "context": {"reserveTokens": 1024, "keepRecentTokens": 4096}
}"#;

fn paths(root: &std::path::Path) -> LocalRuntimePaths {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(root.join("models.jsonc"), MODELS).expect("models");
    std::fs::write(root.join("rustx.jsonc"), BOOTSTRAP).expect("rustx.jsonc");
    LocalRuntimePaths {
        models: root.join("models.jsonc"),
        config: root.join("rustx.jsonc"),
        skill_paths: Vec::new(),
        no_skills: false,
        no_builtin_tools: false,
        no_tools: false,
        startup_session: StartupSession::Empty,
        session_name: None,
        tools: None,
        exclude_tools: Vec::new(),
        workspace,
        runtime_root: root.join("runtime"),
    }
}

/// The same startup arguments a client repeats when it replaces the process
/// to complete a Session switch it has already published.
fn continuing(paths: &LocalRuntimePaths) -> LocalRuntimePaths {
    LocalRuntimePaths {
        startup_session: StartupSession::ContinueActive,
        ..paths.clone()
    }
}

fn dependencies() -> LocalRuntimeDependencies {
    LocalRuntimeDependencies {
        credentials: Arc::new(MapCredentialEnvironment::new([(
            "RUSTX_ISSUE88_KEY".to_owned(),
            "test-only-secret".to_owned(),
        )])),
        ..LocalRuntimeDependencies::default()
    }
}

fn request_id(value: u64) -> RequestId {
    RequestId::new(value)
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

async fn session_request(
    endpoint: &rustx::runtime_client::RuntimeClientEndpoint,
    request: RuntimeClientRequest,
) -> rustx::runtime_client::types::RuntimeClientResponse {
    endpoint.handle_request_async(request).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn native_new_resume_name_and_quiescence_are_product_operations() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = paths(root.path());
    let workspace = paths.workspace.clone();
    std::fs::write(workspace.join("workspace-owned.txt"), b"do not branch me")
        .expect("workspace marker");
    let workspace_before = workspace_snapshot(&workspace);
    let dependencies = dependencies();

    let product = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("compose root product");
    let endpoint = product.endpoint();
    let initialized = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: request_id(1),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
    });
    let Some(RuntimeClientResult::Initialized { .. }) = initialized.result else {
        panic!("initialize must succeed: {initialized:?}");
    };

    let current = session_request(
        &endpoint,
        RuntimeClientRequest::SessionGet { id: request_id(2) },
    )
    .await;
    let Some(RuntimeClientResult::Session { session: root_view }) = current.result else {
        panic!("session_get must return native metadata: {current:?}");
    };
    let root_session = root_view.id.clone();
    let root_conversation = root_view.active_conversation_id.clone();

    let renamed = session_request(
        &endpoint,
        RuntimeClientRequest::SessionName {
            id: request_id(3),
            name: "root transcript".to_owned(),
        },
    )
    .await;
    let Some(RuntimeClientResult::SessionChanged { session, .. }) = renamed.result else {
        panic!("session_name must return metadata: {renamed:?}");
    };
    assert_eq!(session.name.as_deref(), Some("root transcript"));

    let model_set = endpoint.handle_request(RuntimeClientRequest::ModelSet {
        id: request_id(31),
        config: Box::new(rustx::model::session::SessionModelConfig::of(
            serde_json::from_value(serde_json::json!("local/second-model"))
                .expect("second model reference"),
        )),
    });
    let Some(RuntimeClientResult::ModelSet { model }) = model_set.result else {
        panic!("model_set must update the active Session model: {model_set:?}");
    };
    assert_eq!(model.configured.model.to_string(), "local/second-model");

    let catalog = SessionCatalog::open_existing(root.path().join("runtime").as_path())
        .expect("open catalog")
        .expect("read catalog");
    assert_eq!(
        catalog
            .list_page(None, 0, rustx::local_runtime::SESSION_LIST_PAGE_LIMIT)
            .expect("list page")
            .sessions
            .len(),
        1
    );
    let root_id = root_session.clone();
    let root_store_path = root
        .path()
        .join("runtime")
        .join("sessions")
        .join(&root_id)
        .join("conversations")
        .join(root_conversation.as_str())
        .join("conversation.sqlite");
    let root_store =
        rustx::durable::SqliteConversationStore::open(root_conversation.clone(), &root_store_path)
            .expect("root store");
    let canonical_before = root_store.load_canonical().expect("canonical before new");

    let created = session_request(
        &endpoint,
        RuntimeClientRequest::SessionNew { id: request_id(4) },
    )
    .await;
    let Some(RuntimeClientResult::SessionChanged {
        session: new_view,
        restart_required,
        ..
    }) = created.result
    else {
        panic!("session_new must return a replacement: {created:?}");
    };
    assert!(restart_required);
    assert_ne!(new_view.id, root_session);
    assert_ne!(new_view.active_conversation_id, root_conversation);

    // A duplicate command cannot publish a second transition after the
    // first command has released the only active runtime.
    let duplicate = session_request(
        &endpoint,
        RuntimeClientRequest::SessionNew { id: request_id(5) },
    )
    .await;
    assert!(matches!(
        duplicate.error,
        Some(RuntimeClientError::SessionRestartRequired { .. })
    ));
    assert_eq!(
        root_store
            .load_canonical()
            .expect("root canonical after new"),
        canonical_before,
        "new never rewinds the previous lineage"
    );
    let catalog_after_new = SessionCatalog::open_existing(root.path().join("runtime").as_path())
        .expect("open catalog after new")
        .expect("reopen catalog after new");
    assert_eq!(
        catalog_after_new
            .list_page(None, 0, rustx::local_runtime::SESSION_LIST_PAGE_LIMIT)
            .expect("list page")
            .sessions
            .len(),
        2
    );

    drop(endpoint);
    drop(product);

    // Recomposition resolves the catalog's published active node and runs
    // ordinary ConversationRuntime recovery for that independent lineage.
    let resumed = LocalSessionProduct::compose(&continuing(&paths), &dependencies)
        .await
        .expect("compose selected new session");
    assert_eq!(
        resumed.runtime().conversation_id().as_str(),
        new_view.active_conversation_id.as_str()
    );
    let resumed_endpoint = resumed.endpoint();
    let initialized = resumed_endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: request_id(6),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
    });
    let Some(RuntimeClientResult::Initialized { snapshot, .. }) = initialized.result else {
        panic!("resumed runtime must initialize: {initialized:?}");
    };
    assert_eq!(
        snapshot.model.configured.model.to_string(),
        "local/test-model",
        "a new Session uses the current runtime default, not the previous Session choice"
    );

    // `/resume` selects the old persisted Session through the native owner;
    // it does not swap a transcript in the current client.
    let selected = session_request(
        &resumed_endpoint,
        RuntimeClientRequest::SessionSelect {
            id: request_id(7),
            session_id: root_session.clone(),
            node_id: None,
        },
    )
    .await;
    let Some(RuntimeClientResult::SessionChanged {
        session: selected_view,
        restart_required,
        ..
    }) = selected.result
    else {
        panic!("session_select must return a replacement: {selected:?}");
    };
    assert!(restart_required);
    assert_eq!(selected_view.id, root_session);
    drop(resumed_endpoint);
    drop(resumed);

    let restored = LocalSessionProduct::compose(&continuing(&paths), &dependencies)
        .await
        .expect("compose resumed root session");
    assert_eq!(
        restored.runtime().conversation_id().as_str(),
        root_conversation.as_str()
    );
    let restored_endpoint = restored.endpoint();
    let restored_initialized = restored_endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: request_id(8),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
    });
    let Some(RuntimeClientResult::Initialized { snapshot, .. }) = restored_initialized.result
    else {
        panic!("restored runtime must initialize");
    };
    assert!(snapshot.attempt.is_none());
    assert!(snapshot.background.is_empty());
    assert_eq!(
        workspace_snapshot(&workspace),
        workspace_before,
        "Session branching and replacement never mutate workspace state"
    );
}

/// A launch is not a resume.
///
/// Startup begins on an empty Session and leaves an already-used Session as
/// history reachable through `/resume`; an active Session that was never used
/// is reused, so repeated launches cannot accumulate empty rows; and
/// `--continue` is the one way to bind the catalog's published active
/// selection again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_begins_on_an_empty_session_unless_continue_is_requested() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = paths(root.path());
    let dependencies = dependencies();
    let runtime_root = root.path().join("runtime");

    let first = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("first launch");
    let first_conversation = first.runtime().conversation_id().clone();
    drop(first);

    // The first launch left its Session unused, so the second launch is that
    // same empty Session rather than another one beside it.
    let second = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("second launch");
    assert_eq!(second.runtime().conversation_id(), &first_conversation);
    drop(second);
    assert_eq!(session_ids(&runtime_root).len(), 1);

    // One canonical user message is the whole difference: the Session has
    // been used, so the next launch must not open it.
    let used_session = active_session_id(&runtime_root);
    use_session(
        &runtime_root,
        &used_session,
        &first_conversation,
        "issue88-startup-user",
    );

    let third = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("third launch");
    let fresh_conversation = third.runtime().conversation_id().clone();
    assert_ne!(
        fresh_conversation, first_conversation,
        "a launch never opens a Session that already has history"
    );
    drop(third);

    // The used Session was not rewritten, replaced, or hidden: it is durable
    // history the selector still lists.
    let listed = session_ids(&runtime_root);
    assert_eq!(listed.len(), 2);
    assert!(listed.contains(&used_session));

    // Selecting the used Session publishes it as the active one, and the
    // process replacement that completes the switch asks for it explicitly.
    let switching = LocalSessionProduct::compose(&continuing(&paths), &dependencies)
        .await
        .expect("continue the active Session");
    let endpoint = switching.endpoint();
    let initialized = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: request_id(1),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION_V1,
    });
    assert!(matches!(
        initialized.result,
        Some(RuntimeClientResult::Initialized { .. })
    ));
    let selected = session_request(
        &endpoint,
        RuntimeClientRequest::SessionSelect {
            id: request_id(2),
            session_id: used_session.to_string(),
            node_id: None,
        },
    )
    .await;
    assert!(matches!(
        selected.result,
        Some(RuntimeClientResult::SessionChanged {
            restart_required: true,
            ..
        })
    ));
    drop(endpoint);
    drop(switching);

    let continued = LocalSessionProduct::compose(&continuing(&paths), &dependencies)
        .await
        .expect("compose the selected Session");
    assert_eq!(
        continued.runtime().conversation_id(),
        &first_conversation,
        "--continue binds the published active selection"
    );
    drop(continued);

    // The next ordinary launch leaves that selection as history again.
    let relaunched = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("ordinary relaunch");
    assert_ne!(relaunched.runtime().conversation_id(), &first_conversation);
}

/// Naming a Session at launch binds it and publishes that selection.
///
/// This is the startup form of `/resume`: the destination is committed to the
/// catalog before the first runtime is composed, so the replacement spawn
/// that continues the active selection lands on the same lineage without
/// naming it, and an identity that does not exist fails the launch rather
/// than opening something else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn naming_a_startup_session_binds_it_and_publishes_the_selection() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = paths(root.path());
    let dependencies = dependencies();
    let runtime_root = root.path().join("runtime");

    // One used Session, then an ordinary relaunch that leaves it as history.
    let first = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("first launch");
    let historical_conversation = first.runtime().conversation_id().clone();
    drop(first);
    let historical = active_session_id(&runtime_root);
    let historical_node = active_node_id(&runtime_root);
    use_session(
        &runtime_root,
        &historical,
        &historical_conversation,
        "issue88-named-user",
    );

    let relaunched = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("ordinary relaunch");
    let fresh_conversation = relaunched.runtime().conversation_id().clone();
    drop(relaunched);
    assert_ne!(fresh_conversation, historical_conversation);
    assert_ne!(active_session_id(&runtime_root), historical);

    // The named Session is bound directly — no empty Session is published
    // beside it, and the catalog now publishes it as the active selection.
    let named = LocalSessionProduct::compose(&selecting(&paths, &historical, None), &dependencies)
        .await
        .expect("launch on the named Session");
    assert_eq!(named.runtime().conversation_id(), &historical_conversation);
    drop(named);
    assert_eq!(active_session_id(&runtime_root), historical);
    assert_eq!(
        session_ids(&runtime_root).len(),
        2,
        "naming a Session publishes no Session of its own"
    );

    // A replacement spawn completing a switch never names its destination;
    // the selection a named launch published is what it continues.
    let continued = LocalSessionProduct::compose(&continuing(&paths), &dependencies)
        .await
        .expect("replacement spawn");
    assert_eq!(
        continued.runtime().conversation_id(),
        &historical_conversation
    );
    drop(continued);

    // The named node is part of the selection, and both identities are
    // checked against the catalog before anything is composed.
    let node = LocalSessionProduct::compose(
        &selecting(&paths, &historical, Some(&historical_node)),
        &dependencies,
    )
    .await
    .expect("launch on the named node");
    assert_eq!(node.runtime().conversation_id(), &historical_conversation);
    drop(node);

    let unknown_session = rustx::local_runtime::SessionId::new("session-absent");
    assert!(
        LocalSessionProduct::compose(&selecting(&paths, &unknown_session, None), &dependencies)
            .await
            .is_err(),
        "an unknown Session identity fails the launch"
    );
    let unknown_node = rustx::local_runtime::SessionNodeId::new("node-absent");
    assert!(
        LocalSessionProduct::compose(
            &selecting(&paths, &historical, Some(&unknown_node)),
            &dependencies
        )
        .await
        .is_err(),
        "an unknown node identity fails the launch"
    );
    assert_eq!(
        active_session_id(&runtime_root),
        historical,
        "a rejected launch changes no published selection"
    );
}

/// A launch name labels the Session the launch bound; it never chooses one.
///
/// This is `/name` moved to the command line, and the distinction it keeps is
/// the one the whole naming model rests on: a Session is *identified* by the
/// identity the catalog published and *recognized* by the label a user gave
/// it. So a name follows the Session that was bound rather than selecting a
/// Session, a later launch's empty Session inherits nothing, and no identity
/// is ever resolved from a name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_launch_name_labels_the_bound_session_and_never_selects_one() {
    let root = tempfile::tempdir().expect("temp root");
    let paths = paths(root.path());
    let dependencies = dependencies();
    let runtime_root = root.path().join("runtime");

    let first = LocalSessionProduct::compose(&named(&paths, "auth refactor"), &dependencies)
        .await
        .expect("named launch");
    let first_conversation = first.runtime().conversation_id().clone();
    drop(first);
    let first_session = active_session_id(&runtime_root);
    assert_eq!(
        session_rows(&runtime_root)
            .into_iter()
            .find(|row| row.id == first_session)
            .expect("the bound Session is listed")
            .name
            .as_deref(),
        Some("auth refactor"),
        "--name names the Session this launch bound"
    );

    // The next ordinary launch starts on a Session of its own, and a name is
    // no more inheritable than the conversation it labelled.
    use_session(
        &runtime_root,
        &first_session,
        &first_conversation,
        "issue88-named-launch",
    );
    let relaunched = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("ordinary relaunch");
    drop(relaunched);
    let rows = session_rows(&runtime_root);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter()
            .filter(|row| row.name.as_deref() == Some("auth refactor"))
            .count(),
        1,
        "the name stayed with the Session it was given to"
    );

    // Naming a launch that continues renames exactly that Session, which is
    // what typing `/name` in it would have done.
    let continued =
        LocalSessionProduct::compose(&named(&continuing(&paths), "session picker"), &dependencies)
            .await
            .expect("named continuation");
    drop(continued);
    let active = active_session_id(&runtime_root);
    assert_ne!(active, first_session);
    assert_eq!(
        session_rows(&runtime_root)
            .into_iter()
            .find(|row| row.id == active)
            .expect("the continued Session is listed")
            .name
            .as_deref(),
        Some("session picker")
    );

    // A name is not an identity, and nothing resolves one.
    assert!(
        LocalSessionProduct::compose(
            &selecting(
                &paths,
                &rustx::local_runtime::SessionId::new("auth refactor"),
                None
            ),
            &dependencies
        )
        .await
        .is_err(),
        "a display name never names a Session to open"
    );
}

/// The startup arguments of a launch that names the Session it binds.
fn named(paths: &LocalRuntimePaths, name: &str) -> LocalRuntimePaths {
    LocalRuntimePaths {
        session_name: Some(name.to_owned()),
        ..paths.clone()
    }
}

/// The `/resume` rows of the persisted catalog.
fn session_rows(runtime_root: &std::path::Path) -> Vec<rustx::local_runtime::SessionSummary> {
    SessionCatalog::open_existing(runtime_root)
        .expect("open catalog")
        .expect("catalog exists")
        .list_page(None, 0, rustx::local_runtime::SESSION_LIST_PAGE_LIMIT)
        .expect("list page")
        .sessions
}

/// The startup arguments of a launch that names where it starts.
fn selecting(
    paths: &LocalRuntimePaths,
    session: &rustx::local_runtime::SessionId,
    node: Option<&rustx::local_runtime::SessionNodeId>,
) -> LocalRuntimePaths {
    LocalRuntimePaths {
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
/// `models.jsonc`. Selecting that Session is metadata-valid — the catalog
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
    let first = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("first launch");
    let doomed_conversation = first.runtime().conversation_id().clone();
    drop(first);
    let doomed_session = active_session_id(&runtime_root);
    use_session(
        &runtime_root,
        &doomed_session,
        &doomed_conversation,
        "issue88-doomed-user",
    );

    // A second Session becomes the active one; the first is history.
    let second = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("second launch");
    drop(second);
    let active_before = active_session_id(&runtime_root);
    assert_ne!(active_before, doomed_session);

    // The history Session records a model that `models.jsonc` no longer
    // offers. Nothing about the catalog is invalid; only composition can
    // discover this.
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&catalog_path).expect("read catalog"))
            .expect("catalog json");
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
    let doomed = LocalRuntimePaths {
        startup_session: StartupSession::Select {
            session: doomed_session.clone(),
            node: None,
        },
        session_name: Some("a name this launch never earned".to_owned()),
        ..paths.clone()
    };
    let _failure = LocalSessionProduct::compose(&doomed, &dependencies)
        .await
        .expect_err("a Session whose model no longer exists cannot be composed");

    assert_eq!(
        std::fs::read(&catalog_path).expect("read catalog"),
        catalog_before,
        "a failed launch rewrote the catalog"
    );
    assert_eq!(
        active_session_id(&runtime_root),
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
    let recovered = LocalSessionProduct::compose(&continuing(&paths), &dependencies)
        .await
        .expect("the untouched active selection still composes");
    assert_eq!(active_session_id(&runtime_root), active_before);
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

    let first = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("first launch");
    let used_conversation = first.runtime().conversation_id().clone();
    drop(first);
    let used_session = active_session_id(&runtime_root);
    use_session(
        &runtime_root,
        &used_session,
        &used_conversation,
        "issue88-empty-user",
    );
    let sessions_before = session_ids(&runtime_root);
    let catalog_before =
        std::fs::read(runtime_root.join("sessions").join("catalog.json")).expect("read catalog");

    // The active Session has history, so this launch must publish a new
    // empty one. It plans that publication, seeds its destination database,
    // and then fails to compose: the Workspace it was pointed at is a
    // regular file, which only the conversation tool runtime discovers.
    let broken_workspace = root.path().join("workspace-is-a-file");
    std::fs::write(&broken_workspace, b"not a directory").expect("workspace file");
    let doomed = LocalRuntimePaths {
        workspace: broken_workspace,
        ..paths.clone()
    };
    let _failure = LocalSessionProduct::compose(&doomed, &dependencies)
        .await
        .expect_err("a Workspace that is not a directory cannot be composed");

    assert_eq!(
        session_ids(&runtime_root),
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
    let doomed = LocalRuntimePaths {
        workspace: broken_workspace,
        ..paths.clone()
    };
    let _failure = LocalSessionProduct::compose(&doomed, &dependencies)
        .await
        .expect_err("a Workspace that is not a directory cannot be composed");

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
    let recovered = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("the untouched runtime root still composes");
    assert_eq!(
        session_ids(&runtime_root).len(),
        1,
        "the successful launch published exactly one Session"
    );
    drop(recovered);
}

/// The active node of the catalog's published active Session.
fn active_node_id(runtime_root: &std::path::Path) -> rustx::local_runtime::SessionNodeId {
    SessionCatalog::open_existing(runtime_root)
        .expect("open catalog")
        .expect("catalog exists")
        .active_snapshot()
        .expect("active snapshot")
        .active_node
}

/// The persisted Session identities, in catalog order.
fn session_ids(runtime_root: &std::path::Path) -> Vec<rustx::local_runtime::SessionId> {
    SessionCatalog::open_existing(runtime_root)
        .expect("open catalog")
        .expect("catalog exists")
        .list_page(None, 0, rustx::local_runtime::SESSION_LIST_PAGE_LIMIT)
        .expect("list page")
        .sessions
        .into_iter()
        .map(|summary| summary.id)
        .collect()
}

/// The catalog's published active Session identity.
fn active_session_id(runtime_root: &std::path::Path) -> rustx::local_runtime::SessionId {
    SessionCatalog::open_existing(runtime_root)
        .expect("open catalog")
        .expect("catalog exists")
        .active_snapshot()
        .expect("active snapshot")
        .id
}

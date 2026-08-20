//! Deterministic native Session product lifecycle regressions (Issue #88).
//!
//! The test drives the real Rust product boundary, not a TUI transcript cache.
//! Protocol responses are the synchronization points: no readiness sleeps or
//! timing assumptions are involved.

use std::sync::Arc;

use rustx::durable::ConversationStore;
use rustx::local_runtime::SessionCatalog;
use rustx::local_runtime::composition::{
    LocalRuntimeDependencies, LocalRuntimePaths, LocalSessionProduct,
};
use rustx::model::catalog::MapCredentialEnvironment;
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
  "conversationId": "conversation-issue88-root",
  "agentId": "agent-issue88",
  "model": {"model": "local/test-model"},
  "context": {"reserveTokens": 1024, "keepRecentTokens": 4096}
}"#;

fn paths(root: &std::path::Path) -> LocalRuntimePaths {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(root.join("models.json"), MODELS).expect("models");
    std::fs::write(root.join("bootstrap.json"), BOOTSTRAP).expect("bootstrap");
    LocalRuntimePaths {
        models: root.join("models.json"),
        session: root.join("bootstrap.json"),
        workspace,
        runtime_root: root.join("runtime"),
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
    let root_conversation = root_view.nodes[0].conversation_id.clone();

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
    assert_eq!(session.name, "root transcript");

    let model_set = endpoint.handle_request(RuntimeClientRequest::ModelSet {
        id: request_id(31),
        config: Box::new(rustx::model::session::SessionModelConfig::of(
            serde_json::from_value(serde_json::json!("local/second-model"))
                .expect("second model reference"),
        )),
    });
    let Some(RuntimeClientResult::ModelSet { model }) = model_set.result else {
        panic!("model_set must update the active Session config: {model_set:?}");
    };
    assert_eq!(model.configured.model.to_string(), "local/second-model");

    let catalog = SessionCatalog::open(root.path().join("runtime").as_path(), &config())
        .expect("read catalog");
    assert_eq!(catalog.list().len(), 1);
    let root_node = root_view
        .nodes
        .iter()
        .find(|node| node.id == root_view.active_node)
        .expect("root active node");
    let root_id = root_session.clone();
    let root_store_path = root
        .path()
        .join("runtime")
        .join("sessions")
        .join(&root_id)
        .join("conversations")
        .join(root_node.conversation_id.as_str())
        .join("conversation.sqlite");
    let root_store = rustx::durable::SqliteConversationStore::open(
        root_node.conversation_id.clone(),
        &root_store_path,
    )
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
    assert_ne!(new_view.nodes[0].conversation_id, root_conversation);

    // A duplicate command cannot publish a second transition after the
    // first command has released the only active runtime.
    let duplicate = session_request(
        &endpoint,
        RuntimeClientRequest::SessionNew { id: request_id(5) },
    )
    .await;
    assert!(matches!(
        duplicate.error,
        Some(RuntimeClientError::SessionFailure { .. })
    ));
    assert_eq!(
        root_store
            .load_canonical()
            .expect("root canonical after new"),
        canonical_before,
        "new never rewinds the previous lineage"
    );
    let catalog_after_new = SessionCatalog::open(root.path().join("runtime").as_path(), &config())
        .expect("reopen catalog after new");
    assert_eq!(catalog_after_new.list().len(), 2);

    drop(endpoint);
    drop(product);

    // Recomposition resolves the catalog's published active node and runs
    // ordinary ConversationRuntime recovery for that independent lineage.
    let resumed = LocalSessionProduct::compose(&paths, &dependencies)
        .await
        .expect("compose selected new session");
    assert_eq!(
        resumed.runtime().conversation_id().as_str(),
        new_view.nodes[0].conversation_id.as_str()
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
        "local/second-model"
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

    let restored = LocalSessionProduct::compose(&paths, &dependencies)
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
}

fn config() -> rustx::local_runtime::LocalConversationConfig {
    rustx::local_runtime::LocalConversationConfig::from_json_slice(BOOTSTRAP.as_bytes())
        .expect("bootstrap config")
}

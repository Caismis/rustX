//! Exact endpoint lifetime tests, included as `endpoint::retirement_tests`.
use super::*;
use crate::durable::SqliteConversationStore;
use crate::durable::{CanonicalStore, JournalStore, ObservationStore};
use crate::runtime::identity::ConversationId;
use crate::runtime_client::{
    RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientCursor, RuntimeClientResult,
};

fn fixture() -> (
    Arc<SqliteConversationStore>,
    RuntimeClientHost,
    RuntimeClientEndpoint,
) {
    let store = Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
    store.initialize(&[]).unwrap();
    let host = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
    let endpoint = RuntimeClientEndpoint::new_read_only(&host);
    (store, host, endpoint)
}

fn initialize(endpoint: &RuntimeClientEndpoint, id: u64) -> super::super::AttachmentId {
    let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: RequestId::new(id),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
    });
    let Some(RuntimeClientResult::Initialized {
        attachment_id,
        read_only: true,
        ..
    }) = response.result
    else {
        panic!("original read-only endpoint admission: {response:?}");
    };
    attachment_id
}

fn observe_request(id: u64, cursor: RuntimeClientCursor) -> RuntimeClientRequest {
    RuntimeClientRequest::ObserveEvents {
        id: RequestId::new(id),
        after_cursor: cursor,
    }
}

#[tokio::test]
async fn retirement_wakes_only_its_registered_wait_and_preserves_original_cut() {
    let (store, host, endpoint) = fixture();
    let other = RuntimeClientEndpoint::new_read_only(&host);
    initialize(&endpoint, 1);
    initialize(&other, 2);
    let before = host.checkpoint().unwrap();
    let source = store.observation_read_cut().unwrap();
    let observing = endpoint.handle_request_async(observe_request(3, before.cursor));
    let unrelated = other.handle_request_async(observe_request(4, before.cursor));
    tokio::pin!(observing, unrelated);
    // Each real request reaches its installed subscription before retirement.
    assert!(futures_util::poll!(&mut observing).is_pending());
    assert!(futures_util::poll!(&mut unrelated).is_pending());
    endpoint.retirement().retire();
    let std::task::Poll::Ready(response) = futures_util::poll!(&mut observing) else {
        panic!("retirement immediately closes its installed native wait");
    };
    assert!(
        matches!(
            response.error,
            Some(RuntimeClientError::ObservationUnavailable { .. })
        ),
        "{response:?}"
    );
    assert!(
        futures_util::poll!(&mut unrelated).is_pending(),
        "another endpoint remains live"
    );
    other.retirement().retire();
    let std::task::Poll::Ready(response) = futures_util::poll!(&mut unrelated) else {
        panic!("retirement immediately closes the second native wait");
    };
    assert!(
        matches!(
            response.error,
            Some(RuntimeClientError::ObservationUnavailable { .. })
        ),
        "{response:?}"
    );
    assert_eq!(host.checkpoint().unwrap(), before);
    assert_eq!(store.observation_read_cut().unwrap(), source);
    assert_eq!(store.presentation_frontier().unwrap(), 0);
    assert!(store.load_canonical().unwrap().is_empty());
}

#[tokio::test]
async fn retirement_fences_a_previously_cloned_attachment_at_native_subscribe_admission() {
    let (store, host, endpoint) = fixture();
    let identity = initialize(&endpoint, 1);
    let before = host.checkpoint().unwrap();
    let source = store.observation_read_cut().unwrap();
    // This is the genuine Arc cloned by asynchronous endpoint dispatch.
    let cloned = endpoint
        .attachment
        .lock()
        .unwrap()
        .attachment
        .as_ref()
        .unwrap()
        .clone();
    let retirement = endpoint.retirement();
    retirement.retire();
    retirement.retire();
    let response = cloned
        .handle_request_async(observe_request(2, before.cursor))
        .await;
    assert!(
        matches!(response.error, Some(RuntimeClientError::NotAttached)),
        "{response:?}"
    );
    // An operation that already passed the attachment's atomic check still
    // reaches this exact native host lock. A stale identity cannot subscribe.
    assert!(matches!(
        host.inner.observe_events(&identity, before.cursor).await,
        Err(RuntimeClientError::NotAttached)
    ));
    assert!(matches!(
        host.inner.subscribe_events(&identity, before.cursor),
        Err(RuntimeClientError::NotAttached)
    ));
    let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: RequestId::new(3),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
    });
    assert!(
        matches!(response.error, Some(RuntimeClientError::NotAttached)),
        "{response:?}"
    );
    assert_eq!(host.checkpoint().unwrap(), before);
    assert_eq!(store.observation_read_cut().unwrap(), source);
    assert_eq!(store.presentation_frontier().unwrap(), 0);
}

#[test]
fn ordinary_detach_reinitializes_but_permanent_retirement_and_weak_owner_cannot() {
    let (store, host, endpoint) = fixture();
    let first = initialize(&endpoint, 1);
    let before = host.checkpoint().unwrap();
    let response = endpoint.handle_request(RuntimeClientRequest::Detach {
        id: RequestId::new(2),
    });
    assert!(response.error.is_none(), "{response:?}");
    let second = initialize(&endpoint, 3);
    assert_ne!(first, second);
    let retirement = endpoint.retirement();
    assert!(retirement.is_alive());
    retirement.retire();
    let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
        id: RequestId::new(4),
        protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
    });
    assert!(
        matches!(response.error, Some(RuntimeClientError::NotAttached)),
        "{response:?}"
    );
    drop(endpoint);
    assert!(!retirement.is_alive(), "the retirement handle is weak");
    retirement.retire();
    assert!(matches!(
        host.inner.subscribe_events(&second, before.cursor),
        Err(RuntimeClientError::NotAttached)
    ));
    assert_eq!(host.checkpoint().unwrap(), before);
    assert_eq!(store.presentation_frontier().unwrap(), 0);
}

//! Shared App Server semantic expectations for direct and future #36 drivers.
//!
//! Include this test-only module with `#[path = ".../support/app_server_conformance.rs"]`.
//! Supply one fresh connection and two distinct, idle Sessions in Policy mode.
//! Drivers implement message exchange only; framing, process setup, and transport
//! failure tests belong to their adapters. No production transport API is added.

use futures_util::future::BoxFuture;
use rustx::app_server::connection::AppServerConnection;
use rustx::app_server::protocol::*;
use rustx::local_runtime::session::SessionId;
use rustx::runtime::types::ApprovalMode;
use rustx::runtime_client::event::RuntimeClientEvent;

/// A driver must support concurrent requests on one connection and preserve IDs.
pub trait AppServerConformanceDriver: Sync {
    fn request(&self, request: Request) -> BoxFuture<'_, Response>;
    fn next_notification(&self) -> BoxFuture<'_, Notification>;
}

/// Direct baseline; future stdio/WebSocket drivers exchange the same DTOs.
#[allow(dead_code)] // Transport-only integration targets use the concrete drivers.
pub struct DirectDriver<'a>(pub &'a AppServerConnection);

impl AppServerConformanceDriver for DirectDriver<'_> {
    fn request(&self, request: Request) -> BoxFuture<'_, Response> {
        Box::pin(self.0.handle_request(request))
    }

    fn next_notification(&self) -> BoxFuture<'_, Notification> {
        Box::pin(self.0.next_notification())
    }
}

fn initialize(version: u16) -> Method {
    Method::Initialize(InitializeParams {
        protocol_version: version,
        client: ClientIdentity {
            name: "conformance".into(),
            version: "1".into(),
        },
        presentation: PresentationCapabilities::default(),
    })
}

async fn call(driver: &impl AppServerConformanceDriver, id: i64, call: Method) -> MethodResult {
    let Response::Success(response) = driver
        .request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call,
        })
        .await
    else {
        panic!("expected success for request {id}")
    };
    assert_eq!(response.jsonrpc, JsonRpcVersion::V2);
    assert_eq!(response.id, RequestId::Integer(id));
    response.result
}

/// One semantic parity scenario. The caller supplies only an outer liveness guard.
/// All observations come from protocol snapshots/events, not test-owned state.
#[allow(clippy::too_many_lines)]
pub async fn representative_scenario(
    driver: &impl AppServerConformanceDriver,
    sessions: [SessionId; 2],
) {
    assert_ne!(sessions[0], sessions[1]);
    let mismatch_id = RequestId::String("version-mismatch".into());
    let Response::Failure(failure) = driver
        .request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: mismatch_id.clone(),
            call: initialize(APP_SERVER_PROTOCOL_VERSION + 1),
        })
        .await
    else {
        panic!("unsupported version accepted")
    };
    assert_eq!(failure.id, Some(mismatch_id));
    assert_eq!(
        failure.error.data,
        Some(ErrorData::UnsupportedVersion {
            supported: APP_SERVER_PROTOCOL_VERSION,
            requested: APP_SERVER_PROTOCOL_VERSION + 1,
        })
    );
    assert!(matches!(
        call(driver, 0, initialize(APP_SERVER_PROTOCOL_VERSION)).await,
        MethodResult::Initialized {
            protocol_version: APP_SERVER_PROTOCOL_VERSION,
            ..
        }
    ));

    let (a, b) = tokio::join!(
        call(
            driver,
            1,
            Method::SessionAttach {
                session_id: sessions[0].clone(),
                node_id: None
            }
        ),
        call(
            driver,
            2,
            Method::SessionAttach {
                session_id: sessions[1].clone(),
                node_id: None
            }
        ),
    );
    let MethodResult::Attached {
        target: a,
        snapshot: snapshot_a,
        cursor: cursor_a,
    } = a
    else {
        panic!("attach A")
    };
    let MethodResult::Attached {
        target: b,
        snapshot: snapshot_b,
        cursor: cursor_b,
    } = b
    else {
        panic!("attach B")
    };
    assert_eq!(a.session_id, sessions[0]);
    assert_eq!(b.session_id, sessions[1]);
    assert_ne!(a.conversation_id, b.conversation_id);
    assert_eq!(snapshot_a.conversation_id, a.conversation_id);
    assert_eq!(snapshot_b.conversation_id, b.conversation_id);
    assert_eq!(snapshot_a.effective_approval_mode, ApprovalMode::Policy);
    assert_eq!(snapshot_b.effective_approval_mode, ApprovalMode::Policy);

    let (changed_a, unchanged_b) = tokio::join!(
        call(
            driver,
            3,
            Method::ApprovalModeSet {
                target: a.clone(),
                mode: ApprovalMode::FullAccess
            }
        ),
        call(driver, 4, Method::SessionSnapshot { target: b.clone() }),
    );
    assert!(matches!(
        changed_a,
        MethodResult::ApprovalMode {
            effective_approval_mode: ApprovalMode::FullAccess,
            pending_approval_mode: None,
            ..
        }
    ));
    let MethodResult::Snapshot { snapshot, .. } = unchanged_b else {
        panic!("snapshot B")
    };
    assert_eq!(snapshot.conversation_id, b.conversation_id);
    assert_eq!(snapshot.effective_approval_mode, ApprovalMode::Policy);

    let notification = driver.next_notification().await;
    assert_eq!(notification.jsonrpc, JsonRpcVersion::V2);
    let NotificationMethod::Event {
        target,
        cursor,
        event,
    } = notification.notification
    else {
        panic!("event A")
    };
    assert_eq!(target, a);
    assert!(cursor > cursor_a);
    assert!(matches!(
        *event,
        RuntimeClientEvent::ApprovalModeChanged {
            effective_approval_mode: ApprovalMode::FullAccess,
            pending_approval_mode: None,
            ..
        }
    ));
    let cursor_a = cursor;

    // Park the fan-in on the old registration before replacing it. A closed
    // subscription here means resubscribe, never Session residency ending.
    let parked = driver.next_notification();
    futures_util::pin_mut!(parked);
    assert!(futures_util::poll!(parked.as_mut()).is_pending());
    assert!(matches!(
        call(
            driver,
            40,
            Method::SessionSubscribe {
                target: a.clone(),
                after_cursor: cursor_a,
            }
        )
        .await,
        MethodResult::Subscribed { .. }
    ));
    let MethodResult::Boundaries {
        boundaries,
        next_offset,
        ..
    } = call(
        driver,
        41,
        Method::SessionBoundaries {
            target: a.clone(),
            offset: 0,
            limit: 32,
        },
    )
    .await
    else {
        panic!("boundary page")
    };
    assert!(boundaries.is_empty());
    assert!(next_offset.is_none());

    let (changed_a, changed_b) = tokio::join!(
        call(
            driver,
            5,
            Method::ApprovalModeSet {
                target: a.clone(),
                mode: ApprovalMode::Policy
            }
        ),
        call(
            driver,
            6,
            Method::ApprovalModeSet {
                target: b.clone(),
                mode: ApprovalMode::FullAccess
            }
        ),
    );
    assert!(matches!(
        changed_a,
        MethodResult::ApprovalMode {
            effective_approval_mode: ApprovalMode::Policy,
            ..
        }
    ));
    assert!(matches!(
        changed_b,
        MethodResult::ApprovalMode {
            effective_approval_mode: ApprovalMode::FullAccess,
            ..
        }
    ));
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..2 {
        let next = if index == 0 {
            parked.as_mut().await
        } else {
            driver.next_notification().await
        };
        let NotificationMethod::Event {
            target,
            cursor,
            event,
        } = next.notification
        else {
            panic!("routed event")
        };
        let (expected, before) = if target == a {
            (ApprovalMode::Policy, cursor_a)
        } else {
            assert_eq!(target, b);
            (ApprovalMode::FullAccess, cursor_b)
        };
        assert!(seen.insert(target.session_id));
        assert!(cursor > before);
        let RuntimeClientEvent::ApprovalModeChanged {
            effective_approval_mode,
            pending_approval_mode,
            ..
        } = *event
        else {
            panic!("approval transition")
        };
        assert_eq!(effective_approval_mode, expected);
        assert_eq!(pending_approval_mode, None);
    }

    assert!(matches!(
        call(driver, 7, Method::SessionDetach { target: a.clone() }).await,
        MethodResult::Detached {}
    ));
    let MethodResult::Attached {
        target: replacement,
        snapshot,
        ..
    } = call(
        driver,
        8,
        Method::SessionAttach {
            session_id: a.session_id.clone(),
            node_id: None,
        },
    )
    .await
    else {
        panic!("reattach A")
    };
    assert_ne!(replacement.attachment_id, a.attachment_id);
    assert_eq!(replacement.runtime_incarnation, a.runtime_incarnation);
    assert_eq!(replacement.conversation_id, a.conversation_id);
    assert_eq!(snapshot.conversation_id, a.conversation_id);
    assert_eq!(snapshot.effective_approval_mode, ApprovalMode::Policy);
    assert_eq!(
        snapshot.approval_mode_revision,
        snapshot_a.approval_mode_revision + 2
    );
    let MethodResult::Snapshot { snapshot, .. } =
        call(driver, 9, Method::SessionSnapshot { target: b.clone() }).await
    else {
        panic!("snapshot B")
    };
    assert_eq!(snapshot.conversation_id, b.conversation_id);
    assert_eq!(snapshot.effective_approval_mode, ApprovalMode::FullAccess);
    assert_eq!(
        snapshot.approval_mode_revision,
        snapshot_b.approval_mode_revision + 1
    );
    call(
        driver,
        10,
        Method::SessionDetach {
            target: replacement,
        },
    )
    .await;
    call(driver, 11, Method::SessionDetach { target: b }).await;
}

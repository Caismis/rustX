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
            call: initialize(6),
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
            requested: 6,
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
        cursor: _cursor_a,
        ..
    } = a
    else {
        panic!("attach A")
    };
    let MethodResult::Attached {
        target: b,
        snapshot: snapshot_b,
        cursor: _cursor_b,
        ..
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

    // Every carrier validates exact parent authority before native child lookup.
    let child_id = rustx::runtime::identity::AgentId::new(a.conversation_id.as_str());
    let response = driver
        .request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(374),
            call: Method::AgentTranscript {
                target: a.clone(),
                agent_id: child_id.clone(),
                before: None,
                limit: 32,
            },
        })
        .await;
    let Response::Failure(failure) = response else {
        panic!("a Conversation id is not a Subagent authority")
    };
    assert_eq!(
        failure.error.data,
        Some(ErrorData::UnknownAgent { agent_id: child_id })
    );

    // Reconciliation is native, scope identified and does not fabricate a
    // resource generation for unchanged inputs.
    for (sequence, target) in [(3, &a), (4, &b)] {
        let result = call(
            driver,
            sequence,
            Method::ConfigurationReconcile {
                target: rustx::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await;
        assert!(matches!(
            result,
            MethodResult::ConfigurationApplication { .. }
        ));
        loop {
            let notification = driver.next_notification().await;
            if let NotificationMethod::ConfigurationChanged { application } =
                notification.notification
                && application.scope == target.session_id.to_string()
                    && application.units.values().all(|unit| !matches!(unit,
                        rustx::local_runtime::configuration::application::UnitApplication::Preparing)) {
                    assert!(application.candidate.is_none());
                    break;
                }
        }
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
    let Response::Failure(stale) = driver
        .request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(80),
            call: Method::TurnCancel { target: a.clone() },
        })
        .await
    else {
        panic!("obsolete controller accepted")
    };
    assert_eq!(stale.error.data, Some(ErrorData::StaleAttachment));
    assert_ne!(replacement.attachment_id, a.attachment_id);
    assert_eq!(replacement.runtime_incarnation, a.runtime_incarnation);
    assert_eq!(replacement.conversation_id, a.conversation_id);
    assert_eq!(snapshot.conversation_id, a.conversation_id);
    assert_eq!(snapshot.effective_approval_mode, ApprovalMode::Policy);
    assert_eq!(
        snapshot.resources.revision.get(),
        snapshot_a.resources.revision.get()
    );
    let MethodResult::Snapshot { snapshot, .. } = call(
        driver,
        9,
        Method::SessionSnapshot {
            trace_records: vec![],
            target: b.clone(),
        },
    )
    .await
    else {
        panic!("snapshot B")
    };
    assert_eq!(snapshot.conversation_id, b.conversation_id);
    assert_eq!(snapshot.effective_approval_mode, ApprovalMode::Policy);
    assert_eq!(
        snapshot.resources.revision.get(),
        snapshot_b.resources.revision.get()
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

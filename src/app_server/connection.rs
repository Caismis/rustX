//! One connection negotiates once and routes independent Session attachments.
//! Only the attachment map is connection-owned; all semantic work is delegated.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::protocol::{
    APP_SERVER_PROTOCOL_VERSION, AttachmentTarget, ErrorData, Failure, InitializeParams,
    JsonRpcVersion, Method, MethodResult, Notification, NotificationMethod, Request, RequestId,
    Response, RpcError, ServerCapabilities, Success, UserInputBlock,
};
use crate::local_runtime::session::SessionId;
use crate::local_runtime::session_controller::SessionController;
use crate::local_runtime::session_runtime_manager::{ManagedRuntimeClient, RuntimeManagerError};
use crate::runtime_client::attachment::RuntimeAttachment;
use crate::runtime_client::host::EventDelivery;
use crate::runtime_client::types::{RuntimeClientError, RuntimeClientResult};

use super::host::{AppServerHost, AttachmentPermit, HostAdmissionError};

const MAX_ATTACHMENTS: usize = 32;

fn valid_request_id(id: &RequestId) -> bool {
    id.is_valid()
}

struct RouteTable {
    attachment_limit: usize,
    closed: bool,
    active: BTreeMap<SessionId, Arc<Route>>,
    reserved: std::collections::BTreeSet<SessionId>,
}

impl Default for RouteTable {
    fn default() -> Self {
        Self {
            attachment_limit: MAX_ATTACHMENTS,
            closed: false,
            active: BTreeMap::new(),
            reserved: std::collections::BTreeSet::new(),
        }
    }
}

struct AttachReservation {
    table: Arc<Mutex<RouteTable>>,
    session: SessionId,
    committed: bool,
}

impl AttachReservation {
    fn new(
        table: &Arc<Mutex<RouteTable>>,
        session: &SessionId,
        host: &AppServerHost,
    ) -> Result<Self, RpcError> {
        let mut routes = table.lock().expect("routes mutex");
        if routes.closed {
            return Err(domain(ErrorData::StaleAttachment));
        }
        if routes.active.contains_key(session) || routes.reserved.contains(session) {
            return Err(domain(ErrorData::ControllerInUse));
        }
        if routes.active.len() + routes.reserved.len() >= routes.attachment_limit {
            host.attachment_capacity_refused();
            return Err(domain(ErrorData::AttachmentCapacity));
        }
        routes.reserved.insert(session.clone());
        Ok(Self {
            table: table.clone(),
            session: session.clone(),
            committed: false,
        })
    }

    fn commit<T>(
        mut self,
        attach: impl FnOnce() -> Result<(Arc<Route>, T), RpcError>,
    ) -> Result<T, RpcError> {
        let mut routes = self.table.lock().expect("routes mutex");
        routes.reserved.remove(&self.session);
        if routes.closed {
            return Err(domain(ErrorData::StaleAttachment));
        }
        // Claim and publish under the same close boundary: a closed connection
        // must never acquire even a temporary external attachment. No await.
        let (route, result) = attach()?;
        routes.active.insert(self.session.clone(), route);
        self.committed = true;
        Ok(result)
    }
}

impl Drop for AttachReservation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        self.table
            .lock()
            .expect("routes mutex")
            .reserved
            .remove(&self.session);
    }
}

fn release_route(host: &AppServerHost, table: &Mutex<RouteTable>, route: &Arc<Route>) {
    let mut routes = table.lock().expect("routes mutex");
    if routes
        .active
        .get(&route.target.session_id)
        .is_some_and(|current| Arc::ptr_eq(current, route))
    {
        routes.active.remove(&route.target.session_id);
        route.registration.retire();
        host.revocations().revoke(|| route.attachment.detach());
        host.uploads().revoke_route(route);
        route.external.release();
        route.capacity.release();
    }
}

pub(super) struct Route {
    pub(super) target: AttachmentTarget,
    pub(super) client: ManagedRuntimeClient,
    pub(super) attachment: RuntimeAttachment,
    registration: Arc<super::registration::Slot>,
    external: crate::local_runtime::session_runtime_manager::RuntimeResidencyPin,
    capacity: AttachmentPermit,
}

/// A transport may share this connection across concurrent request handlers.
/// A single notification consumer drives fan-in directly, without pump tasks.
#[derive(Clone)]
pub struct AppServerConnection {
    host: AppServerHost,
    sessions: SessionController,
    initialized: Arc<Mutex<Option<InitializeParams>>>,
    routes: Arc<Mutex<RouteTable>>,
    changed: Arc<tokio::sync::Notify>,
    reader: Arc<tokio::sync::Mutex<()>>,
    next_route: Arc<std::sync::atomic::AtomicUsize>,
    configuration_versions: Arc<Mutex<std::collections::BTreeMap<String, u64>>>,
    /// The native Session-metadata invalidation log (Issue #386) and this
    /// connection's cursor into it.
    ///
    /// The cursor is taken at **construction**, before this connection can
    /// serve any request, so the bootstrap ordering is closed by construction:
    /// a publication older than the connection is already reflected in every
    /// read the connection can make, and a publication newer than it is
    /// delivered live. The log is level-triggered, so nothing is lost if the
    /// notification reader starts late, pauses, or never runs.
    summary_invalidations: Arc<crate::local_runtime::session::SessionSummaryInvalidations>,
    summary_invalidations_delivered: Arc<Mutex<u64>>,
    ownership_invalidations_delivered: Arc<Mutex<u64>>,
    /// The configuration sources whose automatic repair observations this
    /// connection may see (Issue #464), with what it delivered of each.
    source_repairs: Arc<Mutex<RepairInterest>>,
    /// Delivery access granted by this connection's transport authentication,
    /// never by a request. Close cancels it, retiring every admitted fence.
    delivery_access: Option<tokio_util::sync::CancellationToken>,
    /// This connection's in-flight delivery requests, by exact request id.
    deliveries: Arc<super::delivery_access::Operations>,
    associations: Arc<super::admission::Associations>,
}

/// Which source repair observations one connection may see.
///
/// The User source always; a Workspace source once this connection addressed
/// it in a request — exactly the sources its own `configuration/sourcesRead`
/// could read — so no other Workspace identity reaches it. Each admission
/// delivers the source's current level once, then every later change; the set
/// is finite and forgets the least recently addressed Workspace first, so
/// addressing it again is a new admission.
struct RepairInterest {
    delivered: std::collections::BTreeMap<
        crate::local_runtime::configuration::repair::SourceKey,
        RepairCursor,
    >,
    workspaces: std::collections::VecDeque<std::path::PathBuf>,
}

/// Where one admitted source's observations stand for its connection.
#[derive(Clone, Copy)]
enum RepairCursor {
    /// Admitted; its current level — a receipt or none — is still owed.
    Admitted,
    /// Delivered through this native ledger sequence, a level that did or did
    /// not hold a receipt: a delivered receipt the finite ledger later forgets
    /// is owed its absence, a delivered absence nothing more.
    Delivered { sequence: u64, receipt: bool },
}

/// Workspace sources one connection observes for repair notices.
const REPAIR_INTEREST_WORKSPACES: usize = 64;

impl Default for RepairInterest {
    fn default() -> Self {
        Self {
            delivered: [(
                crate::local_runtime::configuration::repair::SourceKey::User,
                RepairCursor::Admitted,
            )]
            .into(),
            workspaces: std::collections::VecDeque::new(),
        }
    }
}

impl RepairInterest {
    /// Admit one Workspace source; whether it was newly admitted. Addressing
    /// an admitted source only makes it the most recently addressed.
    fn observe(&mut self, directory: std::path::PathBuf) -> bool {
        use crate::local_runtime::configuration::repair::SourceKey;
        let admitted =
            if let Some(position) = self.workspaces.iter().position(|known| *known == directory) {
                self.workspaces.remove(position);
                false
            } else {
                self.delivered.insert(
                    SourceKey::Workspace(directory.clone()),
                    RepairCursor::Admitted,
                );
                true
            };
        self.workspaces.push_back(directory);
        if self.workspaces.len() > REPAIR_INTEREST_WORKSPACES
            && let Some(forgotten) = self.workspaces.pop_front()
        {
            self.delivered.remove(&SourceKey::Workspace(forgotten));
        }
        admitted
    }
}

/// One response and, for a delivery request, the owner of its publication
/// commit. Transports carry both through the bounded outbound queue and commit
/// at the physical write; in-process callers commit on return.
pub(crate) struct Reply {
    pub(super) response: Response,
    pub(super) publication: Option<super::delivery_access::Publication>,
    pub(crate) association: Option<super::admission::Association>,
    pub(crate) may_have_committed: bool,
    pub(crate) registration: Option<super::registration::Publication>,
}

pub(super) enum Prepared {
    Ready(Option<Box<Reply>>),
    Pending(futures_util::future::BoxFuture<'static, Option<Reply>>),
}
impl Prepared {
    fn ready(reply: Reply) -> Self {
        Self::Ready(Some(Box::new(reply)))
    }
    async fn resolve(self) -> Option<Reply> {
        match self {
            Self::Ready(reply) => reply.map(|reply| *reply),
            Self::Pending(work) => work.await,
        }
    }
}

impl Reply {
    fn plain(response: Response) -> Self {
        Self {
            response,
            publication: None,
            association: None,
            may_have_committed: false,
            registration: None,
        }
    }

    /// The produced response, before its publication decision.
    #[cfg(test)]
    pub(crate) fn produced(&self) -> &Response {
        &self.response
    }

    /// Exercise failure substitution at the writer without changing publication ownership.
    #[cfg(test)]
    pub(crate) fn force_encoding_overflow_for_test(&mut self) {
        let Response::Success(success) = &mut self.response else {
            panic!("successful read required")
        };
        let MethodResult::SessionFileBytes { data, .. } = &mut success.result else {
            panic!("delivery bytes required")
        };
        *data = "x".repeat(super::transport::MAX_MESSAGE_BYTES);
    }

    /// Commits now. A delivery cancelled or revoked earlier is replaced by
    /// its typed failure for the same request id.
    pub(crate) fn commit(self) -> Response {
        if let Some(registration) = &self.registration {
            let record =
                super::transport::serialize_record(&self.response).expect("bounded direct receipt");
            if let super::registration::Acceptance::Accepted(record, ()) =
                registration.hand_off(&record, |_| std::task::Poll::Ready(Ok(())))
            {
                return serde_json::from_str(&record).expect("bounded receipt");
            }
        }
        match &self.publication {
            Some(publication) => publication.publish(self.response),
            None => self.response,
        }
    }
}

impl AppServerConnection {
    pub(crate) fn server_draining(&self) -> bool {
        self.host.server_draining()
    }

    pub(crate) fn transport_failure(&self) {
        self.host.transport_failure();
    }

    #[cfg(test)]
    pub(crate) fn registration_for_test(
        &self,
        target: &AttachmentTarget,
    ) -> Arc<super::registration::Slot> {
        self.route(target).unwrap().registration.clone()
    }

    #[cfg(test)]
    pub(crate) fn attachment_counts(&self) -> (usize, usize) {
        let routes = self.routes.lock().expect("routes mutex");
        (routes.active.len(), routes.reserved.len())
    }

    #[cfg(test)]
    pub(crate) fn new_with_attachment_limit_for_test(host: AppServerHost, limit: usize) -> Self {
        let connection = Self::new(host);
        connection
            .routes
            .lock()
            .expect("routes mutex")
            .attachment_limit = limit;
        connection
    }

    #[must_use]
    pub fn new(host: AppServerHost) -> Self {
        let sessions = host.manager().session_controller();
        let summary_invalidations = sessions.summary_invalidations();
        let delivered = summary_invalidations.frontier();
        Self {
            summary_invalidations,
            summary_invalidations_delivered: Arc::new(Mutex::new(delivered)),
            ownership_invalidations_delivered: Arc::new(Mutex::new(delivered)),
            sessions,
            host,
            initialized: Arc::new(Mutex::new(None)),
            routes: Arc::new(Mutex::new(RouteTable::default())),
            changed: Arc::new(tokio::sync::Notify::new()),
            reader: Arc::new(tokio::sync::Mutex::new(())),
            configuration_versions: Arc::default(),
            source_repairs: Arc::default(),
            next_route: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            delivery_access: None,
            deliveries: Arc::default(),
            associations: Arc::default(),
        }
    }

    /// A connection whose transport authenticated delivery access: the owner
    /// of an explicitly delegated stdio pair, or a WebSocket client offering
    /// the separate delivery credential. Only transport composition calls
    /// this; the token is that transport authority, cancelled by revocation
    /// or close.
    #[must_use]
    pub(crate) fn with_delivery_access(
        host: AppServerHost,
        authorization: tokio_util::sync::CancellationToken,
    ) -> Self {
        let mut connection = Self::new(host);
        connection.delivery_access = Some(authorization);
        connection
    }

    fn capabilities(&self) -> ServerCapabilities {
        ServerCapabilities {
            delivery_access: self.delivery_access.is_some(),
            ..ServerCapabilities::default()
        }
    }

    /// Delivery reads/locations address only this connection's own exact
    /// attachment. Without transport-granted access nothing is looked up.
    /// The request registers under its exact id before native admission, so
    /// `delivery/cancel` on this connection can name it from then on.
    async fn delivery(
        &self,
        id: &RequestId,
        target: &AttachmentTarget,
        locator: crate::runtime_client::content::CanonicalContentLocator,
        delivery_index: usize,
        access: super::delivery_access::Access,
    ) -> (
        Result<MethodResult, RpcError>,
        Option<super::delivery_access::Publication>,
    ) {
        if let Err(error) = self.admitted() {
            return (Err(error), None);
        }
        let Some(authority) = self
            .delivery_access
            .clone()
            .filter(|token| !token.is_cancelled())
        else {
            return (Err(super::delivery_access::unauthorized()), None);
        };
        let route = match self.route(target) {
            Ok(route) => route,
            Err(error) => return (Err(error), None),
        };
        let Some(publication) = self.deliveries.register(
            id,
            &authority,
            route.clone(),
            self.host.revocations().clone(),
            #[cfg(test)]
            self.host.file_read_probe(),
        ) else {
            return (Err(domain(ErrorData::InvalidParams)), None);
        };
        #[cfg(test)]
        publication.probe().before_admission.enter().await;
        let result = super::delivery_access::request(
            self.host.clone(),
            route,
            locator,
            delivery_index,
            super::delivery_access::Roots::OriginalMapping,
            access,
            publication.token(),
        )
        .await;
        (result, Some(publication))
    }

    /// Cancels this connection's transport-granted delivery authority: every
    /// in-flight delivery request fails its next native fence, and no
    /// uncommitted delivery response is published.
    pub(crate) fn revoke_delivery_access(&self) {
        if let Some(access) = &self.delivery_access {
            self.host.revocations().revoke(|| access.cancel());
        }
    }

    /// Permanently release this connection's external claims, even while callers
    /// retain references. The route-table lock linearizes close against attach
    /// reservation/commit. Already admitted runtime operations remain server-owned.
    /// Calling close again has no effect; it never unloads or cancels a runtime.
    /// # Panics
    /// Panics if the routing mutex is poisoned.
    pub fn close(&self) {
        let mut routes = self.routes.lock().expect("routes mutex");
        routes.closed = true;
        let active = std::mem::take(&mut routes.active);
        // One revocation: the delivery authority and every attachment.
        self.host.revocations().revoke(|| {
            if let Some(access) = &self.delivery_access {
                access.cancel();
            }
            for route in active.values() {
                route.registration.retire();
                route.attachment.detach();
            }
        });
        for (_, route) in active {
            self.host.uploads().revoke_route(&route);
            route.external.release();
            route.capacity.release();
        }
        self.associations.close();
        self.changed.notify_one();
    }

    #[cfg(test)]
    pub(crate) fn closed_for_test(&self) -> bool {
        self.routes.lock().expect("routes mutex").closed
    }

    /// Decode a strict JSON-RPC request before performing any semantic action.
    /// Notifications never receive responses and do not invoke request-only methods.
    /// # Panics
    /// Panics if a connection routing mutex is poisoned.
    pub async fn handle_json(&self, json: &str) -> Option<Response> {
        Some(self.reply_json(json).await?.commit())
    }

    /// [`Self::handle_json`] without committing: the transport commits a
    /// delivery response at its physical write.
    pub(super) fn prepare_json(&self, json: &str) -> Prepared {
        match Self::decode(json) {
            Ok(Some(request)) => self.prepare_request(request),
            Ok(None) => Prepared::Ready(None),
            Err(response) => Prepared::ready(Reply::plain(response)),
        }
    }

    pub(super) async fn reply_json(&self, json: &str) -> Option<Reply> {
        self.prepare_json(json).resolve().await
    }

    fn decode(json: &str) -> Result<Option<Request>, Response> {
        if json.len() > super::transport::MAX_MESSAGE_BYTES {
            return Err(failure(None, rpc_error(-32600, "Invalid Request", None)));
        }
        let value: serde_json::Value = match serde_json::from_str(json) {
            Ok(value) => value,
            Err(_) => {
                return Err(failure(None, rpc_error(-32700, "Parse error", None)));
            }
        };
        let Some(object) = value.as_object() else {
            return Err(failure(None, rpc_error(-32600, "Invalid Request", None)));
        };
        let id = object
            .get("id")
            .and_then(|value| serde_json::from_value::<RequestId>(value.clone()).ok())
            .filter(valid_request_id);
        if object.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0")
            || !object
                .get("method")
                .is_some_and(serde_json::Value::is_string)
            || object
                .keys()
                .any(|key| !matches!(key.as_str(), "jsonrpc" | "id" | "method" | "params"))
            || (object.contains_key("id") && id.is_none())
        {
            return Err(failure(id, rpc_error(-32600, "Invalid Request", None)));
        }
        let Some(id) = id else {
            return Ok(None);
        };
        // Decoding rounds a number literal before any DTO can see it, so the
        // provider-native parameter domain is decided on the original text.
        if !super::wire::request_params_numbers_are_exact(json) {
            return Err(failure(Some(id), rpc_error(-32602, "Invalid params", None)));
        }
        // Decode the original bytes, not the Value above: materializing a Value
        // first would erase duplicate fields before typed validation.
        if let Ok(request) = serde_json::from_str::<Request>(json) {
            Ok(Some(request))
        } else {
            // Probe only the method tag, using the same derived vocabulary.
            // A closed enum mismatch inside params must never be classified
            // as an unknown method (even when its value equals the method).
            let unknown = serde_json::from_value::<Method>(serde_json::json!({
                "method": object["method"]
            }))
            .is_err_and(|error| error.to_string().starts_with("unknown variant"));
            Err(failure(
                Some(id),
                rpc_error(
                    if unknown { -32601 } else { -32602 },
                    if unknown {
                        "Method not found"
                    } else {
                        "Invalid params"
                    },
                    None,
                ),
            ))
        }
    }

    /// Correlation is preserved even when unrelated requests complete out of order.
    /// # Panics
    /// Panics if a connection routing mutex is poisoned.
    pub async fn handle_request(&self, request: Request) -> Response {
        self.reply(request).await.commit()
    }

    /// The response plus, for a delivery, its uncommitted publication.
    pub(crate) async fn reply(&self, request: Request) -> Reply {
        self.prepare_request(request)
            .resolve()
            .await
            .expect("typed request has a reply")
    }

    fn prepare_request(&self, request: Request) -> Prepared {
        let id = &request.id;
        if !id.is_valid() {
            return Prepared::ready(Reply::plain(failure(
                None,
                rpc_error(-32600, "Invalid Request", None),
            )));
        }
        if matches!(&request.call, Method::DeliveryCancel { request_id } | Method::AgentObserveCancel { request_id } if !request_id.is_valid())
            || matches!(&request.call, Method::CompactContext { request_id, .. } if !RequestId::is_valid_string(request_id.as_str()))
        {
            return Prepared::ready(Reply::plain(failure(
                Some(id.clone()),
                rpc_error(-32602, "Invalid params", None),
            )));
        }
        let association = match self.associations.admit(id, &request.call) {
            Ok(association) => association,
            Err(error) => {
                return Prepared::ready(Reply::plain(failure(Some(id.clone()), domain(error))));
            }
        };
        let connection = self.clone();
        Prepared::Pending(Box::pin(async move {
            Some(connection.execute(request, association).await)
        }))
    }

    /// Synchronous admission seam; callers deliberately control the first poll.
    #[cfg(test)]
    pub(crate) fn admitted_request_for_test(
        &self,
        request: Request,
    ) -> futures_util::future::BoxFuture<'static, Reply> {
        let prepared = self.prepare_request(request);
        Box::pin(async move { prepared.resolve().await.expect("request reply") })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "One carrier match preserves exact admission and publication ownership across every request kind"
    )]
    async fn execute(&self, request: Request, association: super::admission::Association) -> Reply {
        let may_have_committed = request.call.may_commit();
        let id = request.id;
        let (mut result, publication) = match request.call {
            call @ Method::AgentObserve { .. } => {
                let control = association
                    .observation_control()
                    .expect("observation admitted");
                let _cancel_wait_on_drop = control.cancel.clone().drop_guard();
                if control.cancel.is_cancelled() {
                    (Err(domain(ErrorData::ObservationCancelled)), None)
                } else {
                    (
                        self.dispatch(call, Some(control), Some(association.clone()))
                            .await,
                        None,
                    )
                }
            }
            Method::AgentDeliveryRead {
                target,
                agent_id,
                locator,
                delivery_index,
            } => {
                self.delivery(
                    &id,
                    &target,
                    locator,
                    delivery_index,
                    super::delivery_access::Access::AgentBytes(agent_id),
                )
                .await
            }
            Method::AgentDeliveryLocate {
                target,
                agent_id,
                locator,
                delivery_index,
            } => {
                self.delivery(
                    &id,
                    &target,
                    locator,
                    delivery_index,
                    super::delivery_access::Access::AgentLocation(agent_id),
                )
                .await
            }
            Method::DeliveryRead {
                target,
                locator,
                delivery_index,
            } => {
                self.delivery(
                    &id,
                    &target,
                    locator,
                    delivery_index,
                    super::delivery_access::Access::Bytes,
                )
                .await
            }
            Method::DeliveryLocate {
                target,
                locator,
                delivery_index,
            } => {
                self.delivery(
                    &id,
                    &target,
                    locator,
                    delivery_index,
                    super::delivery_access::Access::Location,
                )
                .await
            }
            call => (self.ordinary(call, association.clone()).await, None),
        };
        // The active wait ended; its ID remains reserved through response disposition.
        association.finish_observation();
        let registration = match &result {
            Ok(MethodResult::Subscribed {
                target,
                registration_id,
                ..
            }) => self.route(target).ok().map(|route| {
                route
                    .registration
                    .receipt_for(registration_id, &id)
                    .expect("bounded receipt error")
            }),
            _ => None,
        };
        if matches!(&result, Ok(MethodResult::Subscribed { .. })) && registration.is_none() {
            result = Err(domain(ErrorData::StaleAttachment));
        }
        let response = match result {
            Ok(result) => Response::Success(Box::new(Success {
                jsonrpc: JsonRpcVersion::V2,
                id,
                result,
            })),
            Err(error) => failure(Some(id), error),
        };
        Reply {
            response,
            publication,
            association: Some(association),
            may_have_committed,
            registration,
        }
    }

    async fn ordinary(
        &self,
        call: Method,
        association: super::admission::Association,
    ) -> Result<MethodResult, RpcError> {
        // Catalog mutations retain their owner if the protocol waiter leaves.
        // Runtime operations already transfer ownership at start_operation;
        // attachment reservations intentionally remain caller-cancellable.
        if matches!(
            &call,
            Method::SessionEditorRead { .. }
                | Method::SessionHistory { .. }
                | Method::SessionStatistics { .. }
                | Method::SessionTraceHistory { .. }
                | Method::SessionTraceHistoryDetail { .. }
                | Method::SessionTracePage { .. }
                | Method::SessionTraceValueRead { .. }
                | Method::SessionTraceContentRead { .. }
                | Method::SessionHistoryContentRead { .. }
                | Method::SessionHistoryPage { .. }
                | Method::SessionHistoryValueRead { .. }
                | Method::SessionCreate { .. }
                | Method::SessionName { .. }
                | Method::SessionDelete { .. }
                | Method::SessionFork { .. }
                | Method::SessionBranch { .. }
                | Method::SessionRecoverDeletion { .. }
                | Method::SessionSwitchNode { .. }
                | Method::SourcesWrite { .. }
                | Method::SourcesRead { .. }
                | Method::SourcesPage { .. }
                | Method::SourcesValueRead { .. }
                | Method::SettingsRead { .. }
                | Method::SettingsValueRead { .. }
                | Method::SessionApplicationValueRead { .. }
                | Method::SessionConfiguration { .. }
                | Method::ConfigurationReconcile { .. }
                | Method::AdoptConfiguration { .. }
        ) {
            let connection = self.clone();
            tokio::spawn(async move {
                Box::pin(connection.dispatch(call, None, Some(association))).await
            })
            .await
            .unwrap_or_else(|_| Err(domain(ErrorData::OperationFailed)))
        } else {
            Box::pin(self.dispatch(call, None, Some(association))).await
        }
    }

    /// Every request except `initialize` requires an open, initialized connection.
    fn admitted(&self) -> Result<(), RpcError> {
        if self.routes.lock().expect("routes mutex").closed {
            return Err(domain(ErrorData::StaleAttachment));
        }
        if self.initialized.lock().expect("initialize mutex").is_none() {
            return Err(domain(ErrorData::NotInitialized));
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn dispatch(
        &self,
        method: Method,
        observation_cancel: Option<super::admission::ObservationControl>,
        physical_association: Option<super::admission::Association>,
    ) -> Result<MethodResult, RpcError> {
        if self.routes.lock().expect("routes mutex").closed {
            return Err(domain(ErrorData::StaleAttachment));
        }
        if let Method::Initialize(params) = method {
            if params.protocol_version != APP_SERVER_PROTOCOL_VERSION {
                return Err(domain(ErrorData::UnsupportedVersion {
                    supported: APP_SERVER_PROTOCOL_VERSION,
                    requested: params.protocol_version,
                }));
            }
            if params.client.name.is_empty()
                || params.client.name.len() > 128
                || params.client.version.len() > 128
            {
                return Err(domain(ErrorData::InvalidParams));
            }
            let mut state = self.initialized.lock().expect("initialize mutex");
            if state.is_some() {
                return Err(domain(ErrorData::AlreadyInitialized));
            }
            *state = Some(params);
            drop(state);
            // Source repair notices begin with initialization; wake a parked
            // notification reader so already-recorded receipts are announced.
            self.changed.notify_one();
            return Ok(MethodResult::Initialized {
                authority_id: self.host.authority_id().to_owned(),
                protocol_version: APP_SERVER_PROTOCOL_VERSION,
                capabilities: self.capabilities(),
            });
        }
        if self.initialized.lock().expect("initialize mutex").is_none() {
            return Err(domain(ErrorData::NotInitialized));
        }
        if matches!(method, Method::ServerDiagnostics {}) {
            return Ok(MethodResult::Diagnostics {
                snapshot: self.host.diagnostics(),
            });
        }
        self.observe_repair_source(&method).await;
        if let Method::AgentObserveCancel { request_id } = method {
            return Ok(MethodResult::AgentObserveCancelled {
                accepted: self.associations.cancel_observation(&request_id),
            });
        }
        if let Method::DeliveryCancel { request_id } = method {
            if self.delivery_access.is_none() {
                return Err(super::delivery_access::unauthorized());
            }
            return Ok(MethodResult::DeliveryCancel {
                accepted: self.deliveries.cancel(&request_id),
            });
        }
        let request_lane = super::RequestLane::for_method(&method);
        if let Some(target) = runtime_target(&method) {
            let route = self.route(target)?;
            if let Method::AgentSendMessage { agent_id, .. } = &method {
                route
                    .attachment
                    .operation_authority()
                    .map_err(client_error)?
                    .check_agent_message_target(agent_id)
                    .map_err(client_error)?;
            }
            let client = route.client.clone();
            let changed = self.changed.clone();
            let sessions = self.sessions.clone();
            let manager = self.host.manager().clone();
            let upload_host = self.host.clone();
            let receiver = {
                let routes = self.routes.lock().expect("routes mutex");
                if routes.closed
                    || !routes
                        .active
                        .get(&target.session_id)
                        .is_some_and(|active| Arc::ptr_eq(active, &route))
                {
                    return Err(domain(ErrorData::StaleAttachment));
                }
                // Route close -> host drain -> residency claim: host drain
                // cannot interpose after request admission but before its native
                // operation lease. Only capture authority here; execution is
                // owned by the manager task outside these admission locks.
                self.host
                    .admit_request(request_lane, |request_owner| {
                        client.start_operation(move |operation| {
                            let configuration = matches!(
                                method,
                                Method::ConfigurationGet { .. }
                                    | Method::ConfigurationPage { .. }
                                    | Method::ConfigurationValueRead { .. }
                            )
                            .then(|| operation.configuration_source());
                            let authority = route.attachment.operation_authority();
                            async move {
                                let _request = request_owner;
                                let _physical_association = physical_association;
                                dispatch_runtime(
                                    method,
                                    route,
                                    authority.map_err(client_error)?,
                                    changed,
                                    sessions,
                                    manager,
                                    configuration,
                                    upload_host,
                                    observation_cancel,
                                )
                                .await
                            }
                        })
                    })
                    .map_err(host_error)?
                    .map_err(manager_error)?
            };
            return receiver
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?;
        }
        let _physical_association = physical_association;
        let request_owner = self
            .host
            .admit_request(request_lane, std::convert::identity)
            .map_err(host_error)?;
        match method {
            Method::SessionDetach { target } => {
                // Removing a connection relationship needs no live-runtime lease.
                let route = self.route(&target)?;
                release_route(&self.host, &self.routes, &route);
                self.changed.notify_one();
                Ok(MethodResult::Detached {})
            }
            Method::SessionDelete {
                session_id,
                expected_target_revision,
            } => {
                let manager = self.host.manager().clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                tokio::spawn(async move {
                    let _request = request_owner;
                    let result = manager
                        .delete_session(&session_id, &expected_target_revision)
                        .await
                        .map(deletion)
                        .map_err(manager_error);
                    let _ = sender.send(result);
                });
                receiver
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))?
            }
            Method::SessionSwitchNode { target, node_id } => {
                let route = self.route(&target)?;
                let manager = self.host.manager().clone();
                let host = self.host.clone();
                let routes = self.routes.clone();
                let changed = self.changed.clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                // Cleanup is server-owned even if the initiating caller goes away.
                tokio::spawn(async move {
                    let _request = request_owner;
                    let result = async {
                        manager
                            .unload_incarnation(&target.conversation_id, target.runtime_incarnation)
                            .await
                            .map_err(manager_error)?;
                        // Explicit selection is a catalog commit, unlike a cold
                        // attachment/read. Publish ownership retirement before
                        // admitting the successor runtime; failed composition
                        // leaves the committed node selected, never resurrects A.
                        let session = manager
                            .session_controller()
                            .set_current_node(&target.session_id, &node_id)
                            .await
                            .map_err(session_error)?;
                        // Node selection is committed. The successor attaches
                        // independently; observation admission must not turn a
                        // confirmed catalog mutation into an ambiguous failure.
                        Ok(MethodResult::Session { session })
                    }
                    .await;
                    release_route(&host, &routes, &route);
                    changed.notify_one();
                    let _ = sender.send(result);
                });
                receiver
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))?
            }
            Method::ServerInfo {} => Ok(MethodResult::ServerInfo {
                capabilities: self.capabilities(),
            }),
            Method::SessionExportPrepare { session_id } => {
                let download = super::archive_download::prepare(&self.host, session_id.clone())
                    .await
                    .map_err(|reason| {
                        let data = match reason {
                            crate::session_archive::SessionArchivePrepareError::UnknownSession => {
                                ErrorData::UnknownSession { session_id }
                            }
                            crate::session_archive::SessionArchivePrepareError::Busy => {
                                ErrorData::RequestCapacity
                            }
                            reason => ErrorData::ArchivePreparationFailed { reason },
                        };
                        rpc_error(-32000, &reason.to_string(), Some(data))
                    })?;
                Ok(MethodResult::SessionArchive { download })
            }
            Method::SessionList {
                query,
                offset,
                limit,
            } => {
                let page = self
                    .sessions
                    .list_sessions(query.as_deref(), offset, limit)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::Sessions {
                    sessions: page.sessions,
                    next_offset: page.next_offset,
                })
            }
            Method::SessionSummary { session_id } => Ok(MethodResult::SessionSummary {
                summary: self
                    .sessions
                    .read_session_summary(&session_id)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionHistory {
                session_id,
                node_id,
                at,
                limit,
            } => {
                if limit == 0 || limit > 32 {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (node_id, upper) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), None)
                    .await?;
                let at = history_anchor(at, upper.as_ref())?;
                let (conversation_id, window) = self
                    .sessions
                    .read_history(&session_id, Some(&node_id), at, limit)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionHistory {
                    conversation_id,
                    window,
                })
            }
            Method::SessionTraceHistory {
                session_id,
                node_id,
                before,
                limit,
            } => {
                if limit == 0 || limit > crate::runtime_client::trace::TRACE_PAGE_LIMIT {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (conversation_id, page) = self
                    .sessions
                    .read_trace(&session_id, node_id.as_ref(), move |projection| {
                        projection.page(before.as_ref(), limit)
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionTraceHistory {
                    conversation_id,
                    page,
                })
            }
            Method::SessionTraceHistoryDetail {
                session_id,
                node_id,
                record_id,
            } => {
                if record_id.len() > 256 {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (node_id, cut) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), None)
                    .await?;
                let (conversation_id, page) = self
                    .sessions
                    .read_trace_observation(&session_id, Some(&node_id), move |store| {
                        let cut = match cut {
                            Some(cut) => cut,
                            None => crate::runtime_client::trace_observation::history_cut(store)?,
                        };
                        crate::runtime_client::trace_observation::history_detail(
                            store, &cut, &record_id,
                        )
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionTraceHistoryDetail {
                    conversation_id,
                    page: page.map_err(client_error)?,
                })
            }
            Method::SessionTracePage {
                session_id,
                node_id,
                cut,
                domain: collection,
                offset,
                limit,
            } => {
                if limit == 0 || limit > 32 || offset > 9_007_199_254_740_991 {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (node_id, _) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), Some(&cut))
                    .await?;
                let (_, page) = self
                    .sessions
                    .read_trace_observation(&session_id, Some(&node_id), move |store| {
                        crate::runtime_client::trace_observation::history_page(
                            store,
                            &cut,
                            &collection,
                            offset,
                            limit,
                        )
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::CheckpointPage {
                    page: page.map_err(client_error)?,
                })
            }
            Method::SessionTraceValueRead {
                session_id,
                node_id,
                cut,
                domain: collection,
                item_offset,
                path,
                offset,
                max_bytes,
            } => {
                if path.len() > 32
                    || max_bytes == 0
                    || max_bytes > 65_536
                    || offset > 9_007_199_254_740_991
                    || item_offset > 9_007_199_254_740_991
                {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (node_id, _) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), Some(&cut))
                    .await?;
                let (_, chunk) = self
                    .sessions
                    .read_trace_observation(&session_id, Some(&node_id), move |store| {
                        crate::runtime_client::trace_observation::history_value_read(
                            store,
                            &cut,
                            &collection,
                            item_offset,
                            &path,
                            (offset, max_bytes),
                        )
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::CheckpointValueChunk {
                    chunk: chunk.map_err(client_error)?,
                })
            }
            Method::SessionTraceContentRead {
                session_id,
                node_id,
                cut,
                content_ref,
                offset,
                max_bytes,
            } => {
                self.history_content_read(
                    session_id,
                    node_id,
                    cut,
                    content_ref,
                    (offset, max_bytes),
                )
                .await
            }
            Method::SessionHistoryContentRead {
                session_id,
                node_id,
                cut,
                content_ref,
                offset,
                max_bytes,
            } => {
                // The source is the exact native historical page cut, not an ACK
                // or a new observation frontier. Reuse native cold/Live cut checks.
                let cut = history_checkpoint_cut(cut);
                return self
                    .history_content_read(
                        session_id,
                        node_id,
                        cut,
                        content_ref,
                        (offset, max_bytes),
                    )
                    .await;
            }
            Method::SessionHistoryPage {
                session_id,
                node_id,
                cut,
                domain: collection,
                offset,
                limit,
            } => {
                if limit == 0
                    || limit > 32
                    || offset > 9_007_199_254_740_991
                    || collection.len() > 1024
                {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let evidence = history_checkpoint_cut(cut);
                let (node_id, _) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), Some(&evidence))
                    .await?;
                let (_, page) = self
                    .sessions
                    .read_trace_observation(&session_id, Some(&node_id), move |store| {
                        crate::runtime_client::trace_observation::validate_history_cut(
                            store, &evidence,
                        )?;
                        crate::runtime_client::history_observation::page(
                            store,
                            &evidence,
                            &collection,
                            offset,
                            limit,
                        )
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::CheckpointPage {
                    page: page.map_err(client_error)?,
                })
            }
            Method::SessionHistoryValueRead {
                session_id,
                node_id,
                cut,
                domain: collection,
                item_offset,
                path,
                offset,
                max_bytes,
            } => {
                if path.len() > 32
                    || path.iter().any(|field| field.len() > 256)
                    || collection.len() > 1024
                    || max_bytes == 0
                    || max_bytes > 65_536
                    || offset > 9_007_199_254_740_991
                    || item_offset > 9_007_199_254_740_991
                {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let evidence = history_checkpoint_cut(cut);
                let (node_id, _) = self
                    .history_trace_cut(&session_id, node_id.as_ref(), Some(&evidence))
                    .await?;
                let (_, chunk) = self
                    .sessions
                    .read_trace_observation(&session_id, Some(&node_id), move |store| {
                        crate::runtime_client::trace_observation::validate_history_cut(
                            store, &evidence,
                        )?;
                        crate::runtime_client::history_observation::value_read(
                            store,
                            &evidence,
                            &collection,
                            item_offset,
                            &path,
                            (offset, max_bytes),
                        )
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::CheckpointValueChunk {
                    chunk: chunk.map_err(client_error)?,
                })
            }
            Method::SessionStatistics {
                session_id,
                node_id,
            } => {
                let reading = self
                    .sessions
                    .read_statistics(&session_id, node_id.as_ref())
                    .await
                    .map_err(session_error)?
                    .map_err(client_error)?;
                Ok(MethodResult::SessionStatistics {
                    conversation_id: reading.conversation_id,
                    statistics: reading.statistics,
                    occupancy: reading.occupancy,
                    read_cut: reading.read_cut,
                })
            }
            Method::SessionEditorRead {
                locator,
                block_index,
                offset,
                max_bytes,
            } => {
                super::editor_read::read(
                    self.sessions.clone(),
                    locator,
                    block_index,
                    offset,
                    max_bytes,
                )
                .await
            }
            Method::SessionRead { session_id } => Ok(MethodResult::Session {
                session: self
                    .sessions
                    .read_session(&session_id)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionCreate { settings } => {
                let result = self
                    .host
                    .manager()
                    .create_session(settings)
                    .await
                    .map_err(session_error)?;
                transition(result, &self.sessions).await
            }
            Method::SessionName { session_id, name } => Ok(MethodResult::Session {
                session: self
                    .sessions
                    .rename_session(&session_id, &name)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionNode {
                session_id,
                node_id,
            } => Ok(MethodResult::Node {
                node: self
                    .sessions
                    .observe_node(&session_id, &node_id)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionNodeOriginHistory {
                session_id,
                node_id,
                limit,
            } => {
                if limit == 0 || limit > 32 {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (_destination, origin) = self
                    .sessions
                    .acquire_read_origin(&session_id, &node_id)
                    .await
                    .map_err(session_error)?;
                let (_, upper) = self
                    .history_trace_cut(&origin.source_session, Some(&origin.source_node), None)
                    .await?;
                let at = history_anchor(
                    crate::durable::reading::ConversationWindowAt::Canonical {
                        locator: origin.source_locator,
                        cut: None,
                    },
                    upper.as_ref(),
                )?;
                let (conversation_id, window) = self
                    .sessions
                    .read_history(&origin.source_session, Some(&origin.source_node), at, limit)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionHistory {
                    conversation_id,
                    window,
                })
            }
            Method::SessionNodeValueRead {
                reference,
                offset,
                max_bytes,
            } => Ok(MethodResult::NodeValueChunk {
                chunk: self
                    .sessions
                    .node_value_read(&reference, offset, max_bytes)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionTree {
                session_id,
                offset,
                limit,
            } => {
                let (nodes, next_offset) = self
                    .sessions
                    .observe_tree(&session_id, offset, limit)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::Tree { nodes, next_offset })
            }
            Method::SessionFork {
                session_id,
                node_id,
                surface_revision,
                boundary,
                side,
            } => {
                let boundary = match boundary {
                    Some(locator) => Some(
                        self.sessions
                            .resolve_boundary(
                                &session_id,
                                node_id.as_ref(),
                                surface_revision,
                                &locator,
                                side,
                            )
                            .await
                            .map_err(session_error)?,
                    ),
                    None => None,
                };
                let result = self
                    .sessions
                    .copy_lineage(
                        &session_id,
                        node_id.as_ref(),
                        surface_revision,
                        boundary.as_ref(),
                        false,
                        side,
                    )
                    .await
                    .map_err(session_error)?;
                transition(result, &self.sessions).await
            }
            Method::SessionBranch {
                session_id,
                node_id,
                surface_revision,
                boundary,
                side,
            } => {
                let boundary = self
                    .sessions
                    .resolve_boundary(
                        &session_id,
                        Some(&node_id),
                        surface_revision,
                        &boundary,
                        side,
                    )
                    .await
                    .map_err(session_error)?;
                let result = self
                    .sessions
                    .copy_lineage(
                        &session_id,
                        Some(&node_id),
                        surface_revision,
                        Some(&boundary),
                        true,
                        side,
                    )
                    .await
                    .map_err(session_error)?;
                transition(result, &self.sessions).await
            }
            Method::SessionDeletePreview { session_id } => {
                Ok(deletion(self.sessions.delete_preview(&session_id).await))
            }
            Method::SessionRecoverDeletion { session_id } => {
                let manager = self.host.manager().clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                tokio::spawn(async move {
                    let _request = request_owner;
                    let result = manager
                        .recover_session_deletion(&session_id)
                        .await
                        .map(deletion)
                        .map_err(manager_error);
                    let _ = sender.send(result);
                });
                receiver
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))?
            }
            Method::ConfigurationReconcile { target } => {
                let application = self
                    .host
                    .manager()
                    .reconcile_configuration(&target)
                    .await
                    .map_err(source_settings_error)?;
                Ok(MethodResult::ConfigurationReceipt {
                    scope: application.scope,
                    version: application.version,
                })
            }
            Method::AdoptConfiguration {
                session_id,
                candidate,
                expected_binding,
            } => {
                let application = self
                    .host
                    .manager()
                    .adopt_configuration(&session_id, &candidate, expected_binding)
                    .map_err(|rejection| domain(ErrorData::ConfigurationAdoption { rejection }))?;
                Ok(MethodResult::ConfigurationReceipt {
                    scope: application.scope,
                    version: application.version,
                })
            }
            Method::SessionConfiguration { session_id } => {
                self.sessions
                    .read_session_summary(&session_id)
                    .await
                    .map_err(session_error)?;
                let application = self.host.manager().configuration_application(&session_id);
                let projection = tokio::task::spawn_blocking(move || {
                    application
                        .as_ref()
                        .map(crate::local_runtime::configuration::observation::session_application)
                        .transpose()
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::SessionConfiguration { projection })
            }
            Method::SessionApplicationValueRead {
                session_id,
                reference,
                offset,
                max_bytes,
            } => {
                self.sessions
                    .read_session_summary(&session_id)
                    .await
                    .map_err(session_error)?;
                let application = self
                    .host
                    .manager()
                    .configuration_application(&session_id)
                    .ok_or_else(|| domain(ErrorData::ContentSourceUnavailable))?;
                let chunk = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::session_application_read(
                        &application,
                        &reference,
                        offset,
                        max_bytes,
                    )
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::SessionApplicationValueChunk { chunk })
            }
            Method::SourcesRead { target } => {
                let projection = self
                    .host
                    .manager()
                    .source_settings(&target)
                    .await
                    .map_err(source_settings_error)?;
                let projection = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::bounded(&projection)
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|_| {
                    domain(ErrorData::ObservationUnavailable {
                        diagnostic: "source projection unavailable".into(),
                    })
                })?;
                Ok(MethodResult::SourceSettings {
                    projection: Box::new(projection),
                })
            }
            Method::SourcesPage {
                target,
                revision,
                field,
                offset,
                limit,
            } => {
                let source = self
                    .host
                    .manager()
                    .source_settings(&target)
                    .await
                    .map_err(source_settings_error)?;
                let page = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::page(
                        &source, &revision, field, offset, limit,
                    )
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::SourcePage { page })
            }
            Method::SourcesValueRead {
                target,
                reference,
                offset,
                max_bytes,
            } => {
                let source = self
                    .host
                    .manager()
                    .source_settings(&target)
                    .await
                    .map_err(source_settings_error)?;
                let chunk = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::read(
                        &source, &reference, offset, max_bytes,
                    )
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::SourceValueChunk { chunk })
            }
            Method::McpProbe {
                target,
                id,
                expected_revision,
            } => {
                let result = self
                    .host
                    .manager()
                    .probe_mcp(target, id, expected_revision)
                    .await
                    .map_err(source_error)?;
                Ok(MethodResult::McpProbe { result })
            }
            Method::SourcesWrite {
                target,
                expected_revision,
                mutation,
            } => {
                let receipt = self
                    .host
                    .manager()
                    .write_source_settings(&target, expected_revision, mutation)
                    .await
                    .map_err(source_settings_error)?;
                Ok(MethodResult::SourcesWritten {
                    revision: receipt.revision,
                    configuration_revision: receipt.configuration_revision,
                })
            }
            Method::SettingsRead { session_id } => {
                let (revision, settings) = self
                    .sessions
                    .read_settings(&session_id)
                    .await
                    .map_err(session_error)?;
                let projection = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::session_settings(
                        revision, &settings,
                    )
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::Settings { projection })
            }
            Method::SettingsValueRead {
                session_id,
                reference,
                offset,
                max_bytes,
            } => {
                let (revision, settings) = self
                    .sessions
                    .read_settings(&session_id)
                    .await
                    .map_err(session_error)?;
                let chunk = tokio::task::spawn_blocking(move || {
                    crate::local_runtime::configuration::observation::session_settings_read(
                        revision, &settings, &reference, offset, max_bytes,
                    )
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
                .map_err(|error| source_read_error(&error))?;
                Ok(MethodResult::SettingsValueChunk { chunk })
            }
            Method::SessionAttachmentStatus { session_id } => {
                let target = self
                    .routes
                    .lock()
                    .expect("routes mutex")
                    .active
                    .get(&session_id)
                    .map(|route| route.target.clone());
                Ok(MethodResult::AttachmentStatus {
                    target,
                    configuration_revision: self
                        .host
                        .manager()
                        .configuration_application(&session_id)
                        .map(|application| application.version),
                })
            }
            Method::SessionAttach {
                session_id,
                node_id,
            } => {
                let capacity = self.host.admit_attachment().map_err(host_error)?;
                let reservation = AttachReservation::new(&self.routes, &session_id, &self.host)?;
                // Reservation is request-scoped; once claimed, Loading is
                // manager-scoped. Dropping this request does not roll it back.
                let runtime = self
                    .host
                    .manager()
                    .load(&session_id, node_id.as_ref())
                    .await
                    .map_err(manager_error)?;
                let result = reservation.commit(|| {
                    let client = runtime.client();
                    let (attachment, external) = client.attach().map_err(manager_error)?;
                    let target = AttachmentTarget {
                        session_id: session_id.clone(),
                        conversation_id: runtime.conversation_id().clone(),
                        runtime_incarnation: runtime.incarnation_id(),
                        attachment_id: attachment.attachment_id().clone(),
                    };
                    Ok((
                        Arc::new(Route {
                            target: target.clone(),
                            client,
                            attachment,
                            registration: Arc::default(),
                            external,
                            capacity,
                        }),
                        MethodResult::Attached {
                            configuration_revision: self
                                .host
                                .manager()
                                .configuration_application(&session_id)
                                .map(|application| application.version),
                            attachment_kind: super::protocol::AttachmentKind::Controller,
                            observation_source: super::protocol::ObservationSource::Live,
                            target,
                        },
                    ))
                })?;
                if let MethodResult::Attached { target, .. } = &result {
                    self.host.register_file_route(&self.route(target)?);
                }
                self.changed.notify_one();
                Ok(result)
            }
            _ => Err(domain(ErrorData::OperationFailed)),
        }
    }

    async fn history_content_read(
        &self,
        session_id: crate::local_runtime::session::SessionId,
        node_id: Option<crate::local_runtime::session::SessionNodeId>,
        cut: crate::runtime_client::checkpoint::CheckpointReadCut,
        content_ref: crate::runtime_client::content::ContentRef,
        range: (u64, u32),
    ) -> Result<MethodResult, RpcError> {
        let (offset, max_bytes) = range;
        if offset > 9_007_199_254_740_991
            || max_bytes == 0
            || max_bytes > 65_536
            || matches!(
                content_ref.source,
                crate::runtime_client::content::ContentSource::LivePublicationPrefix { .. }
            )
        {
            return Err(domain(ErrorData::InvalidParams));
        }
        let (node_id, _) = self
            .history_trace_cut(&session_id, node_id.as_ref(), Some(&cut))
            .await?;
        let (_, chunk) = self
            .sessions
            .read_trace_observation(&session_id, Some(&node_id), move |store| {
                // Native source validation checks Conversation ownership,
                // immutable cut, representation and committed source frontier.
                crate::runtime_client::trace_observation::validate_history_cut(store, &cut)?;
                let source = cut
                    .source_cut
                    .as_ref()
                    .ok_or(RuntimeClientError::ReadCutExpired)?;
                store
                    .validate_content_source_cut(&content_ref, source)
                    .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?;
                store
                    .content_read(&content_ref, offset, max_bytes)
                    .map_err(RuntimeClientError::content_read_error)
            })
            .await
            .map_err(session_error)?;
        Ok(MethodResult::ContentChunk {
            chunk: chunk.map_err(client_error)?,
        })
    }

    async fn history_trace_cut(
        &self,
        session: &crate::local_runtime::session::SessionId,
        node: Option<&crate::local_runtime::session::SessionNodeId>,
        requested: Option<&crate::runtime_client::checkpoint::CheckpointReadCut>,
    ) -> Result<
        (
            crate::local_runtime::session::SessionNodeId,
            Option<crate::runtime_client::checkpoint::CheckpointReadCut>,
        ),
        RpcError,
    > {
        let selected = match node {
            Some(node) => node.clone(),
            None => {
                self.sessions
                    .read_session_summary(session)
                    .await
                    .map_err(session_error)?
                    .active_node
            }
        };
        let node = self
            .sessions
            .observe_node(session, &selected)
            .await
            .map_err(session_error)?;
        if requested.is_some_and(|cut| {
            cut.conversation_id != node.conversation_id || cut.activation_id.is_some()
        }) {
            return Err(domain(ErrorData::ReadCutExpired));
        }
        let manager = self.host.manager().clone();
        let upper = tokio::task::spawn_blocking(move || {
            manager.history_observation_cut(&node.conversation_id)
        })
        .await
        .map_err(|_| domain(ErrorData::OperationFailed))?
        .map_err(manager_error)?;
        if let (Some(requested), Some(upper)) = (requested, &upper) {
            let source = requested
                .source_cut
                .as_ref()
                .ok_or_else(|| domain(ErrorData::ReadCutExpired))?;
            let native = upper
                .source_cut
                .as_ref()
                .ok_or_else(|| domain(ErrorData::ReadCutExpired))?;
            if requested.through > upper.through
                || requested.revision > upper.revision
                || !source.read.reconstructible_from(&native.read)
                || source.inherited_through > native.inherited_through
            {
                return Err(domain(ErrorData::ReadCutExpired));
            }
        }
        Ok((selected, upper))
    }

    fn route(&self, target: &AttachmentTarget) -> Result<Arc<Route>, RpcError> {
        let route = self
            .routes
            .lock()
            .expect("routes mutex")
            .active
            .get(&target.session_id)
            .cloned()
            .ok_or_else(|| domain(ErrorData::StaleAttachment))?;
        if route.target != *target {
            return Err(domain(ErrorData::StaleAttachment));
        }
        Ok(route)
    }

    /// Admit the Workspace source a request addresses into this connection's
    /// repair observations, before the request runs: a Session open that
    /// fails on an unrepairable source still announces why.
    ///
    /// Admission is the linearization point: under the interest lock the
    /// source enters owing its current level, and the connection's
    /// notification wakeup is signalled after the lock is released. A parked
    /// reader therefore rechecks and delivers that level with no ledger
    /// change; a reader between its check and its wait keeps the stored
    /// permit. Re-addressing an admitted source changes nothing it delivers.
    async fn observe_repair_source(&self, method: &Method) {
        let directory = match method {
            Method::SourcesRead { target }
            | Method::SourcesWrite { target, .. }
            | Method::ConfigurationReconcile { target }
            | Method::McpProbe { target, .. } => target
                .validate()
                .ok()
                .and_then(|()| target.workspace().map(std::path::Path::to_path_buf)),
            Method::SessionCreate { settings } => std::fs::canonicalize(&settings.cwd).ok(),
            Method::SessionAttach { session_id, .. } => self
                .sessions
                .read_settings(session_id)
                .await
                .ok()
                .and_then(|(_, settings)| std::fs::canonicalize(&settings.cwd).ok()),
            _ => None,
        };
        if let Some(directory) = directory.filter(|directory| directory.is_dir()) {
            let admitted = self
                .source_repairs
                .lock()
                .expect("source repair interest")
                .observe(directory);
            if admitted {
                self.changed.notify_one();
            }
        }
    }

    /// The next source repair observation this connection has not delivered.
    /// Level-triggered against the native ledger and per-source cursors.
    ///
    /// A newly admitted source delivers its current level — its receipt, or
    /// none, even when the ledger has forgotten it or never held it — at the
    /// ledger sequence it was read at; every later change of it follows. The
    /// level and that sequence are read under one ledger lock, and the cursor
    /// is set to it while the interest lock is held throughout, so a change
    /// committed after that read carries a higher sequence and is delivered
    /// next, and one committed before it is part of the level: nothing is
    /// lost, stale or delivered twice.
    ///
    /// A source this connection keeps observing may still lose its ledger
    /// entry, which the finite ledger forgets only while recording another
    /// source and so advancing the sequence. When the level last delivered
    /// held a receipt, that absence is delivered once, at the sequence it was
    /// read at, so no client keeps a receipt the ledger forgot; a delivered
    /// absence is not repeated however the ledger advances. A later entry of
    /// the source is newer than both and follows.
    pub(crate) fn next_source_repair(&self) -> Option<NotificationMethod> {
        use crate::local_runtime::configuration::repair::{SourceChange, SourceKey};
        use crate::local_runtime::configuration::settings::SourceTarget;
        // A v48 notice reaches only a connection that negotiated v48.
        if self.initialized.lock().expect("initialize mutex").is_none() {
            return None;
        }
        let ledger = self.host.manager().source_repairs();
        let mut interest = self.source_repairs.lock().expect("source repair interest");
        for (key, cursor) in &mut interest.delivered {
            let (sequence, receipt) = match *cursor {
                RepairCursor::Admitted => ledger.level(key),
                RepairCursor::Delivered {
                    sequence: delivered,
                    receipt: held,
                } => match ledger.changed_after(key, delivered) {
                    Some(SourceChange::Entry(sequence, receipt)) => (sequence, receipt),
                    Some(SourceChange::Absent(sequence)) if held && sequence > delivered => {
                        (sequence, None)
                    }
                    _ => continue,
                },
            };
            *cursor = RepairCursor::Delivered {
                sequence,
                receipt: receipt.is_some(),
            };
            let target = match key {
                SourceKey::User => SourceTarget::User,
                SourceKey::Workspace(directory) => SourceTarget::Workspace {
                    directory: directory.clone(),
                },
            };
            return Some(NotificationMethod::SourceRepairChanged {
                target,
                receipt,
                sequence,
            });
        }
        None
    }

    /// Whether this connection still observes `target`'s repairs.
    #[cfg(test)]
    pub(crate) fn observes_repairs_of(
        &self,
        target: &crate::local_runtime::configuration::settings::SourceTarget,
    ) -> bool {
        self.source_repairs
            .lock()
            .expect("source repair interest")
            .delivered
            .contains_key(&target.into())
    }

    /// The next configuration application this connection has not delivered yet.
    ///
    /// Level-triggered against the manager's current applications and this
    /// connection's per-scope version cursor.
    fn next_configuration_change(&self) -> Option<NotificationMethod> {
        let mut versions = self
            .configuration_versions
            .lock()
            .expect("configuration notification versions");
        let application = self
            .host
            .manager()
            .next_configuration_application(&versions)?;
        let projection =
            crate::local_runtime::configuration::observation::session_application(&application)
                .ok()?;
        versions.insert(application.scope, application.version);
        Some(NotificationMethod::ConfigurationChanged {
            application: projection,
        })
    }

    fn next_ownership_invalidation(&self) -> Option<NotificationMethod> {
        let mut delivered = self
            .ownership_invalidations_delivered
            .lock()
            .expect("ownership invalidation cursor");
        let (sequence, session_id, retired_through) = self
            .summary_invalidations
            .next_ownership_after(*delivered)?;
        *delivered = sequence;
        Some(NotificationMethod::OwnershipRetired {
            session_id,
            retired_through: retired_through.to_string(),
        })
    }

    fn next_summary_invalidation(&self) -> Option<NotificationMethod> {
        let mut delivered = self
            .summary_invalidations_delivered
            .lock()
            .expect("summary invalidation cursor");
        let (sequence, session_id, catalog_changed) =
            self.summary_invalidations.next_after(*delivered)?;
        *delivered = sequence;
        Some(NotificationMethod::SummaryInvalidated {
            session_id,
            catalog_changed,
        })
    }

    /// The next level-triggered notification owed outside any attachment, in
    /// priority order: Session metadata, configuration application, then
    /// configuration source repair.
    fn next_level_notification(&self) -> Option<NotificationMethod> {
        self.next_ownership_invalidation()
            .or_else(|| self.next_summary_invalidation())
            .or_else(|| self.next_configuration_change())
            .or_else(|| self.next_source_repair())
    }

    /// Wait for one routed notification. No per-Session event queues or pump tasks.
    /// # Panics
    /// Panics if a connection routing mutex is poisoned.
    pub async fn next_notification(&self) -> Notification {
        loop {
            let (notification, publication) = self.next_notification_candidate().await;
            if let Some(publication) = publication {
                let record = super::transport::serialize_record(&notification)
                    .expect("bounded native notification");
                if matches!(
                    publication.hand_off(&record, |_| std::task::Poll::Ready(Ok(()))),
                    super::registration::Acceptance::Discarded
                ) {
                    continue;
                }
            }
            return notification;
        }
    }

    pub(crate) async fn next_notification_candidate(
        &self,
    ) -> (Notification, Option<super::registration::Publication>) {
        let _reader = self.reader.lock().await;
        let mut configuration_changes = self.host.manager().configuration_changes();
        let mut summary_invalidations = self.summary_invalidations.changes();
        let mut source_repairs = self.host.manager().source_repairs().subscribe();
        loop {
            if let Some(notification) = self.next_level_notification() {
                return (
                    Notification {
                        jsonrpc: JsonRpcVersion::V2,
                        notification,
                    },
                    None,
                );
            }
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let mut routes: Vec<_> = self
                .routes
                .lock()
                .expect("routes mutex")
                .active
                .values()
                .cloned()
                .collect();
            if routes.is_empty() {
                tokio::select! {
                    () = &mut changed => {},
                    _ = configuration_changes.changed() => {},
                    _ = summary_invalidations.changed() => {},
                    _ = source_repairs.changed() => {},
                }
                continue;
            }
            // A continuously ready Session cannot starve other attachments.
            let start = self
                .next_route
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                % routes.len();
            routes.rotate_left(start);
            let pending: Vec<_> = routes
                .into_iter()
                .map(|route| {
                    Box::pin(async move {
                        // The exact registration this poll parks on travels with
                        // its delivery: a `Closed` only ends residency if it is
                        // still the attachment's current registration.
                        let (registration, subscription) =
                            route.registration.wait_following().await;
                        let delivery = if route.client.validate().is_ok() {
                            subscription.next().await
                        } else {
                            EventDelivery::Closed
                        };
                        (route, delivery, Some(subscription), registration)
                    })
                })
                .collect();
            let (route, delivery, observed, registration) = tokio::select! {
                () = &mut changed => continue,
                _ = configuration_changes.changed() => continue,
                _ = summary_invalidations.changed() => continue,
                _ = source_repairs.changed() => continue,
                result = futures_util::future::select_all(pending) => result.0,
            };
            if let Some(candidate) =
                self.routed_notification(&route, delivery, observed, registration)
            {
                return candidate;
            }
        }
    }
    fn routed_notification(
        &self,
        route: &Arc<Route>,
        delivery: EventDelivery,
        observed: Option<crate::runtime_client::host::EventSubscription>,
        registration: super::registration::Registration,
    ) -> Option<(Notification, Option<super::registration::Publication>)> {
        let target = route.target.clone();
        let mut publication = None;
        let notification = match delivery {
            EventDelivery::Event(event) => {
                let registration_id = registration.id.clone();
                publication = Some(registration.event(event.cursor));
                NotificationMethod::Event {
                    registration_id,
                    target,
                    cursor: event.cursor,
                    event: Box::new(event.event),
                }
            }
            EventDelivery::ResyncRequired {
                after_cursor: _,
                earliest_serviceable,
            } => {
                let registration_id = registration.id.clone();
                let (recovery, after_cursor) = registration.recovery();
                publication = Some(recovery);
                NotificationMethod::ResyncRequired {
                    registration_id,
                    target,
                    reason: super::protocol::ResyncReason::ReplayUnavailable,
                    checkpoint_required: true,
                    failed_at_cursor: None,
                    after_cursor,
                    earliest_serviceable,
                }
            }
            EventDelivery::Closed
                if observed
                    .is_some_and(|subscription| route.attachment.superseded(&subscription)) =>
            {
                // A resync replaced this registration while the consumer was
                // parked on it. The attachment is alive and has a newer
                // registration; retiring its route here would end the
                // attachment that asked for the repair.
                return None;
            }
            EventDelivery::Closed | EventDelivery::Exhausted => {
                release_route(&self.host, &self.routes, route);
                NotificationMethod::Closed { target }
            }
            EventDelivery::Pending => unreachable!("async delivery never returns Pending"),
        };
        Some((
            Notification {
                jsonrpc: JsonRpcVersion::V2,
                notification,
            },
            publication,
        ))
    }
}

impl Drop for AppServerConnection {
    fn drop(&mut self) {
        if Arc::strong_count(&self.initialized) == 1 {
            self.close();
        }
    }
}

#[allow(clippy::too_many_lines)] // Exhaustive public RPC routing keeps new variants from silently bypassing their owner.
fn runtime_target(method: &Method) -> Option<&AttachmentTarget> {
    match method {
        Method::ConfigurationGet { target, .. }
        | Method::ConfigurationValueRead { target, .. }
        | Method::ConfigurationPage { target, .. }
        | Method::ModelGet { target, .. }
        | Method::ModelCatalog { target, .. }
        | Method::ModelSet { target, .. }
        | Method::ModelSelect { target, .. }
        | Method::Capability { target, .. }
        | Method::AgentArtifactRead { target, .. }
        | Method::ArtifactRead { target, .. }
        | Method::SessionUploadPrepare { target, .. }
        | Method::SessionUploadStatus { target, .. }
        | Method::Trace { target, .. }
        | Method::TraceDetail { target, .. }
        | Method::TraceLocateTool { target, .. }
        | Method::Transcript { target, .. }
        | Method::ConversationTurns { target, .. }
        | Method::Goal { target, .. }
        | Method::JobStatus { target, .. }
        | Method::JobList { target, .. }
        | Method::JobWait { target, .. }
        | Method::JobCancel { target, .. }
        | Method::AgentStatus { target, .. }
        | Method::AgentList { target, .. }
        | Method::AgentObserve { target, .. }
        | Method::AgentStatistics { target, .. }
        | Method::AgentSendMessage { target, .. }
        | Method::AgentWait { target, .. }
        | Method::AgentInterrupt { target, .. }
        | Method::AgentTrace { target, .. }
        | Method::AgentTraceDetail { target, .. }
        | Method::AgentTraceLocateTool { target, .. }
        | Method::AgentTranscript { target, .. }
        | Method::AgentTurns { target, .. }
        | Method::CompactContext { target, .. }
        | Method::SessionBoundaries { target, .. }
        | Method::SessionCheckpoint { target, .. }
        | Method::ConversationContentRead { target, .. }
        | Method::ConversationCheckpointPage { target, .. }
        | Method::ConversationCheckpointValueRead { target, .. }
        | Method::SessionSubscribe { target, .. }
        | Method::TurnStart { target, .. }
        | Method::TurnSteer { target, .. }
        | Method::InboundEdit { target, .. }
        | Method::InboundRemove { target, .. }
        | Method::TurnCancel { target, .. }
        | Method::InteractionRespond { target, .. }
        | Method::InteractionCancel { target, .. } => Some(target),
        Method::AgentDeliveryRead { .. }
        | Method::AgentDeliveryLocate { .. }
        | Method::DeliveryRead { .. }
        | Method::DeliveryLocate { .. }
        | Method::DeliveryCancel { .. }
        | Method::SessionSwitchNode { .. }
        | Method::AgentObserveCancel { .. }
        | Method::Initialize(_)
        | Method::ServerInfo { .. }
        | Method::ServerDiagnostics { .. }
        | Method::SessionExportPrepare { .. }
        | Method::SessionList { .. }
        | Method::SessionCreate { .. }
        | Method::SessionRead { .. }
        | Method::SessionHistory { .. }
        | Method::SessionTraceHistory { .. }
        | Method::SessionTraceHistoryDetail { .. }
        | Method::SessionTracePage { .. }
        | Method::SessionTraceValueRead { .. }
        | Method::SessionTraceContentRead { .. }
        | Method::SessionHistoryContentRead { .. }
        | Method::SessionHistoryPage { .. }
        | Method::SessionHistoryValueRead { .. }
        | Method::SessionStatistics { .. }
        | Method::SessionSummary { .. }
        | Method::SessionName { .. }
        | Method::SessionNode { .. }
        | Method::SessionNodeOriginHistory { .. }
        | Method::SessionNodeValueRead { .. }
        | Method::SessionTree { .. }
        | Method::SessionFork { .. }
        | Method::SessionBranch { .. }
        | Method::SessionDeletePreview { .. }
        | Method::SessionDelete { .. }
        | Method::SessionRecoverDeletion { .. }
        | Method::SessionAttach { .. }
        | Method::SessionEditorRead { .. }
        | Method::SessionDetach { .. }
        | Method::SessionAttachmentStatus { .. }
        | Method::SourcesRead { .. }
        | Method::McpProbe { .. }
        | Method::SourcesPage { .. }
        | Method::SourcesValueRead { .. }
        | Method::SourcesWrite { .. }
        | Method::SettingsRead { .. }
        | Method::SettingsValueRead { .. }
        | Method::ConfigurationReconcile { .. }
        | Method::SessionApplicationValueRead { .. }
        | Method::SessionConfiguration { .. }
        | Method::AdoptConfiguration { .. } => None,
    }
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // Explicit native operation owners.
async fn dispatch_runtime(
    method: Method,
    route: Arc<Route>,
    authority: Arc<crate::runtime_client::host::ClientInner>,
    changed: Arc<tokio::sync::Notify>,
    sessions: SessionController,
    manager: crate::local_runtime::session_runtime_manager::SessionRuntimeManager,
    configuration: Option<
        crate::local_runtime::session_runtime_manager::ConfigurationObservationSource,
    >,
    upload_host: AppServerHost,
    observation_cancel: Option<super::admission::ObservationControl>,
) -> Result<MethodResult, RpcError> {
    match method {
        Method::ConfigurationGet { target } => {
            let projection = tokio::task::spawn_blocking(move || {
                let view = configuration
                    .as_ref()
                    .and_then(|source| source.read(&manager, &target.session_id))
                    .ok_or("configuration source unavailable".to_owned())?;
                crate::local_runtime::configuration::effective_observation::bounded(&view)
            })
            .await
            .map_err(|_| domain(ErrorData::OperationFailed))?
            .map_err(|error| source_read_error(&error))?;
            Ok(MethodResult::EffectiveConfiguration {
                projection: Box::new(projection),
            })
        }
        Method::ConfigurationValueRead {
            target,
            reference,
            offset,
            max_bytes,
        } => {
            let chunk = tokio::task::spawn_blocking(move || {
                let view = configuration
                    .as_ref()
                    .and_then(|source| source.read(&manager, &target.session_id))
                    .ok_or("configuration source unavailable".to_owned())?;
                crate::local_runtime::configuration::effective_observation::read(
                    &view, &reference, offset, max_bytes,
                )
            })
            .await
            .map_err(|_| domain(ErrorData::OperationFailed))?
            .map_err(|error| source_read_error(&error))?;
            Ok(MethodResult::ConfigurationValueChunk { chunk })
        }
        Method::ConfigurationPage {
            target,
            revision,
            field,
            offset,
            limit,
        } => {
            let page = tokio::task::spawn_blocking(move || {
                let view = configuration
                    .as_ref()
                    .and_then(|source| source.read(&manager, &target.session_id))
                    .ok_or("configuration source unavailable".to_owned())?;
                crate::local_runtime::configuration::effective_observation::page(
                    &view, &revision, field, offset, limit,
                )
            })
            .await
            .map_err(|_| domain(ErrorData::OperationFailed))?
            .map_err(|error| source_read_error(&error))?;
            Ok(MethodResult::ConfigurationPage { page })
        }
        Method::AgentArtifactRead {
            agent_id,
            artifact_id,
            ..
        } => Ok(MethodResult::ArtifactBytes {
            data: authority
                .agent_artifact_read(&agent_id, &artifact_id)
                .map_err(client_error)?,
        }),
        Method::ArtifactRead {
            target: _,
            artifact_id,
        } => Ok(MethodResult::ArtifactBytes {
            data: authority
                .artifact_read(&artifact_id)
                .map_err(client_error)?,
        }),
        Method::SessionUploadPrepare {
            target: _,
            operation_id,
            files,
        } => {
            let transfer =
                super::upload_transfer::prepare(&upload_host, route, operation_id, files)
                    .await
                    .map_err(|_| domain(ErrorData::InvalidParams))?;
            Ok(MethodResult::UploadPrepared { transfer })
        }
        Method::SessionUploadStatus {
            target,
            operation_id,
        } => {
            let outcome =
                super::upload_transfer::status(&upload_host, &target.session_id, &operation_id)
                    .await
                    .map_err(|_| domain(ErrorData::InvalidParams))?;
            Ok(MethodResult::UploadStatus { outcome })
        }
        Method::ModelGet { target: _ } => read_domain(authority, "model".into()).await,
        Method::ModelCatalog { target: _ } => {
            read_domain(authority, "model_catalog.models".into()).await
        }
        Method::ModelSet { target, config } => {
            let (_, settings_revision) = manager
                .set_model(&target.session_id, *config)
                .await
                .map_err(model_selection_error)?;
            Ok(MethodResult::ModelSet { settings_revision })
        }
        Method::ModelSelect { target, choice } => {
            let (_, settings_revision) = manager
                .select_catalog_model(&target.session_id, choice)
                .await
                .map_err(model_selection_error)?;
            Ok(MethodResult::ModelSet { settings_revision })
        }
        Method::Capability { target: _ } => read_domain(authority, "capabilities".into()).await,
        Method::Trace {
            target: _,
            records,
            before,
            limit,
        } => native_result(authority.trace_page(before, limit, records)),
        Method::TraceDetail {
            target: _,
            record_id,
        } => native_result(authority.trace_detail(record_id)),
        Method::TraceLocateTool { locator, .. } => {
            native_result(authority.trace_locate_tool(locator).await)
        }
        Method::Transcript {
            target: _,
            at,
            limit,
        } => Ok(MethodResult::TranscriptWindow {
            window: authority
                .conversation_window(&at, limit)
                .map_err(client_error)?,
        }),
        Method::ConversationTurns {
            target: _,
            offset,
            limit,
        } => Ok(MethodResult::ConversationTurns {
            page: authority
                .conversation_turns(offset, limit)
                .map_err(client_error)?,
        }),
        Method::Goal {
            control: crate::goal::GoalControl::Show,
            ..
        } => read_domain(authority, "goal".into()).await,
        Method::Goal { target: _, control } => native_result(authority.goal_control(control)),
        Method::JobStatus { job_id, .. } => {
            read_domain(authority, format!("jobs.{}", job_id.as_str())).await
        }
        Method::JobList { .. } => read_domain(authority, "jobs".into()).await,
        Method::JobWait { job_id, .. } => native_result(authority.job_wait(&job_id).await),
        Method::JobCancel { job_id, .. } => native_result(authority.job_cancel_intent(&job_id)),
        Method::AgentObserve {
            agent_id, after, ..
        } => {
            let control = observation_cancel.expect("Observe owns synchronous request admission");
            let read = authority.agent_observe_with_deadline(&agent_id, after, control.deadline);
            let cancel = control.cancel;
            {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => Err(domain(ErrorData::ObservationCancelled)),
                    result = read => native_result(result),
                }
            }
        }
        Method::AgentStatus { agent_id, .. } => {
            read_domain(authority, format!("agents.{}", agent_id.as_str())).await
        }
        Method::AgentList { .. } => read_domain(authority, "agents".into()).await,
        Method::AgentSendMessage {
            target,
            agent_id,
            message,
            attachments,
        } => {
            authority.agent_status(&agent_id).map_err(client_error)?;
            let uploads = manager
                .session_controller()
                .uploaded_content(&target.session_id, &attachments)
                .await
                .map_err(|_| domain(ErrorData::InvalidParams))?;
            let references = uploads
                .into_iter()
                .filter_map(|block| match block {
                    crate::message::types::UserContentBlock::UploadedFile(file) => Some(file),
                    _ => None,
                })
                .collect();
            native_result(
                authority
                    .agent_send_message(&agent_id, message, references)
                    .await,
            )
        }
        Method::AgentWait { agent_id, .. } => native_result(authority.agent_wait(&agent_id).await),
        Method::AgentInterrupt { agent_id, .. } => {
            native_result(authority.agent_interrupt_intent(&agent_id))
        }
        Method::AgentStatistics { agent_id, .. } => {
            native_result(authority.agent_statistics(&agent_id).await)
        }
        Method::AgentTrace {
            agent_id,
            before,
            limit,
            records,
            ..
        } => native_result(
            authority
                .agent_trace(&agent_id, before, limit, records)
                .await,
        ),
        Method::AgentTraceDetail {
            agent_id,
            record_id,
            ..
        } => native_result(authority.agent_trace_detail(&agent_id, record_id).await),
        Method::AgentTraceLocateTool {
            agent_id, locator, ..
        } => native_result(authority.agent_trace_locate_tool(&agent_id, locator).await),
        Method::AgentTranscript {
            agent_id,
            at,
            limit,
            ..
        } => match authority.agent_transcript_window(&agent_id, &at, limit) {
            Err(RuntimeClientError::RuntimeFailure { .. }) => {
                Err(domain(ErrorData::AgentHistoryUnavailable { agent_id }))
            }
            result => result
                .map(|window| MethodResult::TranscriptWindow { window })
                .map_err(client_error),
        },
        Method::AgentTurns {
            agent_id,
            offset,
            limit,
            ..
        } => match authority.agent_turns(&agent_id, offset, limit) {
            Err(RuntimeClientError::RuntimeFailure { .. }) => {
                Err(domain(ErrorData::AgentHistoryUnavailable { agent_id }))
            }
            result => result
                .map(|page| MethodResult::ConversationTurns { page })
                .map_err(client_error),
        },
        Method::CompactContext {
            target: _,
            request_id,
        } => native_result(authority.compact_context(Some(request_id)).await),
        Method::SessionBoundaries {
            target: _,
            offset,
            limit,
        } => {
            let (surface_revision, boundaries, next_offset) = authority
                .user_message_boundaries(offset, limit)
                .map_err(client_error)?;
            Ok(MethodResult::Boundaries {
                surface_revision,
                boundaries,
                next_offset,
            })
        }
        Method::ConversationCheckpointValueRead {
            target: _,
            agent_id,
            cut,
            domain: collection,
            item_offset,
            path,
            offset,
            max_bytes,
        } => {
            if path.len() > 32 || path.iter().any(|part| part.len() > 256) || collection.len() > 256
            {
                return Err(domain(ErrorData::InvalidParams));
            }
            let chunk = if let Some(agent_id) = agent_id {
                authority
                    .agent_checkpoint_value_read(
                        &agent_id,
                        &cut,
                        &collection,
                        item_offset,
                        &path,
                        (offset, max_bytes),
                    )
                    .await
            } else {
                authority
                    .checkpoint_value_read_async(
                        &cut,
                        &collection,
                        item_offset,
                        &path,
                        (offset, max_bytes),
                    )
                    .await
            }
            .map_err(client_error)?;
            Ok(MethodResult::CheckpointValueChunk { chunk })
        }
        Method::ConversationContentRead {
            target: _,
            agent_id,
            content_ref,
            offset,
            max_bytes,
        } => {
            let chunk = if let Some(agent_id) = agent_id {
                authority
                    .agent_content_read(&agent_id, &content_ref, offset, max_bytes)
                    .await
            } else {
                tokio::task::spawn_blocking(move || {
                    authority.content_read(&content_ref, offset, max_bytes)
                })
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?
            }
            .map_err(client_error)?;
            Ok(MethodResult::ContentChunk { chunk })
        }
        Method::ConversationCheckpointPage {
            target: _,
            agent_id,
            cut,
            domain: collection,
            offset,
            limit,
        } => {
            let page = if let Some(agent_id) = agent_id {
                authority
                    .agent_checkpoint_page(&agent_id, &cut, &collection, offset, limit)
                    .await
            } else {
                authority
                    .checkpoint_page_async(&cut, &collection, offset, limit)
                    .await
            }
            .map_err(client_error)?;
            Ok(MethodResult::CheckpointPage { page })
        }
        Method::SessionCheckpoint {
            target: _,
            trace_records,
        } => {
            let checkpoint = tokio::task::spawn_blocking(move || {
                authority.checkpoint_with_trace(&trace_records)
            })
            .await
            .map_err(|_| domain(ErrorData::OperationFailed))?
            .map_err(client_error)?;
            Ok(MethodResult::Checkpoint {
                checkpoint: Box::new(checkpoint),
            })
        }
        Method::SessionSubscribe {
            target: _,
            after_cursor,
        } => {
            let registration = route.registration.replace(
                || {
                    route
                        .attachment
                        .subscribe_events(after_cursor)
                        .map_err(client_error)
                },
                after_cursor,
            )?;
            changed.notify_one();
            Ok(MethodResult::Subscribed {
                registration_id: registration.id,
                target: route.target.clone(),
                after_cursor,
            })
        }
        Method::TurnStart { target, content } | Method::TurnSteer { target, content } => {
            let receipts: Vec<_> = content
                .iter()
                .filter_map(|block| match block {
                    UserInputBlock::Upload(receipt) => Some(receipt.clone()),
                    UserInputBlock::Text(_) => None,
                })
                .collect();
            let mut uploads = sessions
                .uploaded_content(&target.session_id, &receipts)
                .await
                .map_err(session_error)?
                .into_iter();
            let mut canonical = Vec::with_capacity(content.len());
            for block in content {
                match block {
                    UserInputBlock::Text(text) => {
                        canonical.push(crate::message::types::UserContentBlock::Text(text));
                    }
                    UserInputBlock::Upload(_) => {
                        canonical.push(uploads.next().expect("validated receipt collection"));
                    }
                }
            }
            native_result(authority.submit_session_inbound(canonical))
        }
        Method::InboundEdit {
            target: _,
            expected,
            text,
        } => Ok(MethodResult::InboundMutation {
            outcome: authority
                .edit_pending(&expected, &text)
                .map_err(client_error)?,
        }),
        Method::InboundRemove {
            target: _,
            expected,
        } => Ok(MethodResult::InboundMutation {
            outcome: authority.remove_pending(&expected).map_err(client_error)?,
        }),
        Method::TurnCancel { target: _ } => native_result(authority.cancel_current_attempt()),
        Method::InteractionRespond {
            target: _,
            interaction,
            response,
        } => native_result(authority.respond_interaction(&interaction, response).await),
        Method::InteractionCancel {
            target: _,
            interaction,
        } => native_result(authority.cancel_interaction(&interaction).await),

        _ => Err(domain(ErrorData::OperationFailed)),
    }
}

async fn read_domain(
    authority: Arc<crate::runtime_client::host::ClientInner>,
    collection: String,
) -> Result<MethodResult, RpcError> {
    native_result(
        tokio::task::spawn_blocking(move || authority.domain_page(&collection, 0, 32))
            .await
            .map_err(|_| {
                domain(ErrorData::ObservationUnavailable {
                    diagnostic: "domain materialization interrupted".into(),
                })
            })?,
    )
}

async fn transition(
    result: crate::local_runtime::session_controller::SessionTransitionResult,
    sessions: &SessionController,
) -> Result<MethodResult, RpcError> {
    let node = sessions
        .observe_node(&result.session.id, &result.session.active_node)
        .await
        .map_err(|_| domain(ErrorData::OutcomeUnknown))?;
    let editor = super::editor_read::locator(result.session.id.clone(), &node);
    Ok(MethodResult::SessionTransition {
        session: result.session,
        editor,
        durability_diagnostic: result
            .durability_diagnostic
            .map(|_| "Committed; durable barrier not confirmed".into()),
    })
}
fn deletion(result: crate::local_runtime::session::deletion::SessionDeleteResult) -> MethodResult {
    MethodResult::Deletion {
        result: crate::app_server::session_deletion::project(result),
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "Keep the single native-result carrier mapping together"
)]
fn native_result(
    result: Result<RuntimeClientResult, RuntimeClientError>,
) -> Result<MethodResult, RpcError> {
    Ok(match result.map_err(client_error)? {
        RuntimeClientResult::DomainPage { page } => MethodResult::DomainPage { page },
        RuntimeClientResult::CheckpointValueChunk { chunk } => {
            MethodResult::CheckpointValueChunk { chunk }
        }
        RuntimeClientResult::InteractionFields { .. }
        | RuntimeClientResult::InteractionValueChunk { .. } => {
            return Err(domain(ErrorData::OperationFailed));
        }
        RuntimeClientResult::ContextCompacted { compaction } => {
            MethodResult::ContextCompacted { compaction }
        }
        RuntimeClientResult::TracePage { page } => MethodResult::Trace { page },
        RuntimeClientResult::TraceToolLocation { location } => {
            MethodResult::TraceToolLocation { location }
        }
        RuntimeClientResult::TranscriptPage { .. } => {
            unreachable!("App Server transcript reads use cut-bound windows")
        }
        RuntimeClientResult::TranscriptWindow { window } => {
            MethodResult::TranscriptWindow { window: *window }
        }
        RuntimeClientResult::ConversationTurns { page } => MethodResult::ConversationTurns { page },
        RuntimeClientResult::GoalMutation { reference } => MethodResult::GoalMutation { reference },
        RuntimeClientResult::JobCancellationAccepted { job_id } => {
            MethodResult::JobCancellationAccepted { job_id }
        }
        RuntimeClientResult::AgentInterruptionAccepted {
            agent_id,
            activation_id,
        } => MethodResult::AgentInterruptionAccepted {
            agent_id,
            activation_id,
        },
        RuntimeClientResult::JobOutcome { job_id, state } => {
            MethodResult::JobOutcome { job_id, state }
        }
        RuntimeClientResult::AgentObserve { observation } => {
            MethodResult::AgentObserve { observation }
        }
        RuntimeClientResult::AgentStatistics { metrics } => {
            MethodResult::AgentStatistics { metrics }
        }
        RuntimeClientResult::AgentMessage { accepted } => MethodResult::AgentMessage {
            agent_id: accepted.agent_id,
            activation_id: accepted.activation_id,
            resumed: accepted.resumed,
        },
        RuntimeClientResult::AgentWait {
            agent_id,
            activation_id,
            outcome,
        } => MethodResult::AgentWait {
            agent_id,
            activation_id,
            outcome,
        },

        RuntimeClientResult::InboundAccepted {
            message_id,
            inbound_sequence,
        } => MethodResult::InboundAccepted {
            message_id,
            inbound_sequence,
        },
        RuntimeClientResult::AttemptCancellationAccepted { attempt_id } => {
            MethodResult::CancellationAccepted { attempt_id }
        }
        RuntimeClientResult::InteractionResponseAccepted { interaction } => {
            MethodResult::InteractionSettled { interaction }
        }

        _ => return Err(domain(ErrorData::OperationFailed)),
    })
}
pub(super) fn client_error(error: RuntimeClientError) -> RpcError {
    domain(match error {
        RuntimeClientError::AgentNotDelivered { agent_id } => {
            ErrorData::AgentNotDelivered { agent_id }
        }
        RuntimeClientError::AgentDeliveryUnknown { agent_id } => {
            ErrorData::AgentDeliveryUnknown { agent_id }
        }
        RuntimeClientError::JobPublicationAbandoned { job_id } => {
            ErrorData::JobPublicationAbandoned { job_id }
        }
        RuntimeClientError::AgentStopping { agent_id } => ErrorData::AgentStopping { agent_id },
        RuntimeClientError::AgentSettlement { agent_id } => ErrorData::AgentSettlement { agent_id },
        RuntimeClientError::UnknownAgent { agent_id } => ErrorData::UnknownAgent { agent_id },
        RuntimeClientError::ConfigurationAdoption { rejection } => {
            ErrorData::ConfigurationAdoption { rejection }
        }
        // The App Server selects models through `session/setModel`; a Runtime
        // Client model refusal here is a turn or compaction refused because
        // the published catalog no longer admits the configured selection.
        RuntimeClientError::InvalidModelConfiguration { message } => ErrorData::ModelUnavailable {
            diagnostic: message,
        },
        RuntimeClientError::InteractionNotPending { interaction } => {
            ErrorData::InteractionNotPending { interaction }
        }
        RuntimeClientError::InteractionAuditFailed { interaction } => {
            ErrorData::InteractionAuditFailed { interaction }
        }
        RuntimeClientError::AttachmentInUse { .. } => ErrorData::ControllerInUse,
        RuntimeClientError::NotAttached => ErrorData::StaleAttachment,
        RuntimeClientError::ResyncRequired { .. } => ErrorData::ResyncRequired,
        RuntimeClientError::ObservationUnavailable { message } => {
            let mut end = message.len().min(512);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            ErrorData::ObservationUnavailable {
                diagnostic: message[..end].into(),
            }
        }
        RuntimeClientError::ObservationCapacityExceeded => ErrorData::ObservationCapacityExceeded,
        RuntimeClientError::ContentSourceUnavailable => ErrorData::ContentSourceUnavailable,
        RuntimeClientError::InvalidRange => ErrorData::InvalidRange,
        RuntimeClientError::ReadCutExpired => ErrorData::ReadCutExpired,
        RuntimeClientError::InvalidRequest { .. }
        | RuntimeClientError::InteractionInvalidResponse { .. } => ErrorData::InvalidParams,
        _ => ErrorData::InvalidState,
    })
}
pub(super) fn manager_error(error: RuntimeManagerError) -> RpcError {
    match error {
        RuntimeManagerError::ResidencyCapacity => domain(ErrorData::ResidencyCapacity),
        RuntimeManagerError::StaleIncarnation => domain(ErrorData::StaleRuntime),
        RuntimeManagerError::Client(error) => client_error(error),
        RuntimeManagerError::TransitionFailed(detail) => {
            rpc_error(-32000, &detail, Some(ErrorData::OperationFailed))
        }
        RuntimeManagerError::SessionAlreadyResident { .. } => domain(ErrorData::OperationFailed),
    }
}
fn model_selection_error(
    error: crate::local_runtime::session_runtime_manager::ModelSelectionError,
) -> RpcError {
    use crate::local_runtime::session_runtime_manager::ModelSelectionError;
    use crate::runtime::conversation_runtime::ModelUpdateError;
    match error {
        ModelSelectionError::Model(ModelUpdateError::SessionOwned) => {
            domain(ErrorData::OperationFailed)
        }
        ModelSelectionError::Model(ModelUpdateError::Conflict)
        | ModelSelectionError::Persistence(
            crate::local_runtime::session::SessionError::StaleSettings { .. },
        ) => domain(ErrorData::ConfigurationAdoption {
            rejection: crate::local_runtime::configuration::application::AdoptionError::Conflict,
        }),
        ModelSelectionError::Model(ModelUpdateError::Inactive) => domain(ErrorData::StaleRuntime),
        ModelSelectionError::Model(ModelUpdateError::InvalidConfiguration(diagnostic)) => {
            domain(ErrorData::ModelUnavailable { diagnostic })
        }
        ModelSelectionError::Model(ModelUpdateError::DurabilityFailed { message }) => {
            rpc_error(-32000, &message, Some(ErrorData::OperationFailed))
        }
        ModelSelectionError::Persistence(error) => session_error(error),
    }
}

pub(super) fn session_error(error: crate::local_runtime::session::SessionError) -> RpcError {
    use crate::local_runtime::session::SessionError;
    domain(match error {
        SessionError::UnknownSession { session_id } => ErrorData::UnknownSession { session_id },
        SessionError::UnknownNode {
            session_id,
            node_id,
        } => ErrorData::UnknownNode {
            session_id,
            node_id,
        },
        SessionError::StaleSettings { expected, actual } => {
            ErrorData::StaleSettings { expected, actual }
        }
        SessionError::InvalidName => ErrorData::InvalidParams,
        error if error.committed() => ErrorData::CommittedDurabilityUncertain,
        _ => ErrorData::OperationFailed,
    })
}
pub(super) fn domain(data: ErrorData) -> RpcError {
    let message = match &data {
        ErrorData::SessionFileRead { reason } => match reason {
            crate::tools::session_files::SessionFileReadFailure::Missing => {
                "Delivered Session file is missing"
            }
            crate::tools::session_files::SessionFileReadFailure::Unauthorized => {
                "Session file access is not authorized"
            }
            crate::tools::session_files::SessionFileReadFailure::Unavailable => {
                "Original delivery Session filesystem is unavailable"
            }
            crate::tools::session_files::SessionFileReadFailure::NotRegular => {
                "Delivered target is not a regular file"
            }
            crate::tools::session_files::SessionFileReadFailure::Replaced => {
                "Session file was replaced during read; reopen explicitly"
            }
            crate::tools::session_files::SessionFileReadFailure::TooLarge => {
                "Delivered Session file exceeds 512 KiB"
            }
            crate::tools::session_files::SessionFileReadFailure::Capacity => {
                "Session file read capacity reached"
            }
            crate::tools::session_files::SessionFileReadFailure::ReadFailed => {
                "Session file read failed"
            }
        },
        ErrorData::ObservationCancelled => "Child conversation observation was cancelled",
        ErrorData::DeliveryCancelled => "Delivery request was cancelled before publication",
        ErrorData::AgentNotDelivered { .. } => "Agent input was not delivered",
        ErrorData::AgentDeliveryUnknown { .. } => {
            "Agent input acceptance was not acknowledged; delivery is unknown, do not replay automatically"
        }
        ErrorData::JobPublicationAbandoned { .. } => {
            "Job terminal publication was abandoned; no durable terminal result is available"
        }
        ErrorData::AgentStopping { .. } => {
            "Agent is stopping or admitting an activation; retry after settlement"
        }
        ErrorData::AgentSettlement { .. } => {
            "Agent is unavailable; physical settlement, publication, or workspace authority requires explicit repair"
        }
        ErrorData::UnknownAgent { .. } => "Unknown Agent in this conversation",
        ErrorData::ModelUnavailable { .. } => {
            "The Session's configured Model is not in its published catalog; select one it provides"
        }
        _ => "Operation rejected",
    };
    rpc_error(-32000, message, Some(data))
}
fn rpc_error(code: i32, message: &str, data: Option<ErrorData>) -> RpcError {
    let mut end = message.len().min(512);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    RpcError {
        code,
        message: message[..end].into(),
        data,
    }
}
pub(super) fn failure(id: Option<RequestId>, error: RpcError) -> Response {
    Response::Failure(Failure {
        jsonrpc: JsonRpcVersion::V2,
        id,
        error,
    })
}

pub(super) fn host_error(error: HostAdmissionError) -> RpcError {
    domain(match error {
        HostAdmissionError::ServerDraining => ErrorData::ServerDraining,
        HostAdmissionError::RequestCapacity => ErrorData::RequestCapacity,
        HostAdmissionError::AttachmentCapacity => ErrorData::AttachmentCapacity,
    })
}

fn source_error(error: crate::local_runtime::configuration::settings::SettingsError) -> RpcError {
    use crate::local_runtime::configuration::settings::SettingsError;
    domain(match error {
        SettingsError::Conflict {
            scope,
            expected,
            actual,
        } => ErrorData::SourceConflict {
            scope,
            expected,
            actual,
        },
        SettingsError::Invalid => ErrorData::InvalidParams,
        SettingsError::Io => ErrorData::OperationFailed,
        SettingsError::Committed => ErrorData::CommittedDurabilityUncertain,
    })
}

fn history_checkpoint_cut(
    cut: crate::durable::observation::ObservationReadCut,
) -> crate::runtime_client::checkpoint::CheckpointReadCut {
    crate::runtime_client::checkpoint::CheckpointReadCut {
        canonical_foreground: None,
        live_publication: None,
        conversation_id: cut.read.conversation_id.clone(),
        activation_id: None,
        revision: 0,
        through: cut.read.journal,
        source_cut: Some(cut),
        domain_revisions: std::collections::BTreeMap::new(),
        collection_cuts: std::collections::BTreeMap::new(),
    }
}

fn history_anchor(
    at: crate::durable::reading::ConversationWindowAt,
    upper: Option<&crate::runtime_client::checkpoint::CheckpointReadCut>,
) -> Result<crate::durable::reading::ConversationWindowAt, RpcError> {
    use crate::durable::reading::ConversationWindowAt as At;
    if matches!(&at, At::Canonical { locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId { message_id }, .. } if message_id.as_str().len()>256)
    {
        return Err(domain(ErrorData::InvalidParams));
    }
    let Some(upper) = upper else { return Ok(at) };
    let cut = &upper
        .source_cut
        .as_ref()
        .ok_or_else(|| domain(ErrorData::ReadCutExpired))?
        .read;
    let check = |selected: &crate::durable::reading::ConversationReadCut| {
        if selected.reconstructible_from(cut) {
            Ok(())
        } else {
            Err(domain(ErrorData::ReadCutExpired))
        }
    };
    Ok(match at {
        At::Latest => At::Older {
            before: crate::durable::TranscriptCursor::new(
                cut.transcript
                    .checked_add(1)
                    .ok_or_else(|| domain(ErrorData::ReadCutExpired))?,
            ),
            cut: Some(cut.clone()),
        },
        At::Older { before, cut: None } => At::Older {
            before,
            cut: Some(cut.clone()),
        },
        At::Older {
            before,
            cut: Some(selected),
        } => {
            check(&selected)?;
            At::Older {
                before,
                cut: Some(selected),
            }
        }
        At::Canonical { locator, cut: None } => At::Canonical {
            locator,
            cut: Some(cut.clone()),
        },
        At::Canonical {
            locator,
            cut: Some(selected),
        } => {
            check(&selected)?;
            At::Canonical {
                locator,
                cut: Some(selected),
            }
        }
        At::Message { id, cut: None } => At::Message {
            id,
            cut: Some(cut.clone()),
        },
        At::Message {
            id,
            cut: Some(selected),
        } => {
            check(&selected)?;
            At::Message {
                id,
                cut: Some(selected),
            }
        }
        At::Newer {
            after,
            cut: selected,
        } => {
            check(&selected)?;
            At::Newer {
                after,
                cut: selected,
            }
        }
        At::Turn { id, cut: selected } => {
            check(&selected)?;
            At::Turn { id, cut: selected }
        }
        At::TurnOrdinal {
            ordinal,
            cut: selected,
        } => {
            check(&selected)?;
            At::TurnOrdinal {
                ordinal,
                cut: selected,
            }
        }
    })
}

fn source_settings_error(
    error: crate::local_runtime::session_runtime_manager::SourceSettingsError,
) -> RpcError {
    match error {
        crate::local_runtime::session_runtime_manager::SourceSettingsError::Source(error) => {
            source_error(error)
        }
    }
}

fn source_read_error(error: &str) -> RpcError {
    domain(if error == "source read cut expired" {
        ErrorData::ReadCutExpired
    } else if error == "configuration source unavailable" {
        ErrorData::ObservationUnavailable {
            diagnostic: "configuration source unavailable".into(),
        }
    } else {
        ErrorData::InvalidParams
    })
}

#[cfg(test)]
mod capacity_tests {
    #[test]
    fn agent_delivery_failures_preserve_the_owner_three_way_contract() {
        let agent_id = crate::runtime::identity::AgentId::new("agent-delivery");
        let missing = super::client_error(
            crate::runtime_client::types::RuntimeClientError::AgentNotDelivered {
                agent_id: agent_id.clone(),
            },
        );
        assert_eq!(
            missing.data,
            Some(super::ErrorData::AgentNotDelivered {
                agent_id: agent_id.clone()
            })
        );
        assert!(missing.message.contains("input was not delivered"));
        let unknown = super::client_error(
            crate::runtime_client::types::RuntimeClientError::AgentDeliveryUnknown {
                agent_id: agent_id.clone(),
            },
        );
        assert_eq!(
            unknown.data,
            Some(super::ErrorData::AgentDeliveryUnknown { agent_id })
        );
        assert!(unknown.message.contains("do not replay automatically"));
    }

    #[test]
    fn job_publication_abandonment_preserves_typed_failure_and_identity() {
        let job_id = crate::runtime::identity::ToolExecutionId::new(
            "exec_0199c989-03a0-7000-8000-000000000001",
        );
        let error = super::native_result(Err(
            crate::runtime_client::types::RuntimeClientError::JobPublicationAbandoned {
                job_id: job_id.clone(),
            },
        ))
        .unwrap_err();
        assert_eq!(
            error.data,
            Some(super::ErrorData::JobPublicationAbandoned { job_id })
        );
        assert!(error.message.contains("no durable terminal result"));
    }

    #[test]
    fn job_listing_preserves_bounded_page_metadata_on_the_wire() {
        let page = crate::runtime_client::checkpoint::CheckpointPage {
            collections: Vec::new(),
            cursor: crate::runtime_client::types::RuntimeClientCursor::new(0),
            read_cut: crate::runtime_client::checkpoint::CheckpointReadCut {
                canonical_foreground: None,
                live_publication: None,
                collection_cuts: std::collections::BTreeMap::new(),
                conversation_id: crate::runtime::identity::ConversationId::new(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
                ),
                activation_id: None,
                revision: 0,
                through: 0,
                source_cut: None,
                domain_revisions: std::collections::BTreeMap::default(),
            },
            domain: "jobs".into(),
            total: 67,
            items: Vec::new(),
            next_offset: Some(32),
        };
        let result = super::native_result(Ok(
            crate::runtime_client::types::RuntimeClientResult::DomainPage { page },
        ))
        .unwrap();
        let super::MethodResult::DomainPage { page } = result else {
            panic!("page");
        };
        assert_eq!(page.total, 67);
        assert_eq!(page.next_offset, Some(32));
    }

    #[test]
    fn agent_settlement_wire_error_does_not_advise_retry() {
        let error = super::client_error(
            crate::runtime_client::types::RuntimeClientError::AgentSettlement {
                agent_id: crate::runtime::identity::AgentId::new("agent-unavailable"),
            },
        );
        assert!(
            matches!(&error.data, Some(super::ErrorData::AgentSettlement { agent_id }) if agent_id.as_str() == "agent-unavailable")
        );
        assert!(error.message.contains("explicit repair"));
        assert!(!error.message.contains("retry"));
    }

    #[test]
    fn production_route_table_starts_with_32_attachment_slots() {
        let routes = super::RouteTable::default();
        assert_eq!(routes.attachment_limit, 32);
        assert!(routes.active.is_empty());
        assert!(routes.reserved.is_empty());
    }
}

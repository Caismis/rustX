//! One connection negotiates once and routes independent Session attachments.
//! Only the attachment map is connection-owned; all semantic work is delegated.
//!
//! This owner admits every request — decoding, association, host admission,
//! attachment reservation and registration — and accepts its final reply.
//! Admitted requests reach one handler per request family, chosen by the
//! exhaustive [`family::Family`] classification; the handlers own no locks,
//! admission or reply acceptance of their own.
mod auxiliary;
mod catalog;
mod configuration;
mod family;
mod history;
mod notifications;
mod runtime;
mod schedule;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use self::family::Family;
use super::protocol::{
    APP_SERVER_PROTOCOL_VERSION, AttachmentTarget, ErrorData, InitializeParams, JsonRpcVersion,
    Method, MethodResult, Request, RequestId, Response, RpcError, ServerCapabilities, Success,
};
use super::rpc_error::{client_error, domain, failure, host_error, manager_error, rpc_error};
use crate::local_runtime::session::SessionId;
use crate::local_runtime::session_controller::SessionController;
use crate::local_runtime::session_runtime_manager::ManagedRuntimeClient;
use crate::runtime_client::attachment::RuntimeAttachment;

use super::host::{AppServerHost, AttachmentPermit};
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
    source_repairs: Arc<Mutex<notifications::RepairInterest>>,
    /// Delivery access granted by this connection's transport authentication,
    /// never by a request. Close cancels it, retiring every admitted fence.
    delivery_access: Option<tokio_util::sync::CancellationToken>,
    /// This connection's in-flight delivery requests, by exact request id.
    deliveries: Arc<super::delivery_access::Operations>,
    associations: Arc<super::admission::Associations>,
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
    /// Capture execution uncertainty before encoding or authorization can
    /// replace the produced response. Both publication paths use this fact.
    pub(super) fn preserve_outcome(&mut self) -> bool {
        let uncertain = match &self.response {
            Response::Success(_) => self.may_have_committed,
            Response::Failure(failure) => matches!(
                failure.error.data,
                Some(ErrorData::OutcomeUnknown | ErrorData::CommittedDurabilityUncertain)
            ),
        };
        if let Some(publication) = &mut self.publication {
            publication.preserve_outcome(uncertain);
        }
        uncertain
    }

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

    /// Inject an oversized uncertain response while retaining the real
    /// admitted request's publication owner.
    #[cfg(test)]
    pub(crate) fn force_uncertain_failure_for_test(&mut self, data: ErrorData) {
        assert!(matches!(
            data,
            ErrorData::OutcomeUnknown | ErrorData::CommittedDurabilityUncertain
        ));
        let Response::Success(success) = &self.response else {
            panic!("admitted receipt required")
        };
        self.response = failure(
            Some(success.id.clone()),
            RpcError {
                code: -32000,
                message: "x".repeat(super::transport::MAX_MESSAGE_BYTES),
                data: Some(data),
            },
        );
    }

    /// Commits now. A delivery cancelled or revoked earlier is replaced by
    /// its typed failure for the same request id.
    pub(crate) fn commit(mut self) -> Response {
        self.preserve_outcome();
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
        locator: crate::observation::content::CanonicalContentLocator,
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
            call @ (Method::AuxiliaryClassify { .. }
            | Method::AuxiliaryBtw { .. }
            | Method::AuxiliaryStatus { .. }
            | Method::AuxiliaryResult { .. }
            | Method::AuxiliaryCancel { .. }
            | Method::AuxiliaryDiscard { .. }
            | Method::AuxiliarySubmissionLookup { .. }) => {
                self.auxiliary_request(&id, call, association.clone()).await
            }

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
                | Method::SessionForkProjection { .. }
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

    /// Route one request to its family after the connection-level gates.
    ///
    /// Negotiation and cancellation are answered here; a runtime operation
    /// is admitted against its exact attachment; every other family runs
    /// under one host request admission held until it settles.
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
        let family = Family::of(&method);
        if let Family::Runtime(target) = family {
            let route = self.route(target)?;
            return self
                .admit_runtime(
                    method,
                    route,
                    request_lane,
                    observation_cancel,
                    physical_association,
                )
                .await;
        }
        let _physical_association = physical_association;
        let request_owner = self
            .host
            .admit_request(request_lane, std::convert::identity)
            .map_err(host_error)?;
        // Source observation may await the Session catalog. Only an admitted
        // request owns that work; a draining host must refuse without it.
        self.observe_repair_source(&method).await;
        match family {
            Family::Connection => match method {
                Method::ServerInfo {} => Ok(MethodResult::ServerInfo {
                    capabilities: self.capabilities(),
                }),
                // Negotiation, diagnostics and cancellation were answered above.
                _ => Err(domain(ErrorData::OperationFailed)),
            },
            Family::Attachment => self.attachment(method).await,
            Family::Catalog => self.catalog(method, request_owner).await,
            Family::Scheduler => self.schedule(method, request_owner).await,
            Family::History => self.history(method).await,
            Family::Configuration => self.configuration(method).await,
            // Deliveries are registered by `execute`; runtime operations
            // were admitted above.
            Family::Delivery | Family::Runtime(_) => Err(domain(ErrorData::OperationFailed)),
        }
    }

    /// Admit one runtime operation against its exact attachment and hand it
    /// to the manager-owned native operation.
    async fn admit_runtime(
        &self,
        method: Method,
        route: Arc<Route>,
        request_lane: super::RequestLane,
        observation_cancel: Option<super::admission::ObservationControl>,
        physical_association: Option<super::admission::Association>,
    ) -> Result<MethodResult, RpcError> {
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
        if let Method::CompactContext { request_id, .. } = &method {
            let request_id = request_id.clone();
            let receiver = {
                let routes = self.routes.lock().expect("routes mutex");
                if routes.closed
                    || !routes
                        .active
                        .get(&route.target.session_id)
                        .is_some_and(|active| Arc::ptr_eq(active, &route))
                {
                    return Err(domain(ErrorData::StaleAttachment));
                }
                self.host
                    .admit_request(request_lane, |request_owner| {
                        client.start_compaction_operation(move |_| {
                            let authority = route.attachment.operation_authority();
                            async move {
                                let tail =
                                    authority?.admit_compaction_transfer(Some(request_id))?;
                                let completion: futures_util::future::BoxFuture<'static, _> =
                                    Box::pin(async move {
                                        let _request = request_owner;
                                        let _physical_association = physical_association;
                                        tail.await
                                    });
                                Ok(completion)
                            }
                        })
                    })
                    .map_err(host_error)?
                    .map_err(manager_error)?
            };
            return runtime::native_result(
                receiver
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))?,
            );
        }
        let receiver = {
            let routes = self.routes.lock().expect("routes mutex");
            if routes.closed
                || !routes
                    .active
                    .get(&route.target.session_id)
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
                            runtime::dispatch_runtime(
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
        receiver
            .await
            .map_err(|_| domain(ErrorData::OperationFailed))?
    }

    /// This connection's attachment relationships. Attach reserves the
    /// Session's slot under the route-table lock, loads outside it, and
    /// publishes the route under the same close boundary.
    async fn attachment(&self, method: Method) -> Result<MethodResult, RpcError> {
        match method {
            Method::SessionDetach { target } => {
                // Removing a connection relationship needs no live-runtime lease.
                let route = self.route(&target)?;
                release_route(&self.host, &self.routes, &route);
                self.changed.notify_one();
                Ok(MethodResult::Detached {})
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
            // `Family::of` routes only attachment requests here.
            _ => Err(domain(ErrorData::OperationFailed)),
        }
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
}

impl Drop for AppServerConnection {
    fn drop(&mut self) {
        if Arc::strong_count(&self.initialized) == 1 {
            self.close();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn production_route_table_starts_with_32_attachment_slots() {
        let routes = super::RouteTable::default();
        assert_eq!(routes.attachment_limit, 32);
        assert!(routes.active.is_empty());
        assert!(routes.reserved.is_empty());
    }
}

//! The runtime-owned model adapter interface.

use crate::model::types::{ModelProtocol, ModelRequest};
use crate::runtime::cancellation::CancellationSignal;
use rustx_types::model::adapter::stream::ModelStream;

pub trait ModelAdapter: Send + Sync {
    /// The protocol this adapter speaks.
    fn protocol(&self) -> ModelProtocol;

    /// Executes one canonical model request as one provider-independent stream.
    ///
    /// The returned stream is the only channel of information about the
    /// invocation; the adapter retains no hidden state afterwards.
    fn stream(&self, request: ModelRequest, cancellation: CancellationSignal) -> ModelStream;

    /// Dispatch an adapter-prepared immutable request after the execution
    /// owner's admission boundary. In-process adapters consume only the
    /// canonical input; native HTTP adapters use the prepared body.
    fn stream_prepared(
        &self,
        request: super::validation::PreparedModelRequest,
        cancellation: CancellationSignal,
    ) -> ModelStream {
        self.stream(std::sync::Arc::unwrap_or_clone(request.input), cancellation)
    }
}

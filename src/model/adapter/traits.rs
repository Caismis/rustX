//! The runtime-owned model adapter interface.
//!
//! All three M2 protocols (`OpenAI` Chat Completions, `OpenAI` Responses,
//! and `Anthropic` Messages) implement exactly this interface. The interface is
//! provider-independent in both directions: the input is a provider-neutral
//! [`ModelRequest`], the output is a [`ModelStreamItem`] stream, and no
//! provider SDK type appears in any signature.

use std::pin::Pin;

use futures_util::Stream;

use crate::model::error::ModelError;
use crate::model::event::ModelEvent;
use crate::model::types::{ModelProtocol, ModelRequest};
use crate::runtime::cancellation::CancellationSignal;

/// Ephemeral provider-derived progress that is not canonical model output.
///
/// Adapters use this only when provider activity cannot yet be represented by
/// a [`ModelEvent`]. The runtime consumes it for request-local deadline
/// semantics; it is never assembled, published, journaled, or persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelStreamProgress {
    /// The provider generated semantic model output that is not yet
    /// attributable to a canonical event.
    Generation,
    /// The opened provider stream is alive without proving generation began.
    Liveness,
}

/// One item produced by a provider-independent model stream.
///
/// `Event` carries canonical model facts consumed by the Agent Loop or
/// summarizer. `Progress` is execution-only evidence from an adapter: it
/// drives request deadlines but cannot enter canonical assembly or any
/// durable/output plane.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelStreamItem {
    /// A canonical normalized model event.
    Event(ModelEvent),
    /// Ephemeral provider-derived generation or liveness evidence.
    Progress(ModelStreamProgress),
}

/// A boxed provider-independent model stream.
pub type ModelStream = Pin<Box<dyn Stream<Item = ModelStreamItem> + Send + 'static>>;

/// One provider model protocol execution adapter.
///
/// Implementations must guarantee:
///
/// - one `stream` invocation performs exactly one provider request attempt
///   (no hidden retry, no reconnect, no failover);
/// - at most one canonical terminal event per stream (`Completed` or `Failed`);
/// - no stream items after a terminal event;
/// - cancellation propagates by terminating with `Failed(Cancelled)` and
///   dropping the underlying provider stream;
/// - a request rejected before provider execution produces a terminal
///   `Failed` without `Started` and without any provider request.
pub trait ModelAdapter: Send + Sync {
    /// The protocol this adapter speaks.
    fn protocol(&self) -> ModelProtocol;

    /// Executes one canonical model request as one provider-independent stream.
    ///
    /// The returned stream is the only channel of information about the
    /// invocation; the adapter retains no hidden state afterwards.
    fn stream(&self, request: ModelRequest, cancellation: CancellationSignal) -> ModelStream;
}

/// A single-item stream that fails with the given normalized error, without
/// `Started` and without any provider request.
#[must_use]
pub fn model_stream_of_failure(error: ModelError) -> ModelStream {
    Box::pin(futures_util::stream::once(async move {
        ModelStreamItem::Event(ModelEvent::Failed { error })
    }))
}

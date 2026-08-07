//! The runtime-owned model adapter interface.
//!
//! All three M2 protocols (`OpenAI` Chat Completions, `OpenAI` Responses,
//! and `Anthropic` Messages) implement exactly this interface. The interface is
//! provider-independent in both directions: the input is a canonical
//! [`ModelRequest`], the output is a canonical [`ModelEvent`] stream, and no
//! provider SDK type appears in any signature.

use std::pin::Pin;

use futures_util::Stream;

use crate::model::adapter::cancellation::ModelCancellation;
use crate::model::error::ModelError;
use crate::model::event::ModelEvent;
use crate::model::types::{ModelProtocol, ModelRequest};

/// A boxed canonical model event stream.
pub type ModelEventStream = Pin<Box<dyn Stream<Item = ModelEvent> + Send + 'static>>;

/// One provider model protocol execution adapter.
///
/// Implementations must guarantee:
///
/// - one `stream` invocation performs exactly one provider request attempt
///   (no hidden retry, no reconnect, no failover);
/// - at most one terminal event per stream (`Completed` or `Failed`);
/// - no events after a terminal event;
/// - cancellation propagates by terminating with `Failed(Cancelled)` and
///   dropping the underlying provider stream;
/// - a request rejected before provider execution produces a terminal
///   `Failed` without `Started` and without any provider request.
pub trait ModelAdapter: Send + Sync {
    /// The protocol this adapter speaks.
    fn protocol(&self) -> ModelProtocol;

    /// Executes one canonical model request as a canonical event stream.
    ///
    /// The returned stream is the only channel of information about the
    /// invocation; the adapter retains no hidden state afterwards.
    fn stream(&self, request: ModelRequest, cancellation: ModelCancellation) -> ModelEventStream;
}

/// A single-event stream that fails with the given normalized error, without
/// `Started` and without any provider request.
#[must_use]
pub fn model_event_stream_of_failure(error: ModelError) -> ModelEventStream {
    Box::pin(futures_util::stream::once(async move {
        ModelEvent::Failed { error }
    }))
}

/// A single-event stream that completes with the given finish reason.
#[must_use]
pub fn model_event_stream_of_completion(
    finish_reason: crate::model::finish::ModelFinishReason,
    usage: Option<crate::model::types::ModelUsage>,
) -> ModelEventStream {
    Box::pin(futures_util::stream::once(async move {
        ModelEvent::Completed {
            finish_reason,
            usage,
        }
    }))
}

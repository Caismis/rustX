//! Provider-independent model stream item vocabulary.
//!
//! All three M2 protocols emit the same stream contract; the item types are
//! pure value vocabulary with no adapter or request ownership.

use std::pin::Pin;

use futures_util::Stream;

use crate::model::error::ModelError;
use crate::model::event::ModelEvent;

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

/// A single-item stream that fails with the given normalized error, without
/// `Started` and without any provider request.
pub fn model_stream_of_failure(error: ModelError) -> ModelStream {
    Box::pin(futures_util::stream::once(async move {
        ModelStreamItem::Event(ModelEvent::Failed { error })
    }))
}

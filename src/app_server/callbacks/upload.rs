//! Hosted receive state belongs to the pending callback and its subscription.

use super::{CallbackReply, Callbacks, State, Validity};
use crate::app_server::{
    host_upload::{
        ChunkAck, HostUploadBeginParams, HostUploadExpected, HostUploadMethod, HostUploadRefusal,
        HostUploadSlot, ValidatedEnd, decode_chunk_data,
    },
    protocol::{RequestId, ServerRequestMethod},
};
use crate::message::content::TextBlock;
use crate::tools::{
    ToolExecutionResult, ToolExecutionStatus, ToolResultContent,
    limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES as PREVIEW_BYTES,
    types::{ManagedOutputContinuation, TruncationState},
};
use rustx_process_supervisor::output_text::{IncrementalUtf8Decoder, TextPreviewCapture};
use std::{sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

pub(super) const MAX_ACTIVE: usize = 32;
const MAX_PER_HOST: usize = 4;
const MAX_PER_CONVERSATION: usize = 8;
// Retain an inexpensive tombstone per recipient until its subscription or
// callback ends. Cycling UploadIds cannot reopen any of these slots.
const MAX_SLOTS_PER_REQUEST: usize = 8;
const TOTAL_BUDGET: Duration = Duration::from_secs(60);
const IDLE_BUDGET: Duration = Duration::from_secs(10);

pub(super) enum Candidate {
    Active(Box<Receiving>),
    Retired(HostUploadRefusal),
}

pub(super) struct Receiving {
    slot: HostUploadSlot,
    preview: Preview,
    utf8: IncrementalUtf8Decoder,
    total_deadline: Instant,
    idle_deadline: Instant,
    permit: OwnedSemaphorePermit,
}

enum Preview {
    Items(Vec<String>),
    Combined(TextPreviewCapture),
}

pub(crate) struct CompletedUpload {
    end: ValidatedEnd,
    preview: Preview,
    deadline: Instant,
    _permit: OwnedSemaphorePermit,
}

impl CompletedUpload {
    pub(super) fn request_id(&self) -> &RequestId {
        &self.end.candidate().request_id
    }

    pub(super) fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    pub(super) fn into_result(self, duration: u64) -> ToolExecutionResult {
        let status = if self.end.success() {
            ToolExecutionStatus::Success
        } else {
            ToolExecutionStatus::Failed {
                error: "application reported a failed tool result".into(),
            }
        };
        let (items, truncated) = match self.preview {
            Preview::Items(items) => (items, false),
            Preview::Combined(preview) => {
                let (text, truncated) = preview.finish();
                (vec![text], truncated)
            }
        };
        let content = items
            .into_iter()
            .map(|text| ToolResultContent::Text(TextBlock { text }))
            .collect();
        let mut result = crate::tools::host::result(status, content, duration);
        if truncated {
            result.truncation = Some(TruncationState {
                truncated: true,
                original_bytes: Some(self.end.total()),
            });
            result.managed_output = Some(Box::new(ManagedOutputContinuation::Unavailable {
                diagnostic: "complete Hosted output was not saved".into(),
            }));
        }
        result
    }
}

struct Step {
    duplicate: bool,
    completed: Option<CompletedUpload>,
}

impl Step {
    const ACCEPTED: Self = Self {
        duplicate: false,
        completed: None,
    };
}

impl Callbacks {
    pub(crate) fn upload(
        &self,
        client: &str,
        method: HostUploadMethod,
    ) -> Result<bool, HostUploadRefusal> {
        let address = method.candidate();
        let id = address.request_id.clone();
        let subscription_id = address.subscription_incarnation.as_str().to_owned();
        let decoded = match &method {
            HostUploadMethod::Chunk(chunk) => decode_chunk_data(&chunk.data_base64)?,
            _ => Vec::new(),
        };
        let step = {
            let mut state = self.state.lock().expect("callbacks");
            state.authorize_upload(&self.incarnation, client, &method)?;
            let now = Instant::now();
            if let HostUploadMethod::Begin(begin) = method {
                state.begin_upload(client, begin, now, &self.upload_budget)
            } else {
                state
                    .pending
                    .get_mut(&id)
                    .expect("authorized callback")
                    .uploads
                    .get_mut(client)
                    .ok_or(HostUploadRefusal::Foreign)?
                    .advance(method, &decoded, now)
            }
        }?;
        // Native lifecycle and cancellation are rechecked through the same
        // claim path used by ordinary JSON-RPC responses.
        if let Some(completed) = step.completed
            && !self.respond_for(
                client,
                CallbackReply::Upload(Box::new(completed)),
                Some(&subscription_id),
            )
        {
            return Err(HostUploadRefusal::Late);
        }
        Ok(step.duplicate)
    }
}

impl State {
    fn authorize_upload(
        &self,
        incarnation: &str,
        client: &str,
        method: &HostUploadMethod,
    ) -> Result<(), HostUploadRefusal> {
        let address = method.candidate();
        let pending = self
            .pending
            .get(&address.request_id)
            .ok_or(HostUploadRefusal::Late)?;
        let participant = self.clients.get(client).ok_or(HostUploadRefusal::Foreign)?;
        let subscription = participant
            .threads
            .get(&pending.thread)
            .ok_or(HostUploadRefusal::Foreign)?;
        let ServerRequestMethod::ToolCall(call) = &pending.request.call else {
            return Err(HostUploadRefusal::Foreign);
        };
        if address.callback_incarnation.as_str() != incarnation
            || address.connection_incarnation.as_str() != client
            || address.subscription_incarnation.as_str() != subscription.id
            || pending.recipients.get(client) != Some(&subscription.id)
            || call.thread_id != address.thread_id
            || call.turn_id != address.turn_id
            || call.call_id != address.call_id
        {
            return Err(HostUploadRefusal::Foreign);
        }
        if participant.closed.is_cancelled()
            || !subscription.lifecycle.is_running()
            || matches!(&pending.validity, Some(Validity::Tool(signal)) if signal.is_cancelled())
        {
            return Err(HostUploadRefusal::Late);
        }
        Ok(())
    }

    fn begin_upload(
        &mut self,
        client: &str,
        begin: HostUploadBeginParams,
        now: Instant,
        budget: &Arc<Semaphore>,
    ) -> Result<Step, HostUploadRefusal> {
        let id = &begin.candidate.request_id;
        let pending = &self.pending[id];
        if pending.uploads.contains_key(client) {
            return self
                .pending
                .get_mut(id)
                .expect("authorized callback")
                .uploads
                .get_mut(client)
                .expect("existing upload")
                .advance(HostUploadMethod::Begin(begin), &[], now);
        }
        if pending.uploads.len() >= MAX_SLOTS_PER_REQUEST {
            return Err(HostUploadRefusal::Capacity);
        }
        let thread = pending.thread.clone();
        let (mut on_host, mut in_conversation) = (0, 0);
        for pending in self
            .pending
            .values_mut()
            .filter(|pending| !pending.uploads.is_empty())
        {
            for (owner, candidate) in &mut pending.uploads {
                candidate.expire(now);
                if matches!(candidate, Candidate::Active(_)) {
                    on_host += usize::from(owner == client);
                    in_conversation += usize::from(pending.thread == thread);
                }
            }
        }
        if on_host >= MAX_PER_HOST || in_conversation >= MAX_PER_CONVERSATION {
            return Err(HostUploadRefusal::Capacity);
        }
        let permit = Arc::clone(budget)
            .try_acquire_owned()
            .map_err(|_| HostUploadRefusal::Capacity)?;
        let receiving = Receiving::begin(&begin, now, permit)?;
        self.pending
            .get_mut(id)
            .expect("authorized callback")
            .uploads
            .insert(client.to_owned(), Candidate::Active(Box::new(receiving)));
        Ok(Step::ACCEPTED)
    }
}

impl Receiving {
    fn begin(
        begin: &HostUploadBeginParams,
        now: Instant,
        permit: OwnedSemaphorePermit,
    ) -> Result<Self, HostUploadRefusal> {
        let address = &begin.candidate;
        // The caller has matched all broker-side identity fields. Only
        // UploadId is selected at Begin by the authenticated Host.
        let mut slot = HostUploadSlot::new(HostUploadExpected {
            callback_incarnation: address.callback_incarnation.clone(),
            connection_incarnation: address.connection_incarnation.clone(),
            subscription_incarnation: address.subscription_incarnation.clone(),
            request_id: address.request_id.clone(),
            thread_id: address.thread_id.clone(),
            turn_id: address.turn_id.clone(),
            call_id: address.call_id.clone(),
            upload_id: address.upload_id.clone(),
        });
        slot.begin(address.clone(), begin.success, &begin.items)?;
        let total: u64 = begin.items.iter().map(|item| item.length.get()).sum();
        let preview = if total <= PREVIEW_BYTES as u64 {
            Preview::Items(vec![String::new(); begin.items.len()])
        } else {
            Preview::Combined(TextPreviewCapture::new(PREVIEW_BYTES))
        };
        Ok(Self {
            slot,
            preview,
            utf8: IncrementalUtf8Decoder::default(),
            total_deadline: now + TOTAL_BUDGET,
            idle_deadline: now + IDLE_BUDGET,
            permit,
        })
    }
}

impl Candidate {
    fn expire(&mut self, now: Instant) {
        if matches!(self, Self::Active(receiving) if now >= receiving.total_deadline || now >= receiving.idle_deadline)
        {
            *self = Self::Retired(HostUploadRefusal::Expired);
        }
    }

    fn advance(
        &mut self,
        method: HostUploadMethod,
        decoded: &[u8],
        now: Instant,
    ) -> Result<Step, HostUploadRefusal> {
        self.expire(now);
        let receiving = match self {
            Self::Active(receiving) => receiving,
            Self::Retired(reason) => return Err(*reason),
        };
        match method {
            HostUploadMethod::Begin(begin) => {
                receiving
                    .slot
                    .begin(begin.candidate, begin.success, &begin.items)?;
                Ok(Step {
                    duplicate: true,
                    ..Step::ACCEPTED
                })
            }
            HostUploadMethod::Chunk(chunk) => {
                let duplicate = receiving.slot.chunk(
                    &chunk.candidate,
                    chunk.item_index,
                    chunk.sequence.get(),
                    chunk.offset.get(),
                    decoded,
                    chunk.sha256.as_bytes(),
                )? == ChunkAck::Duplicate;
                if !duplicate {
                    let text = receiving.utf8.push(decoded);
                    match &mut receiving.preview {
                        Preview::Items(items) => items[chunk.item_index as usize].push_str(&text),
                        Preview::Combined(preview) => preview.push(&text),
                    }
                    receiving.idle_deadline = now + IDLE_BUDGET;
                }
                Ok(Step {
                    duplicate,
                    ..Step::ACCEPTED
                })
            }
            HostUploadMethod::End(end) => {
                let end = receiving.slot.end(
                    &end.candidate,
                    end.success,
                    &end.items,
                    end.total.get(),
                    end.digest.as_bytes(),
                )?;
                let Self::Active(receiving) =
                    std::mem::replace(self, Self::Retired(HostUploadRefusal::Late))
                else {
                    unreachable!("verified the active candidate");
                };
                Ok(Step {
                    duplicate: false,
                    completed: Some(CompletedUpload {
                        end,
                        preview: receiving.preview,
                        deadline: receiving.total_deadline.min(receiving.idle_deadline),
                        _permit: receiving.permit,
                    }),
                })
            }
            HostUploadMethod::Abort(abort) => {
                receiving.slot.abort(&abort.candidate)?;
                *self = Self::Retired(HostUploadRefusal::Late);
                Ok(Step::ACCEPTED)
            }
        }
    }
}

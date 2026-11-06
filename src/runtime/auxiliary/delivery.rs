//! Ephemeral native results. Reservation precedes execution admission; readers
//! receive counted copies without consuming the result or renewing its TTL.

use crate::runtime::identity::{AuxiliaryCallId, RequestId};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const RESULT_RECORD_BYTES_MAX: usize = 65_536;
pub const RESULT_TEXT_BYTES_MAX: usize = 8_192;
pub const CONVERSATION_RESULT_SLOTS: usize = 16;
pub const PROCESS_RESULT_SLOTS: usize = 128;
pub const RESULT_RETENTION: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConsumerResult {
    Classification { label: String },
    Btw { text: String },
    Rejected { reason: ConsumerFailure },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerFailure {
    InvalidOutput,
    Refused,
    OutputTooLarge,
    InputTooLarge,
    ExecutionFailed,
    Cancelled,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuxiliaryResult {
    pub call_id: AuxiliaryCallId,
    pub final_request_id: Option<RequestId>,
    pub execution: Option<crate::model::AuxiliaryCallOutcome>,
    pub value: ConsumerResult,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResultAvailability {
    Pending,
    Retained,
    Expired,
    Discarded,
    Unavailable,
}

struct Capacity {
    slots: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
}
impl Capacity {
    fn new(slots: usize) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(slots)),
            bytes: Arc::new(Semaphore::new(slots * RESULT_RECORD_BYTES_MAX)),
        }
    }
    fn reserve(&self) -> Result<Budget, &'static str> {
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| "auxiliary result capacity is full")?;
        let bytes = self
            .bytes
            .clone()
            .try_acquire_many_owned(
                u32::try_from(RESULT_RECORD_BYTES_MAX).expect("constant byte capacity"),
            )
            .map_err(|_| "auxiliary result byte capacity is full")?;
        Ok(Budget { _slot: slot, bytes })
    }
}
struct Budget {
    _slot: OwnedSemaphorePermit,
    bytes: OwnedSemaphorePermit,
}
impl Budget {
    fn shrink(&mut self, length: usize) {
        let excess = self.bytes.num_permits().saturating_sub(length);
        drop(self.bytes.split(excess));
    }
}
struct Budgets {
    process: Budget,
    conversation: Budget,
}
impl Budgets {
    fn reserve(process: &Capacity, conversation: &Capacity) -> Result<Self, &'static str> {
        // No waiting while a preceding resource is retained. Drop unwinds in
        // reverse order on every partial refusal.
        let process = process.reserve()?;
        let conversation = conversation.reserve()?;
        Ok(Self {
            process,
            conversation,
        })
    }
    fn shrink(&mut self, bytes: usize) {
        self.process.shrink(bytes);
        self.conversation.shrink(bytes);
    }
}
struct Entry {
    result: Option<AuxiliaryResult>,
    completed: Option<tokio::time::Instant>,
    budgets: Budgets,
}
struct Inner {
    retired: Mutex<
        std::collections::VecDeque<(AuxiliaryCallId, ResultAvailability, tokio::time::Instant)>,
    >,
    changed: Arc<tokio::sync::Notify>,
    stop: tokio_util::sync::CancellationToken,
    entries: Mutex<BTreeMap<AuxiliaryCallId, Entry>>,
    retained: Capacity,
    copies: Capacity,
}
#[derive(Clone)]
pub(crate) struct DeliveryOwner(Arc<Inner>);
pub(crate) struct ResultReservation {
    owner: Weak<Inner>,
    budgets: Option<Budgets>,
}
/// The physical response must retain this owner through its final disposition.
pub struct ResultCopy {
    pub(crate) result: Option<AuxiliaryResult>,
    _budgets: Budgets,
}
impl ResultCopy {
    /// The authorized result remains counted until this copy is dropped.
    #[must_use]
    pub fn result(&self) -> Option<&AuxiliaryResult> {
        self.result.as_ref()
    }
}
fn process_retained() -> &'static Capacity {
    static CAP: OnceLock<Capacity> = OnceLock::new();
    CAP.get_or_init(|| Capacity::new(PROCESS_RESULT_SLOTS))
}
fn process_copies() -> &'static Capacity {
    static CAP: OnceLock<Capacity> = OnceLock::new();
    CAP.get_or_init(|| Capacity::new(PROCESS_RESULT_SLOTS))
}

impl DeliveryOwner {
    pub(crate) fn new() -> Self {
        let inner = Arc::new(Inner {
            retired: Mutex::default(),
            changed: Arc::default(),
            stop: tokio_util::sync::CancellationToken::new(),
            entries: Mutex::new(BTreeMap::new()),
            retained: Capacity::new(CONVERSATION_RESULT_SLOTS),
            copies: Capacity::new(CONVERSATION_RESULT_SLOTS),
        });
        tokio::spawn(cleanup(
            Arc::downgrade(&inner),
            inner.changed.clone(),
            inner.stop.clone(),
        ));
        Self(inner)
    }
    pub(crate) fn reserve(&self) -> Result<ResultReservation, &'static str> {
        Ok(ResultReservation {
            owner: Arc::downgrade(&self.0),
            budgets: Some(Budgets::reserve(process_retained(), &self.0.retained)?),
        })
    }
    pub(crate) fn availability(&self, call: &AuxiliaryCallId) -> ResultAvailability {
        let mut entries = self.0.entries.lock().expect("auxiliary results");
        expire(&self.0, &mut entries, call);
        entries.get(call).map_or_else(
            || {
                let mut retired = self.0.retired.lock().expect("result disposition");
                retired.retain(|(_, _, at)| tokio::time::Instant::now() < *at + RESULT_RETENTION);
                retired
                    .iter()
                    .find(|(id, _, _)| id == call)
                    .map_or(ResultAvailability::Unavailable, |(_, state, _)| *state)
            },
            |entry| {
                if entry.result.is_some() {
                    ResultAvailability::Retained
                } else {
                    ResultAvailability::Pending
                }
            },
        )
    }
    pub(crate) fn read(&self, call: &AuxiliaryCallId) -> Result<Option<ResultCopy>, &'static str> {
        let mut entries = self.0.entries.lock().expect("auxiliary results");
        expire(&self.0, &mut entries, call);
        let Some(result) = entries.get(call).and_then(|entry| entry.result.as_ref()) else {
            return Ok(None);
        };
        let mut budgets = Budgets::reserve(process_copies(), &self.0.copies)?;
        let bytes = encoded_len(result)?;
        budgets.shrink(bytes);
        Ok(Some(ResultCopy {
            result: Some(result.clone()),
            _budgets: budgets,
        }))
    }
    pub(crate) fn discard(&self, call: &AuxiliaryCallId) -> bool {
        // Removing a pending entry prevents later result retention without
        // cancelling the independently owned execution.
        let mut entries = self.0.entries.lock().expect("auxiliary results");
        let removed = entries.remove(call).is_some();
        if removed {
            remember(&self.0, call.clone(), ResultAvailability::Discarded);
        }
        removed
    }
    pub(crate) fn complete(&self, result: AuxiliaryResult) -> Result<(), &'static str> {
        let bytes = encoded_len(&result)?;
        let call = result.call_id.clone();
        let mut entries = self.0.entries.lock().expect("auxiliary results");
        let Some(entry) = entries.get_mut(&call) else {
            return Ok(());
        };
        if entry.completed.is_some() {
            return Err("auxiliary result already completed");
        }
        let now = tokio::time::Instant::now();
        entry.budgets.shrink(bytes);
        entry.completed = Some(now);
        entry.result = Some(result);
        drop(entries);
        self.0.changed.notify_one();
        Ok(())
    }
}
impl ResultReservation {
    pub(crate) fn install(mut self, call: AuxiliaryCallId) -> Result<(), &'static str> {
        let owner = self
            .owner
            .upgrade()
            .ok_or("auxiliary delivery owner closed")?;
        let mut entries = owner.entries.lock().expect("auxiliary results");
        if entries.contains_key(&call) {
            return Err("auxiliary result identity already reserved");
        }
        entries.insert(
            call,
            Entry {
                result: None,
                completed: None,
                budgets: self.budgets.take().expect("one reservation transfer"),
            },
        );
        Ok(())
    }
}
fn expire(owner: &Inner, entries: &mut BTreeMap<AuxiliaryCallId, Entry>, call: &AuxiliaryCallId) {
    if entries
        .get(call)
        .and_then(|entry| entry.completed)
        .is_some_and(|at| tokio::time::Instant::now() >= at + RESULT_RETENTION)
    {
        entries.remove(call);
        remember(owner, call.clone(), ResultAvailability::Expired);
    }
}
fn remember(owner: &Inner, call: AuxiliaryCallId, state: ResultAvailability) {
    let mut retired = owner.retired.lock().expect("result disposition");
    retired
        .retain(|(id, _, at)| id != &call && tokio::time::Instant::now() < *at + RESULT_RETENTION);
    if retired.len() == CONVERSATION_RESULT_SLOTS {
        retired.pop_front();
    }
    retired.push_back((call, state, tokio::time::Instant::now()));
    owner.changed.notify_one();
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
async fn cleanup(
    owner: Weak<Inner>,
    changed: Arc<tokio::sync::Notify>,
    stop: tokio_util::sync::CancellationToken,
) {
    loop {
        let deadline = {
            let Some(owner) = owner.upgrade() else {
                return;
            };
            let mut entries = owner.entries.lock().expect("auxiliary results");
            let expired: Vec<_> = entries
                .iter()
                .filter(|(_, entry)| {
                    entry
                        .completed
                        .is_some_and(|at| tokio::time::Instant::now() >= at + RESULT_RETENTION)
                })
                .map(|(call, _)| call.clone())
                .collect();
            for call in expired {
                expire(&owner, &mut entries, &call);
            }
            let mut retired = owner.retired.lock().expect("result disposition");
            retired.retain(|(_, _, at)| tokio::time::Instant::now() < *at + RESULT_RETENTION);
            entries
                .values()
                .filter_map(|entry| entry.completed)
                .chain(retired.iter().map(|(_, _, at)| *at))
                .min()
                .map(|at| at + RESULT_RETENTION)
        };
        tokio::select! {
            () = stop.cancelled() => return,
            () = changed.notified() => {},
            () = async { if let Some(at) = deadline { tokio::time::sleep_until(at).await; } else { std::future::pending().await } } => {},
        }
    }
}
fn encoded_len(result: &AuxiliaryResult) -> Result<usize, &'static str> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|n| *n <= RESULT_RECORD_BYTES_MAX - 4096)
                .ok_or_else(|| {
                    std::io::Error::other(
                        "auxiliary encoded result exceeds its envelope reservation",
                    )
                })?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Counter(0);
    serde_json::to_writer(&mut count, result)
        .map_err(|_| "auxiliary encoded result exceeds its envelope reservation")?;
    Ok(count.0 + 4096)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn result(call: AuxiliaryCallId) -> AuxiliaryResult {
        AuxiliaryResult {
            call_id: call,
            final_request_id: None,
            execution: Some(crate::model::AuxiliaryCallOutcome::Completed),
            value: ConsumerResult::Btw {
                text: "answer".into(),
            },
        }
    }
    #[tokio::test(start_paused = true)]
    async fn repeatable_reads_do_not_renew_retention_or_pin_capacity() {
        let owner = DeliveryOwner::new();
        let call = AuxiliaryCallId::generate();
        owner.reserve().unwrap().install(call.clone()).unwrap();
        owner.complete(result(call.clone())).unwrap();
        let copy = owner.read(&call).unwrap().unwrap();
        tokio::time::advance(
            RESULT_RETENTION
                .checked_sub(Duration::from_secs(1))
                .unwrap(),
        )
        .await;
        assert!(owner.read(&call).unwrap().is_some());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(owner.availability(&call), ResultAvailability::Expired);
        assert!(owner.read(&call).unwrap().is_none());
        assert_eq!(copy.result, Some(result(call)));
        assert_eq!(
            owner.0.retained.slots.available_permits(),
            CONVERSATION_RESULT_SLOTS
        );
        assert_eq!(
            owner.0.copies.slots.available_permits(),
            CONVERSATION_RESULT_SLOTS - 1
        );
        drop(copy);
        assert_eq!(
            owner.0.copies.slots.available_permits(),
            CONVERSATION_RESULT_SLOTS
        );
    }
    #[tokio::test]
    async fn admission_is_bounded_and_discard_wins_late_completion() {
        let owner = DeliveryOwner::new();
        let mut calls = Vec::new();
        for _ in 0..CONVERSATION_RESULT_SLOTS {
            let call = AuxiliaryCallId::generate();
            owner.reserve().unwrap().install(call.clone()).unwrap();
            calls.push(call);
        }
        assert!(owner.reserve().is_err());
        let call = &calls[0];
        assert!(owner.discard(call));
        owner.complete(result(call.clone())).unwrap();
        assert!(owner.read(call).unwrap().is_none());
        assert!(owner.reserve().is_ok());
    }
    #[test]
    fn escaped_output_and_metadata_obey_complete_record_bound() {
        let mut result = result(AuxiliaryCallId::generate());
        result.value = ConsumerResult::Btw {
            text: "\u{0000}".repeat(RESULT_TEXT_BYTES_MAX),
        };
        assert!(encoded_len(&result).unwrap() <= RESULT_RECORD_BYTES_MAX);
        result.value = ConsumerResult::Btw {
            text: "\u{0000}".repeat(RESULT_RECORD_BYTES_MAX),
        };
        assert!(encoded_len(&result).is_err());
    }
}

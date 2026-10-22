//! Process and Conversation capacity reserved without waiting or partial leaks.

use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Shared bound for physical Calls, required writers and independent retirement.
pub(super) const PROCESS_CALL_LIMIT: usize = 32;

/// The precise nonblocking reservation that refused admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityDomain {
    ProcessTotal,
    ProcessGeneral,
    ConversationTotal,
    ConversationGeneral,
    RequiredWriter,
}

/// No auxiliary work or durable admission was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuxiliaryBusy {
    pub domain: CapacityDomain,
}

impl std::fmt::Display for AuxiliaryBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "auxiliary capacity unavailable: {:?}", self.domain)
    }
}

impl std::error::Error for AuxiliaryBusy {}

/// Capacity shared by every participating Conversation in a native process.
#[derive(Debug)]
pub struct ProcessCapacity {
    total: Arc<Semaphore>,
    general: Arc<Semaphore>,
    writers: Arc<Semaphore>,
}

impl ProcessCapacity {
    /// Creates an isolated process composition with 32 total / 24 general slots.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            total: Arc::new(Semaphore::new(PROCESS_CALL_LIMIT)),
            general: Arc::new(Semaphore::new(24)),
            writers: Arc::new(Semaphore::new(PROCESS_CALL_LIMIT)),
        })
    }

    pub(crate) fn shared_process() -> Arc<Self> {
        static PROCESS: OnceLock<Arc<ProcessCapacity>> = OnceLock::new();
        PROCESS.get_or_init(Self::new).clone()
    }

    /// Creates one Conversation's 4 total / 3 general capacity domain.
    #[must_use]
    pub fn conversation(self: &Arc<Self>) -> ConversationCapacity {
        ConversationCapacity {
            process: self.clone(),
            total: Arc::new(Semaphore::new(4)),
            general: Arc::new(Semaphore::new(3)),
        }
    }
}

/// One Conversation's reservations against its local and process domains.
#[derive(Clone, Debug)]
pub struct ConversationCapacity {
    process: Arc<ProcessCapacity>,
    total: Arc<Semaphore>,
    general: Arc<Semaphore>,
}

impl ConversationCapacity {
    /// Reserves ordinary auxiliary work without borrowing protected capacity.
    ///
    /// # Errors
    /// Returns the exhausted domain immediately and rolls back partial reservations.
    pub fn reserve_general(&self) -> Result<CallPermit, AuxiliaryBusy> {
        self.reserve(false)
    }

    // Only native Runtime/Compaction code can select this admission class.
    pub(crate) fn reserve_compaction(&self) -> Result<CallPermit, AuxiliaryBusy> {
        self.reserve(true)
    }

    fn reserve(&self, compaction: bool) -> Result<CallPermit, AuxiliaryBusy> {
        let process_total = take(&self.process.total, CapacityDomain::ProcessTotal)?;
        let process_general = if compaction {
            None
        } else {
            Some(take(&self.process.general, CapacityDomain::ProcessGeneral)?)
        };
        let conversation_total = take(&self.total, CapacityDomain::ConversationTotal)?;
        let conversation_general = if compaction {
            None
        } else {
            Some(take(&self.general, CapacityDomain::ConversationGeneral)?)
        };
        let writer = take(&self.process.writers, CapacityDomain::RequiredWriter)?;
        Ok(CallPermit {
            _writer: writer,
            _conversation_general: conversation_general,
            _conversation_total: conversation_total,
            _process_general: process_general,
            _process_total: process_total,
        })
    }
}

/// Reservations retained by the native owner through execution and confirmation.
/// A consumer's reply/observer handle must never own or release this value.
#[derive(Debug)]
pub struct CallPermit {
    // Declaration order releases in reverse acquisition order.
    _writer: OwnedSemaphorePermit,
    _conversation_general: Option<OwnedSemaphorePermit>,
    _conversation_total: OwnedSemaphorePermit,
    _process_general: Option<OwnedSemaphorePermit>,
    _process_total: OwnedSemaphorePermit,
}

fn take(
    semaphore: &Arc<Semaphore>,
    domain: CapacityDomain,
) -> Result<OwnedSemaphorePermit, AuxiliaryBusy> {
    semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| AuxiliaryBusy { domain })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    // Successful workers return their permits rather than releasing them. Every
    // contender therefore observes the same admission wave, not serial reuse.
    fn contend(
        conversations: Vec<ConversationCapacity>,
        compaction: bool,
    ) -> (Vec<CallPermit>, Vec<CapacityDomain>) {
        std::thread::scope(|scope| {
            let mut releases = Vec::new();
            let workers: Vec<_> = conversations
                .into_iter()
                .map(|conversation| {
                    let (release, ready) = std::sync::mpsc::channel::<Arc<Barrier>>();
                    releases.push(release);
                    std::thread::Builder::new()
                        .spawn_scoped(scope, move || {
                            let start = ready.recv().ok()?;
                            start.wait();
                            Some(if compaction {
                                conversation.reserve_compaction()
                            } else {
                                conversation.reserve_general()
                            })
                        })
                        .expect("spawn capacity contender")
                })
                .collect();
            // No worker enters the fixed-count barrier until every spawn has
            // succeeded. A spawn panic drops these senders, so existing workers
            // exit instead of deadlocking scoped-thread cleanup at the barrier.
            let start = Arc::new(Barrier::new(workers.len() + 1));
            for release in releases {
                release.send(start.clone()).unwrap();
            }
            start.wait();
            let mut admitted = Vec::new();
            let mut refused = Vec::new();
            for worker in workers {
                match worker
                    .join()
                    .expect("capacity contender panicked")
                    .expect("capacity contender was released")
                {
                    Ok(permit) => admitted.push(permit),
                    Err(busy) => refused.push(busy.domain),
                }
            }
            (admitted, refused)
        })
    }

    fn available(conversation: &ConversationCapacity) -> [usize; 5] {
        [
            conversation.process.total.available_permits(),
            conversation.process.general.available_permits(),
            conversation.total.available_permits(),
            conversation.general.available_permits(),
            conversation.process.writers.available_permits(),
        ]
    }

    #[test]
    fn ordinary_work_leaves_one_local_compaction_slot() {
        let process = ProcessCapacity::new();
        let conversation = process.conversation();
        let ordinary: Vec<_> = (0..3)
            .map(|_| conversation.reserve_general().unwrap())
            .collect();
        assert_eq!(
            conversation.reserve_general().unwrap_err().domain,
            CapacityDomain::ConversationGeneral
        );
        let compaction = conversation.reserve_compaction().unwrap();
        assert_eq!(
            conversation.reserve_compaction().unwrap_err().domain,
            CapacityDomain::ConversationTotal
        );
        drop(ordinary);
        drop(compaction);
        assert!(conversation.reserve_general().is_ok());
    }

    #[test]
    fn conversations_share_process_limits_and_reserved_writer_progress() {
        let process = ProcessCapacity::new();
        let conversations: Vec<_> = (0..9).map(|_| process.conversation()).collect();
        let mut permits = Vec::new();
        for conversation in conversations.iter().take(8) {
            for _ in 0..3 {
                permits.push(conversation.reserve_general().unwrap());
            }
        }
        assert_eq!(
            conversations[8].reserve_general().unwrap_err().domain,
            CapacityDomain::ProcessGeneral
        );
        for conversation in conversations.iter().take(8) {
            permits.push(conversation.reserve_compaction().unwrap());
        }
        assert_eq!(
            conversations[8].reserve_compaction().unwrap_err().domain,
            CapacityDomain::ProcessTotal
        );
        drop(permits);
        assert!(conversations[8].reserve_general().is_ok());
    }

    #[test]
    fn repeated_partial_refusal_does_not_leak_process_capacity() {
        let process = ProcessCapacity::new();
        let full = process.conversation();
        let mut permits: Vec<_> = (0..3).map(|_| full.reserve_general().unwrap()).collect();
        for _ in 0..100 {
            assert!(full.reserve_general().is_err());
        }
        let others: Vec<_> = (0..7).map(|_| process.conversation()).collect();
        for conversation in &others {
            for _ in 0..3 {
                permits.push(conversation.reserve_general().unwrap());
            }
        }
        assert_eq!(permits.len(), 24);
        assert!(process.conversation().reserve_general().is_err());
    }

    #[test]
    fn simultaneous_local_contenders_keep_the_fourth_call_and_writer_protected() {
        let process = ProcessCapacity::new();
        let conversation = process.conversation();
        let (general, refused) = contend(vec![conversation.clone(); 16], false);
        assert_eq!(general.len(), 3);
        assert_eq!(refused.len(), 13);
        // A losing general contender can briefly hold the fourth total slot,
        // so another nonblocking contender may refuse at that earlier layer.
        assert!(refused.iter().all(|domain| matches!(
            domain,
            CapacityDomain::ConversationTotal | CapacityDomain::ConversationGeneral
        )));
        assert_eq!(available(&conversation), [29, 21, 1, 0, 29]);

        let (protected, refused) = contend(vec![conversation.clone(); 16], true);
        assert_eq!(protected.len(), 1);
        assert_eq!(refused, vec![CapacityDomain::ConversationTotal; 15]);
        assert_eq!(available(&conversation), [28, 21, 0, 0, 28]);

        drop(general);
        assert_eq!(available(&conversation), [31, 24, 3, 3, 31]);
        let (replacement, refused) = contend(vec![conversation.clone(); 3], false);
        assert_eq!(replacement.len(), 3);
        assert!(refused.is_empty());
        drop(protected);
        drop(replacement);
        assert_eq!(available(&conversation), [32, 24, 4, 3, 32]);
    }

    #[test]
    fn simultaneous_process_contenders_preserve_twenty_four_plus_eight_and_recover() {
        let process = ProcessCapacity::new();
        let conversations: Vec<_> = (0..40).map(|_| process.conversation()).collect();
        let (general, refused) = contend(conversations.clone(), false);
        assert_eq!(general.len(), 24);
        assert_eq!(refused.len(), 16);
        assert!(refused.iter().all(|domain| matches!(
            domain,
            CapacityDomain::ProcessTotal | CapacityDomain::ProcessGeneral
        )));
        assert_eq!(process.total.available_permits(), 8);
        assert_eq!(process.general.available_permits(), 0);
        assert_eq!(process.writers.available_permits(), 8);

        let (protected, refused) = contend(conversations.clone(), true);
        assert_eq!(protected.len(), 8);
        assert_eq!(refused, vec![CapacityDomain::ProcessTotal; 32]);
        assert_eq!(process.total.available_permits(), 0);
        assert_eq!(process.writers.available_permits(), 0);

        // Releasing only the protected class cannot refund a general permit.
        drop(protected);
        let (unexpected, refused) = contend(conversations.clone(), false);
        assert!(unexpected.is_empty());
        assert_eq!(refused.len(), 40);
        assert!(refused.iter().all(|domain| matches!(
            domain,
            CapacityDomain::ProcessTotal | CapacityDomain::ProcessGeneral
        )));
        assert_eq!(process.total.available_permits(), 8);
        assert_eq!(process.general.available_permits(), 0);
        assert_eq!(process.writers.available_permits(), 8);

        drop(general);
        for conversation in &conversations {
            assert_eq!(available(conversation), [32, 24, 4, 3, 32]);
        }
        let (recovered, refused) = contend(conversations, true);
        assert_eq!(recovered.len(), 32);
        assert_eq!(refused, vec![CapacityDomain::ProcessTotal; 8]);
        drop(recovered);
        assert_eq!(available(&process.conversation()), [32, 24, 4, 3, 32]);
    }

    #[test]
    fn refusal_at_every_acquisition_layer_rolls_back_all_earlier_permits() {
        for (domain, compaction) in [
            (CapacityDomain::ProcessTotal, false),
            (CapacityDomain::ProcessGeneral, false),
            (CapacityDomain::ConversationTotal, false),
            (CapacityDomain::ConversationGeneral, false),
            (CapacityDomain::RequiredWriter, false),
            (CapacityDomain::ProcessTotal, true),
            (CapacityDomain::ConversationTotal, true),
            (CapacityDomain::RequiredWriter, true),
        ] {
            let process = ProcessCapacity::new();
            let conversation = process.conversation();
            let (semaphore, limit) = match domain {
                CapacityDomain::ProcessTotal => (&process.total, 32),
                CapacityDomain::ProcessGeneral => (&process.general, 24),
                CapacityDomain::ConversationTotal => (&conversation.total, 4),
                CapacityDomain::ConversationGeneral => (&conversation.general, 3),
                CapacityDomain::RequiredWriter => (&process.writers, 32),
            };
            // Isolate each layer, including the otherwise unreachable writer
            // shortage, without replacing the production acquisition path.
            let blocker = semaphore.clone().try_acquire_many_owned(limit).unwrap();
            let before = available(&conversation);
            let (admitted, refused) = contend(vec![conversation.clone(); 3], compaction);
            assert!(admitted.is_empty(), "{domain:?}, protected={compaction}");
            assert_eq!(refused, vec![domain; 3]);
            assert_eq!(available(&conversation), before, "rollback at {domain:?}");

            drop(blocker);
            assert_eq!(available(&conversation), [32, 24, 4, 3, 32]);
            let (general, refused) = contend(vec![conversation.clone(); 3], false);
            assert_eq!(general.len(), 3);
            assert!(refused.is_empty());
            let protected = conversation.reserve_compaction().unwrap();
            assert_eq!(available(&conversation), [28, 21, 0, 0, 28]);
            drop(general);
            drop(protected);
            assert_eq!(available(&conversation), [32, 24, 4, 3, 32]);
        }
    }
}

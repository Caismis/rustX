//! Process and Conversation capacity reserved without waiting or partial leaks.

use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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
            total: Arc::new(Semaphore::new(32)),
            general: Arc::new(Semaphore::new(24)),
            writers: Arc::new(Semaphore::new(32)),
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
}

//! Issue #63 — durable pending inbound inbox: backend-independent conformance
//! and deterministic crash/transaction regressions over the `SQLite` backend.
//!
//! Every test here speaks to the public `rustx::durable` domain operations
//! (`accept_inbound`, `select_pending_batch`, `adopt_pending_batch`,
//! `load_pending`, `load_canonical`) so a future M11 `PostgreSQL` backend can
//! share the same observable contract. Durability regressions reopen a
//! file-backed store across an explicit drop, which is the exact "process
//! died here" boundary — no sleeps and no timing assumptions participate.

use chrono::{DateTime, TimeZone, Utc};
use rustx::durable::{
    AcceptedInbound, InboundDraft, InboundStore, InboundStoreError, SqliteInboundStore,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{InboundKind, UserContentBlock, UserSource};
use rustx::runtime::identity::{AgentId, ConversationId, MessageId};
use std::sync::Arc;
use tempfile::tempdir;

fn fixed_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0)
        .single()
        .expect("valid fixed time")
}

fn text_blocks(text: &str) -> Vec<UserContentBlock> {
    vec![UserContentBlock::Text(TextBlock {
        text: text.to_owned(),
    })]
}

fn human(text: &str) -> InboundDraft {
    InboundDraft {
        message_id: None,
        source: UserSource::Human,
        kind: InboundKind::Message,
        content: text_blocks(text),
        timestamp: fixed_time(),
        correlation: None,
    }
}

fn runtime(text: &str) -> InboundDraft {
    InboundDraft {
        source: UserSource::Runtime,
        ..human(text)
    }
}

fn agent(text: &str) -> InboundDraft {
    InboundDraft {
        source: UserSource::Agent {
            agent_id: AgentId::new("agent-b"),
        },
        ..human(text)
    }
}

/// Opens a file-backed store at a fresh temp path and returns it with the
/// path retained so the same file can be reopened.
fn file_store() -> (SqliteInboundStore, std::path::PathBuf) {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("inbound.db");
    let store = SqliteInboundStore::open(ConversationId::new("conv-1"), &path).expect("open store");
    // Leak the temp dir so the file survives this scope; tests reopen it.
    std::mem::forget(dir);
    (store, path)
}

/// Accepted Human, Runtime, and Agent inbound each survive an immediate
/// reopen (process death) before any adoption, with their exact identity,
/// sequence, provenance, and timestamp intact.
#[test]
fn accepted_inbound_survives_reopen_before_adoption() {
    let (store, path) = file_store();
    let drafts = [human("hi"), runtime("notice"), agent("from agent")];
    let mut accepted: Vec<AcceptedInbound> = Vec::new();
    for draft in &drafts {
        accepted.push(store.accept_inbound(draft.clone()).expect("accept"));
    }
    assert_eq!(
        accepted
            .iter()
            .map(|a| a.sequence.get())
            .collect::<Vec<_>>(),
        vec![1, 2, 3],
        "one sequence domain, strict order"
    );
    drop(store);

    let reopened =
        SqliteInboundStore::open(ConversationId::new("conv-1"), &path).expect("reopen store");
    let pending = reopened.load_pending().expect("load pending");
    assert_eq!(pending.len(), 3);
    assert_eq!(pending[0].message_id, accepted[0].message_id);
    assert_eq!(pending[0].sequence, accepted[0].sequence);
    assert_eq!(pending[0].message.source, UserSource::Human);
    assert_eq!(pending[1].message.source, UserSource::Runtime);
    assert_eq!(
        pending[2].message.source,
        UserSource::Agent {
            agent_id: AgentId::new("agent-b"),
        }
    );
    for item in &pending {
        assert_eq!(
            item.message.timestamp,
            Some(fixed_time()),
            "the persisted producer timestamp is preserved"
        );
    }
    assert!(
        reopened
            .load_canonical()
            .expect("load canonical")
            .is_empty(),
        "no canonical adoption happened before the crash"
    );
    let _ = path;
}

/// Mixed provenance shares one durable per-conversation sequence domain and
/// the committed order remains stable after reopening the store.
#[test]
fn mixed_provenance_uses_one_durable_sequence_domain() {
    let (store, path) = file_store();
    store.accept_inbound(human("a")).expect("human");
    store.accept_inbound(runtime("b")).expect("runtime");
    store.accept_inbound(agent("c")).expect("agent");
    drop(store);

    let reopened = SqliteInboundStore::open(ConversationId::new("conv-1"), &path).expect("reopen");
    let pending = reopened.load_pending().expect("load");
    assert_eq!(
        pending.iter().map(|i| i.sequence.get()).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "committed order is stable across reopen"
    );
    assert_eq!(pending[0].message_id.as_str(), "conv-1-inbound-1");
    assert_eq!(pending[1].message_id.as_str(), "conv-1-inbound-2");
    assert_eq!(pending[2].message_id.as_str(), "conv-1-inbound-3");
    let _ = path;
}

/// A safe boundary freezes a finite watermark: an item accepted after the
/// selection is excluded from the selected batch and belongs to the next one.
#[test]
fn finite_watermark_excludes_post_watermark_arrivals() {
    let store =
        Arc::new(SqliteInboundStore::in_memory(ConversationId::new("conv-1")).expect("in-memory"));
    store.accept_inbound(human("A")).expect("A");
    store.accept_inbound(human("B")).expect("B");
    let batch = store
        .select_pending_batch()
        .expect("select")
        .expect("batch");
    assert_eq!(batch.watermark.get(), 2);
    // A post-watermark arrival.
    store.accept_inbound(runtime("C")).expect("C");
    // The selected batch is frozen: adoption through its watermark adopts
    // exactly A and B, never C.
    let adopted = store.adopt_pending_batch(batch.watermark).expect("adopt");
    assert_eq!(adopted.len(), 2);
    let remaining = store
        .select_pending_batch()
        .expect("select")
        .expect("remaining");
    assert_eq!(remaining.watermark.get(), 3);
    assert_eq!(remaining.items.len(), 1);
    assert_eq!(remaining.items[0].message_id.as_str(), "conv-1-inbound-3");
}

/// Crash before adoption leaves the item pending; crash after adoption leaves
/// it canonical exactly once and never re-adoptable.
#[test]
fn adoption_is_atomic_and_exactly_once_across_reopen() {
    let (store, path) = file_store();
    store.accept_inbound(human("one")).expect("accept");
    store.accept_inbound(human("two")).expect("accept");
    let batch = store
        .select_pending_batch()
        .expect("select")
        .expect("batch");
    let adopted = store.adopt_pending_batch(batch.watermark).expect("adopt");
    assert_eq!(adopted.len(), 2);
    drop(store);

    // Crash after the adoption commit: canonical owns both messages exactly
    // once and they are no longer independently re-adoptable.
    let reopened = SqliteInboundStore::open(ConversationId::new("conv-1"), &path).expect("reopen");
    assert!(
        reopened.load_pending().expect("load pending").is_empty(),
        "no pending record survives adoption"
    );
    let canonical = reopened.load_canonical().expect("load canonical");
    assert_eq!(canonical.len(), 2);
    assert_eq!(
        canonical
            .iter()
            .map(rustx::conversation::message_id_of)
            .collect::<Vec<_>>(),
        vec![
            MessageId::new("conv-1-inbound-1"),
            MessageId::new("conv-1-inbound-2"),
        ],
        "each adopted inbound is a distinct canonical message in sequence order"
    );
    // Adopting the same watermark again finds nothing pending.
    let again = reopened
        .adopt_pending_batch(rustx::runtime::inbound::InboundSequence::new(2))
        .expect("adopt again");
    assert!(
        again.is_empty(),
        "an adopted item can never re-enter adoption"
    );
    let _ = path;
}

/// A producer retry with the same committed correlation is exactly-once: it
/// returns the same acceptance (even after adoption) and never allocates a
/// second semantic delivery.
#[test]
fn producer_correlation_retry_is_exactly_once() {
    let store =
        Arc::new(SqliteInboundStore::in_memory(ConversationId::new("conv-1")).expect("in-memory"));
    let draft = InboundDraft {
        message_id: Some(MessageId::new("background-exec_1-terminal")),
        source: UserSource::Runtime,
        kind: InboundKind::Message,
        content: text_blocks("settled"),
        timestamp: fixed_time(),
        correlation: Some("background-terminal:exec_1".to_owned()),
    };
    let first = store.accept_inbound(draft.clone()).expect("accept");
    assert_eq!(first.sequence.get(), 1);
    let retry = store.accept_inbound(draft).expect("retry");
    assert_eq!(retry.sequence, first.sequence);
    assert_eq!(retry.message_id, first.message_id);
    assert!(retry.retried);
    assert_eq!(store.load_pending().expect("load").len(), 1);

    // Adopt, then retry again: the retry still resolves to the same
    // acceptance and produces no new pending/canonical delivery.
    store.adopt_pending_batch(first.sequence).expect("adopt");
    let after_adopt = store
        .accept_inbound(InboundDraft {
            message_id: Some(MessageId::new("background-exec_1-terminal")),
            source: UserSource::Runtime,
            kind: InboundKind::Message,
            content: text_blocks("settled"),
            timestamp: fixed_time(),
            correlation: Some("background-terminal:exec_1".to_owned()),
        })
        .expect("retry after adoption");
    assert_eq!(after_adopt.sequence, first.sequence);
    assert!(after_adopt.retried);
    assert!(
        store.load_pending().expect("load").is_empty(),
        "no duplicate pending delivery is manufactured"
    );
    assert_eq!(
        store.load_canonical().expect("load").len(),
        1,
        "the canonical delivery stays exactly-once"
    );
}

/// A failed acceptance returns no success and leaves no visible pending item
/// or consumed sequence.
#[test]
fn failed_acceptance_leaves_nothing() {
    let store =
        Arc::new(SqliteInboundStore::in_memory(ConversationId::new("conv-1")).expect("in-memory"));
    // Empty content is rejected before any durable work.
    let empty = InboundDraft {
        content: Vec::new(),
        ..human("empty")
    };
    assert!(matches!(
        store.accept_inbound(empty),
        Err(InboundStoreError::EmptyContent)
    ));
    // A producer-supplied duplicate message id is rejected and consumes
    // nothing.
    store.accept_inbound(human("ok")).expect("ok");
    let duplicate = InboundDraft {
        message_id: Some(MessageId::new("conv-1-inbound-1")),
        ..human("duplicate")
    };
    assert!(matches!(
        store.accept_inbound(duplicate),
        Err(InboundStoreError::DuplicateMessageId(_))
    ));
    assert_eq!(store.load_pending().expect("load").len(), 1);
    let next = store.accept_inbound(human("next")).expect("next");
    assert_eq!(
        next.sequence.get(),
        2,
        "no sequence was consumed by failures"
    );
}

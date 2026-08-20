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
    AcceptedInbound, ConversationStore, ConversationStoreError, InboundDraft,
    SqliteConversationStore,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, ToolMessageBlock,
    UserContentBlock, UserSource,
};
use rustx::runtime::identity::{AgentId, ConversationId, MessageId, ToolCallId, ToolId};
use rustx::tools::types::{ToolExecutionResult, ToolExecutionStatus};
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
fn file_store() -> (SqliteConversationStore, std::path::PathBuf) {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("inbound.db");
    let store =
        SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("open store");
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
        SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("reopen store");
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

    let reopened =
        SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("reopen");
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
    let store = Arc::new(
        SqliteConversationStore::in_memory(ConversationId::new("conv-1")).expect("in-memory"),
    );
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
    let reopened =
        SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("reopen");
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
    let store = Arc::new(
        SqliteConversationStore::in_memory(ConversationId::new("conv-1")).expect("in-memory"),
    );
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
    let store = Arc::new(
        SqliteConversationStore::in_memory(ConversationId::new("conv-1")).expect("in-memory"),
    );
    // Empty content is rejected before any durable work.
    let empty = InboundDraft {
        content: Vec::new(),
        ..human("empty")
    };
    assert!(matches!(
        store.accept_inbound(empty),
        Err(ConversationStoreError::EmptyContent)
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
        Err(ConversationStoreError::DuplicateMessageId(_))
    ));
    assert_eq!(store.load_pending().expect("load").len(), 1);
    let next = store.accept_inbound(human("next")).expect("next");
    assert_eq!(
        next.sequence.get(),
        2,
        "no sequence was consumed by failures"
    );
}

fn assistant_block(id: &str) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: vec![AssistantContentBlock::Text(TextBlock {
            text: format!("assistant {id}"),
        })],
    })
}

fn tool_block(id: &str) -> MessageBlock {
    MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new(id),
        tool_call_id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-a"),
        result: ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: Vec::new(),
            duration_ms: 1,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        },
    })
}

/// Issue #63 store identity (Finding 4): the durable database binds itself to
/// one `ConversationId` on first creation and rejects a reopen under a
/// different identity without mutating the existing data.
#[test]
fn store_identity_binds_on_create_and_rejects_a_mismatched_reopen() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("inbound.db");
    // First open binds the database to conv-A.
    let store = SqliteConversationStore::open(ConversationId::new("conv-A"), &path).expect("open");
    store.accept_inbound(human("hi")).expect("accept");
    drop(store);
    // Reopen as conv-A succeeds with the original data intact.
    let reopened =
        SqliteConversationStore::open(ConversationId::new("conv-A"), &path).expect("reopen");
    assert_eq!(reopened.load_pending().expect("load").len(), 1);
    drop(reopened);
    // Reopen as conv-B is a typed failure.
    let mismatch = SqliteConversationStore::open(ConversationId::new("conv-B"), &path);
    assert!(matches!(
        mismatch,
        Err(ConversationStoreError::ConversationIdMismatch { stored, requested })
            if stored == ConversationId::new("conv-A") && requested == ConversationId::new("conv-B")
    ));
    // No mutation: conv-A still owns its accepted pending item.
    let again =
        SqliteConversationStore::open(ConversationId::new("conv-A"), &path).expect("reopen A");
    assert_eq!(
        again.load_pending().expect("load").len(),
        1,
        "the rejected open mutated nothing"
    );
}

/// Issue #63 canonical adoption (Finding 2): the durable Message Ledger is a
/// complete ordered prefix — it preserves Assistant and Tool facts that occur
/// between two inbound adoption commits, not a filtered subsequence.
#[test]
fn canonical_ledger_preserves_intervening_assistant_and_tool_facts_across_reopen() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("inbound.db");
    let store = SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("open");
    // Initial canonical prefix.
    store
        .initialize(&[MessageBlock::User(
            rustx::message::types::UserMessageBlock {
                id: MessageId::new("msg-user-0"),
                content: text_blocks("start"),
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            },
        )])
        .expect("seed");
    // Inbound batch 1.
    store.accept_inbound(human("A")).expect("accept A");
    store
        .adopt_pending_batch(rustx::runtime::inbound::InboundSequence::new(1))
        .expect("adopt A");
    // Intervening canonical facts (assistant + tool) between the two
    // inbound adoptions, appended through the canonical durability seam.
    store
        .append_canonical(&assistant_block("assistant-1"))
        .expect("assistant");
    store.append_canonical(&tool_block("tool-1")).expect("tool");
    // Inbound batch 2.
    store.accept_inbound(human("B")).expect("accept B");
    store
        .adopt_pending_batch(rustx::runtime::inbound::InboundSequence::new(2))
        .expect("adopt B");
    drop(store);

    // Reopen: the durable ledger is the complete ordered prefix, never a
    // filtered subsequence.
    let reopened =
        SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("reopen");
    let canonical = reopened.load_canonical().expect("load canonical");
    let ids: Vec<String> = canonical
        .iter()
        .map(|block| rustx::conversation::message_id_of(block).into_string())
        .collect();
    assert_eq!(
        ids,
        vec![
            "msg-user-0",
            "conv-1-inbound-1",
            "assistant-1",
            "tool-1",
            "conv-1-inbound-2",
        ],
        "the durable ledger is the exact canonical ordering"
    );
    assert!(
        reopened.load_pending().expect("load pending").is_empty(),
        "both inbound batches were adopted exactly once"
    );
}

/// Issue #63 (seed identity): reopening an existing durable conversation
/// verifies that the re-supplied bootstrap initial messages equal the
/// persisted initial prefix instead of silently ignoring a mismatch.
#[test]
fn initialize_verifies_the_initial_history() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("inbound.db");
    let initial = [MessageBlock::User(
        rustx::message::types::UserMessageBlock {
            id: MessageId::new("msg-user-0"),
            content: text_blocks("start"),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        },
    )];
    let store = SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("open");
    store.initialize(&initial).expect("seed");
    drop(store);

    // A matching re-supply is accepted.
    let reopened =
        SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("reopen");
    reopened.initialize(&initial).expect("matching seed");
    drop(reopened);

    // A mismatched re-supply is a typed failure, not a silent ignore.
    let reopened =
        SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("reopen");
    let mismatch = reopened.initialize(&[MessageBlock::User(
        rustx::message::types::UserMessageBlock {
            id: MessageId::new("msg-user-OTHER"),
            content: text_blocks("different"),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        },
    )]);
    assert!(matches!(
        mismatch,
        Err(ConversationStoreError::InitialHistoryMismatch)
    ));
}

/// Issue #63 bootstrap identity: the durable store records one immutable
/// bootstrap initial-history identity (exact message count + content
/// digest) at the first seed, and every reopen must re-supply an initial
/// history exactly equal to the original — never a shorter prefix, never
/// an empty replacement, never the same identities with changed content.
#[test]
fn initial_history_identity_is_exact() {
    let user = |id: &str, text: &str| {
        MessageBlock::User(rustx::message::types::UserMessageBlock {
            id: MessageId::new(id),
            content: text_blocks(text),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    };
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("inbound.db");
    let original = vec![user("msg-a", "A"), user("msg-b", "B")];

    // First bootstrap establishes the identity (and the seed rows).
    let store = SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("open");
    store.initialize(&original).expect("first bootstrap");
    // Grow the durable Ledger beyond the bootstrap boundary, exactly as
    // live execution does (adopted inbound, assistant facts, ...).
    store.accept_inbound(human("C")).expect("accept C");
    store
        .adopt_pending_batch(rustx::runtime::inbound::InboundSequence::new(1))
        .expect("adopt C");
    drop(store);

    // Reopen with the exact original: accepted even though the Ledger has
    // grown past the bootstrap boundary.
    let reopened =
        SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("reopen");
    reopened
        .initialize(&original)
        .expect("the exact original initial history is accepted");

    // A shorter prefix is rejected: the boundary is not inferred from the
    // current Ledger.
    assert!(
        matches!(
            reopened.initialize(&original[..1]),
            Err(ConversationStoreError::InitialHistoryMismatch)
        ),
        "a shorter prefix of the original bootstrap must be rejected"
    );
    // An empty replacement of a non-empty bootstrap is rejected.
    assert!(
        matches!(
            reopened.initialize(&[]),
            Err(ConversationStoreError::InitialHistoryMismatch)
        ),
        "an empty replacement of a non-empty bootstrap must be rejected"
    );
    // The same identities with changed semantic content are rejected.
    assert!(
        matches!(
            reopened.initialize(&[user("msg-a", "changed"), user("msg-b", "B")]),
            Err(ConversationStoreError::InitialHistoryMismatch)
        ),
        "changed content under the same identities must be rejected"
    );
    // A longer re-supply (original plus extra messages) is rejected.
    assert!(
        matches!(
            reopened.initialize(&[user("msg-a", "A"), user("msg-b", "B"), user("msg-c", "C")]),
            Err(ConversationStoreError::InitialHistoryMismatch)
        ),
        "a superset of the original bootstrap must be rejected"
    );
    drop(reopened);
    // The exact original is still accepted after every rejected attempt:
    // a failed validation mutates nothing.
    let reopened =
        SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("reopen");
    reopened
        .initialize(&original)
        .expect("rejected validations never consume the identity");
}

/// Issue #63 bootstrap identity: an explicitly empty initial history is a
/// valid bootstrap, recorded distinctly from "never initialized" — a later
/// empty re-supply is accepted and a non-empty one is rejected.
#[test]
fn empty_initial_history_is_an_explicit_bootstrap_identity() {
    let user = |id: &str, text: &str| {
        MessageBlock::User(rustx::message::types::UserMessageBlock {
            id: MessageId::new(id),
            content: text_blocks(text),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    };
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("inbound.db");

    // First bootstrap with an explicitly empty initial history.
    let store = SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("open");
    store
        .initialize(&[])
        .expect("an empty initial history is a valid bootstrap");
    drop(store);

    // Reopen: empty matches the recorded empty bootstrap exactly.
    let reopened =
        SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("reopen");
    reopened
        .initialize(&[])
        .expect("the recorded empty bootstrap accepts an empty re-supply");
    // A non-empty re-supply is rejected: "initialized empty" is not
    // "uninitialized".
    assert!(
        matches!(
            reopened.initialize(&[user("msg-a", "A")]),
            Err(ConversationStoreError::InitialHistoryMismatch)
        ),
        "a non-empty re-supply over an empty bootstrap must be rejected"
    );
}

/// Issue #63 bootstrap identity: a canonical Ledger that exists without
/// its bootstrap identity fails closed — the initial-history boundary is
/// never guessed from the current Ledger content.
#[test]
fn ledger_without_bootstrap_identity_fails_closed() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("inbound.db");
    let store = SqliteConversationStore::open(ConversationId::new("conv-1"), &path).expect("open");
    // Build canonical content without ever seeding (accepted + adopted
    // inbound appends to the Ledger directly).
    store.accept_inbound(human("A")).expect("accept A");
    store
        .adopt_pending_batch(rustx::runtime::inbound::InboundSequence::new(1))
        .expect("adopt A");
    assert_eq!(store.load_canonical().expect("load").len(), 1);

    // A first seed over an orphan Ledger cannot establish an exact
    // bootstrap boundary: it fails closed instead of guessing.
    assert!(
        matches!(
            store.initialize(&[]),
            Err(ConversationStoreError::Storage(_))
        ),
        "an orphan canonical Ledger fails closed"
    );
}

/// Issue #63 (correlation identity): reusing an idempotency key with a
/// conflicting semantic payload is a typed conflict, never a silent return of
/// the original acceptance.
#[test]
fn correlation_conflict_is_rejected_typed() {
    let store = Arc::new(
        SqliteConversationStore::in_memory(ConversationId::new("conv-1")).expect("in-memory"),
    );
    let base = InboundDraft {
        message_id: Some(MessageId::new("background-exec_1-terminal")),
        source: UserSource::Runtime,
        kind: InboundKind::Message,
        content: text_blocks("settled"),
        timestamp: fixed_time(),
        correlation: Some("background-terminal:exec_1".to_owned()),
    };
    store.accept_inbound(base.clone()).expect("accept");
    let conflict = store
        .accept_inbound(InboundDraft {
            content: text_blocks("different payload"),
            ..base.clone()
        })
        .expect_err("conflicting payload must be rejected");
    assert!(matches!(
        conflict,
        ConversationStoreError::CorrelationConflict { ref correlation } if correlation == "background-terminal:exec_1"
    ));
    // The original acceptance is unchanged.
    assert_eq!(store.load_pending().expect("load").len(), 1);
}

/// The remaining restart gate is structural: an incomplete tool turn fails
/// closed, while durable Surface compaction history is safe to reopen.
#[test]
fn recovery_safety_fails_closed_on_incomplete_or_compacted_prefixes() {
    use rustx::conversation::recovery_safety;
    let user = |id: &str| {
        MessageBlock::User(rustx::message::types::UserMessageBlock {
            id: MessageId::new(id),
            content: text_blocks("hi"),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(fixed_time()),
        })
    };
    let assistant = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("assistant-1"),
        content: vec![AssistantContentBlock::ToolCall(
            rustx::tools::types::ToolCall {
                id: rustx::runtime::identity::ToolCallId::new("call-1"),
                tool_id: rustx::runtime::identity::ToolId::new("tool-a"),
                name: "alpha".to_owned(),
                arguments: serde_json::json!({}),
            },
        )],
    });
    let tool = tool_block("call-1");

    // Complete tool group is safe.
    recovery_safety(&[user("u0"), assistant.clone(), tool.clone()]).expect("complete is safe");

    // Incomplete tool tail fails closed.
    assert!(matches!(
        recovery_safety(&[user("u0"), assistant.clone()]),
        Err(rustx::conversation::RecoverySafetyError::IncompleteToolTurn { .. })
    ));

    // Compaction summaries are ordinary durable Ledger facts whose Surface
    // replacement is validated by the M8 store, so the predicate does not
    // reject them.
    let summary = MessageBlock::User(rustx::message::types::UserMessageBlock {
        id: MessageId::new("summary-1"),
        content: text_blocks("earlier context"),
        source: UserSource::Runtime,
        kind: InboundKind::CompactionSummary,
        timestamp: None,
    });
    recovery_safety(&[user("u0"), user("u1"), summary])
        .expect("durable Surface history makes compaction restart-safe");
}

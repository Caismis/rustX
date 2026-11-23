//! The per-connection conditional-write owner: one outstanding staged
//! preparation at a time, Client-assigned tickets, and the shared physical
//! commit kernel.
//!
//! Tickets are nonzero `u64` scalars issued monotonically on this
//! connection; a ticket is consumed by exactly one `CommitWrite` or
//! `AbortWrite`, never reused, and a foreign or stale ticket never touches
//! the live preparation. The staged object is the private `.rustx-write-*`
//! sibling from `rustx-confined-fs`: preparing mutates nothing named by the
//! request, aborting or dropping the owner removes only the proven private
//! stage, and a `CommitWrite` crossing that publishes but then fails is
//! reported as `CommitUnknown`, never as a pre-commit denial.

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use rustx_confined_fs::{CommitUnknown, Expectation, PackageDirectory, WriteTarget};
use rustx_execution_protocol::{
    CancelDisposition, ClientReply, DecimalU64, EditRefusal, MAX_EDIT_FILE_BYTES,
    MAX_EDIT_REPLACEMENTS, MAX_EDIT_REQUEST_BYTES, MAX_PREPARATIONS_PER_CONNECTION,
    MAX_RAW_CHUNK_BYTES, MAX_WRITE_BYTES, ReadError, TextReplacement, WriteExpectation,
};

/// The per-ticket publish arbitration shared between the data lane's
/// `CommitWrite` crossing and the control lane's `CancelMutation`.
///
/// One ticket moves `Prepared` — `commit` has not started — to
/// `Committing` when the request is granted, and only the publish gate
/// itself decides whether the staged bytes cross: a cancellation that
/// landed while the commit was still staging wins (`Cancelled`), one
/// that lands after the gate meets `Crossed` and is answered
/// [`CancelDisposition::AlreadyCrossed`]. Cells live in the shared map
/// only while the data lane can still reach them; the control lane
/// never blocks unrelated tickets — each cell carries its own lock and
/// no filesystem work runs under it.
pub(crate) struct Arbitration {
    /// Live ticket cells by ticket number.
    cells: Mutex<std::collections::BTreeMap<u64, std::sync::Arc<TicketCell>>>,
    /// The publish/revoke grant: the publish gate holds a read guard
    /// exactly across the one crossing syscall, so a `Revoke` — which
    /// takes the write guard — cannot complete until every admitted
    /// crossing has physically finished. Once set, no later gate or
    /// commit grant is admitted.
    revoked: std::sync::RwLock<bool>,
    /// Wakes the control worker when a crossing is queued or the worker
    /// should stop.
    wake: std::sync::Condvar,
    /// The worker stop flag — set once on teardown, polled per turn.
    stopped: Mutex<bool>,
}

/// The one lockable state a `CancelMutation` arbitrates against.
struct TicketCell {
    state: Mutex<TicketState>,
}

/// The arbitration lifecycle of one ticket — `Prepared` and
/// `Committing` are still cancellable; `Cancelled` and `Crossed` are
/// terminal answers.
#[derive(Debug)]
enum TicketState {
    /// Prepared but `CommitWrite` not yet dispatched.
    Prepared,
    /// `CommitWrite` granted; private staging may still be running.
    Committing,
    /// A `CancelMutation` won before the publish gate.
    Cancelled,
    /// The publish gate passed — the crossing cannot un-happen.
    Crossed,
}

/// The publish gate's verdict for one staged commit.
pub(crate) enum PublishGate {
    /// The crossing syscall ran inside the gate and succeeded — the
    /// publish cannot un-happen; the ticket is `Crossed`.
    Crossed,
    /// A control-lane cancellation or revocation won before the
    /// crossing ran: drop the stage, reply `WriteCancelled`.
    Cancelled,
    /// The crossing syscall ran and failed before publication — the
    /// stage is still private, the ticket still answerable. Surfaced
    /// so the caller reports the typed write error, never a phantom
    /// `Crossed`.
    Failed(std::io::Error),
}

impl Arbitration {
    pub(crate) fn new() -> Self {
        Self {
            cells: Mutex::new(std::collections::BTreeMap::new()),
            revoked: std::sync::RwLock::new(false),
            wake: std::sync::Condvar::new(),
            stopped: Mutex::new(false),
        }
    }

    /// Registers a freshly issued ticket as `Prepared` — called from the
    /// data lane the moment a preparation is admitted. A revocation
    /// already granted makes the cell dead weight, so it is not
    /// registered; one racing the check is harmless — the gate still
    /// fences it.
    fn prepared(&self, ticket: DecimalU64) {
        if *self.revoked.read().expect("revoke grant") {
            return;
        }
        self.cells.lock().expect("arbitration cells").insert(
            ticket.get(),
            std::sync::Arc::new(TicketCell {
                state: Mutex::new(TicketState::Prepared),
            }),
        );
    }

    /// Marks a ticket `Committing` under its own lock and returns the
    /// cell for the later publish gate. `None` means the ticket is
    /// unknown or the control lane already fenced all commits.
    fn begin_commit(&self, ticket: DecimalU64) -> Option<std::sync::Arc<TicketCell>> {
        if *self.revoked.read().expect("revoke grant") {
            return None;
        }
        let cell = self
            .cells
            .lock()
            .expect("arbitration cells")
            .get(&ticket.get())
            .cloned()?;
        let mut state = cell.state.lock().expect("ticket cell");
        match *state {
            TicketState::Prepared | TicketState::Committing | TicketState::Cancelled => {
                // A cancellation already in `Cancelled` stays: the gate
                // reads it and answers WriteCancelled — begin never
                // revives a cancelled ticket.
                if matches!(*state, TicketState::Prepared) {
                    *state = TicketState::Committing;
                }
                drop(state);
                Some(cell)
            }
            TicketState::Crossed => None,
        }
    }

    /// The irreversible publish gate: `cross` — the one
    /// `linkat`/`renameat` closure confined-fs hands over — runs under
    /// the publish read grant and the ticket's own lock, so a
    /// `CancelMutation` never observes `Crossed` before the crossing
    /// physically succeeded and a `Revoke` cannot report completion
    /// while the syscall is still in flight. A won cancellation or a
    /// granted revocation refuses before `cross` runs; `Crossed` is
    /// recorded iff the syscall returned success, and the `Committed`
    /// wake reaches the control worker only then. A failed syscall
    /// leaves the ticket answerable — never `AlreadyCrossed`. The
    /// guards span exactly the arbitration decision and the one crossing
    /// syscall — no other filesystem work.
    fn publish_gate(
        &self,
        cell: &TicketCell,
        cross: &mut dyn FnMut() -> std::io::Result<()>,
    ) -> PublishGate {
        let grant = self.revoked.read().expect("revoke grant");
        let mut state = cell.state.lock().expect("ticket cell");
        if matches!(*state, TicketState::Cancelled) || *grant {
            return PublishGate::Cancelled;
        }
        match cross() {
            Ok(()) => {
                *state = TicketState::Crossed;
                drop(state);
                drop(grant);
                self.wake.notify_one();
                PublishGate::Crossed
            }
            Err(source) => PublishGate::Failed(source),
        }
    }

    /// The control lane's `CancelMutation` arbitration — bounded to the
    /// cell lookup and the cell lock, never any filesystem work.
    pub(crate) fn cancel(&self, ticket: DecimalU64) -> CancelDisposition {
        let cell = self
            .cells
            .lock()
            .expect("arbitration cells")
            .get(&ticket.get())
            .cloned();
        let Some(cell) = cell else {
            return CancelDisposition::Unknown;
        };
        let mut state = cell.state.lock().expect("ticket cell");
        match *state {
            TicketState::Prepared | TicketState::Committing | TicketState::Cancelled => {
                *state = TicketState::Cancelled;
                CancelDisposition::CancelledBeforeCommit
            }
            TicketState::Crossed => CancelDisposition::AlreadyCrossed,
        }
    }

    /// Fences all new commit grants — the control lane's `Revoke`.
    ///
    /// The write grant drains every in-flight publish gate first: the
    /// call may wait for a crossing kernel call already admitted —
    /// never for staging or verification, which run outside the grant —
    /// so a returned `Revoked` acknowledgement means no publish crossing
    /// can still complete and no later crossing can ever start.
    pub(crate) fn revoke(&self) {
        *self.revoked.write().expect("revoke grant") = true;
    }

    /// Whether the control lane's `Revoke` already fenced this
    /// arbitration — the data lane uses it to answer a live ticket's
    /// commit honestly: a fenced commit is cancelled, not malformed. A
    /// `Revoke` queued behind an in-flight crossing makes this check
    /// wait it out, so it never answers ahead of the acknowledgement.
    pub(crate) fn is_revoked(&self) -> bool {
        *self.revoked.read().expect("revoke grant")
    }

    pub(crate) fn check_grant(&self) -> std::io::Result<()> {
        self.with_grant(|| Ok(()))
    }

    pub(crate) fn with_grant(
        &self,
        crossing: impl FnOnce() -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        let grant = self.revoked.read().expect("revoke grant");
        if *grant {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "execution grant revoked",
            ));
        }
        crossing()
    }

    /// Whether the control worker was asked to stop.
    pub(crate) fn worker_stopped(&self) -> bool {
        *self.stopped.lock().expect("worker stop")
    }

    /// Removes a terminal ticket cell — after `WriteAborted`,
    /// `WriteCancelled`, or a finished commit.
    fn remove(&self, ticket: DecimalU64) {
        self.cells
            .lock()
            .expect("arbitration cells")
            .remove(&ticket.get());
    }

    /// The worker's stop request: idempotent, wakes the wait.
    pub(crate) fn stop_worker(&self) {
        *self.stopped.lock().expect("worker stop") = true;
        self.wake.notify_one();
    }

    /// Bounded wait for a worker wake — a queued `Committed`, the stop
    /// flag, or the poll deadline.
    pub(crate) fn wait_worker(&self, timeout: std::time::Duration) -> bool {
        let stopped = self.stopped.lock().expect("worker stop");
        let (stopped, _) = self
            .wake
            .wait_timeout_while(stopped, timeout, |stopped| !*stopped)
            .expect("worker stop");
        *stopped
    }
}

/// The staged-write state of one authenticated connection.
///
/// A preparation retains the verified `WriteTarget` and keeps its
/// bounded planned bytes in memory — nothing is staged on disk before
/// the authorization grant, so a preapproval payload is never
/// enumerable by search, metadata, reads, or another mutation. At most
/// `MAX_PREPARATIONS_PER_CONNECTION` preparations are live, and their
/// payloads share one per-connection budget.
pub(super) struct Writes {
    /// The next ticket to issue; tickets start at 1 and only ever grow.
    next: u64,
    /// The live preparations by ticket.
    prepared: std::collections::BTreeMap<u64, Preparation>,
    /// Planned bytes currently held across all live preparations.
    payload_bytes: usize,
    /// The per-ticket publish arbitration shared with the control lane.
    arbitration: std::sync::Arc<Arbitration>,
}

/// One live ticketed preparation: the retained target plus the planned
/// payload in memory — complete for a `PrepareWrite`/`PrepareEdit`,
/// accumulating for a `BeginWrite`/`WriteChunk` sequence.
struct Preparation {
    target: WriteTarget,
    payload: Vec<u8>,
    /// The declared complete payload length.
    expected: usize,
}

impl Writes {
    pub(super) fn new(arbitration: std::sync::Arc<Arbitration>) -> Self {
        Self {
            next: 1,
            prepared: std::collections::BTreeMap::new(),
            payload_bytes: 0,
            arbitration,
        }
    }

    pub(super) fn ticket_sequence(&self) -> u64 {
        self.next
    }

    pub(super) fn continue_ticket_sequence(&mut self, next: u64) {
        self.next = next;
    }

    /// Retains the named target under `expectation` — the shared first
    /// phase of `PrepareWrite`, `BeginWrite`, and `PrepareEdit`. The
    /// deepest existing ancestor is retained and the still-missing
    /// interior names are carried for the commit's staged subtree;
    /// nothing physical is created.
    fn retain(
        root: &PackageDirectory,
        relative_path: &Path,
        expectation: &WriteExpectation,
    ) -> Result<WriteTarget, ReadError> {
        // A write names one file leaf; unlike the query operations an
        // empty path is invalid — the workspace root is never a target.
        if relative_path.as_os_str().is_empty() || crate::relative_entry(relative_path).is_err() {
            return Err(ReadError::Invalid);
        }
        let Some(leaf) = relative_path.file_name() else {
            return Err(ReadError::Invalid);
        };
        let (parent, missing) =
            match root.descend_deepest(relative_path.parent().unwrap_or_else(|| Path::new(""))) {
                Ok(found) => found,
                Err(source) => return Err(crate::read_error(&source)),
            };
        let expected = match expectation {
            WriteExpectation::Current => Expectation::Current,
            WriteExpectation::Absent => Expectation::Absent,
            WriteExpectation::Revision(revision) => Expectation::Revision(revision),
        };
        WriteTarget::prepare(parent, leaf, missing, expected)
            .map_err(|source| crate::read_error(&source))
    }

    /// Registers `preparation` under a fresh ticket, refusing when the
    /// bounded map or the payload budget is full.
    fn admit(
        &mut self,
        target: WriteTarget,
        payload: Vec<u8>,
        expected: usize,
        reply: impl FnOnce(DecimalU64, Option<rustx_execution_protocol::FileStamp>) -> ClientReply,
    ) -> ClientReply {
        if self.prepared.len() >= MAX_PREPARATIONS_PER_CONNECTION
            || self.payload_bytes + expected > MAX_PREPARATIONS_PER_CONNECTION * MAX_WRITE_BYTES
        {
            return error(ReadError::Busy);
        }
        let Some(ticket) = self.next_ticket() else {
            return error(ReadError::Unavailable);
        };
        let original = target
            .original()
            .map(|metadata| crate::stamp(metadata).stamp);
        self.payload_bytes += expected;
        self.prepared.insert(
            ticket.get(),
            Preparation {
                target,
                payload,
                expected,
            },
        );
        self.arbitration.prepared(ticket);
        reply(ticket, original)
    }

    /// Handles a `PrepareWrite`: validates the relative leaf, bounds and
    /// decodes the payload, and retains the verified target with the
    /// planned bytes held in memory. The named target is not mutated and
    /// nothing is staged on disk before the commit grant.
    pub(super) fn prepare(
        &mut self,
        root: &PackageDirectory,
        relative_path: &Path,
        expectation: &WriteExpectation,
        data_base64: &str,
    ) -> ClientReply {
        // The encoded length decides before any allocation: a payload
        // that cannot decode inside the chunk cap is refused up front.
        if data_base64.len() > MAX_RAW_CHUNK_BYTES.div_ceil(3) * 4 {
            return error(ReadError::Invalid);
        }
        let bytes = match base64::engine::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            data_base64,
        ) {
            Ok(bytes) if bytes.len() <= MAX_RAW_CHUNK_BYTES => bytes,
            _ => return error(ReadError::Invalid),
        };
        let target = match Self::retain(root, relative_path, expectation) {
            Ok(target) => target,
            Err(kind) => return error(kind),
        };
        let expected = bytes.len();
        self.admit(target, bytes, expected, |ticket, original| {
            ClientReply::WritePrepared { ticket, original }
        })
    }

    /// Handles a `BeginWrite`: retains the target and opens a ticket whose
    /// planned payload — `total` bytes, at most [`MAX_WRITE_BYTES`] —
    /// then accumulates through `WriteChunk` exchanges at deterministic
    /// absolute offsets.
    pub(super) fn begin(
        &mut self,
        root: &PackageDirectory,
        relative_path: &Path,
        expectation: &WriteExpectation,
        total: DecimalU64,
    ) -> ClientReply {
        let Ok(expected) = usize::try_from(total.get()) else {
            return error(ReadError::Invalid);
        };
        if expected == 0 || expected > MAX_WRITE_BYTES {
            return error(ReadError::Invalid);
        }
        let target = match Self::retain(root, relative_path, expectation) {
            Ok(target) => target,
            Err(kind) => return error(kind),
        };
        self.admit(target, Vec::new(), expected, |ticket, original| {
            ClientReply::WritePrepared { ticket, original }
        })
    }

    /// Handles a `WriteChunk`: appends the decoded bytes at the ticket's
    /// next absolute offset. A foreign ticket, a gap, an overrun past
    /// the declared total, or an undecodable chunk is refused without
    /// disturbing any other preparation.
    pub(super) fn chunk(
        &mut self,
        ticket: DecimalU64,
        offset: DecimalU64,
        data_base64: &str,
    ) -> ClientReply {
        if data_base64.len() > MAX_RAW_CHUNK_BYTES.div_ceil(3) * 4 {
            return error(ReadError::Invalid);
        }
        let chunk = match base64::engine::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            data_base64,
        ) {
            Ok(chunk) if chunk.len() <= MAX_RAW_CHUNK_BYTES => chunk,
            _ => return error(ReadError::Invalid),
        };
        let Some(preparation) = self.prepared.get_mut(&ticket.get()) else {
            return error(ReadError::Invalid);
        };
        if offset.get() != preparation.payload.len() as u64
            || preparation.payload.len() + chunk.len() > preparation.expected
        {
            return error(ReadError::Invalid);
        }
        preparation.payload.extend_from_slice(&chunk);
        ClientReply::WriteChunkReceived { ticket, offset }
    }

    /// Handles a `PrepareEdit`: retains the named original, reads it back
    /// through the retained descriptor — never the name — applies the
    /// bounded text-edit algorithm, and holds the planned result bytes in
    /// memory under the ticket. A malformed request is an `Error`; a
    /// semantic `rustx_text_edit` failure is an `EditRefused` carrying
    /// only the typed reason — no path or anchor text. Either way no
    /// ticket is spent and the authorization gate is never reached.
    pub(super) fn prepare_edit(
        &mut self,
        root: &PackageDirectory,
        relative_path: &Path,
        expectation: &WriteExpectation,
        replacements: &[TextReplacement],
    ) -> ClientReply {
        let request_bytes = replacements.iter().fold(0_usize, |total, edit| {
            total
                .saturating_add(edit.old_text.len())
                .saturating_add(edit.new_text.len())
        });
        if matches!(expectation, WriteExpectation::Absent)
            || replacements.is_empty()
            || replacements.len() > MAX_EDIT_REPLACEMENTS
            || request_bytes > MAX_EDIT_REQUEST_BYTES
            || replacements.iter().any(|edit| edit.old_text.is_empty())
        {
            return error(ReadError::Invalid);
        }
        let target = match Self::retain(root, relative_path, expectation) {
            Ok(target) => target,
            Err(kind) => return error(kind),
        };
        // An edit always names an existing original: `Current` on an
        // absent leaf is a not-found, and a leaf below a missing subtree
        // can never exist.
        if target.original().is_none() {
            return error(ReadError::NotFound);
        }
        let original = crate::stamp(
            target
                .original()
                .expect("an edit always requires an existing original"),
        )
        .stamp;
        let bytes = match target.read_original_bounded(MAX_EDIT_FILE_BYTES as u64) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::FileTooLarge => {
                return ClientReply::EditRefused {
                    reason: EditRefusal::TooLarge,
                };
            }
            Err(source) => return error(crate::read_error(&source)),
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return error(ReadError::InvalidUtf8);
        };
        // `TextReplacement`/`EditRefusal` are the kernel's own public
        // types — the same definitions, never a second shape.
        let updated = match rustx_text_edit::apply(&text, replacements) {
            Ok(updated) => updated,
            Err(failure) => {
                return ClientReply::EditRefused { reason: failure };
            }
        };
        let published_size = DecimalU64::new(updated.len() as u64);
        let payload = updated.into_bytes();
        let expected = payload.len();
        self.admit(target, payload, expected, |ticket, _| {
            ClientReply::EditPrepared {
                ticket,
                original,
                published_size,
            }
        })
    }

    /// Handles a `CommitWrite` for the exact live ticket: the retained
    /// target stages the planned bytes privately — only now, under the
    /// grant — then the shared publish gate arbitrates the crossing
    /// against any control-lane `CancelMutation` under the ticket's own
    /// lock. A cancellation that won before the gate gets the typed
    /// `WriteCancelled` and the private stage is dropped with nothing
    /// named written; a post-crossing failure is `CommitUnknown`.
    /// A wrong or incomplete ticket is refused without disturbing any
    /// preparation.
    pub(super) fn commit(&mut self, ticket: DecimalU64) -> ClientReply {
        // A foreign or still-incomplete ticket is refused without
        // disturbing anything: the preparation, its payload
        // accounting, and the arbitration cell all stay exactly as the
        // peer left them — the ticket remains chunkable and
        // cancellable.
        let Some(preparation) = self.prepared.get(&ticket.get()) else {
            return error(ReadError::Invalid);
        };
        if preparation.payload.len() != preparation.expected {
            return error(ReadError::Invalid);
        }
        let Some(cell) = self.arbitration.begin_commit(ticket) else {
            // The ticket is live but unfundable: a revocation already
            // fenced the arbitration — the commit is cancelled, never
            // malformed, and nothing ever crosses.
            if self.arbitration.is_revoked() {
                return ClientReply::WriteCancelled { ticket };
            }
            return error(ReadError::Invalid);
        };
        let Preparation {
            target,
            payload,
            expected,
        } = self
            .prepared
            .remove(&ticket.get())
            .expect("live ticket checked");
        self.payload_bytes = self.payload_bytes.saturating_sub(expected);
        // Private staging first — nothing named is written, and no
        // shared arbitration lock is held while the kernel works.
        let staged = target.stage(|file| file.write_all(&payload));
        let result = match staged {
            Ok(staged) => {
                // The gate runs inside `publish_with_gate`, wrapped
                // around the actual `linkat`/`renameat` syscall under
                // the ticket lock: `Crossed` can only be recorded
                // after the crossing physically succeeded.
                let arbitration = self.arbitration.clone();
                let mut cancelled = false;
                match staged.publish_with_gate(|cross| {
                    match arbitration.publish_gate(&cell, cross) {
                        PublishGate::Crossed => Ok(()),
                        // The staged write's own drop cleans the
                        // private stage — a proven zero-write cancel.
                        PublishGate::Cancelled => {
                            cancelled = true;
                            Err(std::io::Error::new(
                                std::io::ErrorKind::Interrupted,
                                "cancelled before publish",
                            ))
                        }
                        PublishGate::Failed(source) => Err(source),
                    }
                }) {
                    Ok(published) => published.finish(),
                    Err(source) => {
                        if cancelled {
                            self.arbitration.remove(ticket);
                            return ClientReply::WriteCancelled { ticket };
                        }
                        Err(source)
                    }
                }
            }
            Err(source) => Err(source),
        };
        self.arbitration.remove(ticket);
        match result {
            Ok(metadata) => ClientReply::Written {
                ticket,
                metadata: crate::stamp(&metadata).stamp,
            },
            Err(source) => write_failure(ticket, &source),
        }
    }

    /// Releases an issued ticket. An already settled ticket also proves
    /// absence. A zero or future ticket has no such proof.
    pub(super) fn abort(&mut self, ticket: DecimalU64) -> ClientReply {
        if ticket.get() == 0 || ticket.get() >= self.next {
            return error(ReadError::Invalid);
        }
        self.arbitration.remove(ticket);
        if let Some(preparation) = self.prepared.remove(&ticket.get()) {
            self.payload_bytes = self.payload_bytes.saturating_sub(preparation.expected);
        }
        ClientReply::WriteAborted { ticket }
    }

    /// Issues the next monotonically increasing nonzero ticket, refusing
    /// the operation on overflow instead of ever reusing one.
    fn next_ticket(&mut self) -> Option<DecimalU64> {
        let ticket = DecimalU64::new(self.next);
        self.next = self.next.checked_add(1)?;
        Some(ticket)
    }
}

/// Distinguishes a post-crossing failure — reported as `CommitUnknown`,
/// never as a pre-commit denial — from an ordinary refused write.
fn write_failure(ticket: DecimalU64, source: &std::io::Error) -> ClientReply {
    match source.get_ref() {
        Some(kind) if kind.is::<CommitUnknown>() => ClientReply::CommitUnknown { ticket },
        _ => error(crate::read_error(source)),
    }
}

fn error(kind: ReadError) -> ClientReply {
    ClientReply::Error { kind }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixture() -> (tempfile::TempDir, PackageDirectory) {
        let dir = tempfile::tempdir().unwrap();
        // macOS temp dirs traverse a `/var` symlink — the kernel proves
        // roots by descriptor, so open the canonical path.
        let path = dir.path().canonicalize().unwrap();
        (dir, PackageDirectory::open(&path).unwrap())
    }

    fn encode(bytes: &[u8]) -> String {
        base64::engine::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
    }

    fn stage_names(root: &Path) -> Vec<PathBuf> {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into())
            .filter(|name: &PathBuf| {
                let name = name.to_str().unwrap();
                name.starts_with(".rustx-write-") || name.starts_with(".rustx-mkdir-")
            })
            .collect()
    }

    #[test]
    fn a_prepared_write_commits_and_the_same_ticket_never_commits_twice() {
        let (dir, root) = fixture();
        let mut writes = Writes::new(std::sync::Arc::new(Arbitration::new()));
        fs::write(dir.path().join("file"), b"before").unwrap();
        let original = root.entry_metadata(std::path::Path::new("file")).unwrap();

        let ClientReply::WritePrepared {
            ticket,
            original: reported,
        } = writes.prepare(
            &root,
            std::path::Path::new("file"),
            &WriteExpectation::Revision(original.revision.clone()),
            &encode(b"after"),
        )
        else {
            panic!("expected a prepared write")
        };
        assert_eq!(reported.unwrap().revision, original.revision);
        // Prepare never mutates the named target.
        assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"before");

        let ClientReply::Written {
            ticket: echoed,
            metadata,
        } = writes.commit(ticket)
        else {
            panic!("expected a committed write")
        };
        assert_eq!(echoed, ticket);
        assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"after");
        assert_ne!(metadata.revision, original.revision);

        // The consumed ticket is dead: a replayed commit can never
        // write twice — and the live preparation was already consumed.
        let reply = writes.commit(ticket);
        assert_eq!(
            reply,
            ClientReply::Error {
                kind: ReadError::Invalid
            }
        );
        assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"after");
    }

    #[test]
    fn abort_after_clean_commit_refusal_settles_only_the_consumed_ticket() {
        let (dir, root) = fixture();
        let mut writes = Writes::new(std::sync::Arc::new(Arbitration::new()));
        let prepare = |writes: &mut Writes, leaf| {
            let ClientReply::WritePrepared { ticket, .. } = writes.prepare(
                &root,
                Path::new(leaf),
                &WriteExpectation::Absent,
                &encode(b"new"),
            ) else {
                panic!("prepared write");
            };
            ticket
        };
        let consumed = prepare(&mut writes, "original");
        let separate = prepare(&mut writes, "separate");
        fs::write(dir.path().join("original"), b"competing").unwrap();
        assert_eq!(
            writes.commit(consumed),
            ClientReply::Error {
                kind: ReadError::Conflict
            }
        );
        assert_eq!(
            writes.abort(consumed),
            ClientReply::WriteAborted { ticket: consumed }
        );
        assert_eq!(
            writes.abort(consumed),
            ClientReply::WriteAborted { ticket: consumed }
        );
        assert_eq!(
            writes.abort(DecimalU64::new(separate.get() + 1)),
            ClientReply::Error {
                kind: ReadError::Invalid
            }
        );
        assert_eq!(
            writes.abort(DecimalU64::new(0)),
            ClientReply::Error {
                kind: ReadError::Invalid
            }
        );
        assert!(
            matches!(writes.commit(separate), ClientReply::Written { ticket, .. } if ticket == separate)
        );
        assert_eq!(fs::read(dir.path().join("original")).unwrap(), b"competing");
        assert_eq!(fs::read(dir.path().join("separate")).unwrap(), b"new");
    }

    #[test]
    fn a_foreign_or_stale_ticket_leaves_the_live_preparation_intact() {
        let (dir, root) = fixture();
        let mut writes = Writes::new(std::sync::Arc::new(Arbitration::new()));
        let ClientReply::WritePrepared { ticket, .. } = writes.prepare(
            &root,
            std::path::Path::new("file"),
            &WriteExpectation::Absent,
            &encode(b"data"),
        ) else {
            panic!("expected a prepared write")
        };

        for foreign in [
            DecimalU64::new(0),
            DecimalU64::new(2),
            DecimalU64::new(u64::MAX),
        ] {
            assert_eq!(
                writes.commit(foreign),
                ClientReply::Error {
                    kind: ReadError::Invalid
                }
            );
            assert_eq!(
                writes.abort(foreign),
                ClientReply::Error {
                    kind: ReadError::Invalid
                }
            );
        }
        // The real ticket still works: only it consumes the stage.
        let reply = writes.abort(ticket);
        assert_eq!(reply, ClientReply::WriteAborted { ticket });
        assert!(!dir.path().join("file").exists());
        assert_eq!(stage_names(dir.path()), Vec::<PathBuf>::new());
    }

    #[test]
    fn concurrent_approvals_share_the_bounded_ticket_budget() {
        let (_dir, root) = fixture();
        let mut writes = Writes::new(std::sync::Arc::new(Arbitration::new()));
        // Multiple preparations stay live independently — approvals are
        // no longer serialized through one outstanding slot.
        let mut tickets = Vec::new();
        for index in 0..MAX_PREPARATIONS_PER_CONNECTION {
            let name = format!("file{index}");
            let ClientReply::WritePrepared { ticket, .. } = writes.prepare(
                &root,
                std::path::Path::new(&name),
                &WriteExpectation::Absent,
                &encode(b"data"),
            ) else {
                panic!("expected a prepared write")
            };
            tickets.push(ticket);
        }
        // The ninth is a typed busy refusal, never a silent replacement.
        let reply = writes.prepare(
            &root,
            std::path::Path::new("overflow"),
            &WriteExpectation::Absent,
            &encode(b"x"),
        );
        assert_eq!(
            reply,
            ClientReply::Error {
                kind: ReadError::Busy
            }
        );
        // Each held ticket aborts independently; none disturbs another.
        for ticket in tickets {
            assert_eq!(writes.abort(ticket), ClientReply::WriteAborted { ticket });
        }
    }

    #[test]
    fn a_leaf_replaced_between_prepare_and_commit_is_rejected_at_the_target() {
        let (dir, root) = fixture();
        let mut writes = Writes::new(std::sync::Arc::new(Arbitration::new()));
        fs::write(dir.path().join("file"), b"before").unwrap();
        let original = root.entry_metadata(std::path::Path::new("file")).unwrap();
        let ClientReply::WritePrepared { ticket, .. } = writes.prepare(
            &root,
            std::path::Path::new("file"),
            &WriteExpectation::Revision(original.revision.clone()),
            &encode(b"staged"),
        ) else {
            panic!("expected a prepared write")
        };

        // An outside actor replaces the leaf: the retained target's
        // revision no longer matches, so the commit is a conflict and
        // the outside bytes are never overwritten.
        fs::write(dir.path().join("file"), b"outside").unwrap();
        let reply = writes.commit(ticket);
        assert_eq!(
            reply,
            ClientReply::Error {
                kind: ReadError::Conflict
            }
        );
        assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"outside");
        assert_eq!(stage_names(dir.path()), Vec::<PathBuf>::new());
    }

    #[test]
    fn dropping_the_owner_never_publishes_and_leaves_no_stage() {
        let (dir, root) = fixture();
        {
            let mut writes = Writes::new(std::sync::Arc::new(Arbitration::new()));
            let ClientReply::WritePrepared { .. } = writes.prepare(
                &root,
                std::path::Path::new("file"),
                &WriteExpectation::Absent,
                &encode(b"orphaned"),
            ) else {
                panic!("expected a prepared write")
            };
            // The plan is held in memory: nothing is staged on disk
            // before the commit grant — a dropped owner is the
            // no-side-effect abort, never an implicit publish.
            assert_eq!(stage_names(dir.path()), Vec::<PathBuf>::new());
        }
        assert_eq!(stage_names(dir.path()), Vec::<PathBuf>::new());
        assert!(!dir.path().join("file").exists());
    }

    #[test]
    fn an_interior_dot_segment_is_invalid_and_stages_nothing() {
        let (dir, root) = fixture();
        fs::create_dir(dir.path().join("dir")).unwrap();
        fs::write(dir.path().join("dir").join("file"), b"before").unwrap();
        let mut writes = Writes::new(std::sync::Arc::new(Arbitration::new()));
        // `components()` would silently elide the interior `.` — the raw
        // spelling is what the gate must refuse.
        let reply = writes.prepare(
            &root,
            std::path::Path::new("dir/./file"),
            &WriteExpectation::Absent,
            &encode(b"staged"),
        );
        assert_eq!(
            reply,
            ClientReply::Error {
                kind: ReadError::Invalid
            }
        );
        assert_eq!(
            fs::read(dir.path().join("dir").join("file")).unwrap(),
            b"before"
        );
        assert_eq!(stage_names(dir.path()), Vec::<PathBuf>::new());
        assert_eq!(stage_names(&dir.path().join("dir")), Vec::<PathBuf>::new());
        // A NUL and an interior `..` fail the same raw-segment gate.
        for path in ["dir/fi\0le", "dir/../dir/file"] {
            assert_eq!(
                writes.prepare(
                    &root,
                    std::path::Path::new(path),
                    &WriteExpectation::Absent,
                    &encode(b"x")
                ),
                ClientReply::Error {
                    kind: ReadError::Invalid
                },
                "{path:?}"
            );
        }
    }

    #[test]
    fn oversized_or_malformed_payloads_are_refused_before_staging() {
        let (dir, root) = fixture();
        let mut writes = Writes::new(std::sync::Arc::new(Arbitration::new()));
        for encoded in [
            // Valid base64 that decodes past the decoded cap.
            encode(&vec![b'x'; MAX_RAW_CHUNK_BYTES + 1]),
            // An encoded length that cannot fit inside the cap.
            "A".repeat(MAX_RAW_CHUNK_BYTES.div_ceil(3) * 4 + 4),
            // Not base64 at all.
            String::from("!!!"),
        ] {
            let reply = writes.prepare(
                &root,
                std::path::Path::new("file"),
                &WriteExpectation::Absent,
                &encoded,
            );
            assert_eq!(
                reply,
                ClientReply::Error {
                    kind: ReadError::Invalid
                }
            );
        }
        assert_eq!(stage_names(dir.path()), Vec::<PathBuf>::new());
    }

    /// A cancellation acknowledged before the gate is honest
    /// end-to-end: the commit reply is `WriteCancelled`, the named
    /// target stays absent, no staging name or missing-parent directory
    /// survives, and an unrelated competitor is byte-identical. The
    /// ticket is terminal — a replayed commit can never publish later.
    #[test]
    fn a_cancelled_commit_publishes_nothing_and_leaves_no_stage() {
        let (dir, root) = fixture();
        let arbitration = std::sync::Arc::new(Arbitration::new());
        let mut writes = Writes::new(arbitration.clone());
        fs::write(dir.path().join("competitor"), b"untouched").unwrap();

        let ClientReply::WritePrepared { ticket, .. } = writes.prepare(
            &root,
            std::path::Path::new("missing/sub/file"),
            &WriteExpectation::Absent,
            &encode(b"data"),
        ) else {
            panic!("expected a prepared write")
        };
        // The control lane's arbitration: the cancel lands while the
        // ticket is still `Prepared` — before any crossing exists.
        assert_eq!(
            arbitration.cancel(ticket),
            CancelDisposition::CancelledBeforeCommit
        );

        // The data lane's commit can never succeed after the cancelled
        // acknowledgement was delivered.
        assert_eq!(
            writes.commit(ticket),
            ClientReply::WriteCancelled { ticket }
        );
        assert!(!dir.path().join("missing").exists());
        assert_eq!(stage_names(dir.path()), Vec::<PathBuf>::new());
        assert_eq!(
            fs::read(dir.path().join("competitor")).unwrap(),
            b"untouched"
        );
        // The ticket is dead: a replay is a typed refuse, never a write.
        assert_eq!(
            writes.commit(ticket),
            ClientReply::Error {
                kind: ReadError::Invalid
            }
        );
        assert!(!dir.path().join("missing").exists());
    }

    /// Revocation fences the gate itself: a commit granted while the
    /// arbitration was live still cannot cross once `revoke` landed —
    /// the atomic flag is read under the ticket lock, so a commit
    /// parked between grant and gate is refused, never published after
    /// a reported revocation.
    #[test]
    fn a_revocation_fences_a_commit_at_the_gate_itself() {
        let (dir, root) = fixture();
        let arbitration = std::sync::Arc::new(Arbitration::new());
        let mut writes = Writes::new(arbitration.clone());
        let ClientReply::WritePrepared { ticket, .. } = writes.prepare(
            &root,
            std::path::Path::new("file"),
            &WriteExpectation::Absent,
            &encode(b"data"),
        ) else {
            panic!("expected a prepared write")
        };
        arbitration.revoke();

        assert_eq!(
            writes.commit(ticket),
            ClientReply::WriteCancelled { ticket }
        );
        assert!(!dir.path().join("file").exists());
        assert_eq!(stage_names(dir.path()), Vec::<PathBuf>::new());
    }

    /// The write grant drains the gate: a `Revoke` issued while a
    /// crossing syscall is physically in flight cannot report completion
    /// until the syscall returns — and the acknowledgement then forbids
    /// every later crossing.
    #[test]
    fn a_revocation_waits_for_an_in_flight_crossing_then_fences_forever() {
        use std::sync::mpsc;

        let arbitration = std::sync::Arc::new(Arbitration::new());
        let ticket = DecimalU64::new(11);
        arbitration.prepared(ticket);
        let cell = arbitration
            .cells
            .lock()
            .expect("arbitration cells")
            .get(&ticket.get())
            .cloned()
            .expect("the ticket is registered");
        *cell.state.lock().expect("ticket cell") = TicketState::Committing;

        // The gate is parked inside the crossing syscall.
        let (in_cross_tx, in_cross_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let gate_arbitration = arbitration.clone();
        let gate_cell = cell.clone();
        let gate = std::thread::spawn(move || {
            gate_arbitration.publish_gate(&gate_cell, &mut || {
                in_cross_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
        });
        in_cross_rx.recv().unwrap();

        let (revoked_tx, revoked_rx) = mpsc::channel();
        let revoke_arbitration = arbitration.clone();
        let revoker = std::thread::spawn(move || {
            revoke_arbitration.revoke();
            revoked_tx.send(()).unwrap();
        });
        assert!(
            revoked_rx
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "the acknowledgement must wait out an in-flight crossing"
        );

        release_tx.send(()).unwrap();
        assert!(matches!(gate.join().unwrap(), PublishGate::Crossed));
        revoked_rx.recv().unwrap();
        revoker.join().unwrap();

        // After the acknowledgement every later crossing is fenced: a
        // new grant is never issued and the syscall never runs.
        let cell = std::sync::Arc::new(TicketCell {
            state: Mutex::new(TicketState::Committing),
        });
        let mut crossed = false;
        let verdict = arbitration.publish_gate(&cell, &mut || {
            crossed = true;
            Ok(())
        });
        assert!(matches!(verdict, PublishGate::Cancelled));
        assert!(!crossed, "no crossing runs after the acknowledgement");
    }

    /// The gate runs the crossing under the ticket lock: a `CancelMutation`
    /// that arrives while the syscall is in flight parks on that lock and
    /// then receives the *true* post-syscall answer — `AlreadyCrossed`
    /// only because the crossing physically succeeded. A cancel that
    /// reaches the cell first means the crossing never runs at all.
    #[test]
    fn a_cancel_during_the_crossing_gets_the_true_post_syscall_answer() {
        use std::sync::mpsc;

        let arbitration = std::sync::Arc::new(Arbitration::new());
        let ticket = DecimalU64::new(7);
        arbitration.prepared(ticket);
        let cell = arbitration
            .cells
            .lock()
            .expect("arbitration cells")
            .get(&ticket.get())
            .cloned()
            .expect("the ticket is registered");
        *cell.state.lock().expect("ticket cell") = TicketState::Committing;

        // The gate holds the ticket lock across the crossing itself —
        // proven by the cell lock being held while `cross` runs.
        let (in_cross_tx, in_cross_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let gate_arbitration = arbitration.clone();
        let gate_cell = cell.clone();
        let gate = std::thread::spawn(move || {
            gate_arbitration.publish_gate(&gate_cell, &mut || {
                in_cross_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
        });
        in_cross_rx.recv().unwrap();
        assert!(
            cell.state.try_lock().is_err(),
            "the ticket lock spans the crossing syscall"
        );

        // A cancel during the crossing parks on the same lock; only the
        // syscall's real result decides its disposition.
        let cancel_arbitration = arbitration.clone();
        let canceller = std::thread::spawn(move || cancel_arbitration.cancel(ticket));
        release_tx.send(()).unwrap();
        assert!(matches!(gate.join().unwrap(), PublishGate::Crossed));
        assert_eq!(canceller.join().unwrap(), CancelDisposition::AlreadyCrossed);

        // Reverse ordering on a fresh ticket: a cancel that reached the
        // cell first means the crossing never runs.
        let ticket = DecimalU64::new(8);
        arbitration.prepared(ticket);
        let cell = arbitration
            .cells
            .lock()
            .expect("arbitration cells")
            .get(&ticket.get())
            .cloned()
            .expect("the ticket is registered");
        *cell.state.lock().expect("ticket cell") = TicketState::Committing;
        assert_eq!(
            arbitration.cancel(ticket),
            CancelDisposition::CancelledBeforeCommit
        );
        let mut crossed = false;
        let verdict = arbitration.publish_gate(&cell, &mut || {
            crossed = true;
            Ok(())
        });
        assert!(matches!(verdict, PublishGate::Cancelled));
        assert!(!crossed, "a won cancellation prevents the crossing");
    }

    /// A crossing that attempted but failed before publication is never
    /// `AlreadyCrossed`: the syscall's error surfaces as
    /// `PublishGate::Failed` and a later cancel still answers
    /// `CancelledBeforeCommit` — the honest "no write happened".
    #[test]
    fn a_failed_crossing_is_never_reported_as_already_crossed() {
        let arbitration = std::sync::Arc::new(Arbitration::new());
        let ticket = DecimalU64::new(3);
        arbitration.prepared(ticket);
        let cell = arbitration
            .cells
            .lock()
            .expect("arbitration cells")
            .get(&ticket.get())
            .cloned()
            .expect("the ticket is registered");
        *cell.state.lock().expect("ticket cell") = TicketState::Committing;

        let verdict = arbitration.publish_gate(&cell, &mut || {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "rename failed",
            ))
        });
        assert!(matches!(verdict, PublishGate::Failed(_)));
        // Attempted but not published: the cell stayed `Committing`, so
        // the cancel wins — never a phantom `AlreadyCrossed`.
        assert_eq!(
            arbitration.cancel(ticket),
            CancelDisposition::CancelledBeforeCommit
        );
    }

    /// A commit on a still-incomplete ticket is refused without
    /// disturbing anything: the preparation, its payload accounting,
    /// and the arbitration cell stay live — the ticket accepts the rest
    /// of its payload and commits normally afterwards.
    #[test]
    fn an_incomplete_commit_refuses_without_disturbing_the_ticket() {
        let (dir, root) = fixture();
        let mut writes = Writes::new(std::sync::Arc::new(Arbitration::new()));
        let ClientReply::WritePrepared { ticket, .. } = writes.begin(
            &root,
            std::path::Path::new("file"),
            &WriteExpectation::Absent,
            DecimalU64::new(4),
        ) else {
            panic!("expected a prepared write")
        };
        assert_eq!(
            writes.chunk(ticket, DecimalU64::new(0), &encode(b"ab")),
            ClientReply::WriteChunkReceived {
                ticket,
                offset: DecimalU64::new(0)
            }
        );
        // Incomplete: refused — and the ticket stays alive, not orphaned.
        assert_eq!(
            writes.commit(ticket),
            ClientReply::Error {
                kind: ReadError::Invalid
            }
        );
        assert_eq!(
            writes.chunk(ticket, DecimalU64::new(2), &encode(b"cd")),
            ClientReply::WriteChunkReceived {
                ticket,
                offset: DecimalU64::new(2)
            }
        );
        assert!(matches!(
            writes.commit(ticket),
            ClientReply::Written { ticket: echoed, .. } if echoed == ticket
        ));
        assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"abcd");
    }
}

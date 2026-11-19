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

use rustx_confined_fs::{CommitUnknown, Expectation, PackageDirectory, WriteTarget};
use rustx_execution_protocol::{
    ClientReply, DecimalU64, EditRefusal, MAX_EDIT_FILE_BYTES, MAX_EDIT_REPLACEMENTS,
    MAX_EDIT_REQUEST_BYTES, MAX_PREPARATIONS_PER_CONNECTION, MAX_RAW_CHUNK_BYTES, MAX_WRITE_BYTES,
    ReadError, TextReplacement, WriteExpectation,
};

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
    pub(super) fn new() -> Self {
        Self {
            next: 1,
            prepared: std::collections::BTreeMap::new(),
            payload_bytes: 0,
        }
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
    /// target stages the planned bytes — only now, under the grant — and
    /// publishes them through the shared kernel. A wrong or incomplete
    /// ticket is refused without disturbing any preparation; a
    /// post-crossing failure is `CommitUnknown`.
    pub(super) fn commit(&mut self, ticket: DecimalU64) -> ClientReply {
        let Some(preparation) = self.prepared.get(&ticket.get()) else {
            return error(ReadError::Invalid);
        };
        if preparation.payload.len() != preparation.expected {
            return error(ReadError::Invalid);
        }
        let Preparation {
            target,
            payload,
            expected,
        } = self
            .prepared
            .remove(&ticket.get())
            .expect("live ticket checked");
        self.payload_bytes = self.payload_bytes.saturating_sub(expected);
        let result = target
            .stage(|file| file.write_all(&payload))
            .and_then(rustx_confined_fs::StagedWrite::publish)
            .and_then(rustx_confined_fs::PublishedWrite::finish);
        match result {
            Ok(metadata) => ClientReply::Written {
                ticket,
                metadata: crate::stamp(&metadata).stamp,
            },
            Err(source) => write_failure(ticket, &source),
        }
    }

    /// Handles an `AbortWrite` for the exact live ticket: the retained
    /// target and its in-memory payload drop — nothing physical was ever
    /// staged, so nothing needs cleaning.
    pub(super) fn abort(&mut self, ticket: DecimalU64) -> ClientReply {
        let Some(preparation) = self.prepared.remove(&ticket.get()) else {
            return error(ReadError::Invalid);
        };
        self.payload_bytes = self.payload_bytes.saturating_sub(preparation.expected);
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
        let mut writes = Writes::new();
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
    fn a_foreign_or_stale_ticket_leaves_the_live_preparation_intact() {
        let (dir, root) = fixture();
        let mut writes = Writes::new();
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
        let mut writes = Writes::new();
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
        let mut writes = Writes::new();
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
            let mut writes = Writes::new();
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
        let mut writes = Writes::new();
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
        let mut writes = Writes::new();
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
}

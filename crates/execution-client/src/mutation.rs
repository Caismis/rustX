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

use rustx_confined_fs::{CommitUnknown, PackageDirectory, StagedWrite, WriteTarget};
use rustx_execution_protocol::{ClientReply, DecimalU64, MAX_RAW_CHUNK_BYTES, ReadError};

/// The staged-write state of one authenticated connection.
pub(super) struct Writes {
    /// The next ticket to issue; tickets start at 1 and only ever grow.
    next: u64,
    /// The one live preparation: its ticket and the private staged object.
    prepared: Option<(DecimalU64, StagedWrite)>,
}

impl Writes {
    pub(super) fn new() -> Self {
        Self {
            next: 1,
            prepared: None,
        }
    }

    /// Handles a `PrepareWrite`: validates the relative leaf, bounds and
    /// decodes the payload, retains the verified target, and stages the
    /// bytes privately under it. The named target is not mutated.
    pub(super) fn prepare(
        &mut self,
        root: &PackageDirectory,
        relative_path: &Path,
        expected_revision: Option<&str>,
        data_base64: &str,
    ) -> ClientReply {
        if self.prepared.is_some() {
            return error(ReadError::Invalid);
        }
        // A write names one file leaf; unlike the query operations an
        // empty path is invalid — the workspace root is never a target.
        if relative_path.as_os_str().is_empty() || crate::relative_entry(relative_path).is_err() {
            return error(ReadError::Invalid);
        }
        // The encoded length decides before any allocation: a payload that
        // cannot decode inside the decoded cap is refused up front.
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
        let Some(leaf) = relative_path.file_name() else {
            return error(ReadError::Invalid);
        };
        let parent = match root.descend(relative_path.parent().unwrap_or_else(|| Path::new(""))) {
            Ok(parent) => parent,
            Err(source) => return error(crate::read_error(&source)),
        };
        let target = match WriteTarget::prepare(parent, leaf, expected_revision) {
            Ok(target) => target,
            Err(source) => return error(crate::read_error(&source)),
        };
        let original = target
            .original()
            .map(|metadata| crate::stamp(metadata).stamp);
        let Some(ticket) = self.next_ticket() else {
            return error(ReadError::Unavailable);
        };
        let staged = match target.stage(move |file| file.write_all(&bytes)) {
            Ok(staged) => staged,
            Err(source) => return error(crate::read_error(&source)),
        };
        self.prepared = Some((ticket, staged));
        ClientReply::WritePrepared { ticket, original }
    }

    /// Handles a `CommitWrite` for the exact live ticket: publishes the
    /// staged bytes through the shared kernel and returns the published
    /// descriptor's stamp. A wrong ticket is refused without disturbing the
    /// live preparation; a post-crossing failure is `CommitUnknown`.
    pub(super) fn commit(&mut self, ticket: DecimalU64) -> ClientReply {
        if self.prepared.as_ref().map(|(live, _)| *live) != Some(ticket) {
            return error(ReadError::Invalid);
        }
        let (ticket, staged) = self.prepared.take().expect("live ticket checked");
        match staged.publish() {
            Err(source) => write_failure(ticket, &source),
            Ok(published) => match published.finish() {
                Ok(metadata) => ClientReply::Written {
                    ticket,
                    metadata: crate::stamp(&metadata).stamp,
                },
                Err(source) => write_failure(ticket, &source),
            },
        }
    }

    /// Handles an `AbortWrite` for the exact live ticket: the staged object
    /// is dropped, removing only its own proven private stage.
    pub(super) fn abort(&mut self, ticket: DecimalU64) -> ClientReply {
        if self.prepared.as_ref().map(|(live, _)| *live) != Some(ticket) {
            return error(ReadError::Invalid);
        }
        let (ticket, _staged) = self.prepared.take().expect("live ticket checked");
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
            .filter(|name: &PathBuf| name.to_str().unwrap().starts_with(".rustx-write-"))
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
            Some(&original.revision),
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
        let ClientReply::WritePrepared { ticket, .. } =
            writes.prepare(&root, std::path::Path::new("file"), None, &encode(b"data"))
        else {
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
    fn one_outstanding_preparation_per_connection() {
        let (_dir, root) = fixture();
        let mut writes = Writes::new();
        let ClientReply::WritePrepared { ticket, .. } =
            writes.prepare(&root, std::path::Path::new("one"), None, &encode(b"one"))
        else {
            panic!("expected a prepared write")
        };
        let reply = writes.prepare(&root, std::path::Path::new("two"), None, &encode(b"two"));
        assert_eq!(
            reply,
            ClientReply::Error {
                kind: ReadError::Invalid
            }
        );
        assert_eq!(writes.abort(ticket), ClientReply::WriteAborted { ticket });
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
            Some(&original.revision),
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
    fn dropping_the_owner_cleans_an_unpublished_stage() {
        let (dir, root) = fixture();
        {
            let mut writes = Writes::new();
            let ClientReply::WritePrepared { .. } = writes.prepare(
                &root,
                std::path::Path::new("file"),
                None,
                &encode(b"orphaned"),
            ) else {
                panic!("expected a prepared write")
            };
            assert_eq!(stage_names(dir.path()).len(), 1);
            // EOF/revoke drops the owner: the staged object's own
            // dev:inode-proven cleanup removes only its private stage.
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
            None,
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
                writes.prepare(&root, std::path::Path::new(path), None, &encode(b"x")),
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
            let reply = writes.prepare(&root, std::path::Path::new("file"), None, &encoded);
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

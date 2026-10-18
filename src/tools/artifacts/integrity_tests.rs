use super::{ArtifactReadHandle, ArtifactStore, seal};
use crate::runtime::identity::{ArtifactId, ConversationId};
use std::cell::RefCell;
use std::io::{Read as _, Seek as _, Write as _};
use std::path::Path;

type CopyHook = Box<dyn FnMut(&Path, &str, u64)>;
type FinishHook = Box<dyn FnOnce(&Path) -> std::io::Result<()>>;
thread_local! {
    static READ: RefCell<Option<CopyHook>> = const { RefCell::new(None) };
    static VERIFIED: RefCell<Option<CopyHook>> = const { RefCell::new(None) };
    static FINISH: RefCell<Option<FinishHook>> = const { RefCell::new(None) };
}
pub(super) fn copy_read(root: &Path, id: &str, offset: u64) {
    READ.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(root, id, offset);
        }
    });
}
pub(super) fn copy_verified(root: &Path, id: &str, offset: u64) {
    VERIFIED.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(root, id, offset);
        }
    });
}
pub(super) fn before_seal_header(root: &Path) -> std::io::Result<()> {
    let hook = FINISH.with(|hook| hook.borrow_mut().take());
    hook.map_or(Ok(()), |hook| hook(root))
}
struct ClearHooks;
impl Drop for ClearHooks {
    fn drop(&mut self) {
        READ.with(|hook| *hook.borrow_mut() = None);
        VERIFIED.with(|hook| *hook.borrow_mut() = None);
        FINISH.with(|hook| *hook.borrow_mut() = None);
    }
}
struct CopyFixture {
    source_dir: tempfile::TempDir,
    destination_dir: tempfile::TempDir,
    source: ArtifactStore,
    destination: ArtifactStore,
    id: ArtifactId,
    bytes: Vec<u8>,
}
impl CopyFixture {
    fn new() -> Self {
        let source_dir = tempfile::tempdir().unwrap();
        let destination_dir = tempfile::tempdir().unwrap();
        let source = ArtifactStore::new(ConversationId::generate(), source_dir.path()).unwrap();
        let destination =
            ArtifactStore::new(ConversationId::generate(), destination_dir.path()).unwrap();
        let id = source.create_artifact().unwrap();
        let bytes = (0..4 * seal::CHUNK + 17)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect::<Vec<_>>();
        let mut writer = source.open_writer(&id).unwrap();
        // Nonaligned producer writes must not become digest chunk boundaries.
        for part in bytes.chunks(777) {
            writer.write_all(part).unwrap();
        }
        writer.finish().unwrap();
        Self {
            source_dir,
            destination_dir,
            source,
            destination,
            id,
            bytes,
        }
    }
    fn copy(&self) -> std::io::Result<ArtifactId> {
        self.destination
            .copy_from(&self.source, &self.id, &mut (64 * 1024 * 1024))
    }
    fn target(&self, id: &ArtifactId) -> Vec<u8> {
        let mut reader =
            ArtifactStore::open_archive_reader(self.destination_dir.path(), id).unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        verify(&mut reader, &bytes);
        bytes
    }
}
fn overwrite(root: &Path, id: &str, offset: u64) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(root.join(format!("{id}.bin")))
        .unwrap();
    let length = file.metadata().unwrap().len();
    file.seek(std::io::SeekFrom::Start(offset)).unwrap();
    file.write_all(b"changed").unwrap();
    assert_eq!(file.metadata().unwrap().len(), length);
}
fn verify(reader: &mut ArtifactReadHandle, bytes: &[u8]) {
    reader.seek(std::io::SeekFrom::Start(0)).unwrap();
    let mut verification = reader.verification();
    for (index, part) in bytes.chunks(seal::CHUNK).enumerate() {
        reader
            .verify_chunk(
                &mut verification,
                u64::try_from(index * seal::CHUNK).unwrap(),
                part,
            )
            .unwrap();
    }
    reader.seek(std::io::SeekFrom::End(0)).unwrap();
    reader.finish_verification(verification).unwrap();
}

#[test]
fn abandoned_writer_and_failed_finish_never_produce_a_successful_settlement() {
    let directory = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(ConversationId::generate(), directory.path()).unwrap();
    let id = store.create_artifact().unwrap();
    let mut writer = store.open_writer(&id).unwrap();
    writer.write_all(b"unfinished").unwrap();
    drop(writer);
    assert!(ArtifactStore::open_archive_reader(directory.path(), &id).is_err());
    let id = store.create_artifact().unwrap();
    let mut writer = store.open_writer(&id).unwrap();
    writer.write_all(b"finish must return failure").unwrap();
    let checked_id = id.clone();
    let _hooks = ClearHooks;
    FINISH.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |root| {
            assert!(
                ArtifactStore::open_archive_reader(root, &checked_id).is_err(),
                "exclusive owner still held"
            );
            let header =
                std::fs::read(root.join(format!("{}.reserved", checked_id.as_str()))).unwrap();
            assert_eq!(
                &header[..64],
                &[0; 64],
                "header not published before barriers"
            );
            Err(std::io::Error::other("deterministic settlement fault"))
        }));
    });
    assert!(writer.finish().is_err());
    assert!(ArtifactStore::open_archive_reader(directory.path(), &id).is_err());
}

#[test]
fn native_artifact_copy_rejects_equal_length_tamper_and_keeps_failed_target_unsealed() {
    let fixture = CopyFixture::new();
    overwrite(fixture.source_dir.path(), fixture.id.as_str(), 65_536 + 7);
    assert!(fixture.copy().is_err());
    assert!(
        ArtifactStore::open_archive_reader(
            fixture.destination_dir.path(),
            &ArtifactId::new("artifact_1")
        )
        .is_err()
    );
}

#[test]
fn native_artifact_copy_rejects_later_chunk_changes_after_first_verified_buffer() {
    let fixture = CopyFixture::new();
    let _hooks = ClearHooks;
    VERIFIED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|root, id, offset| {
            if offset == 0 {
                overwrite(root, id, 65_536 + 7);
            }
        }));
    });
    assert!(fixture.copy().is_err());
    assert!(
        ArtifactStore::open_archive_reader(
            fixture.destination_dir.path(),
            &ArtifactId::new("artifact_1")
        )
        .is_err()
    );
}

#[test]
fn native_artifact_copy_uses_same_verified_buffer_despite_after_read_or_verified_changes() {
    for after_verification in [false, true] {
        let fixture = CopyFixture::new();
        let _hooks = ClearHooks;
        let callback: CopyHook = Box::new(|root, id, offset| {
            if offset == 0 {
                overwrite(root, id, 7);
            }
        });
        if after_verification {
            VERIFIED.with(|hook| *hook.borrow_mut() = Some(callback));
        } else {
            READ.with(|hook| *hook.borrow_mut() = Some(callback));
        }
        let id = fixture.copy().unwrap();
        assert_eq!(fixture.target(&id), fixture.bytes);
        assert_ne!(
            std::fs::read(fixture.source.path_of(&fixture.id)).unwrap(),
            fixture.bytes
        );
    }
}

#[test]
fn native_artifact_seal_missing_malformed_and_partial_verification_fail_closed() {
    let fixture = CopyFixture::new();
    let path = fixture
        .source_dir
        .path()
        .join(format!("{}.reserved", fixture.id.as_str()));
    let original = std::fs::read(&path).unwrap();
    let excessive_count = u64::MAX.to_le_bytes();
    for (offset, bytes) in [
        (0, b"NOTASEAL".as_slice()),
        (16, excessive_count.as_slice()),
        (64, &[0; 32]),
        (24, &[0; 32]),
    ] {
        let mut changed = original.clone();
        changed[offset..offset + bytes.len()].copy_from_slice(bytes);
        std::fs::write(&path, changed).unwrap();
        assert!(fixture.copy().is_err());
        std::fs::write(&path, &original).unwrap();
    }
    std::fs::write(&path, &original[..original.len() - 1]).unwrap();
    assert!(fixture.copy().is_err());
    std::fs::write(&path, original).unwrap();
    let mut reader =
        ArtifactStore::open_archive_reader(fixture.source_dir.path(), &fixture.id).unwrap();
    assert!(
        reader.finish_verification(reader.verification()).is_err(),
        "cannot skip verified chunks"
    );
    let id = fixture.copy().unwrap();
    assert_eq!(fixture.target(&id), fixture.bytes);
}

#[test]
fn native_artifact_copy_cannot_reseal_changed_destination_as_an_inherited_original() {
    let fixture = CopyFixture::new();
    let target = fixture.destination_dir.path().to_path_buf();
    let _hooks = ClearHooks;
    VERIFIED.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |_, _, offset| {
            if offset == 4 * 65_536 {
                // Earlier verified source bytes have already been written;
                // changing them must not become a new destination seal.
                overwrite(&target, "artifact_1", 7);
            }
        }));
    });
    assert!(fixture.copy().is_err());
    assert!(
        ArtifactStore::open_archive_reader(
            fixture.destination_dir.path(),
            &ArtifactId::new("artifact_1")
        )
        .is_err()
    );
}

#[test]
fn native_bounded_artifact_reads_require_finish_and_reject_same_length_changes() {
    let directory = tempfile::tempdir().unwrap();
    let store = ArtifactStore::new(ConversationId::generate(), directory.path()).unwrap();
    let unfinished = store.create_artifact().unwrap();
    let mut writer = store.open_writer(&unfinished).unwrap();
    writer.write_all(b"not settled").unwrap();
    drop(writer);
    assert!(store.read_bounded(&unfinished).is_err());
    let bytes = (0..2 * 65_536 + 17)
        .map(|index| u8::try_from(index % 251).unwrap())
        .collect::<Vec<_>>();
    let id = store.put_bounded(&bytes).unwrap();
    assert_eq!(store.read_bounded(&id).unwrap(), bytes);
    overwrite(directory.path(), id.as_str(), 65_536 + 7);
    assert!(store.read_bounded(&id).is_err());
    let cold = ArtifactStore::new(ConversationId::generate(), directory.path()).unwrap();
    assert!(cold.read_bounded(&id).is_err());
}

#[test]
fn settled_artifact_identity_cannot_reseal_after_external_bytefile_removal() {
    let fixture = CopyFixture::new();
    let reservation = fixture
        .source_dir
        .path()
        .join(format!("{}.reserved", fixture.id.as_str()));
    let original_seal = std::fs::read(&reservation).unwrap();
    std::fs::remove_file(fixture.source.path_of(&fixture.id)).unwrap();
    assert!(fixture.source.open_writer(&fixture.id).is_err());
    assert_eq!(std::fs::read(reservation).unwrap(), original_seal);
    assert!(
        !fixture.source.path_of(&fixture.id).exists(),
        "no replacement bytes published"
    );
    assert!(fixture.copy().is_err());
}

//! Real streamed ZIP completion must authenticate each settled artifact buffer.
use super::*;
use crate::durable::CanonicalStore;
use crate::session_archive::{ArtifactReadStage, CHUNK_BYTES};
use std::io::{Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    directory: tempfile::TempDir,
    session: SessionId,
    artifact: PathBuf,
    entry: String,
    bytes: Vec<u8>,
}
impl Fixture {
    fn new(bytes: Vec<u8>) -> Self {
        let (directory, catalog, _) = open_catalog();
        let (conversation, session, _) = append_history(&catalog, &[]);
        let store = store_for(&catalog, &session, &conversation);
        let root = catalog
            .database_path(&session, &conversation)
            .parent()
            .unwrap()
            .to_path_buf();
        let artifacts = ArtifactStore::new(conversation.clone(), &root).unwrap();
        let id = artifacts.create_artifact().unwrap();
        let mut writer = artifacts.open_writer(&id).unwrap();
        writer.write_all(&bytes).unwrap();
        writer.finish().unwrap();
        let mut message = user("archive-integrity", "retained");
        let MessageBlock::User(user) = &mut message else {
            unreachable!()
        };
        user.content.push(UserContentBlock::File(
            crate::message::content::FileReference {
                artifact_id: id.clone(),
                name: Some("original.bin".into()),
                mime_type: None,
                description: None,
            },
        ));
        store.append_canonical(&message).unwrap();
        Self {
            directory,
            session,
            artifact: root.join(format!("{id}.bin")),
            entry: format!("artifacts/{conversation}/{id}/content"),
            bytes,
        }
    }

    fn prepare(&self) -> SessionArchiveCut {
        SessionArchiveProducer::prepare(
            self.directory.path(),
            &self.session,
            &CancellationToken::new(),
        )
        .unwrap()
    }
}

fn overwrite(path: &Path, offset: u64) {
    let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(b"B").unwrap();
}

async fn assert_stream_failure(cut: SessionArchiveCut) {
    let mut stream = cut.stream();
    let mut failure = None;
    while let Some(result) = stream.recv().await {
        if let Err(error) = result {
            failure = Some(error);
            break;
        }
    }
    assert!(
        failure.is_some(),
        "tampered bytes reached successful archive EOF"
    );
    assert!(stream.recv().await.is_none());
    // Await the original worker even if ZIP drop attempted to emit a footer.
    stream.cancel().await;
}

#[tokio::test]
async fn archive_rejects_equal_length_tamper_after_settlement_before_preflight() {
    let fixture = Fixture::new(vec![b'A'; CHUNK_BYTES * 2 + 7]);
    overwrite(&fixture.artifact, 11);
    // Seal/header preflight is bounded; body verification belongs to streaming.
    assert_stream_failure(fixture.prepare()).await;
}

#[tokio::test]
async fn archive_rejects_equal_length_tamper_after_preflight() {
    let fixture = Fixture::new(vec![b'A'; CHUNK_BYTES * 2 + 7]);
    let cut = fixture.prepare();
    overwrite(&fixture.artifact, 11);
    assert_stream_failure(cut).await;
}

#[tokio::test]
async fn archive_rejects_future_chunk_mutation_between_verified_buffers() {
    let fixture = Fixture::new(vec![b'A'; CHUNK_BYTES * 2 + 7]);
    let path = fixture.artifact.clone();
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let cut = fixture
        .prepare()
        .with_artifact_read_hook(move |stage, offset| {
            if stage == ArtifactReadStage::Verified && offset == 0 {
                count.fetch_add(1, Ordering::SeqCst);
                overwrite(&path, u64::try_from(CHUNK_BYTES).unwrap() + 3);
            }
        });
    assert_stream_failure(cut).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn archive_exports_the_verified_buffer_after_source_mutation() {
    let fixture = Fixture::new(vec![b'A'; CHUNK_BYTES * 2 + 7]);
    let path = fixture.artifact.clone();
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let cut = fixture
        .prepare()
        .with_artifact_read_hook(move |stage, offset| {
            if stage == ArtifactReadStage::Read && offset == 0 {
                count.fetch_add(1, Ordering::SeqCst);
                overwrite(&path, 11);
            }
        });
    let files = decode(cut).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(files[&fixture.entry], fixture.bytes);
    assert_eq!(fs::read(&fixture.artifact).unwrap()[11], b'B');
}

#[tokio::test]
async fn archive_checks_completion_for_empty_and_extended_artifacts() {
    let empty = Fixture::new(Vec::new());
    assert!(decode(empty.prepare()).await[&empty.entry].is_empty());
    let cut = empty.prepare();
    fs::write(&empty.artifact, b"B").unwrap();
    assert_stream_failure(cut).await;

    let fixture = Fixture::new(vec![b'A'; CHUNK_BYTES + 7]);
    let path = fixture.artifact.clone();
    let cut = fixture
        .prepare()
        .with_artifact_read_hook(move |stage, offset| {
            if stage == ArtifactReadStage::Verified && offset == u64::try_from(CHUNK_BYTES).unwrap()
            {
                fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(b"B")
                    .unwrap();
            }
        });
    assert_stream_failure(cut).await;
}

#[tokio::test]
async fn archive_rejects_rehashed_chunk_evidence_against_captured_aggregate() {
    use sha2::{Digest as _, Sha256};

    let fixture = Fixture::new(vec![b'A'; CHUNK_BYTES * 2 + 7]);
    let path = fixture.artifact.clone();
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let cut = fixture
        .prepare()
        .with_artifact_read_hook(move |stage, offset| {
            if stage != ArtifactReadStage::Verified {
                return;
            }
            count.fetch_add(1, Ordering::SeqCst);
            if offset == 0 {
                overwrite(&path, u64::try_from(CHUNK_BYTES).unwrap());
                let mut changed = vec![b'A'; CHUNK_BYTES];
                changed[0] = b'B';
                let mut reservation = fs::OpenOptions::new()
                    .write(true)
                    .open(path.with_extension("reserved"))
                    .unwrap();
                // The second chunk digest follows the fixed 64-byte seal header.
                reservation.seek(SeekFrom::Start(64 + 32)).unwrap();
                reservation.write_all(&Sha256::digest(&changed)).unwrap();
            }
        });
    assert_stream_failure(cut).await;
    assert_eq!(hits.load(Ordering::SeqCst), 3, "all chunk checks passed");
}

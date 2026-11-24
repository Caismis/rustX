//! Client-owned physical storage for normalized result imports.

use std::{
    collections::HashMap,
    io::{self, Read},
    path::Path,
    sync::{Arc, Mutex, TryLockError, mpsc},
    time::{Duration, Instant},
};

use rustx_confined_fs::PackageDirectory;
use rustx_execution_protocol::{
    Admission, MAX_RAW_CHUNK_BYTES, OPTIONAL_SAVE_DEADLINE,
    process::OperationId,
    save::{
        CandidateId, Cleanup, ConversationId, Locator, Readability, SaveCapture, SaveCode, SaveKey,
        SaveOwner, SaveReply, SaveRequest, SaveSelection, SaveStorage, SealRecord,
    },
};
use sha2::{Digest, Sha256};

use crate::bash_capture::{CaptureFile, Captures};

const MAX_FILES: usize = 16;
const MAX_BYTES_PER_FILE: u64 = 17 * 1024 * 1024;
const MAX_OPERATIONS: usize = 4096;
const CLEANUP_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) struct Saves {
    admission: Admission,
    files: Captures,
    entries: HashMap<OperationId, Entry>,
    closed: bool,
}

struct Entry {
    owner: SaveOwner,
    capture: SaveCapture,
    expires: Instant,
    storage: Storage,
}

struct Storage {
    file: Option<Arc<CaptureFile>>,
    published: Option<(Locator, SealRecord)>,
}

impl Saves {
    pub(crate) fn new(admission: Admission) -> Self {
        Self {
            admission,
            files: Captures::with_capacity("rustx-save-", MAX_FILES),
            entries: HashMap::new(),
            closed: false,
        }
    }

    /// Cleans abandoned files even when the Server could not send Abort and
    /// no other save requests arrive. The maintenance owner runs off both
    /// transport lanes. A failed unlink leaves the entry and its quota held.
    pub(crate) fn maintain(registry: &Mutex<Self>, stop: &mpsc::Receiver<()>) {
        while let Err(mpsc::RecvTimeoutError::Timeout) = stop.recv_timeout(CLEANUP_INTERVAL) {
            if let Ok(mut saves) = registry.try_lock() {
                saves.reap_expired(Instant::now());
            }
        }
    }

    fn reap_expired(&mut self, now: Instant) {
        if self.closed {
            return;
        }
        for entry in self.entries.values_mut() {
            // A sent Publish can have succeeded even when its reply was
            // lost. Expiry only retires files with no physical publication.
            if now >= entry.expires && entry.storage.published.is_none() {
                entry.storage.remove();
            }
        }
    }

    pub(crate) fn dispatch(&mut self, request: &SaveRequest, now: Instant) -> SaveReply {
        if self.closed {
            return refused(SaveCode::StaleOwner);
        }
        match request {
            SaveRequest::Begin { owner, .. } => self.begin(owner, request, now),
            SaveRequest::Chunk { key, .. }
            | SaveRequest::Seal { key, .. }
            | SaveRequest::Publish { key, .. }
            | SaveRequest::Probe { key }
            | SaveRequest::Abort { key } => {
                let Some(entry) = self.entries.get_mut(&key.owner.operation) else {
                    return refused(SaveCode::StaleOwner);
                };
                // The admitted Server sends Publish only after the business
                // claim. The machine independently checks key and seal.
                if matches!(request, SaveRequest::Publish { .. })
                    && let Err(code) = entry.capture.approve(&key.owner)
                {
                    return refused(code);
                }
                entry.capture.apply(request, now, &mut entry.storage)
            }
        }
    }

    fn begin(&mut self, owner: &SaveOwner, request: &SaveRequest, now: Instant) -> SaveReply {
        if owner.session != self.admission.session
            || owner.instance != self.admission.instance
            || owner.generation != self.admission.generation
            || owner.incarnation != self.admission.server_incarnation
        {
            return refused(SaveCode::StaleOwner);
        }
        if let Some(entry) = self.entries.get_mut(&owner.operation) {
            return entry.capture.apply(request, now, &mut entry.storage);
        }
        if self.entries.len() >= MAX_OPERATIONS {
            return refused(SaveCode::Capacity);
        }
        self.reap_expired(now);
        let file = match self.files.create(owner.operation) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return refused(SaveCode::Capacity);
            }
            Err(_) => return refused(SaveCode::StorageUnavailable),
        };
        let expires = now + OPTIONAL_SAVE_DEADLINE;
        let mut selection = SaveSelection::new(0);
        selection
            .observe(1, now, expires, || Some(owner.clone()))
            .expect("one byte cannot overflow the selection");
        let mut entry = Entry {
            owner: owner.clone(),
            capture: SaveCapture::new(selection, CandidateId::generate()),
            expires,
            storage: Storage {
                file: Some(file),
                published: None,
            },
        };
        let reply = entry.capture.apply(request, now, &mut entry.storage);
        self.entries.insert(owner.operation, entry);
        reply
    }

    pub(crate) fn with_published<T>(
        &self,
        locator: &Locator,
        conversation: &ConversationId,
        read: impl FnOnce(&PackageDirectory, &Path) -> T,
    ) -> io::Result<T> {
        if !self.closed
            && let Some(file) = self.entries.values().find_map(|entry| {
                let (published, _) = entry.storage.published.as_ref()?;
                (entry.owner.conversation == *conversation && published == locator)
                    .then_some(entry.storage.file.as_ref()?)
            })
        {
            return file.read_verified(|root, name, _| read(root, name));
        }
        Err(io::ErrorKind::NotFound.into())
    }

    pub(crate) fn close(&mut self) -> bool {
        self.closed = true;
        let mut clean = true;
        for entry in self.entries.values_mut() {
            clean &= matches!(
                entry.storage.remove(),
                Cleanup::Removed | Cleanup::AlreadyRemoved
            );
        }
        clean && self.files.close().is_ok()
    }

    pub(crate) fn close_by(registry: &Mutex<Self>, cutoff: Instant) -> bool {
        loop {
            match registry.try_lock() {
                Ok(mut saves) => return saves.close(),
                Err(TryLockError::Poisoned(_)) => return false,
                Err(TryLockError::WouldBlock) => {}
            }
            let remaining = cutoff.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            std::thread::sleep(remaining.min(Duration::from_millis(5)));
        }
    }
}

impl Storage {
    fn remove(&mut self) -> Cleanup {
        let Some(file) = self.file.as_ref() else {
            return Cleanup::AlreadyRemoved;
        };
        if file.discard().is_err() {
            return Cleanup::Unknown;
        }
        self.file = None;
        Cleanup::Removed
    }
}

impl SaveStorage for Storage {
    fn append(&mut self, _key: &SaveKey, offset: u64, data: &[u8]) -> Result<(), SaveCode> {
        if offset.saturating_add(data.len() as u64) > MAX_BYTES_PER_FILE {
            return Err(SaveCode::Capacity);
        }
        self.file
            .as_ref()
            .ok_or(SaveCode::StorageUnavailable)?
            .append_at(offset, data)
            .map_err(|_| SaveCode::StorageUnavailable)
    }

    fn publish(&mut self, _key: &SaveKey, seal: &SealRecord) -> Result<Locator, SaveCode> {
        self.file
            .as_ref()
            .ok_or(SaveCode::StorageUnavailable)?
            .sync_for_publication(seal.length)
            .map_err(|_| SaveCode::StorageUnavailable)?;
        let token = OperationId::new();
        let locator = Locator::parse(&format!("save:{:032x}", token.as_u128()))
            .expect("random hexadecimal output token");
        self.published = Some((locator.clone(), *seal));
        Ok(locator)
    }

    fn readability(&mut self, locator: &Locator) -> Readability {
        let Some((current, seal)) = &self.published else {
            return Readability::Missing;
        };
        let Some(file) = &self.file else {
            return Readability::Missing;
        };
        if current != locator {
            return Readability::Missing;
        }
        match file.read_verified(|_, _, file| matches_seal(file, seal)) {
            Ok(Ok(true)) => Readability::Readable,
            Ok(Ok(false)) => Readability::Changed,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Readability::Missing,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => Readability::Changed,
            _ => Readability::Unknown,
        }
    }

    fn cleanup(&mut self, _key: &SaveKey) -> Cleanup {
        self.remove()
    }
}

fn matches_seal(file: std::fs::File, seal: &SealRecord) -> io::Result<bool> {
    let mut digest = Sha256::new();
    let Some(read_limit) = seal.length.checked_add(1) else {
        return Ok(false);
    };
    let mut bounded = file.take(read_limit);
    let mut buffer = vec![0; MAX_RAW_CHUNK_BYTES];
    let mut length = 0;
    loop {
        let count = bounded.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        length += count as u64;
        digest.update(&buffer[..count]);
    }
    Ok(length == seal.length && digest.finalize().as_slice() == seal.sha256.bytes())
}

fn refused(code: SaveCode) -> SaveReply {
    SaveReply::Refused { code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustx_execution_protocol::{
        ClientInstanceId, EnvironmentId, Generation, PROTOCOL_VERSION, SessionId,
        process::ServerIncarnation, save::ContentFormat,
    };

    fn fixture_admission() -> Admission {
        Admission {
            version: PROTOCOL_VERSION,
            server_incarnation: ServerIncarnation::generate(),
            session: SessionId::new("session-1"),
            environment: EnvironmentId::new("env-1"),
            instance: ClientInstanceId::new("instance-1"),
            generation: Generation::new(std::num::NonZeroU64::MIN),
            workspace: "/unused".into(),
        }
    }

    fn begin(saves: &mut Saves, now: Instant) -> SaveKey {
        let admission = &saves.admission;
        let owner = SaveOwner {
            incarnation: admission.server_incarnation,
            session: admission.session.clone(),
            instance: admission.instance.clone(),
            generation: admission.generation,
            conversation: ConversationId::parse("conv-1").unwrap(),
            operation: OperationId::new(),
        };
        let SaveReply::Begun { key } = saves.dispatch(
            &SaveRequest::Begin {
                owner,
                content_format: ContentFormat::NormalizedUtf8V1,
            },
            now,
        ) else {
            panic!("candidate owns a file");
        };
        key
    }

    #[test]
    fn abort_retries_unlink_and_retains_the_slot_until_removal_is_proven() {
        let mut saves = Saves::new(fixture_admission());
        let now = Instant::now();
        let key = begin(&mut saves, now);
        let root = saves.files.path().unwrap();
        let file = saves.entries[&key.owner.operation]
            .storage
            .file
            .as_ref()
            .unwrap();
        file.fail_next_unlink
            .store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(saves.files.available(), MAX_FILES - 1);
        let abort = SaveRequest::Abort { key: key.clone() };
        assert_eq!(
            saves.dispatch(&abort, now),
            SaveReply::Aborted {
                key: key.clone(),
                cleanup: Cleanup::Unknown
            }
        );
        assert_eq!(saves.files.available(), MAX_FILES - 1);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(
            saves.dispatch(&abort, now),
            SaveReply::Aborted {
                key,
                cleanup: Cleanup::Removed
            }
        );
        assert_eq!(saves.files.available(), MAX_FILES);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        assert!(saves.close());
        assert!(!root.exists());
    }

    #[test]
    fn expired_save_cleanup_retries_unlink_and_preserves_published_output() {
        let mut saves = Saves::new(fixture_admission());
        let now = Instant::now();
        let cutoff = now + OPTIONAL_SAVE_DEADLINE;
        let abandoned = begin(&mut saves, now);
        let completed = begin(&mut saves, now);
        let length = 4_u64.into();
        let sha256 =
            rustx_execution_protocol::save::Sha256Hex::from_digest(Sha256::digest(b"done").into());
        let extent = rustx_execution_protocol::save::SaveExtent::Complete;
        assert!(matches!(
            saves.dispatch(
                &SaveRequest::Chunk {
                    key: completed.clone(),
                    sequence: 1_u64.into(),
                    offset: 0_u64.into(),
                    data_base64: "ZG9uZQ==".to_owned(),
                    sha256,
                },
                now
            ),
            SaveReply::Chunked { .. }
        ));
        assert!(matches!(
            saves.dispatch(
                &SaveRequest::Seal {
                    key: completed.clone(),
                    length,
                    sha256,
                    extent,
                },
                now
            ),
            SaveReply::Sealed { .. }
        ));
        let SaveReply::Published { locator, .. } = saves.dispatch(
            &SaveRequest::Publish {
                key: completed.clone(),
                length,
                sha256,
                extent,
            },
            now,
        ) else {
            panic!("completed output was not published");
        };
        saves.entries[&abandoned.owner.operation]
            .storage
            .file
            .as_ref()
            .unwrap()
            .fail_next_unlink
            .store(true, std::sync::atomic::Ordering::Release);
        saves.reap_expired(cutoff);
        assert_eq!(saves.files.available(), MAX_FILES - 2);
        saves.reap_expired(cutoff + CLEANUP_INTERVAL);
        assert_eq!(saves.files.available(), MAX_FILES - 1);
        assert_eq!(
            saves
                .with_published(&locator, &completed.owner.conversation, |root, path| {
                    let mut output = Vec::new();
                    root.file(path).unwrap().read_to_end(&mut output).unwrap();
                    output
                })
                .unwrap(),
            b"done"
        );
        assert!(saves.close());
    }
}

//! Connection-owned, anonymous document scratch. Control records carry 64 KiB
//! chunks; only the existing source transaction can publish an authored file.
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::protocol::{ErrorData, RpcError};
use super::rpc_error::{domain, rpc_error};
use crate::local_runtime::configuration::settings::SourceTarget;
use crate::workflow_file::{MAX_WORKFLOW_DOCUMENT_BYTES, WorkflowFileDocument};

const CAPACITY: usize = 4;
const TTL: Duration = Duration::from_secs(60);
const CHUNK: usize = 65_536;
/// Exact-u64 wire strings add quotes. Twice the normalized document allowance
/// accommodates numeric encoding without shrinking its independent 16 MiB cap.
pub(crate) const MAX_TRANSFER_BYTES: usize = 2 * MAX_WORKFLOW_DOCUMENT_BYTES;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDocumentRef {
    pub id: String,
    pub byte_length: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDocumentChunk {
    pub offset: u32,
    pub data: String,
    pub next_offset: Option<u32>,
}

struct Document {
    target: SourceTarget,
    reference: WorkflowDocumentRef,
    file: std::fs::File,
    written: u32,
    ready: bool,
    used: Instant,
}

#[derive(Default)]
pub(super) struct WorkflowDocuments {
    documents: Mutex<BTreeMap<String, Arc<Mutex<Document>>>>,
    closed: AtomicBool,
    #[cfg(test)]
    finish_gate: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
}

fn invalid() -> RpcError {
    domain(ErrorData::InvalidParams)
}
fn io(_: impl std::fmt::Display) -> RpcError {
    rpc_error(
        -32000,
        "Workflow document scratch I/O failed; check temporary disk space and permissions, then upload the document again",
        Some(ErrorData::OperationFailed),
    )
}

impl WorkflowDocuments {
    #[cfg(test)]
    pub(super) fn gate_finish(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered, observed) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        *self.finish_gate.lock().unwrap() = Some((entered, wait));
        (observed, release)
    }
    pub(super) fn clear(&self) {
        self.closed.store(true, Ordering::Release);
        self.documents.lock().expect("Workflow documents").clear();
    }

    pub(super) fn begin(
        &self,
        target: SourceTarget,
        byte_length: u32,
        sha256: String,
    ) -> Result<WorkflowDocumentRef, RpcError> {
        target.validate().map_err(|_| invalid())?;
        if byte_length == 0
            || byte_length as usize > MAX_TRANSFER_BYTES
            || sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(invalid());
        }
        let mut documents = self.documents.lock().expect("Workflow documents");
        documents.retain(|_, value| {
            value
                .try_lock()
                .map_or(true, |value| value.used.elapsed() < TTL)
        });
        if self.closed.load(Ordering::Acquire) {
            return Err(invalid());
        }
        if documents.len() == CAPACITY {
            return Err(rpc_error(
                -32000,
                "Workflow document staging holds four documents; finish or release one, or reconnect",
                Some(ErrorData::OperationFailed),
            ));
        }
        let reference = WorkflowDocumentRef {
            id: uuid::Uuid::now_v7().to_string(),
            byte_length,
            sha256,
        };
        documents.insert(
            reference.id.clone(),
            Arc::new(Mutex::new(Document {
                target,
                reference: reference.clone(),
                file: tempfile::tempfile().map_err(io)?,
                written: 0,
                ready: false,
                used: Instant::now(),
            })),
        );
        Ok(reference)
    }

    fn with<T>(
        &self,
        target: &SourceTarget,
        reference: &WorkflowDocumentRef,
        operation: impl FnOnce(&mut Document) -> Result<T, RpcError>,
    ) -> Result<T, RpcError> {
        target.validate().map_err(|_| invalid())?;
        let mut documents = self.documents.lock().expect("Workflow documents");
        documents.retain(|_, value| {
            value
                .try_lock()
                .map_or(true, |value| value.used.elapsed() < TTL)
        });
        if self.closed.load(Ordering::Acquire) {
            return Err(invalid());
        }
        let document = documents.get(&reference.id).cloned().ok_or_else(invalid)?;
        drop(documents);
        let mut document = document.lock().expect("Workflow document");
        if document.target != *target || document.reference != *reference {
            return Err(invalid());
        }
        document.used = Instant::now();
        operation(&mut document)
    }

    pub(super) fn write(
        &self,
        target: &SourceTarget,
        reference: &WorkflowDocumentRef,
        offset: u32,
        data: &str,
    ) -> Result<(), RpcError> {
        if data.len() > CHUNK.div_ceil(3) * 4 {
            return Err(invalid());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| invalid())?;
        if bytes.is_empty() || bytes.len() > CHUNK {
            return Err(invalid());
        }
        self.with(target, reference, |document| {
            if document.ready
                || offset != document.written
                || u64::from(offset) + bytes.len() as u64 > u64::from(reference.byte_length)
            {
                return Err(invalid());
            }
            document.file.seek(SeekFrom::End(0)).map_err(io)?;
            document.file.write_all(&bytes).map_err(io)?;
            document.written += u32::try_from(bytes.len()).map_err(|_| invalid())?;
            Ok(())
        })
    }

    pub(super) fn finish(
        &self,
        target: &SourceTarget,
        reference: &WorkflowDocumentRef,
    ) -> Result<(), RpcError> {
        self.with(target, reference, |document| {
            #[cfg(test)]
            if let Some((entered, release)) = self.finish_gate.lock().unwrap().take() {
                let _ = entered.send(());
                let _ = release.recv();
            }
            if document.written != reference.byte_length {
                return Err(invalid());
            }
            document.file.rewind().map_err(io)?;
            let mut hash = Sha256::new();
            let mut buffer = vec![0; CHUNK];
            loop {
                let read = document.file.read(&mut buffer).map_err(io)?;
                if read == 0 {
                    break;
                }
                hash.update(&buffer[..read]);
            }
            if format!("{:x}", hash.finalize()) != reference.sha256 {
                return Err(invalid());
            }
            document.file.rewind().map_err(io)?;
            let parsed = decode_document(&document.file)?;
            parsed.validate().map_err(|error| {
                rpc_error(
                    -32000,
                    &error.reason,
                    Some(ErrorData::WorkflowInvalid {
                        diagnostic: Box::new(
                            crate::local_runtime::workflow_authoring::WorkflowDiagnostic {
                                path: error.path,
                                category: error.category,
                                reason: error.reason.clone(),
                                correction: None,
                                detail: None,
                                line: None,
                                column: None,
                            },
                        ),
                    }),
                )
            })?;
            document.ready = true;
            Ok(())
        })
    }

    pub(super) fn resolve(
        &self,
        target: &SourceTarget,
        reference: &WorkflowDocumentRef,
    ) -> Result<Box<WorkflowFileDocument>, RpcError> {
        self.with(target, reference, |document| {
            if !document.ready {
                return Err(invalid());
            }
            document.file.rewind().map_err(io)?;
            decode_document(&document.file).map(Box::new)
        })
    }

    pub(super) fn capture(
        &self,
        target: &SourceTarget,
        document: &WorkflowFileDocument,
    ) -> Result<WorkflowDocumentRef, RpcError> {
        // Already validated under the native expanded-document policy.
        document.validate().map_err(|_| invalid())?;
        let bytes = super::wire::workflow_document_to_bytes(document).map_err(io)?;
        let reference = self.begin(
            target.clone(),
            u32::try_from(bytes.len()).map_err(|_| invalid())?,
            format!("{:x}", Sha256::digest(&bytes)),
        )?;
        let result = (|| {
            for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
                self.write(
                    target,
                    &reference,
                    u32::try_from(index * CHUNK).map_err(|_| invalid())?,
                    &base64::engine::general_purpose::STANDARD.encode(chunk),
                )?;
            }
            self.finish(target, &reference)
        })();
        if result.is_err() {
            self.release(target, &reference)?;
        }
        result.map(|()| reference)
    }

    pub(super) fn read(
        &self,
        target: &SourceTarget,
        reference: &WorkflowDocumentRef,
        offset: u32,
        max_bytes: u32,
    ) -> Result<WorkflowDocumentChunk, RpcError> {
        if max_bytes == 0 || max_bytes as usize > CHUNK || offset > reference.byte_length {
            return Err(invalid());
        }
        self.with(target, reference, |document| {
            if !document.ready {
                return Err(invalid());
            }
            document
                .file
                .seek(SeekFrom::Start(u64::from(offset)))
                .map_err(io)?;
            let mut bytes = vec![0; (reference.byte_length - offset).min(max_bytes) as usize];
            document.file.read_exact(&mut bytes).map_err(io)?;
            let end = offset + u32::try_from(bytes.len()).map_err(|_| invalid())?;
            Ok(WorkflowDocumentChunk {
                offset,
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
                next_offset: (end < reference.byte_length).then_some(end),
            })
        })
    }

    pub(super) fn release(
        &self,
        target: &SourceTarget,
        reference: &WorkflowDocumentRef,
    ) -> Result<(), RpcError> {
        self.with(target, reference, |_| Ok(()))?;
        self.documents
            .lock()
            .expect("Workflow documents")
            .remove(&reference.id);
        Ok(())
    }
}

fn decode_document(file: &std::fs::File) -> Result<WorkflowFileDocument, RpcError> {
    let mut json = String::new();
    std::io::BufReader::new(file)
        .take(MAX_TRANSFER_BYTES as u64 + 1)
        .read_to_string(&mut json)
        .map_err(io)?;
    if json.len() > MAX_TRANSFER_BYTES {
        return Err(invalid());
    }
    super::wire::workflow_document_from_json(&json).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document() -> WorkflowFileDocument {
        serde_json::from_value(serde_json::json!({
            "description":"Document scratch contract",
            "block":{"input":{"type":"object","properties":{},"additionalProperties":false},
                "nodes":{"start":{"type":"start"},"done":{"type":"return","outputs":[]}},
                "edges":[{"from":"start","to":"done"}]}
        }))
        .unwrap()
    }

    #[test]
    fn document_content_preserves_exact_numeric_domains_and_raw_parameter_validation() {
        let mut source = document();
        source.definition.timeout_ms = 9_007_199_254_740_993;
        let bytes = super::super::wire::workflow_document_to_bytes(&source).unwrap();
        let json = String::from_utf8(bytes).unwrap();
        assert!(json.contains("\"timeout_ms\":\"9007199254740993\""));
        assert_eq!(
            super::super::wire::workflow_document_from_json(&json).unwrap(),
            source
        );
        assert!(
            super::super::wire::workflow_document_from_json(
                &json.replace("\"9007199254740993\"", "9007199254740993")
            )
            .is_err()
        );
        assert!(
            super::super::wire::workflow_document_from_json(&json.replacen(
                '{',
                "{\"description\":\"duplicate\",",
                1
            ))
            .is_err()
        );

        let json = r#"{"description":"Model parameters","block":{"nodes":{"model":{"type":"model","model":{"mode":"explicit","model":"fixture/model","request_params":{"temperature":0.12345678901234567890}},"messages":[],"output":{"type":"text"}}},"edges":[]}}"#;
        assert!(
            super::super::wire::workflow_document_from_json(json)
                .unwrap_err()
                .contains("binary64-exact")
        );
        assert!(
            super::super::wire::workflow_document_from_json(
                &json.replace("0.12345678901234567890", "0.5")
            )
            .is_ok()
        );
    }

    #[test]
    fn complete_document_integrity_scope_release_and_connection_lifetime_are_authoritative() {
        let documents = WorkflowDocuments::default();
        let target = SourceTarget::User;
        let document = document();
        let reference = documents.capture(&target, &document).unwrap();
        assert_eq!(*documents.resolve(&target, &reference).unwrap(), document);
        let chunk = documents.read(&target, &reference, 0, 65_536).unwrap();
        assert!(chunk.next_offset.is_none());
        assert_eq!(
            super::super::wire::workflow_document_from_json(
                &String::from_utf8(
                    base64::engine::general_purpose::STANDARD
                        .decode(chunk.data)
                        .unwrap()
                )
                .unwrap()
            )
            .unwrap(),
            document
        );
        let elsewhere = tempfile::tempdir().unwrap();
        let other = SourceTarget::Workspace {
            directory: elsewhere.path().canonicalize().unwrap(),
        };
        assert!(documents.resolve(&other, &reference).is_err());
        assert!(
            WorkflowDocuments::default()
                .resolve(&target, &reference)
                .is_err()
        );
        let mut forged = reference.clone();
        forged.sha256 = "0".repeat(64);
        assert!(documents.resolve(&target, &forged).is_err());
        documents.release(&target, &reference).unwrap();
        assert!(documents.resolve(&target, &reference).is_err());
        documents.clear();
        assert!(documents.capture(&target, &document).is_err());
    }

    #[test]
    fn chunk_order_capacity_expiry_and_encoded_bounds_precede_publication() {
        let documents = WorkflowDocuments::default();
        let target = SourceTarget::User;
        assert!(
            documents
                .begin(target.clone(), 33_554_433, "0".repeat(64))
                .is_err()
        );
        assert!(
            documents
                .begin(target.clone(), 16_777_216, "bad".into())
                .is_err()
        );
        let reference = documents
            .begin(target.clone(), 16_777_216, "0".repeat(64))
            .unwrap();
        let chunk = base64::engine::general_purpose::STANDARD.encode(vec![0; CHUNK]);
        assert!(documents.write(&target, &reference, 1, &chunk).is_err());
        documents.write(&target, &reference, 0, &chunk).unwrap();
        assert!(documents.write(&target, &reference, 0, &chunk).is_err());
        let oversized = base64::engine::general_purpose::STANDARD.encode(vec![0; CHUNK + 1]);
        assert!(
            documents
                .write(&target, &reference, 65_536, &oversized)
                .is_err()
        );
        assert!(documents.finish(&target, &reference).is_err());
        assert!(documents.resolve(&target, &reference).is_err());
        assert!(documents.read(&target, &reference, 0, 65_537).is_err());
        for _ in 1..CAPACITY {
            documents.begin(target.clone(), 1, "0".repeat(64)).unwrap();
        }
        assert!(
            documents
                .begin(target.clone(), 1, "0".repeat(64))
                .unwrap_err()
                .message
                .contains("four documents")
        );
        documents
            .with(&target, &reference, |document| {
                document.used = Instant::now().checked_sub(TTL).unwrap();
                Ok(())
            })
            .unwrap();
        documents.begin(target.clone(), 1, "0".repeat(64)).unwrap();
        assert!(documents.resolve(&target, &reference).is_err());
    }

    #[test]
    fn close_never_waits_for_a_document_parser_and_owned_physical_work_settles() {
        let documents = Arc::new(WorkflowDocuments::default());
        let reference = documents.capture(&SourceTarget::User, &document()).unwrap();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let worker = {
            let documents = documents.clone();
            let entered = entered.clone();
            let release = release.clone();
            std::thread::spawn(move || {
                documents.with(&SourceTarget::User, &reference, |_| {
                    entered.wait();
                    release.wait();
                    Ok(())
                })
            })
        };
        entered.wait();
        documents.clear();
        assert!(documents.documents.lock().unwrap().is_empty());
        release.wait();
        worker.join().unwrap().unwrap();
    }

    #[test]
    fn disk_write_failure_and_bad_content_cannot_be_resolved_as_a_ready_document() {
        let documents = WorkflowDocuments::default();
        let target = SourceTarget::User;
        let reference = documents.begin(target.clone(), 1, "0".repeat(64)).unwrap();
        documents
            .with(&target, &reference, |document| {
                document.file = std::fs::File::open("/dev/null").unwrap();
                Ok(())
            })
            .unwrap();
        assert!(documents.write(&target, &reference, 0, "eA==").is_err());
        assert!(documents.resolve(&target, &reference).is_err());
        documents.release(&target, &reference).unwrap();
        let reference = documents.begin(target.clone(), 1, "0".repeat(64)).unwrap();
        documents.write(&target, &reference, 0, "eA==").unwrap();
        assert!(documents.finish(&target, &reference).is_err());
        assert!(documents.resolve(&target, &reference).is_err());
        documents.release(&target, &reference).unwrap();
        assert!(documents.documents.lock().unwrap().is_empty());
    }
}

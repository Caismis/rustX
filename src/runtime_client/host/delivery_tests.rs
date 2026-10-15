// Real canonical delivery selection does not hydrate surrounding Tool bodies.
use super::*;
use crate::message::types::ToolCallOccurrenceRef;
use crate::runtime_client::content::CanonicalContentLocator;
use crate::tools::session_files::{SessionFileReadFailure, SessionFileReference};

fn commit_delivery_result(
    store: &SqliteConversationStore,
    id: &str,
    file: SessionFileReference,
    message_id: MessageId,
    status: crate::tools::types::ToolExecutionStatus,
) -> MessageBlock {
    let proposal_id = MessageId::new(format!("{id}-proposal"));
    let call_id = ToolCallId::new(format!("{id}-call"));
    let tool_id = ToolId::new("original-present-tool");
    store
        .append_canonical(&MessageBlock::Assistant(
            crate::message::types::AssistantMessageBlock {
                id: proposal_id.clone(),
                content: vec![AssistantContentBlock::ToolCall(crate::tools::ToolCall {
                    id: call_id.clone(),
                    tool_id: tool_id.clone(),
                    name: "present".into(),
                    arguments: serde_json::json!({"files":[{"path":file.path}]}),
                })],
            },
        ))
        .unwrap();
    let tool = MessageBlock::Tool(crate::message::types::ToolMessageBlock {
        id: message_id,
        occurrence: ToolCallOccurrenceRef::new(proposal_id, ContentBlockIndex::new(0)),
        tool_call_id: call_id,
        tool_id,
        result: crate::tools::types::ToolExecutionResult {
            status,
            content: vec![crate::tools::types::ToolResultContent::Text(
                crate::message::content::TextBlock {
                    text: "huge-original-body🙂水".repeat(80_000),
                },
            )],
            duration_ms: u64::MAX,
            exit_code: None,
            artifacts: vec![],
            deliveries: vec![file],
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    });
    assert!(serde_json::to_vec(&tool).unwrap().len() > 1_048_576);
    store.append_canonical(&tool).unwrap();
    tool
}

fn assert_delivery_access(
    root: &std::path::Path,
    directory: &std::path::Path,
    file: &SessionFileReference,
) {
    let read = crate::tools::session_files::read_authorized(root, file, || Ok(())).unwrap();
    assert_eq!(read, b"original protected bytes");
    let denied = crate::tools::session_files::read_authorized(root, file, || {
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    })
    .unwrap_err();
    assert_eq!(
        crate::tools::session_files::read_failure(&denied),
        SessionFileReadFailure::Unauthorized
    );
    let foreign = directory.join("foreign-root");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("original 水 report.md"), b"foreign bytes").unwrap();
    let wrong_root =
        crate::tools::session_files::read_authorized(&foreign, file, || Ok(())).unwrap_err();
    assert_eq!(
        crate::tools::session_files::read_failure(&wrong_root),
        SessionFileReadFailure::Unavailable
    );
}

fn assert_delivery_head(
    host: &RuntimeClientHost,
    checkpoint: &crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
    file: &SessionFileReference,
    position: u64,
    deferred: bool,
) {
    use base64::Engine as _;
    let locator = CanonicalContentLocator::ByLedgerPosition { position };
    let descriptor = checkpoint
        .snapshot
        .message_descriptors
        .iter()
        .find(|descriptor| descriptor.locator == locator)
        .unwrap();
    let head = descriptor
        .deliveries
        .as_ref()
        .expect("positive successful-C delivery head");
    assert_eq!(
        (head.total, head.items.len(), head.next_offset),
        (1, 1, None)
    );
    let item = &head.items[0];
    assert_eq!(item.index, 0);
    assert_eq!(item.locator, locator);
    assert_eq!(item.metadata_deferred, deferred);
    assert_eq!(item.file.as_ref(), (!deferred).then_some(file));
    let page = host
        .inner
        .checkpoint_page(&checkpoint.read_cut, &head.domain, 0, 8)
        .unwrap();
    assert_eq!(page.read_cut, checkpoint.read_cut);
    assert_eq!(page.total, 1);
    assert_eq!(page.next_offset, None);
    assert_eq!(page.items.len(), 1);
    assert_eq!(
        page.items[0].value,
        crate::runtime_client::checkpoint::structural_value(item)
    );
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = host
            .inner
            .checkpoint_value_read(
                &checkpoint.read_cut,
                &item.metadata_domain,
                0,
                &[],
                offset,
                16_384,
            )
            .unwrap();
        assert_eq!(chunk.cut, checkpoint.read_cut);
        assert_eq!(chunk.domain, item.metadata_domain);
        assert_eq!(chunk.item_offset, 0);
        assert!(chunk.path.is_empty());
        assert_eq!(chunk.offset, offset);
        assert_eq!(chunk.readable_end, item.encoded_bytes);
        let part = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(!part.is_empty() && part.len() <= 16_384);
        bytes.extend(part);
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(bytes.len() as u64, item.encoded_bytes);
    assert_eq!(
        serde_json::from_slice::<SessionFileReference>(&bytes).unwrap(),
        *file
    );
    let original = host
        .inner
        .store
        .observation_delivery_read(
            checkpoint.read_cut.source_cut.as_ref().unwrap(),
            &locator,
            0,
            0,
            65_536,
        )
        .unwrap();
    assert_eq!(bytes, original.bytes);
    assert!(original.next_offset.is_none());
    assert_eq!(host.checkpoint().unwrap(), *checkpoint);
}

fn create_escaped_delivery_file(root: &std::path::Path) -> String {
    use nix::fcntl::{OFlag, openat};
    use nix::sys::stat::{Mode, mkdirat};
    use std::io::Write as _;

    // Each syscall names one native component, preserving the long original
    // path without depending on the platform's full-path syscall limit.
    let component = "\u{1}".repeat(96);
    let mut directory = std::fs::File::open(root).unwrap();
    let mut relative = std::path::PathBuf::new();
    for _ in 0..32 {
        mkdirat(&directory, component.as_str(), Mode::S_IRWXU).unwrap();
        directory = std::fs::File::from(
            openat(
                &directory,
                component.as_str(),
                OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .unwrap(),
        );
        relative.push(&component);
    }
    let mut file = std::fs::File::from(
        openat(
            &directory,
            "report.md",
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )
        .unwrap(),
    );
    file.write_all(b"escaped original protected bytes").unwrap();
    relative.push("report.md");
    relative.into_os_string().into_string().unwrap()
}

#[tokio::test]
async fn t08_t12_t14_t19_cold_host_selects_tiny_delivery_from_giant_original_tool() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("original-root");
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    std::fs::write(
        root.join("original 水 report.md"),
        b"original protected bytes",
    )
    .unwrap();
    let path = directory.path().join("delivery.sqlite");
    let conversation = ConversationId::generate();
    let file = crate::tools::session_files::declare(
        &root,
        &conversation,
        "original 水 report.md",
        Some("original delivery".into()),
    )
    .unwrap();
    let message_id = MessageId::new("original-delivery🙂水".repeat(80_000));
    assert!(message_id.as_str().len() > 1_048_576);
    let original;
    {
        let owner = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        owner.initialize(&[]).unwrap();
        original = commit_delivery_result(
            &owner,
            "original-delivery",
            file.clone(),
            message_id.clone(),
            crate::tools::types::ToolExecutionStatus::Success,
        );
    }
    let store = Arc::new(SqliteConversationStore::open_existing(conversation, &path).unwrap());
    let host = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
    let checkpoint = host.checkpoint().unwrap();
    let selected = store
        .observation_file_delivery(
            &CanonicalContentLocator::ByLedgerPosition { position: 2 },
            0,
        )
        .unwrap();
    assert_eq!(selected.cut, store.observation_read_cut().unwrap());
    assert_eq!((selected.ledger_position, selected.index), (2, 0));
    assert_eq!(selected.file, file);
    let (rows, bytes) = SqliteConversationStore::content_read_work();
    assert!(rows > 0 && rows <= 2 && bytes > 0 && bytes <= 16_384);
    let from_host = host
        .inner
        .session_file_reference(
            &CanonicalContentLocator::ByLedgerPosition { position: 2 },
            0,
        )
        .unwrap();
    assert_eq!(from_host, file);
    let (rows, bytes) = SqliteConversationStore::content_read_work();
    assert!(rows > 0 && rows <= 2 && bytes > 0 && bytes <= 16_384);
    assert_delivery_access(&root, directory.path(), &from_host);
    assert_delivery_head(&host, &checkpoint, &file, 2, false);
    assert!(
        host.inner
            .session_file_reference(
                &CanonicalContentLocator::ByLedgerPosition { position: 2 },
                1
            )
            .is_err()
    );
    assert!(
        host.inner
            .session_file_reference(
                &CanonicalContentLocator::ByLedgerPosition { position: 3 },
                0
            )
            .is_err()
    );
    assert!(
        host.inner
            .session_file_reference(
                &CanonicalContentLocator::ByLedgerPosition { position: 1 },
                0
            )
            .is_err()
    );
    assert_eq!(host.checkpoint().unwrap(), checkpoint);
    assert_eq!(store.load_messages(&[message_id]).unwrap(), [original]);
    assert!(checkpoint.snapshot.attempt.is_none());
    assert!(store.read_events(None, 32).unwrap().events.is_empty());
}

#[tokio::test]
async fn t08_t14_t19_delivery_corruption_and_failed_tool_keep_native_authority() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("report.md"), b"protected bytes").unwrap();
    let conversation = ConversationId::generate();
    let path = directory.path().join("delivery-fault.sqlite");
    let root = directory.path().canonicalize().unwrap();
    let file =
        crate::tools::session_files::declare(&root, &conversation, "report.md", None).unwrap();
    let originals;
    {
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        store.initialize(&[]).unwrap();
        originals = [
            commit_delivery_result(
                &store,
                "success",
                file.clone(),
                MessageId::new("success-result"),
                crate::tools::types::ToolExecutionStatus::Success,
            ),
            commit_delivery_result(
                &store,
                "failed",
                file,
                MessageId::new("failed-result"),
                crate::tools::types::ToolExecutionStatus::Failed {
                    error: "original failed status".into(),
                },
            ),
        ];
    }
    let store = Arc::new(SqliteConversationStore::open_existing(conversation, &path).unwrap());
    let host = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
    let checkpoint = host.checkpoint().unwrap();
    let source = CanonicalContentLocator::ByLedgerPosition { position: 2 };
    assert!(host.inner.session_file_reference(&source, 0).is_ok());
    assert!(matches!(
        host.inner.session_file_reference(
            &CanonicalContentLocator::ByLedgerPosition { position: 4 },
            0
        ),
        Err(RuntimeClientError::ContentSourceUnavailable)
    ));
    let diagnostic = rusqlite::Connection::open(&path).unwrap();
    let changed = diagnostic.execute("DELETE FROM content_segments WHERE source_kind='canonical_file_delivery' AND source_id='ledger:2' AND block_index=0 AND representation='canonical_json_v1'", []).unwrap();
    assert_eq!(changed, 1, "one original selected span");
    assert!(matches!(
        host.inner.session_file_reference(&source, 0),
        Err(RuntimeClientError::ObservationUnavailable { .. })
    ));
    assert_eq!(host.checkpoint().unwrap(), checkpoint);
    assert_eq!(
        store
            .load_messages(&[
                MessageId::new("success-result"),
                MessageId::new("failed-result")
            ])
            .unwrap(),
        originals
    );
    assert!(store.read_events(None, 32).unwrap().events.is_empty());
}

#[tokio::test]
async fn t08_t12_t14_t19_host_reads_escaped_delivery_metadata_above_sixteen_kib() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("escaped-root");
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let relative = create_escaped_delivery_file(&root);
    let conversation = ConversationId::generate();
    let file = crate::tools::session_files::declare(&root, &conversation, &relative, None).unwrap();
    assert!(serde_json::to_vec(&file).unwrap().len() > 16_384);
    let path = directory.path().join("escaped-delivery.sqlite");
    let original;
    {
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        store.initialize(&[]).unwrap();
        original = commit_delivery_result(
            &store,
            "escaped",
            file.clone(),
            MessageId::new("escaped-result"),
            crate::tools::types::ToolExecutionStatus::Success,
        );
    }
    let store = Arc::new(SqliteConversationStore::open_existing(conversation, &path).unwrap());
    let host = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
    let checkpoint = host.checkpoint().unwrap();
    assert_delivery_head(&host, &checkpoint, &file, 2, true);
    let selected = host
        .inner
        .session_file_reference(
            &CanonicalContentLocator::ByLedgerPosition { position: 2 },
            0,
        )
        .unwrap();
    assert_eq!(selected, file);
    let (rows, bytes) = SqliteConversationStore::content_read_work();
    assert!(rows > 0 && rows <= 2 && bytes > 16_384 && bytes <= 65_536);
    assert_eq!(
        crate::tools::session_files::read_authorized(&root, &selected, || Ok(())).unwrap(),
        b"escaped original protected bytes"
    );
    assert_eq!(host.checkpoint().unwrap(), checkpoint);
    assert_eq!(
        store
            .load_messages(&[MessageId::new("escaped-result")])
            .unwrap(),
        [original]
    );
}

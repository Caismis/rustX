//! Differential scalar composition against the original complete eager producer.
use super::super::{RootKind, scalar, take_work};
use super::*;
use crate::context::ContextGeneration;
use crate::durable::{ConversationStore, SqliteConversationStore};
use crate::model::{
    ModelCapabilities, ModelCompat, ModelInvocationConfig, ModelProtocol, ModelRequest,
    RequestIdentity, RequestParams, RequestSnapshot,
};
use crate::runtime::identity::{AttemptId, CapabilityRevision, ConversationId, ToolId, TurnId};
use crate::runtime::read_layout::{EncodedObservationSource, SeekableObservationSource};
use crate::runtime_client::trace::frozen;

fn snapshot(turn: u32, retry: u32, wide: bool) -> RequestSnapshot {
    let properties: serde_json::Map<String, serde_json::Value> = (0..if wide { 128 } else { 48 })
        .map(|i| {
            (
                format!("field_{i:03}"),
                serde_json::json!({"type":"string","default":"水🙂\\\"\n","opaque":u64::MAX}),
            )
        })
        .collect();
    let schema = serde_json::json!({"properties": properties, "@0": {"$key": -0.0}, "long".repeat(90): [1,2,3]});
    let tools = (0..2)
        .map(|i| crate::tools::types::ModelToolDefinition {
            id: ToolId::new(format!("stable-tool-{i}")),
            name: format!("stable_tool_{i}"),
            description: "stable tool 水🙂".into(),
            input_schema: schema.clone(),
        })
        .collect();
    RequestSnapshot::new(
        RequestIdentity {
            attempt_id: AttemptId::new("shared-scalar-attempt"),
            turn: TurnId::new(turn.to_string()),
            retry_number: retry,
        },
        crate::context::resources::LedgerFrontier(0),
        "stable prompt 水🙂\n\"\\".into(),
        Vec::new(),
        crate::runtime::RuntimeResourceRevision::new(1),
        ModelInvocationConfig {
            model: "frozen-model".into(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: u32::MAX,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        },
        u64::MAX,
        None,
        false,
        tools,
        CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    )
}
fn captured_root(connection: &Connection, snapshot: &RequestSnapshot) -> Root {
    let value = scalar::capture(connection, snapshot).unwrap();
    Root {
        kind: RootKind::Request,
        sequence: 1,
        native: None,
        source_hash: None,
        length: value.prefix.length + value.suffix.length,
        scalar: value.suffix,
        prefix: Some(value.prefix),
        children: value.children,
        nodes: value.nodes,
        messages_start: value.messages_start,
        messages_end: value.messages_end,
    }
}
fn legacy_root(connection: &Connection, snapshot: &RequestSnapshot) -> (Root, Vec<Vec<String>>) {
    let value = frozen::request_detail(snapshot, Vec::<u8>::new());
    let source = SeekableObservationSource::new(&value).unwrap();
    let (fragment, start, end) = fragments::scalar(connection, &source).unwrap();
    let mut paths = Vec::new();
    source
        .visit_fields(|path, _, _, _, _, _, key, _| {
            paths.push(path.to_vec());
            if key.is_some() && path.last().is_some_and(|part| part.starts_with('@')) {
                let mut key_path = path.to_vec();
                key_path.push("$key".into());
                paths.push(key_path);
            }
            Ok(())
        })
        .unwrap();
    (
        Root {
            kind: RootKind::Scalar,
            sequence: 1,
            native: None,
            source_hash: None,
            length: fragment.length,
            scalar: fragment,
            prefix: None,
            children: 0,
            nodes: 0,
            messages_start: start,
            messages_end: end,
        },
        paths,
    )
}
fn assert_oracle(connection: &Connection, root: &Root, snapshot: &RequestSnapshot) {
    let oracle = Connection::open_in_memory().unwrap();
    super::super::create_schema(&oracle).unwrap();
    super::super::install_workspace(&oracle).unwrap();
    let (expected, paths) = legacy_root(&oracle, snapshot);
    crate::runtime::read_layout::INDEX_OPENS.with(|work| work.set(0));
    crate::runtime::read_layout::take_field_preview_work();
    assert_eq!(root.length, expected.length);
    for path in &paths {
        let actual = field_in(connection, root, path).unwrap();
        assert_eq!(
            actual,
            field_in(&oracle, &expected, path).unwrap(),
            "{path:?}"
        );
        let a = selected(connection, root, path).unwrap();
        let e = selected(&oracle, &expected, path).unwrap();
        if !path.is_empty() && path != &["messages"] {
            assert_eq!(a.fact.nodes, e.fact.nodes, "node count {path:?}");
        }
        for ordinal in 0..a.fact.children.min(32) {
            assert_eq!(
                child(connection, root, path, ordinal).unwrap(),
                child(&oracle, &expected, path, ordinal).unwrap()
            );
        }
    }
    for offset in (0..root.length).step_by(257) {
        let end = (offset + 257).min(root.length);
        assert_eq!(
            read_exact(connection, root, offset, end).unwrap(),
            read_exact(&oracle, &expected, offset, end).unwrap()
        );
    }
    for boundary in [root.prefix_length(), root.messages_start, root.messages_end] {
        let start = boundary.saturating_sub(15);
        let end = (boundary + 15).min(root.length);
        assert_eq!(
            read_exact(connection, root, start, end).unwrap(),
            read_exact(&oracle, &expected, start, end).unwrap()
        );
    }
    assert_eq!(
        crate::runtime::read_layout::INDEX_OPENS.with(std::cell::Cell::get),
        0
    );
    assert_eq!(
        crate::runtime::read_layout::take_field_preview_work(),
        (0, 0)
    );
    for invalid in [
        vec!["@0".into()],
        vec!["tools".into(), "00".into()],
        vec!["tools".into(), "2".into()],
    ] {
        assert_eq!(
            selected(connection, root, &invalid).is_err(),
            selected(&oracle, &expected, &invalid).is_err()
        );
    }
}
#[test]
fn shared_scalar_preserves_original_fields_aliases_ordinals_previews_and_cross_boundary_bytes() {
    for wide in [false, true] {
        let connection = Connection::open_in_memory().unwrap();
        super::super::create_schema(&connection).unwrap();
        super::super::install_workspace(&connection).unwrap();
        let original = snapshot(1, 0, wide);
        let first = captured_root(&connection, &original);
        assert_eq!(first.length <= 16384, !wide, "both scalar byte classes");
        assert_oracle(&connection, &first, &original);
        let mut next = snapshot(234, 9, wide);
        next.contributions
            .push(crate::context::contribution::ContributionStart {
                message_id: crate::runtime::identity::MessageId::new("dynamic-contribution"),
                producer: crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::RuntimeToolObservation,
                ),
                metadata: crate::message::ContextKind::RuntimeToolObservation,
                presentation: None,
                emissions: Vec::new(),
                opportunities: crate::context::ContributionOpportunities::default(),
                post_tool_batch_anchor: None,
            });
        take_work();
        let second = captured_root(&connection, &next);
        let work = take_work();
        assert_eq!((work.scalar_captures, work.scalar_reuses), (0, 1));
        assert_eq!(second.scalar.bytes.hash, first.scalar.bytes.hash);
        assert_eq!(second.scalar.fields.hash, first.scalar.fields.hash);
        assert_ne!(
            second.prefix.as_ref().unwrap().bytes.hash,
            first.prefix.as_ref().unwrap().bytes.hash
        );
        assert_eq!(
            work.new_fragment_bytes,
            usize::try_from(second.prefix_length()).unwrap()
        );
        assert_oracle(&connection, &second, &next);
        assert_oracle(&connection, &first, &original);
    }
}

#[test]
fn stable_scalar_first_capture_fuses_geometry_and_reuse_keeps_zero_suffix_walks() {
    use crate::runtime::read_layout::take_field_preview_work;
    let snapshot = snapshot(1, 0, true);
    let prefix = SeekableObservationSource::new(&frozen::request_prefix(&snapshot)).unwrap();
    let stable = EncodedObservationSource::new(&frozen::request_stable(&snapshot))
        .unwrap()
        .index()
        .unwrap();
    take_field_preview_work();
    prefix.visit_field_geometry(|_| Ok(())).unwrap();
    let prefix_work = take_field_preview_work();
    stable.visit_field_geometry(|_| Ok(())).unwrap();
    let stable_work = take_field_preview_work();
    let connection = Connection::open_in_memory().unwrap();
    super::super::create_schema(&connection).unwrap();
    super::super::install_workspace(&connection).unwrap();
    take_work();
    scalar::capture(&connection, &snapshot).unwrap();
    assert_eq!(
        take_field_preview_work(),
        (0, prefix_work.1 + stable_work.1)
    );
    assert_eq!(take_work().scalar_captures, 1);
    scalar::capture(&connection, &snapshot).unwrap();
    assert_eq!(take_field_preview_work(), prefix_work);
    assert_eq!(take_work().scalar_reuses, 1);
}
#[test]
fn shared_scalar_changes_config_exactly_and_preserves_order_and_versions() {
    let connection = Connection::open_in_memory().unwrap();
    super::super::create_schema(&connection).unwrap();
    super::super::install_workspace(&connection).unwrap();
    let original = snapshot(1, 0, false);
    let first = captured_root(&connection, &original);
    // Native revision metadata is retained by each snapshot; exact public bytes
    // decide whether its already-indexed scalar fragment can be shared.
    let mut revision = original.clone();
    revision.runtime_resource_revision = crate::runtime::RuntimeResourceRevision::new(2);
    revision.capability_revision = CapabilityRevision::new(2);
    take_work();
    let same = captured_root(&connection, &revision);
    assert_eq!(take_work().scalar_reuses, 1);
    assert_eq!(same.scalar.fields.hash, first.scalar.fields.hash);
    let mut versions = vec![original.clone(); 7];
    versions[0].invocation.model.push_str("-new");
    versions[1].effective_system_prompt.push(' ');
    versions[2].profile = Some(crate::model::catalog::ModelProfileId::new("profile-v2"));
    versions[3]
        .invocation
        .request_params
        .insert("temperature".into(), serde_json::json!(0.5));
    versions[4].tool_definitions.swap(0, 1);
    versions[5].tool_definitions[0].input_schema["revision"] = serde_json::json!(2);
    versions[6].reasoning_enabled = true;
    for next in versions {
        take_work();
        let changed = captured_root(&connection, &next);
        let work = take_work();
        assert_eq!((work.scalar_captures, work.scalar_reuses), (1, 0));
        assert_ne!(changed.scalar.bytes.hash, first.scalar.bytes.hash);
        assert_oracle(&connection, &changed, &next);
    }
    take_work();
    let retained = captured_root(&connection, &original);
    let work = take_work();
    assert_eq!((work.scalar_captures, work.scalar_reuses), (0, 1));
    assert_eq!(retained.scalar.fields.hash, first.scalar.fields.hash);
    assert_oracle(&connection, &retained, &original);
}
fn commit(
    store: &SqliteConversationStore,
    mut snapshot: RequestSnapshot,
) -> (RequestSnapshot, u64) {
    let history = store.projected_history().unwrap();
    snapshot.history_cut = history.cut;
    snapshot.canonical_frontier =
        crate::context::resources::LedgerFrontier(store.load_head().unwrap().revision.get());
    let request = ModelRequest {
        messages: history.messages,
        images: crate::model::images::ResolvedImages::new(),
        invocation: snapshot.invocation.clone(),
        tools: snapshot.tool_definitions.clone(),
        effective_system_prompt: snapshot.effective_system_prompt.clone(),
        continuation: None,
        continuation_source: None,
    };
    snapshot
        .freeze_selected_input(store.conversation_id(), &request, &history.input_manifest)
        .unwrap();
    let sequence = store
        .commit_model_turn_start(&[], &snapshot, chrono::Utc::now())
        .unwrap()
        .started
        .sequence;
    (snapshot, sequence)
}
#[test]
fn shared_scalar_reopens_without_reindexing_and_transaction_rollback_keeps_no_version() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("shared-scalars.sqlite");
    let conversation = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    store.initialize(&[]).unwrap();
    let (first, sequence) = commit(&store, snapshot(1, 0, false));
    let original = {
        let connection = store.lock().unwrap();
        let root = load_root(&connection, &sequence.to_string()).unwrap();
        assert_oracle(&connection, &root, &first);
        root.scalar.fields.hash
    };
    drop(store);
    let reopened = SqliteConversationStore::open(conversation, &path).unwrap();
    take_work();
    let (second, next_sequence) = commit(&reopened, snapshot(2, 0, false));
    let work = take_work();
    assert_eq!((work.scalar_captures, work.scalar_reuses), (0, 1));
    let connection = reopened.lock().unwrap();
    let root = load_root(&connection, &next_sequence.to_string()).unwrap();
    assert_eq!(root.scalar.fields.hash, original);
    assert_oracle(&connection, &root, &second);
    assert_oracle(
        &connection,
        &load_root(&connection, &sequence.to_string()).unwrap(),
        &first,
    );
    let before: u64 = connection
        .query_row("SELECT count(*) FROM request_source_scalars", [], |row| {
            row.get(0)
        })
        .unwrap();
    connection
        .execute_batch("SAVEPOINT scalar_rollback")
        .unwrap();
    let mut changed = second;
    changed.effective_system_prompt.push_str(" changed");
    captured_root(&connection, &changed);
    connection
        .execute_batch("ROLLBACK TO scalar_rollback; RELEASE scalar_rollback")
        .unwrap();
    let after: u64 = connection
        .query_row("SELECT count(*) FROM request_source_scalars", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(before, after);
    take_work();
    captured_root(&connection, &changed);
    assert_eq!(take_work().scalar_captures, 1);
}
#[test]
fn shared_scalar_corruption_fails_at_registry_reuse_and_authenticated_historical_reads() {
    let connection = Connection::open_in_memory().unwrap();
    super::super::create_schema(&connection).unwrap();
    super::super::install_workspace(&connection).unwrap();
    let original = snapshot(1, 0, false);
    let first = captured_root(&connection, &original);
    connection
        .execute_batch(
            "SAVEPOINT scalar_corruption; UPDATE request_source_scalars SET checksum=zeroblob(32)",
        )
        .unwrap();
    assert!(scalar::capture(&connection, &original).is_err());
    // Roots retain authenticated references; a damaged reuse directory is not
    // consulted by historical readers and cannot silently alter old fields.
    assert_oracle(&connection, &first, &original);
    connection
        .execute_batch(
            "ROLLBACK TO scalar_corruption; RELEASE scalar_corruption; SAVEPOINT page_corruption",
        )
        .unwrap();
    connection
        .execute(
            "UPDATE request_source_pages SET bytes=?1 WHERE hash=?2",
            super::super::params![b"corrupt".as_slice(), first.scalar.fields.hash.as_slice()],
        )
        .unwrap();
    assert!(field_in(&connection, &first, &["tools".into()]).is_err());
    connection
        .execute_batch("ROLLBACK TO page_corruption; RELEASE page_corruption")
        .unwrap();
    connection
        .execute(
            "DELETE FROM request_source_pages WHERE hash=?1",
            [first.scalar.bytes.hash.as_slice()],
        )
        .unwrap();
    assert!(
        read_exact(
            &connection,
            &first,
            first.prefix_length(),
            first.prefix_length() + 1
        )
        .is_err()
    );
}
#[test]
fn shared_scalar_retains_whole_root_byte_and_depth_admission_boundaries() {
    for length in [16383_usize, 16384, 16385] {
        let mut value = snapshot(1, 0, false);
        value.effective_system_prompt.clear();
        let source =
            EncodedObservationSource::new(&frozen::request_detail(&value, Vec::<u8>::new()))
                .unwrap();
        value.effective_system_prompt =
            "p".repeat(length - usize::try_from(source.length()).unwrap());
        let connection = Connection::open_in_memory().unwrap();
        super::super::create_schema(&connection).unwrap();
        super::super::install_workspace(&connection).unwrap();
        let root = captured_root(&connection, &value);
        assert_eq!(root.length, length as u64);
        assert_oracle(&connection, &root, &value);
    }
    for depth in [121, 122, 123, 124, 125, 126, 127, 128, 129] {
        let mut value = snapshot(1, 0, false);
        let mut nested = serde_json::json!(1);
        for _ in 0..depth {
            nested = serde_json::json!([nested]);
        }
        value.tool_definitions[0].input_schema = nested;
        let connection = Connection::open_in_memory().unwrap();
        super::super::create_schema(&connection).unwrap();
        super::super::install_workspace(&connection).unwrap();
        let actual = scalar::capture(&connection, &value);
        let oracle = Connection::open_in_memory().unwrap();
        super::super::create_schema(&oracle).unwrap();
        super::super::install_workspace(&oracle).unwrap();
        let expected =
            SeekableObservationSource::new(&frozen::request_detail(&value, Vec::<u8>::new()))
                .map_err(storage)
                .and_then(|source| fragments::scalar(&oracle, &source));
        assert_eq!(
            actual.is_ok(),
            expected.is_ok(),
            "relative root depth {depth}"
        );
    }
}

fn assert_native_oracle(
    store: &SqliteConversationStore,
    snapshot: &RequestSnapshot,
    sequence: u64,
) {
    let input = store
        .reconstruct_model_request(&snapshot.request_id)
        .unwrap();
    let connection = store.lock().unwrap();
    let expected = super::super::super::legacy_request_source_bytes(
        &connection,
        sequence,
        store.conversation_id(),
        &input.messages,
        snapshot,
    )
    .unwrap();
    let fields = super::super::super::legacy_field_headers_oracle(
        &connection,
        sequence,
        store.conversation_id(),
        &input.messages,
        snapshot,
    )
    .unwrap();
    let root = load_root(&connection, &sequence.to_string()).unwrap();
    assert!(root.native.is_some());
    crate::runtime::read_layout::INDEX_OPENS.with(|work| work.set(0));
    crate::runtime::read_layout::take_field_preview_work();
    for (path, start, end, children, header) in &fields {
        let actual = field_in(&connection, &root, path).unwrap();
        assert_eq!(
            (actual.0, actual.1, actual.2, actual.3),
            (*start, *end, *children, header.clone()),
            "native {path:?}"
        );
    }
    for offset in (0..root.length).step_by(257) {
        let end = (offset + 257).min(root.length);
        assert_eq!(
            read_exact(&connection, &root, offset, end).unwrap(),
            expected[usize::try_from(offset).unwrap()..usize::try_from(end).unwrap()]
        );
    }
    let end_messages = root.messages_start + root.length - (root.base_length().unwrap() - 2);
    for boundary in [root.prefix_length(), root.messages_start, end_messages] {
        let start = boundary.saturating_sub(15);
        let end = (boundary + 15).min(root.length);
        assert_eq!(
            read_exact(&connection, &root, start, end).unwrap(),
            expected[usize::try_from(start).unwrap()..usize::try_from(end).unwrap()]
        );
    }
    let top = page(&connection, &sequence.to_string(), &[], 0, 32).unwrap();
    assert_eq!(top.total, 24);
    assert_eq!(
        top.fields
            .iter()
            .map(|field| &field.path)
            .collect::<Vec<_>>(),
        fields
            .iter()
            .filter(|field| field.0.len() == 1)
            .map(|field| &field.0)
            .collect::<Vec<_>>()
    );
    let properties = page(
        &connection,
        &sequence.to_string(),
        &[
            "tools".into(),
            "0".into(),
            "input_schema".into(),
            "value".into(),
            "properties".into(),
        ],
        0,
        32,
    )
    .unwrap();
    assert_eq!(properties.fields.len(), 32);
    assert_eq!(
        crate::runtime::read_layout::INDEX_OPENS.with(std::cell::Cell::get),
        0
    );
    assert_eq!(
        crate::runtime::read_layout::take_field_preview_work(),
        (0, 0)
    );
}
#[test]
fn shared_scalar_native_messages_and_post_message_fields_keep_geometry_after_reopen() {
    use crate::message::{
        InboundKind, MessageBlock, TextBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("shared-native.sqlite");
    let conversation = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    let history: Vec<_> = (0..3)
        .map(|i| {
            MessageBlock::User(UserMessageBlock {
                id: crate::runtime::identity::MessageId::new(format!("shared-message-{i}")),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: format!("original {i} 水🙂\n\"\\"),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            })
        })
        .collect();
    store.initialize(&history).unwrap();
    let (first, sequence) = commit(&store, snapshot(1, 0, false));
    take_work();
    let (retry, next_sequence) = commit(&store, snapshot(234, 9, false));
    assert_eq!(take_work().scalar_reuses, 1);
    assert_native_oracle(&store, &first, sequence);
    assert_native_oracle(&store, &retry, next_sequence);
    drop(store);
    let reopened = SqliteConversationStore::open(conversation, &path).unwrap();
    assert_native_oracle(&reopened, &first, sequence);
    assert_native_oracle(&reopened, &retry, next_sequence);
}
#[test]
fn shared_scalar_rejects_rehashed_wrong_identity_and_invalid_geometry_before_reuse() {
    use sha2::Digest as _;
    let connection = Connection::open_in_memory().unwrap();
    super::super::create_schema(&connection).unwrap();
    super::super::install_workspace(&connection).unwrap();
    let value = snapshot(1, 0, false);
    captured_root(&connection, &value);
    let original: Vec<u8> = connection
        .query_row("SELECT metadata FROM request_source_scalars", [], |row| {
            row.get(0)
        })
        .unwrap();
    for field in [
        "identity",
        "children",
        "start",
        "messages_end",
        "messages_start",
    ] {
        let bytes = scalar::corrupted_record(&original, field);
        let mut digest = sha2::Sha256::new();
        digest.update(b"rustx-composed-stable-scalar-v1\0");
        digest.update(&bytes);
        let checksum: [u8; 32] = digest.finalize().into();
        connection
            .execute(
                "UPDATE request_source_scalars SET metadata=?1,checksum=?2",
                super::super::params![bytes, checksum.as_slice()],
            )
            .unwrap();
        assert!(
            scalar::capture(&connection, &value).is_err(),
            "must reject changed {field}"
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One real native request, corruption/restore and retained-cut scenario.
fn geometry_only_headers_authenticate_raw_chunks_without_changing_native_history_or_old_cuts() {
    use crate::message::{
        InboundKind, MessageBlock, TextBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::generate(),
        &directory.path().join("geometry-only.sqlite"),
    )
    .unwrap();
    store
        .initialize(&[MessageBlock::User(UserMessageBlock {
            id: crate::runtime::identity::MessageId::new("original-input"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "native original 水🙂".into(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })])
        .unwrap();
    let (first, sequence) = commit(&store, snapshot(1, 0, false));
    let (second, later) = commit(&store, snapshot(2, 0, false));
    let canonical = store.load_canonical().unwrap();
    let journal = serde_json::to_vec(&store.read_events(None, 64).unwrap().events).unwrap();
    let cut = store.observation_read_cut().unwrap();
    let request = store.reconstruct_model_request(&first.request_id).unwrap();
    let connection = store.lock().unwrap();
    let oracle = super::super::super::legacy_field_headers_oracle(
        &connection,
        sequence,
        store.conversation_id(),
        &request.messages,
        &first,
    )
    .unwrap();
    let root = load_root(&connection, &sequence.to_string()).unwrap();
    let parent = ["tools", "0", "input_schema", "value", "properties"].map(str::to_owned);
    let mut path = parent.to_vec();
    path.push("field_000".into());
    let key = oracle
        .iter()
        .find(|(path, ..)| {
            path.first().is_some_and(|p| p == "tools") && path.last().is_some_and(|p| p == "$key")
        })
        .unwrap()
        .0
        .clone();
    let expected =
        super::super::super::field_header_at(&connection, sequence, sequence, &path).unwrap();
    let old_page =
        super::super::super::page_at(&connection, sequence, sequence, &parent, 0, 32).unwrap();
    assert_eq!(old_page.fields.len(), 32);
    assert_eq!(
        serde_json::to_vec(
            &super::super::super::field_header_at(&connection, sequence, later, &path).unwrap()
        )
        .unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    assert!(
        super::super::super::field_header_at(&connection, later, sequence, &path).is_err(),
        "a future request remains unavailable at the historical cut"
    );
    assert_eq!(field_in(&connection, &root, &key).unwrap().3, b"\"\"");
    let original_bytes =
        super::super::index::load_blob(&connection, root.scalar.bytes.hash).unwrap();
    connection
        .execute(
            "UPDATE request_source_pages SET bytes=x'7b7d' WHERE hash=?1",
            [root.scalar.bytes.hash.as_slice()],
        )
        .unwrap();
    assert!(
        connection.is_autocommit(),
        "the corrupt byte page is durably committed"
    );
    // The F and C metadata remain intact: ordinary headers now authenticate
    // their one original byte authority, never a retained preview mirror.
    assert!(
        fragments::fact(&connection, &root.scalar, &path)
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        super::super::super::field_header_at(&connection, sequence, sequence, &path),
        Err(ConversationStoreError::Storage(_))
    ));
    assert!(matches!(
        super::super::super::page_at(&connection, sequence, sequence, &parent, 0, 32),
        Err(ConversationStoreError::Storage(_))
    ));
    assert_eq!(field_in(&connection, &root, &key).unwrap().3, b"\"\"");
    drop(connection);
    assert_eq!(store.load_canonical().unwrap(), canonical);
    assert_eq!(
        serde_json::to_vec(&store.read_events(None, 64).unwrap().events).unwrap(),
        journal
    );
    assert_eq!(store.observation_read_cut().unwrap(), cut);
    assert_eq!(
        store.load_request_snapshot(&first.request_id).unwrap(),
        first
    );
    assert_eq!(
        store.load_request_snapshot(&second.request_id).unwrap(),
        second
    );
    assert_eq!(
        store
            .reconstruct_model_request(&first.request_id)
            .unwrap()
            .messages,
        request.messages
    );
    let connection = store.lock().unwrap();
    connection
        .execute(
            "UPDATE request_source_pages SET bytes=?2 WHERE hash=?1",
            super::super::params![root.scalar.bytes.hash.as_slice(), original_bytes],
        )
        .unwrap();
    assert_eq!(
        serde_json::to_vec(
            &super::super::super::page_at(&connection, sequence, sequence, &parent, 0, 32).unwrap()
        )
        .unwrap(),
        serde_json::to_vec(&old_page).unwrap()
    );
    connection
        .execute_batch("SAVEPOINT corrupt_geometry")
        .unwrap();
    connection
        .execute(
            "UPDATE request_source_pages SET bytes=x'7b7d' WHERE hash=?1",
            [root.scalar.fields.hash.as_slice()],
        )
        .unwrap();
    assert!(
        matches!(
            field_in(&connection, &root, &key),
            Err(ConversationStoreError::Storage(_))
        ),
        "even the empty key preview must authenticate its field"
    );
    connection
        .execute_batch("ROLLBACK TO corrupt_geometry; RELEASE corrupt_geometry")
        .unwrap();
}

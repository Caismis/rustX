// Cold reading retains one native cut and never admits a runtime.
fn cold_statistics_compaction(
    store: &crate::durable::SqliteConversationStore,
    before: &str,
    id: &str,
    model: &str,
) {
    use crate::message::types::{CompactionSummaryMetadata, InboundKind, UserSource};
    use crate::runtime::identity::MessageId;
    let MessageBlock::User(mut summary) = user(id) else {
        unreachable!()
    };
    summary.source = UserSource::Runtime;
    summary.kind = InboundKind::CompactionSummary(
        CompactionSummaryMetadata::new(vec!["native.rs".into()], Vec::new()).unwrap(),
    );
    store
        .commit_compaction(crate::durable::CompactionCommitInput {
            summary,
            span: crate::conversation::SurfaceSpan::new(
                MessageId::new(before),
                MessageId::new(before),
            ),
            expected_revision: store.load_head().unwrap().revision,
            tokens_before: crate::runtime::TokenMeasurement {
                input_tokens: 400,
                source: crate::runtime::TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 200,
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
            occupancy: Some(crate::context::occupancy::ContextOccupancy {
                estimated: true,
                input_tokens: 200,
                context_window_tokens: 8192,
                model: model.into(),
                breakdown: crate::context::occupancy::ContextBreakdown {
                    system_tokens: 7,
                    tool_tokens: 11,
                    message_tokens: 182,
                },
            }),
        })
        .unwrap();
}

#[tokio::test]
async fn t08_t12_t14_t19_cold_statistics_model_reads_keep_the_original_cut_without_runtime_admission()
 {
    let root = tempfile::tempdir().unwrap();
    let controller = SessionController::open(root.path()).unwrap();
    let session = controller
        .create_session(settings(root.path()))
        .await
        .unwrap()
        .session;
    let access = controller
        .acquire_session(&session.id, Some(&session.active_node))
        .await
        .unwrap();
    let store = crate::durable::SqliteConversationStore::open(
        access.node.conversation_id.clone(),
        &access.database_path,
    )
    .unwrap()
    .with_lifecycle(access.allocation);
    store.append_canonical(&user("cold-stats-input")).unwrap();
    let original = "\u{feff}original🙂水".repeat(100_000);
    cold_statistics_compaction(&store, "cold-stats-input", "cold-stats-summary", &original);
    std::fs::write(root.path().join("rustx.toml"), "malformed = [").unwrap();
    let reading = controller
        .read_statistics(&session.id, Some(&session.active_node))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reading.conversation_id, session.active_conversation_id);
    assert_eq!(reading.read_cut.conversation_id, reading.conversation_id);
    assert!(reading.read_cut.activation_id.is_none());
    assert_eq!(
        reading.read_cut.through,
        store.presentation_frontier().unwrap()
    );
    let occupancy = reading.occupancy.as_ref().unwrap();
    assert_eq!(occupancy.input_tokens, 200);
    assert!(occupancy.model.is_none());
    assert!(occupancy.model_deferred);
    let domain = occupancy.model_source.as_ref().unwrap();
    cold_statistics_compaction(
        &store,
        "cold-stats-summary",
        "cold-stats-new-summary",
        "replacement model",
    );
    assert_eq!(
        cold_statistics_read_model(&controller, &session, &reading.read_cut, domain).await,
        original
    );
    assert_eq!(
        controller
            .read_statistics(&session.id, None)
            .await
            .unwrap()
            .unwrap()
            .occupancy
            .unwrap()
            .model
            .as_deref(),
        Some("replacement model")
    );
    assert!(controller.runtime_owner.get().is_none());
    let fault = rusqlite::Connection::open(&access.database_path).unwrap();
    fault
        .execute(
            "UPDATE rustx_store SET observation_statistics=NULL WHERE id=1",
            [],
        )
        .unwrap();
    let missing = controller
        .read_statistics(&session.id, Some(&session.active_node))
        .await
        .unwrap();
    assert!(matches!(
        missing,
        Err(crate::runtime_client::RuntimeClientError::ObservationUnavailable { .. })
    ));
    assert!(controller.runtime_owner.get().is_none());
}

async fn cold_statistics_read_model(
    controller: &SessionController,
    session: &SessionSnapshot,
    cut: &crate::runtime_client::checkpoint::CheckpointReadCut,
    domain: &str,
) -> String {
    use base64::Engine;
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let source_cut = cut.clone();
        let selected = domain.to_owned();
        let (_, chunk) = controller
            .read_trace_observation(&session.id, Some(&session.active_node), move |store| {
                crate::runtime_client::trace_observation::history_value_read(
                    store,
                    &source_cut,
                    &selected,
                    0,
                    &[],
                    (offset, 65_536),
                )
            })
            .await
            .unwrap();
        let chunk = chunk.unwrap();
        assert_eq!(&chunk.cut, cut);
        let raw = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(raw.len() <= 65_536);
        assert!(!raw.is_empty());
        bytes.extend(raw);
        let Some(next) = chunk.next_offset else {
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    serde_json::from_slice::<String>(&bytes).unwrap()
}

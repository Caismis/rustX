//! Explicit end-to-end allocation measurement over sealed media, SQLite,
//! native BTW ownership/retries and the real HTTP adapter.
use crate::scripted_suites::{common, support};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit native maximum-media allocation measurement"]
async fn measure_btw_maximum_shared_media_native_retries_and_cancel_join() {
    use crate::message::{
        content::ImageReference,
        types::{InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource},
    };
    use crate::model::{
        ModelProtocol,
        catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef},
        invocation::ModelBindingRegistry,
        session::{SessionModelConfig, SessionModelState},
    };
    use crate::runtime::{
        auxiliary::{
            delivery::{ConsumerFailure, ConsumerResult},
            submission::{BtwSubmission, CorrelationId},
        },
        identity::{ConversationId, MessageId},
    };
    use std::sync::{Arc, Mutex};
    fn png_bytes(padding: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2048, 2048);
            encoder.set_color(png::ColorType::Rgba);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&vec![0; 16 * 1024 * 1024]).unwrap();
            if padding > 0 {
                writer
                    .write_chunk(png::chunk::ChunkType(*b"ruSt"), &vec![0; padding])
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        bytes
    }
    fn rss() -> Vec<String> {
        std::fs::read_to_string("/proc/self/status")
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
            .map(str::to_owned)
            .collect()
    }
    tokio::time::timeout(std::time::Duration::from_secs(120), Box::pin(async {
    let gates: Vec<_> = (0..9).map(|_| common::HeaderGate::new()).collect();
    let counts = Arc::new(Mutex::new([0usize; 3]));
    let server_gates = gates.clone();
    let seen = counts.clone();
    let server = common::FixtureServer::start_with_body(move |_, _, body| {
        let id = (0..3)
            .find(|id| body.contains(&format!("native-media-question-{id}")))
            .unwrap();
        let ordinal = {
            let mut counts = seen.lock().unwrap();
            let ordinal = counts[id];
            counts[id] += 1;
            ordinal
        };
        let response = if ordinal < 2 {
            common::FixtureReply::json_error(
                503,
                "Retry",
                &serde_json::json!({"error":{"message":"retry after physical exit"}}),
            )
        } else {
            common::sse_fixture("openai_chat", "plain_text.sse")
        };
        response.with_header_gate(server_gates[id * 3 + ordinal].clone())
    })
    .await;
    let model =
        support::model::FixtureModel::text("media/test", ModelProtocol::OpenAiChatCompletions)
            .claiming_input("image")
            .with_context_window(1_000_000);
    let mut document = support::model::fixture_catalog_document(&[model]);
    document.providers.get_mut("media").unwrap().base_url = server.url("/v1");
    let catalog = ModelCatalog::from_document(document).unwrap();
    let registry = ModelBindingRegistry::new(
        catalog
            .resolve(&MapCredentialEnvironment::default())
            .unwrap(),
    )
    .unwrap();
    let model = SessionModelState::new(
        registry,
        SessionModelConfig::of(ModelRef::parse("media/test").unwrap()),
    )
    .unwrap();
    let fixture = support::runtime_client_fixture::RuntimeClientFixture::builder(
        ConversationId::generate().as_str(),
    )
    .session_model(model)
    .build()
    .await;
    let minimal = png_bytes(0);
    let image = png_bytes(256 * 1024 - minimal.len() - 12);
    assert_eq!(image.len(), 256 * 1024);
    let artifacts = fixture.runtime.tool_runtime().artifacts();
    let mut content = Vec::new();
    for _ in 0..16 {
        content.push(UserContentBlock::Image(ImageReference {
            artifact_id: artifacts.put_bounded(&image).unwrap(),
            alt: None,
        }));
    }
    fixture
        .runtime
        .tool_runtime()
        .durable_store()
        .append_canonical(&MessageBlock::User(UserMessageBlock {
            id: MessageId::new("maximum-shared-images"),
            content,
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }))
        .unwrap();
    let initial_rss = rss();
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let attachment = Arc::new(attachment);
    let mut submissions = Vec::new();
    for id in 0..3 {
        let attachment = attachment.clone();
        submissions.push(tokio::spawn(async move {
            attachment
                .btw(BtwSubmission {
                    correlation: CorrelationId::new(format!("native-media-{id}")).unwrap(),
                    question: format!("native-media-question-{id}"),
                })
                .await
                .unwrap()
        }));
    }
    let mut receipts = Vec::new();
    for submission in submissions {
        receipts.push(submission.await.unwrap());
    }
    for id in 0..3 {
        gates[id * 3].wait_entered().await;
    }
    let first_rss = rss();
    assert!(attachment.auxiliary_cancel(&receipts[2].call_id).unwrap());
    let cancelled = loop {
        if let Some(copy) = attachment.auxiliary_result(&receipts[2].call_id).unwrap() {
            break copy.result().unwrap().clone();
        }
        tokio::task::yield_now().await;
    };
    assert_eq!(
        cancelled.value,
        ConsumerResult::Rejected {
            reason: ConsumerFailure::Cancelled
        }
    );
    gates[6].release();
    // Mutate the live sealed files after capture. Retries must use the original
    // verified bytes; an accidental resample would fail integrity or change HTTP.
    for entry in std::fs::read_dir(artifacts.root()).unwrap().flatten() {
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "bin")
        {
            std::fs::write(entry.path(), vec![0; image.len()]).unwrap();
        }
    }
    let mut retry_rss = Vec::new();
    for ordinal in 0..3 {
        if ordinal > 0 {
            for id in 0..2 {
                gates[id * 3 + ordinal].wait_entered().await;
            }
        }
        retry_rss.push(rss());
        for id in 0..2 {
            gates[id * 3 + ordinal].release();
        }
    }
    for receipt in &receipts[..2] {
        loop {
            if let Some(copy) = attachment.auxiliary_result(&receipt.call_id).unwrap() {
                assert!(matches!(
                    copy.result().unwrap().value,
                    ConsumerResult::Btw { .. }
                ));
                break;
            }
            tokio::task::yield_now().await;
        }
    }
    assert_eq!(*counts.lock().unwrap(), [3, 3, 1]);
    let mut bodies = std::collections::BTreeMap::new();
    for index in 0..7 {
        let body = server.request_body(index);
        let id = (0..3)
            .find(|id| body.contains(&format!("native-media-question-{id}")))
            .unwrap();
        if let Some(previous) = bodies.insert(id, body.clone()) {
            assert_eq!(previous, body, "frozen HTTP input across actual retries");
        }
    }
    assert!(
        attachment
            .btw(BtwSubmission {
                correlation: CorrelationId::new("tampered-new-capture".into()).unwrap(),
                question: "a new question must verify the changed Artifact".into(),
            })
            .await
            .is_err()
    );
    assert_eq!(server.attempt_count(), 7);
    let wire_bytes = bodies[&0].len();
    fixture.host.shutdown().await.unwrap();
    println!(
        "native_btw sealed_image_bytes_per_call=4194304 concurrent_calls=3 requests=7 translated_http_bytes={wire_bytes} initial_rss={initial_rss:?} held_first_rss={first_rss:?} held_retry_rss={retry_rss:?} after_join_rss={:?}; RSS includes fixture-captured HTTP bodies",
        rss()
    );
    })).await.expect("native media experiment liveness guard");
}

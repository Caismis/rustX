//! Durable acceptance precedes consumer selection; only the eventual frozen
//! invocation can enforce model capabilities. Real adapters and HTTP fixture.
use super::*;
use crate::message::content::{FileReference, ImageReference};
use crate::model::catalog::ModelRef;
use crate::model::session::SessionModelConfig;
use crate::runtime::conversation_runtime::Gate;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_non_text_inbound_crosses_model_change_before_attempt_admission() {
    bounded(async {
        for modality in ["Image", "File"] {
            let f = Fixture::new().await;
            let managed = f.load(0).await.unwrap().unwrap();
            let native = managed.inspect_runtime().unwrap();
            let client = managed.client();
            let model_a = ModelRef::parse("local/a").unwrap();
            let model_b = ModelRef::parse("local/b").unwrap();
            assert_eq!(client.snapshot().unwrap().0.model.unwrap().configured.model, model_a);

            let gate = Arc::new(Gate::default());
            native.install_admission_gate(gate.clone());
            let release = gate.arm_scoped();
            let artifact_id = native.tool_runtime().artifacts().put_bounded(b"stored bytes").unwrap();
            let content = vec![if modality == "Image" {
                UserContentBlock::Image(ImageReference { artifact_id, alt: None })
            } else {
                UserContentBlock::File(FileReference { artifact_id, name: None, mime_type: None, description: None })
            }];
            client.submit_inbound(content.clone()).expect("model-agnostic durable acceptance");
            tokio::task::spawn_blocking({
                let gate = gate.clone();
                move || gate.wait_entered()
            }).await.unwrap();

            // The admission worker is parked before its coordinator lock. The
            // accepted item is durable, but there is no consuming Attempt yet.
            assert!(!native.has_current_attempt());
            let pending = native.tool_runtime().durable_store().select_pending_batch().unwrap().unwrap();
            assert_eq!(pending.items.len(), 1);
            assert_eq!(pending.items[0].message.content, content);
            let before = client.snapshot().unwrap().0;
            assert_eq!(before.inbound.pending[0].message.content, content);
            assert_eq!(before.model.unwrap().configured.model, model_a);
            assert!(events(&native).iter().all(|event| !matches!(event,
                RuntimeEvent::AttemptStarted { .. } | RuntimeEvent::ModelRequestFailed { .. })));
            assert_eq!(f.provider.attempt_count(), 0);

            native.model_set(SessionModelConfig::of(model_b.clone())).unwrap();
            assert!(!native.has_current_attempt());
            let settled = native.settlement_signal().notified();
            tokio::pin!(settled);
            settled.as_mut().enable();
            drop(release);
            settled.await;

            let snapshot = client.snapshot().unwrap().0;
            assert_eq!(snapshot.attempt.unwrap().model.unwrap().primary.model, model_b);
            assert!(snapshot.inbound.pending.is_empty());
            let requests = native.request_history().page(None, 2).unwrap().snapshots;
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].invocation.model, "b", "the actual frozen request uses B");
            let failures: Vec<_> = events(&native).into_iter().filter_map(|event| match event {
                RuntimeEvent::ModelRequestFailed { error, .. } => Some(error),
                _ => None,
            }).collect();
            assert_eq!(failures.len(), 1, "only the actual model request refuses the content");
            assert_eq!(failures[0].kind, crate::model::ModelErrorKind::Unsupported);
            assert_eq!(failures[0].message, format!(
                "the effective model capabilities do not support {modality} input; the request is rejected before any provider request"));
            assert_eq!(f.provider.attempt_count(), 0, "the real adapter rejects before HTTP I/O");
            assert!(f.provider.request_bodies().is_empty());
            f.close().await;
        }
    }).await;
}

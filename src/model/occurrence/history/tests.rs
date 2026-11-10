use super::*;
use crate::message::types::ContentBlockIndex;
use crate::model::projected::CanonicalMessageRef;
use crate::runtime::identity::{ConversationId, MessageId};

fn origin(index: usize) -> ReplayOrigin {
    ReplayOrigin {
        binding: format!("origin-{index}"),
        protocol: ModelProtocol::OpenAiResponses,
        model: "fixture".into(),
    }
}
fn association(index: u32, origins: Arc<Vec<[u8; 32]>>) -> HistoricalToolAssociation {
    HistoricalToolAssociation {
        call_message: 0,
        call_block: index,
        result_message: index + 1,
        source: CanonicalCallSource {
            assistant: CanonicalMessageRef {
                conversation_id: ConversationId::generate(),
                message_id: MessageId::new("assistant"),
                content_digest: "digest".into(),
            },
            block: ContentBlockIndex::new(index),
        },
        wire_id: Some(ProviderWireCorrelationId(format!("call-{index}"))),
        native_origins: origins,
    }
}
fn policy() -> WirePolicy {
    WirePolicy {
        protocol: ModelProtocol::OpenAiResponses,
        origin: Some(origin(1023)),
        stored_boundary: None,
    }
}

#[test]
fn native_origin_fold_is_origins_plus_calls_and_warm_suffix_only() {
    for calls in [8, 64, 512, 1024] {
        let origins = Arc::new(
            (0..1024)
                .map(|index| origin_key(&origin(index)))
                .collect::<Vec<_>>(),
        );
        let mut history = ToolAssociationHistory::default();
        NATIVE_WORK.with(|work| work.set((0, 0)));
        for index in 0..calls {
            history.push_back(association(index, origins.clone()));
        }
        assert_eq!(
            NATIVE_WORK.with(std::cell::Cell::get),
            (0, 0),
            "append has no unused per-origin summaries"
        );
        history.validate_wire_policy(&policy()).unwrap();
        assert_eq!(
            NATIVE_WORK.with(std::cell::Cell::get),
            (1024, calls as usize)
        );
        let old = history.clone();
        NATIVE_WORK.with(|work| work.set((0, 0)));
        history.push_back(association(calls, origins.clone()));
        history.validate_wire_policy(&policy()).unwrap();
        assert_eq!(NATIVE_WORK.with(std::cell::Cell::get), (0, 1));
        history.push_back(association(calls, origins));
        assert!(history.validate_wire_policy(&policy()).is_err());
        old.validate_wire_policy(&policy()).unwrap();
        println!(
            "native calls={calls} cold_origin_items=1024 cold_call_items={calls} warm_origin_items=0 warm_call_items=1"
        );
    }
}

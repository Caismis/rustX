//! Fragment envelopes retain provenance even after lossy accepted summaries.
use rustx::context::summarizer::SummaryGeneration;
use rustx::message::content::TextBlock;
use rustx::message::types::{
    InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::ModelInputMessage;
use rustx::model::projected::{
    DerivedModelMessage, ProjectedMessage, ProjectedUserKind, SemanticTransformation,
};
use rustx::runtime::identity::{AgentId, CertifiedExtensionIdentity, MessageId};

fn canonical(source: UserSource) -> ModelInputMessage {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new("source"),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "payload ".repeat(500),
        })],
        source,
        kind: InboundKind::Message,
        timestamp: None,
    })
    .into()
}

fn extension_context() -> ModelInputMessage {
    let mut message = canonical(UserSource::Extension {
        contributor: CertifiedExtensionIdentity::new("extension.context").unwrap(),
    });
    let ModelInputMessage::Canonical(MessageBlock::User(user)) = &mut message else {
        unreachable!()
    };
    user.kind = InboundKind::Context(rustx::message::types::ContextKind::ExtensionEnvironment);
    message
}

fn earlier_summary() -> ModelInputMessage {
    ModelInputMessage::Derived(DerivedModelMessage {
        message: ProjectedMessage::User {
            content: vec![UserContentBlock::Text(TextBlock {
                text: "payload ".repeat(500),
            })],
            source: UserSource::Runtime,
            kind: ProjectedUserKind::CompactionContext,
            timestamp: None,
        },
        sources: vec![],
        checkpoint_sources: vec![],
        transformations: vec![SemanticTransformation::Summary],
    })
}

#[test]
fn every_fragment_identifies_its_source_after_a_lossy_summary() {
    let long_extension = "x".repeat(CertifiedExtensionIdentity::MAX_BYTES);
    let cases = [
        (
            canonical(UserSource::Extension {
                contributor: CertifiedExtensionIdentity::new("example.extension").unwrap(),
            }),
            "Extension",
            "example.extension",
        ),
        (
            canonical(UserSource::Extension {
                contributor: CertifiedExtensionIdentity::new(long_extension.clone()).unwrap(),
            }),
            "Extension",
            long_extension.as_str(),
        ),
        (canonical(UserSource::Runtime), "Runtime", "Message"),
        (
            canonical(UserSource::Agent {
                agent_id: AgentId::new("agent-123"),
            }),
            "Agent",
            "agent-123",
        ),
        (
            canonical(UserSource::ExternalSystem),
            "ExternalSystem",
            "Message",
        ),
        (
            extension_context(),
            "extension.context",
            "ExtensionEnvironment",
        ),
        (earlier_summary(), "Earlier summary", "CompactionContext"),
    ];
    for (source, source_label, kind_label) in cases {
        let mut plan = crate::context::chunks::ChunkPlan::new(&vec![source].into(), 1).unwrap();
        let mut previous = None;
        let mut count = 0;
        while !plan.is_complete() {
            let request = plan
                .next(previous.as_ref(), None, None, 1500, true, |request| {
                    Ok(request
                        .model_input()
                        .messages
                        .iter()
                        .map(|message| {
                            let rustx::model::projected::ModelMessageView::User(content) =
                                message.view()
                            else {
                                unreachable!()
                            };
                            let UserContentBlock::Text(text) = &content[0] else {
                                unreachable!()
                            };
                            text.text.len() as u64
                        })
                        .sum())
                })
                .unwrap();
            let input = request.model_input();
            let rustx::model::projected::ModelMessageView::User(content) = input.messages[0].view()
            else {
                unreachable!()
            };
            let UserContentBlock::Text(text) = &content[0] else {
                unreachable!()
            };
            let rendered = &text.text;
            assert!(
                rendered.contains(source_label),
                "missing {source_label}: {rendered}"
            );
            assert!(
                rendered.contains(kind_label),
                "missing {kind_label}: {rendered}"
            );
            plan.accept(&request).unwrap();
            previous = Some(SummaryGeneration {
                text: "accepted summary omits provenance".into(),
                generation: None,
            });
            count += 1;
        }
        assert!(count > 1);
    }
}

#[test]
fn accepting_a_fragment_rejects_modified_source_identity() {
    let source = canonical(UserSource::Runtime);
    let mut plan = crate::context::chunks::ChunkPlan::new(&vec![source].into(), 1).unwrap();
    let mut request = plan.next(None, None, None, 1, true, |_| Ok(1)).unwrap();
    let original = request.clone();
    request.fragment.as_mut().unwrap().sources[0] = rustx::context::chunks::SummarySource::User {
        source: UserSource::Human,
        kind: rustx::context::chunks::SummarySourceKind::Message,
        earlier_summary: false,
    };
    assert!(plan.accept(&request).is_err());
    assert!(!plan.is_complete());
    request.fragment.as_mut().unwrap().sources.clear();
    assert!(plan.accept(&request).is_err());
    assert!(!plan.is_complete());
    plan.accept(&original).unwrap();
    assert!(plan.is_complete());
}

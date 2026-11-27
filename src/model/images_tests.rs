use std::sync::Arc;

use crate::model::images::*;

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let encoder = png::Encoder::new(&mut bytes, width, height);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![0; width as usize * height as usize])
            .unwrap();
    }
    bytes
}

#[test]
fn managed_snapshot_survives_source_deletion_and_text_projection_keeps_correlation() {
    use crate::message::{content::ImageReference, types::*};
    use crate::tools::types::*;
    use rustx_identity::*;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.png");
    let bytes = png(1, 1);
    std::fs::write(&source, &bytes).unwrap();
    let store = crate::tools::artifacts::ArtifactStore::new(
        ConversationId::new("conv_01900000-0000-7000-8000-000000000001"),
        dir.path().join("artifacts"),
    )
    .unwrap();
    let id = store.put_bounded(&bytes).unwrap();
    std::fs::remove_file(source).unwrap();
    let mut result = ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Success,
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    };
    result
        .content
        .push(ToolResultContent::Image(ImageReference {
            artifact_id: id.clone(),
            alt: None,
        }));
    let mut messages = vec![crate::model::input::ModelInputMessage::canonical(
        MessageBlock::Tool(ToolMessageBlock {
            occurrence: ToolCallOccurrenceRef::new(
                MessageId::new("assistant"),
                ContentBlockIndex::new(0),
            ),
            id: MessageId::new("result"),
            tool_call_id: ToolCallId::new("call"),
            tool_id: ToolId::new("tool-read-image"),
            result: result.into(),
        }),
    )];
    let canonical = messages.clone();
    let summary = crate::context::summarizer::SummaryRequest {
        retired: canonical.clone(),
        ..Default::default()
    }
    .model_input();
    let crate::model::projected::ModelMessageView::User(content) = summary.messages[0].view()
    else {
        panic!("summary input")
    };
    assert!(
        content
            .iter()
            .all(|block| matches!(block, UserContentBlock::Text(_)))
    );
    assert!(
        serde_json::to_string(&summary.messages)
            .unwrap()
            .contains(id.as_str())
    );
    let resolved = resolve(&messages, |id| {
        store.read_bounded(id).map_err(|error| error.to_string())
    })
    .unwrap();
    assert_eq!(resolved[&id].as_bytes(), bytes);
    project(&mut messages, true);
    let [crate::model::input::ModelInputMessage::Canonical(MessageBlock::Tool(kept))] =
        messages.as_slice()
    else {
        panic!("one canonical tool result");
    };
    let [crate::model::input::ModelInputMessage::Canonical(MessageBlock::Tool(original))] =
        canonical.as_slice()
    else {
        panic!("one canonical tool result");
    };
    assert!(Arc::ptr_eq(&kept.result, &original.result));
    omit_images(&mut messages);
    let crate::model::input::ModelInputMessage::Canonical(MessageBlock::Tool(projected)) =
        &messages[0]
    else {
        panic!()
    };
    assert_eq!(projected.tool_call_id.as_str(), "call");
    assert!(
        projected
            .result
            .model_facing_projection()
            .as_text()
            .contains("Image artifact")
    );
    assert!(
        matches!(&canonical[0], crate::model::input::ModelInputMessage::Canonical(MessageBlock::Tool(tool)) if matches!(tool.result.content[0], ToolResultContent::Image(_)))
    );
}

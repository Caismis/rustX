//! Provider-neutral, request-only image data and bounded PNG validation.
//! Durable messages retain artifact references; encoded payloads never enter Trace.
use crate::runtime::identity::ArtifactId;
use std::collections::BTreeMap;

/// Ephemeral bytes keyed by the canonical managed artifact identity.
pub type ResolvedImages = BTreeMap<ArtifactId, Vec<u8>>;

/// Validate a complete static PNG snapshot without unbounded decoding.
/// # Errors
/// Rejects unsupported, corrupt, animated, or oversized images.
pub fn validate_png(bytes: &[u8]) -> Result<(), String> {
    const MAX_DECODE: usize = 16 * 1024 * 1024;
    const MAX_DECODER_INTERNAL: usize = 32 * 1024 * 1024;
    if bytes.len() > 256 * 1024 {
        return Err("image exceeds the 256 KiB byte limit".into());
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(
            "image bytes are not PNG (extension/content mismatch or unsupported format)".into(),
        );
    }
    let mut decoder = png::Decoder::new_with_limits(
        std::io::Cursor::new(bytes),
        png::Limits {
            bytes: MAX_DECODER_INTERNAL,
        },
    );
    // Reject dimensions from IHDR before the decoder builds row/frame buffers.
    let header = decoder
        .read_header_info()
        .map_err(|_| "invalid or truncated PNG header")?;
    if header.width > 4096
        || header.height > 4096
        || u64::from(header.width) * u64::from(header.height) > 4_194_304
    {
        return Err("image exceeds dimension or pixel limits".into());
    }
    let mut reader = decoder
        .read_info()
        .map_err(|_| "invalid or truncated PNG metadata")?;
    if reader.info().animation_control.is_some() {
        return Err("animated PNG is unsupported".into());
    }
    let len = reader
        .output_buffer_size()
        .filter(|size| *size <= MAX_DECODE)
        .ok_or("image exceeds decoded allocation limit")?;
    let mut pixels = vec![0; len];
    reader
        .next_frame(&mut pixels)
        .map_err(|_| "invalid or truncated PNG image")?;
    reader
        .finish()
        .map_err(|_| "invalid or truncated PNG end")?;
    Ok(())
}

/// Project image-bearing history for a text-only request without changing its Ledger.
pub fn omit_images(messages: &mut [crate::model::input::ModelInputMessage]) {
    use crate::message::{
        content::TextBlock,
        types::{AssistantContentBlock, MessageBlock, UserContentBlock},
    };
    use crate::tools::types::ToolResultContent;
    let text = |id: &ArtifactId| TextBlock {
        text: format!("[Image artifact {id}; image input unavailable for this invocation]"),
    };
    for message in messages {
        let Some(message) = (match message {
            crate::model::input::ModelInputMessage::Canonical(message) => Some(message),
            crate::model::input::ModelInputMessage::RequestOnly(_) => None,
        }) else {
            continue;
        };
        match message {
            MessageBlock::Tool(tool) => {
                for block in &mut tool.result.content {
                    if let ToolResultContent::Image(image) = block {
                        *block = ToolResultContent::Text(text(&image.artifact_id));
                    }
                }
            }
            MessageBlock::User(user) => {
                for block in &mut user.content {
                    if let UserContentBlock::Image(image) = block {
                        *block = UserContentBlock::Text(text(&image.artifact_id));
                    }
                }
            }
            MessageBlock::Assistant(assistant) => {
                for block in &mut assistant.content {
                    if let AssistantContentBlock::Image(image) = block {
                        *block = AssistantContentBlock::Text(text(&image.artifact_id));
                    }
                }
            }
        }
    }
}

/// Resolve only the finite referenced image set through a runtime-owned boundary.
/// # Errors
/// Missing, unsupported, invalid or oversized artifacts fail before transport.
pub fn resolve(
    messages: &[crate::model::input::ModelInputMessage],
    mut read: impl FnMut(&ArtifactId) -> Result<Vec<u8>, String>,
) -> Result<ResolvedImages, String> {
    use crate::message::types::{MessageBlock, UserContentBlock};
    use crate::tools::types::ToolResultContent;
    let mut images = ResolvedImages::new();
    for message in messages {
        let Some(message) = message.as_canonical() else {
            continue;
        };
        let references: Vec<_> = match message {
            MessageBlock::User(user) => user
                .content
                .iter()
                .filter_map(|block| match block {
                    UserContentBlock::Image(image) => Some(image),
                    _ => None,
                })
                .collect(),
            MessageBlock::Tool(tool) => tool
                .result
                .content
                .iter()
                .filter_map(|block| match block {
                    ToolResultContent::Image(image) => Some(image),
                    _ => None,
                })
                .collect(),
            MessageBlock::Assistant(_) => Vec::new(),
        };
        for image in references {
            if images.contains_key(&image.artifact_id) {
                continue;
            }
            if images.len() >= 16 {
                return Err("request exceeds the 16-image limit".into());
            }
            let bytes = read(&image.artifact_id)?;
            validate_png(&bytes)?;
            images.insert(image.artifact_id.clone(), bytes);
        }
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn validates_complete_snapshot_and_rejects_corruption_truncation_and_limits() {
        let valid = png(1, 1);
        validate_png(&valid).unwrap();
        for end in 0..valid.len() {
            assert!(validate_png(&valid[..end]).is_err(), "truncated at {end}");
        }
        let mut corrupt = valid.clone();
        corrupt[45] ^= 1;
        assert!(validate_png(&corrupt).is_err());
        assert!(validate_png(b"GIF89a").is_err());
        assert!(
            validate_png(&vec![0; 256 * 1024 + 1])
                .unwrap_err()
                .contains("byte limit")
        );
        assert!(
            validate_png(&png(4097, 1))
                .unwrap_err()
                .contains("dimension")
        );
        assert!(
            validate_png(&png(2049, 2048))
                .unwrap_err()
                .contains("pixel")
        );
    }
    #[test]
    fn rejects_decoded_allocation_before_creating_output_buffer() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2048, 2048);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Sixteen);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&vec![0; 2048 * 2048 * 8]).unwrap();
        }
        assert!(bytes.len() < 256 * 1024);
        assert!(
            validate_png(&bytes)
                .unwrap_err()
                .contains("decoded allocation")
        );
    }
    #[test]
    fn rejects_animation() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_animated(1, 0).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0]).unwrap();
        }
        assert!(validate_png(&bytes).unwrap_err().contains("animated"));
    }
    #[test]
    fn managed_snapshot_survives_source_deletion_and_text_projection_keeps_correlation() {
        use crate::message::{content::ImageReference, types::*};
        use crate::runtime::identity::*;
        use crate::tools::types::*;
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
                result,
            }),
        )];
        let canonical = messages.clone();
        let summary = crate::context::summarizer::SummaryRequest {
            retired: canonical
                .iter()
                .filter_map(crate::model::input::ModelInputMessage::as_canonical)
                .cloned()
                .collect(),
        }
        .model_input();
        let MessageBlock::User(summary_user) = &summary.messages[0] else {
            panic!("summary input")
        };
        assert!(
            summary_user
                .content
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
        assert_eq!(resolved[&id], bytes);
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
}

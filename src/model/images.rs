//! Provider-neutral, request-only image data and bounded PNG validation.
//! Durable messages retain artifact references; encoded payloads never enter Trace.
use crate::runtime::identity::ArtifactId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

/// Request image owners keyed by the canonical managed artifact identity.
pub type ResolvedImages = BTreeMap<ArtifactId, FrozenImage>;

/// Immutable encoded PNG bytes and their owner-scoped validation result.
/// Historical reads restore bytes without restoring any in-memory proof.
#[derive(Debug, Clone)]
pub struct FrozenImage(Arc<ImageBytes>);

#[derive(Debug)]
struct ImageBytes {
    bytes: Vec<u8>,
    measurement: OnceLock<Result<ImageMeasurement, String>>,
}

impl FrozenImage {
    /// Validate a fresh image capture and retain its original encoded allocation.
    /// # Errors
    /// Rejects unsupported, corrupt, animated or oversized PNGs.
    pub fn new(bytes: Vec<u8>) -> Result<Self, String> {
        let measurement = measure_png(&bytes)?;
        Ok(Self(Arc::new(ImageBytes {
            bytes,
            measurement: OnceLock::from(Ok(measurement)),
        })))
    }

    /// The exact frozen bytes, including for historical serialization.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0.bytes
    }

    /// Validates restored bytes on first semantic use. A fresh capture and
    /// every clone of the same owner use the one existing proof.
    /// # Errors
    /// Rejects invalid restored image bytes before budgeting or transport.
    pub fn measurement(&self) -> Result<ImageMeasurement, String> {
        self.0
            .measurement
            .get_or_init(|| measure_png(self.as_bytes()))
            .clone()
    }

    pub(crate) fn input_tokens(&self) -> Option<u64> {
        self.measurement().ok().map(ImageMeasurement::input_tokens)
    }
}

impl std::ops::Deref for FrozenImage {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl PartialEq for FrozenImage {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.as_bytes() == other.as_bytes()
    }
}
impl Eq for FrozenImage {}

impl Serialize for FrozenImage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.bytes.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FrozenImage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(Arc::new(ImageBytes {
            bytes: Vec::deserialize(deserializer)?,
            measurement: OnceLock::new(),
        })))
    }
}

#[cfg(test)]
thread_local! {
    static DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn take_decodes() -> usize {
    DECODES.with(|count| count.replace(0))
}

/// Validate a complete static PNG snapshot without unbounded decoding.
/// # Errors
/// Rejects unsupported, corrupt, animated, or oversized images.
pub fn validate_png(bytes: &[u8]) -> Result<(), String> {
    measure_png(bytes).map(|_| ())
}

/// Immutable provider-neutral measurement of a validated static PNG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageMeasurement {
    pub width: u32,
    pub height: u32,
}
impl ImageMeasurement {
    /// Conservative runtime fit charge, not provider billing.
    #[must_use]
    pub fn input_tokens(self) -> u64 {
        1024 + (u64::from(self.width) * u64::from(self.height)).div_ceil(256)
    }
}

/// Validate before deriving immutable dimensions.
/// # Errors
/// Rejects the same unsupported/corrupt/oversized inputs as `validate_png`.
pub fn measure_png(bytes: &[u8]) -> Result<ImageMeasurement, String> {
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
    let measurement = ImageMeasurement {
        width: header.width,
        height: header.height,
    };
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
    #[cfg(test)]
    DECODES.with(|count| count.set(count.get() + 1));
    let mut pixels = vec![0; len];
    reader
        .next_frame(&mut pixels)
        .map_err(|_| "invalid or truncated PNG image")?;
    reader
        .finish()
        .map_err(|_| "invalid or truncated PNG end")?;
    Ok(measurement)
}

/// Project image-bearing history for a text-only request without changing its Ledger.
pub fn omit_images(messages: &mut [crate::model::input::ModelInputMessage]) {
    project(messages, false);
}

/// Newest 16 User/Tool image occurrences retain image transport. Older ones
/// become request-only textual artifact references. Canonical history is never changed.
/// Repeated references count as occurrences because each is a provider-visible block.
pub fn project(messages: &mut [crate::model::input::ModelInputMessage], image_input: bool) {
    project_with_changes(messages, image_input, |_, _| {});
}

/// Report the exact items selected for image downgrade before changing them.
/// Counting image occurrences is the transform's admission rule, not a body
/// comparison or a serialization-based identity proof.
pub(crate) fn project_with_changes(
    messages: &mut [crate::model::input::ModelInputMessage],
    image_input: bool,
    mut changed: impl FnMut(usize, &crate::model::input::ModelInputMessage),
) {
    use crate::message::{
        content::TextBlock,
        types::{AssistantContentBlock, MessageBlock, UserContentBlock},
    };
    use crate::tools::types::ToolResultContent;
    let text = |id: &ArtifactId| TextBlock {
        text: omission_text(id),
    };
    let mut remaining = if image_input { 16 } else { 0 };
    for (index, message) in messages.iter_mut().enumerate().rev() {
        let requires_projection = needs_projection(message, image_input, remaining);
        if requires_projection {
            changed(index, message);
        }
        let Some(message) = (match message {
            crate::model::input::ModelInputMessage::Canonical(message) => Some(message),
            crate::model::input::ModelInputMessage::RequestOnly(_) => None,
            crate::model::input::ModelInputMessage::Derived(derived) => {
                project_derived_images(&mut derived.message, image_input, &mut remaining);
                if requires_projection
                    && !derived
                        .transformations
                        .contains(&crate::model::projected::SemanticTransformation::ImageDowngrade)
                {
                    derived
                        .transformations
                        .push(crate::model::projected::SemanticTransformation::ImageDowngrade);
                }
                None
            }
        }) else {
            continue;
        };
        match message {
            MessageBlock::Tool(tool) => {
                for block in tool.result.content.iter_mut().rev() {
                    if let ToolResultContent::Image(image) = block {
                        if remaining > 0 {
                            remaining -= 1;
                        } else {
                            *block = ToolResultContent::Text(text(&image.artifact_id));
                        }
                    }
                }
            }
            MessageBlock::User(user) => {
                for block in user.content.iter_mut().rev() {
                    if let UserContentBlock::Image(image) = block {
                        if remaining > 0 {
                            remaining -= 1;
                        } else {
                            *block = UserContentBlock::Text(text(&image.artifact_id));
                        }
                    }
                }
            }
            MessageBlock::Assistant(assistant) => {
                for block in &mut assistant.content {
                    if let AssistantContentBlock::Image(image) = block
                        && !image_input
                    {
                        *block = AssistantContentBlock::Text(text(&image.artifact_id));
                    }
                }
            }
        }
    }
}

fn needs_projection(
    message: &crate::model::input::ModelInputMessage,
    enabled: bool,
    remaining: usize,
) -> bool {
    use crate::message::types::{AssistantContentBlock, UserContentBlock};
    use crate::model::projected::ModelMessageView;
    use crate::tools::types::ToolResultContent;
    match message.view() {
        ModelMessageView::User(content) => {
            content
                .iter()
                .filter(|block| matches!(block, UserContentBlock::Image(_)))
                .take(remaining + 1)
                .count()
                > remaining
        }
        ModelMessageView::Tool(_, result) => {
            result
                .content
                .iter()
                .filter(|block| matches!(block, ToolResultContent::Image(_)))
                .take(remaining + 1)
                .count()
                > remaining
        }
        ModelMessageView::Assistant(content) => {
            !enabled
                && content
                    .iter()
                    .any(|block| matches!(block, AssistantContentBlock::Image(_)))
        }
        ModelMessageView::RequestOnly(_) => false,
    }
}

fn project_derived_images(
    message: &mut crate::model::projected::ProjectedMessage,
    enabled: bool,
    remaining: &mut usize,
) {
    use crate::message::{
        content::TextBlock,
        types::{AssistantContentBlock, UserContentBlock},
    };
    use crate::model::projected::ProjectedMessage;
    use crate::tools::types::ToolResultContent;
    let text = |id: &ArtifactId| TextBlock {
        text: omission_text(id),
    };
    match message {
        ProjectedMessage::User { content, .. } => {
            for block in content.iter_mut().rev() {
                if let UserContentBlock::Image(image) = block {
                    if *remaining > 0 {
                        *remaining -= 1;
                    } else {
                        *block = UserContentBlock::Text(text(&image.artifact_id));
                    }
                }
            }
        }
        ProjectedMessage::Tool { result, .. } => {
            for block in result.content.iter_mut().rev() {
                if let ToolResultContent::Image(image) = block {
                    if *remaining > 0 {
                        *remaining -= 1;
                    } else {
                        *block = ToolResultContent::Text(text(&image.artifact_id));
                    }
                }
            }
        }
        ProjectedMessage::Assistant { content } => {
            for block in content {
                if let AssistantContentBlock::Image(image) = block
                    && !enabled
                {
                    *block = AssistantContentBlock::Text(text(&image.artifact_id));
                }
            }
        }
    }
}

/// Number of provider-visible User/Tool image blocks, including repeated identities.
pub(crate) fn image_count<'a>(
    messages: impl IntoIterator<Item = &'a crate::model::input::ModelInputMessage>,
) -> usize {
    use crate::message::types::UserContentBlock;
    use crate::model::projected::ModelMessageView;
    use crate::tools::types::ToolResultContent;
    messages
        .into_iter()
        .map(|message| match message.view() {
            ModelMessageView::User(user) => user
                .iter()
                .filter(|block| matches!(block, UserContentBlock::Image(_)))
                .count(),
            ModelMessageView::Tool(_, result) => result
                .content
                .iter()
                .filter(|block| matches!(block, ToolResultContent::Image(_)))
                .count(),
            _ => 0,
        })
        .sum()
}

/// Resolve only the finite referenced image set through a runtime-owned boundary.
/// # Errors
/// Missing, unsupported, invalid or oversized artifacts fail before transport.
pub fn resolve(
    messages: &[crate::model::input::ModelInputMessage],
    mut read: impl FnMut(&ArtifactId) -> Result<Vec<u8>, String>,
) -> Result<ResolvedImages, String> {
    use crate::message::types::UserContentBlock;
    use crate::model::projected::ModelMessageView;
    use crate::tools::types::ToolResultContent;
    if image_count(messages) > 16 {
        return Err("request exceeds the 16-image limit".into());
    }
    let mut images = ResolvedImages::new();
    for message in messages {
        let references: Vec<_> = match message.view() {
            ModelMessageView::User(user) => user
                .iter()
                .filter_map(|block| match block {
                    UserContentBlock::Image(image) => Some(image),
                    _ => None,
                })
                .collect(),
            ModelMessageView::Tool(_, result) => result
                .content
                .iter()
                .filter_map(|block| match block {
                    ToolResultContent::Image(image) => Some(image),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        for image in references {
            if images.contains_key(&image.artifact_id) {
                continue;
            }
            if images.len() >= 16 {
                return Err("request exceeds the 16-image limit".into());
            }
            let bytes = read(&image.artifact_id)?;
            images.insert(image.artifact_id.clone(), FrozenImage::new(bytes)?);
        }
    }
    Ok(images)
}

/// The exact semantic omission value shared with frozen-input validation.
pub(crate) fn omission_text(id: &ArtifactId) -> String {
    format!("[Image artifact {id}; image omitted from this request]")
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
    fn frozen_owner_reuses_validation_and_restores_only_encoded_bytes() {
        let bytes = png(2, 3);
        let json = serde_json::to_vec(&bytes).unwrap();
        let allocation = bytes.as_ptr();
        take_decodes();
        let captured = FrozenImage::new(bytes).unwrap();
        let peer = captured.clone();
        assert_eq!(captured.as_ptr(), allocation);
        assert_eq!(peer.as_ptr(), allocation);
        assert_eq!(captured.input_tokens(), peer.input_tokens());
        assert_eq!(take_decodes(), 1);
        assert_eq!(serde_json::to_vec(&peer).unwrap(), json);

        let restored: FrozenImage = serde_json::from_slice(&json).unwrap();
        assert_eq!(restored, peer);
        assert_eq!(take_decodes(), 0);
        assert_eq!(
            restored.measurement().unwrap(),
            ImageMeasurement {
                width: 2,
                height: 3
            }
        );
        assert_eq!(restored.clone().input_tokens(), peer.input_tokens());
        assert_eq!(take_decodes(), 1);
        let restored_bad: FrozenImage = serde_json::from_str("[1,2,3]").unwrap();
        assert!(restored_bad.measurement().is_err());

        let owner = Arc::downgrade(&captured.0);
        drop(captured);
        assert_eq!(owner.strong_count(), 1);
        drop(peer);
        assert_eq!(owner.strong_count(), 0);
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
                result,
            }),
        )];
        let canonical = messages.clone();
        let summary = crate::context::summarizer::SummaryRequest {
            retired: canonical.clone(),
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

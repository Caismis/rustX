//! Shared basic content and reference blocks used by canonical messages.
//!
//! These are runtime-owned structures. References identify durable runtime
//! artifacts by opaque id; they never embed a storage SDK type and never use
//! a local filesystem path as the durable identity.

use serde::{Deserialize, Serialize};

use crate::runtime::identity::ArtifactId;

/// A plain text content block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextBlock {
    /// The text content.
    pub text: String,
}

/// A reference to an image artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageReference {
    /// Durable artifact identity of the image.
    pub artifact_id: ArtifactId,
    /// Optional short description or alt text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}

/// A reference to a file artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReference {
    /// Durable artifact identity of the file.
    pub artifact_id: ArtifactId,
    /// Optional display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional MIME type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{FileReference, ImageReference, TextBlock};
    use crate::runtime::identity::ArtifactId;

    /// Shared content blocks round-trip with runtime-owned artifact ids.
    #[test]
    fn shared_content_round_trip() {
        let text = TextBlock {
            text: "hello".to_owned(),
        };
        let image = ImageReference {
            artifact_id: ArtifactId::new("artifact-9"),
            alt: Some("screenshot".to_owned()),
        };
        let file = FileReference {
            artifact_id: ArtifactId::new("artifact-10"),
            name: Some("report.txt".to_owned()),
            mime_type: Some("text/plain".to_owned()),
            description: None,
        };
        for value in [
            serde_json::to_value(&text).expect("text"),
            serde_json::to_value(&image).expect("image"),
            serde_json::to_value(&file).expect("file"),
        ] {
            let json = serde_json::to_string(&value).expect("serialize");
            let _: serde_json::Value = serde_json::from_str(&json).expect("deserialize");
        }
        assert_eq!(
            serde_json::to_string(&file).expect("serialize file"),
            r#"{"artifact_id":"artifact-10","name":"report.txt","mime_type":"text/plain"}"#
        );
    }
}

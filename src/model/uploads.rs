//! One provider-independent rendering of Session-owned uploaded-file facts.
use crate::message::{
    content::{TextBlock, UploadedFileRef},
    types::{MessageBlock, UserContentBlock},
};
use crate::model::input::ModelInputMessage;
use serde::{Deserialize, Serialize};

/// Request-time paths, frozen separately from canonical history for replay.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadProjection {
    pub files: Vec<ResolvedUpload>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedUpload {
    pub file: UploadedFileRef,
    pub path: String,
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('\n', "&#10;")
        .replace('\r', "&#13;")
        .replace('\t', "&#9;")
}
impl UploadProjection {
    /// Render all upload-bearing User items, preserving text bytes and order.
    /// # Errors
    /// A typed file without a Session-resolved path is rejected.
    pub fn apply(&self, messages: &mut [ModelInputMessage]) -> Result<(), String> {
        use std::fmt::Write;
        for message in messages {
            let ModelInputMessage::Canonical(MessageBlock::User(user)) = message else {
                continue;
            };
            if !user
                .content
                .iter()
                .any(|b| matches!(b, UserContentBlock::UploadedFile(_)))
            {
                continue;
            }
            let mut prefix = String::from("<user_uploaded_files>\n");
            for content in &user.content {
                if let UserContentBlock::UploadedFile(reference) = content {
                    let file = self
                        .files
                        .iter()
                        .find(|f| &f.file == reference)
                        .ok_or("unresolved Session upload")?;
                    if !std::path::Path::new(&file.path).is_absolute() {
                        return Err("upload projection path must be absolute".into());
                    }
                    writeln!(
                        &mut prefix,
                        "  <file name=\"{}\" path=\"{}\" />",
                        escape(&reference.name),
                        escape(&file.path)
                    )
                    .expect("string write");
                }
            }
            prefix.push_str("</user_uploaded_files>\n\n");
            // Text block boundaries contribute no caller-unauthored whitespace.
            for content in &user.content {
                if let UserContentBlock::Text(text) = content {
                    prefix.push_str(&text.text);
                }
            }
            let mut content = vec![UserContentBlock::Text(TextBlock { text: prefix })];
            content.extend(
                user.content
                    .iter()
                    .filter(|b| {
                        !matches!(
                            b,
                            UserContentBlock::Text(_) | UserContentBlock::UploadedFile(_)
                        )
                    })
                    .cloned(),
            );
            user.content = content;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::types::{InboundKind, UserMessageBlock, UserSource};
    use crate::runtime::identity::MessageId;
    #[test]
    fn exact_escaped_ordered_projection_preserves_body_and_plain_turns() {
        let first = UploadedFileRef {
            batch_id: "batch".into(),
            name: "a&'b.png".into(),
        };
        let second = UploadedFileRef {
            batch_id: "batch".into(),
            name: "data.csv".into(),
        };
        let body = "\n Please analyze.\r\n  Keep spaces.\n";
        let user = |content| {
            ModelInputMessage::Canonical(MessageBlock::User(UserMessageBlock {
                id: MessageId::new("user"),
                content,
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            }))
        };
        let text = UserContentBlock::Text(TextBlock { text: body.into() });
        let mut plain = vec![user(vec![text.clone()])];
        let unchanged = plain.clone();
        UploadProjection::default().apply(&mut plain).unwrap();
        assert_eq!(plain, unchanged);
        let mut messages = vec![user(vec![
            UserContentBlock::UploadedFile(first.clone()),
            text,
            UserContentBlock::UploadedFile(second.clone()),
        ])];
        let canonical = messages.clone();
        let projection = UploadProjection {
            files: vec![
                ResolvedUpload {
                    file: second,
                    path: "/workspace/data.csv".into(),
                },
                ResolvedUpload {
                    file: first,
                    path: "/a\"&<>'/a.png".into(),
                },
            ],
        };
        projection.apply(&mut messages).unwrap();
        let ModelInputMessage::Canonical(MessageBlock::User(rendered)) = &messages[0] else {
            panic!("user");
        };
        assert_eq!(
            rendered.content,
            vec![UserContentBlock::Text(TextBlock {
                text: format!(
                    "<user_uploaded_files>\n  <file name=\"a&amp;&apos;b.png\" path=\"/a&quot;&amp;&lt;&gt;&apos;/a.png\" />\n  <file name=\"data.csv\" path=\"/workspace/data.csv\" />\n</user_uploaded_files>\n\n{body}"
                )
            })]
        );
        assert_ne!(messages, canonical);
        assert!(
            !serde_json::to_string(&canonical)
                .unwrap()
                .contains("/workspace")
        );
        assert!(
            UploadProjection::default()
                .apply(&mut canonical.clone())
                .is_err()
        );
    }
}

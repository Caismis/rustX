//! One provider-independent rendering of Session-owned uploaded-file facts.
use crate::message::{
    content::{TextBlock, UploadedFileRef},
    types::{MessageBlock, UserContentBlock},
};
use crate::model::input::ModelInputMessage;
use serde::{Deserialize, Serialize};

/// Core-owned, read-only resolution boundary. Implementations supply Session-owned
/// paths; context/model code has no knowledge of their persistence.
pub trait UploadProjectionResolver: std::fmt::Debug + Send + Sync {
    /// Resolve only the upload facts needed by this finite model input.
    /// # Errors
    /// Unknown or unavailable ownership is explicit; no canonical facts are created.
    fn resolve(&self, messages: &[ModelInputMessage]) -> Result<UploadProjection, String>;
}

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
fn escaped_character(character: char) -> Option<&'static str> {
    match character {
        '&' => Some("&amp;"),
        '"' => Some("&quot;"),
        '<' => Some("&lt;"),
        '>' => Some("&gt;"),
        '\'' => Some("&apos;"),
        '\n' => Some("&#10;"),
        '\r' => Some("&#13;"),
        '\t' => Some("&#9;"),
        _ => None,
    }
}
pub(crate) fn escaped_len(value: &str) -> Option<usize> {
    value.chars().try_fold(0usize, |total, character| {
        total.checked_add(escaped_character(character).map_or(character.len_utf8(), str::len))
    })
}
pub(crate) fn escape(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        if let Some(replacement) = escaped_character(character) {
            escaped.push_str(replacement);
        } else {
            escaped.push(character);
        }
    }
    escaped
}

impl UploadProjection {
    /// Render all upload-bearing User items, preserving text bytes and order.
    /// # Errors
    /// A typed file without a Session-resolved path is rejected.
    pub fn apply(&self, messages: &mut [ModelInputMessage]) -> Result<(), String> {
        self.apply_with_changes(messages, |_, _| {})
    }

    /// Validate the same references as rendering without allocating its text.
    pub(crate) fn validate<'a>(
        &self,
        messages: impl IntoIterator<Item = &'a ModelInputMessage>,
    ) -> Result<(), String> {
        let files: std::collections::BTreeMap<_, _> =
            self.files.iter().map(|file| (&file.file, file)).collect();
        if files.len() != self.files.len() {
            return Err("duplicate upload resolution".into());
        }
        for message in messages {
            if let crate::model::projected::ModelMessageView::User(content) = message.view() {
                for block in content {
                    if let UserContentBlock::UploadedFile(reference) = block {
                        let file = files.get(reference).ok_or("unresolved Session upload")?;
                        if !std::path::Path::new(&file.path).is_absolute() {
                            return Err("upload projection path must be absolute".into());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Observe only items the upload renderer actually replaces. The observer
    /// borrows the original value before mutation; no history clone is needed.
    pub(crate) fn apply_with_changes(
        &self,
        messages: &mut [ModelInputMessage],
        mut changed: impl FnMut(usize, &ModelInputMessage),
    ) -> Result<(), String> {
        use std::fmt::Write;
        let files: std::collections::BTreeMap<_, _> =
            self.files.iter().map(|file| (&file.file, file)).collect();
        if files.len() != self.files.len() {
            return Err("duplicate upload resolution".into());
        }
        for (index, message) in messages.iter_mut().enumerate() {
            if !matches!(message.view(), crate::model::projected::ModelMessageView::User(content)
                if content.iter().any(|block| matches!(block, UserContentBlock::UploadedFile(_))))
            {
                continue;
            }
            changed(index, message);
            let user_content = match message {
                ModelInputMessage::Canonical(MessageBlock::User(user)) => &mut user.content,
                ModelInputMessage::Derived(derived) => match &mut derived.message {
                    crate::model::projected::ProjectedMessage::User { content, .. } => content,
                    _ => continue,
                },
                _ => continue,
            };
            let mut prefix = String::from("<user_uploaded_files>\n");
            for content in user_content.iter() {
                if let UserContentBlock::UploadedFile(reference) = content {
                    let file = files.get(reference).ok_or("unresolved Session upload")?;
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
            for content in user_content.iter() {
                if let UserContentBlock::Text(text) = content {
                    prefix.push_str(&text.text);
                }
            }
            let mut content = vec![UserContentBlock::Text(TextBlock { text: prefix })];
            content.extend(
                user_content
                    .iter()
                    .filter(|b| {
                        !matches!(
                            b,
                            UserContentBlock::Text(_) | UserContentBlock::UploadedFile(_)
                        )
                    })
                    .cloned(),
            );
            *user_content = content;
            if let ModelInputMessage::Derived(derived) = message
                && !derived
                    .transformations
                    .contains(&crate::model::projected::SemanticTransformation::UploadRendering)
            {
                derived
                    .transformations
                    .push(crate::model::projected::SemanticTransformation::UploadRendering);
            }
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
    fn escaped_byte_accounting_equals_the_authoritative_renderer() {
        for value in ["plain", "Unicode: λ 雪", "&\"<>'\n\r\t", "/long/file&.txt"] {
            assert_eq!(escaped_len(value), Some(escape(value).len()));
        }
    }
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
        let source = canonical[0].as_canonical().unwrap();
        let mut derived = vec![ModelInputMessage::Derived(
            crate::model::projected::DerivedModelMessage {
                message: crate::model::projected::ProjectedMessage::from(source),
                sources: vec![crate::model::projected::CanonicalMessageRef::of(
                    &crate::runtime::identity::ConversationId::new(
                        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
                    ),
                    source,
                )],
                checkpoint_sources: Vec::new(),
                transformations: vec![crate::model::projected::SemanticTransformation::Summary],
            },
        )];
        projection.apply(&mut derived).unwrap();
        let ModelInputMessage::Derived(item) = &derived[0] else {
            panic!("Derived stays identity-free");
        };
        let crate::model::projected::ProjectedMessage::User { content, .. } = &item.message else {
            panic!("typed User");
        };
        assert_eq!(content, &rendered.content);
        assert!(
            item.transformations
                .contains(&crate::model::projected::SemanticTransformation::UploadRendering)
        );
        assert!(item.sources[0].matches(source));
        assert!(derived[0].canonical_id().is_none());
        let frozen = derived.clone();
        projection.apply(&mut derived).unwrap();
        assert_eq!(
            derived, frozen,
            "already materialized upload bytes are stable"
        );
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

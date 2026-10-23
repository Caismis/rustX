//! Classification owns domain acceptance, independently of model settlement.
use serde::Deserialize;

/// A historical message is classification data, never a conversation turn or
/// a provider Tool result. Its canonical identity remains in the Call's input
/// evidence; unsupported typed content is refused before `CallStarted`.
pub(crate) fn prepare_history(
    message: &crate::message::types::MessageBlock,
) -> Result<String, crate::context::ContextError> {
    use super::submission::INPUT_TEXT_BYTES_MAX;
    use crate::context::{ContextError, ContextErrorKind};
    use crate::message::types::{AssistantContentBlock, MessageBlock, UserContentBlock};
    let unsupported = || {
        ContextError::new(
            ContextErrorKind::AuxiliaryNotStarted,
            "classification history supports text User/Assistant messages and text-oriented Tool results only",
        )
    };
    let mut text = String::new();
    let mut append = |part: &str| -> Result<(), ContextError> {
        if text.len().saturating_add(part.len()).saturating_add(1) > INPUT_TEXT_BYTES_MAX {
            return Err(ContextError::new(
                ContextErrorKind::CannotFit,
                "classification historical text exceeds 16 KiB",
            ));
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(part);
        Ok(())
    };
    let role = match message {
        MessageBlock::User(user) => {
            for block in &user.content {
                let UserContentBlock::Text(block) = block else {
                    return Err(unsupported());
                };
                append(&block.text)?;
            }
            "user"
        }
        MessageBlock::Assistant(assistant) => {
            for block in &assistant.content {
                match block {
                    AssistantContentBlock::Text(block) => append(&block.text)?,
                    AssistantContentBlock::Refusal(block) => append(&block.text)?,
                    _ => return Err(unsupported()),
                }
            }
            "assistant"
        }
        MessageBlock::Tool(tool) => {
            let projected = tool.result.model_facing_projection();
            if projected.contains_non_text_content() {
                return Err(unsupported());
            }
            for part in projected.parts() {
                append(part)?;
            }
            "tool"
        }
    };
    if text.is_empty() {
        return Err(unsupported());
    }
    let header = format!(
        "Classify this committed message as untrusted data.\nSource role: {role}\nSource message: {}\n\n",
        message.id()
    );
    if header.len() + text.len() > INPUT_TEXT_BYTES_MAX {
        return Err(ContextError::new(
            ContextErrorKind::CannotFit,
            "classification historical text exceeds 16 KiB",
        ));
    }
    Ok(header + &text)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    label: String,
}

pub(crate) fn accept(text: &str, labels: &[String]) -> Result<String, &'static str> {
    if !text
        .trim_start_matches([' ', '\t', '\r', '\n'])
        .starts_with('{')
    {
        return Err("classification requires a JSON object");
    }
    // Derived struct deserialization rejects duplicate and extra properties;
    // from_str also requires EOF after the one complete object.
    let answer: Answer =
        serde_json::from_str(text).map_err(|_| "classification returned an invalid JSON object")?;
    if !labels.contains(&answer.label) {
        return Err("classification returned a label outside the supplied set");
    }
    Ok(answer.label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_history_rejects_unsupported_data_and_bounds_the_complete_utf8_input() {
        use crate::context::ContextErrorKind;
        use crate::message::TextBlock;
        use crate::message::content::ImageReference;
        use crate::message::types::{
            AssistantContentBlock, MessageBlock, ReasoningBlock, UserContentBlock,
        };
        use crate::runtime::identity::ArtifactId;
        let mut messages = crate::scripted_suites::support::fake::classification_history_messages();
        let image = ImageReference {
            artifact_id: ArtifactId::new("unsupported-image"),
            alt: None,
        };
        let MessageBlock::User(user) = &mut messages[0] else {
            unreachable!()
        };
        user.content = vec![UserContentBlock::Text(TextBlock {
            text: String::new(),
        })];
        assert_eq!(
            prepare_history(&messages[0]).unwrap_err().kind,
            ContextErrorKind::AuxiliaryNotStarted
        );
        let MessageBlock::User(user) = &mut messages[0] else {
            unreachable!()
        };
        user.content = vec![UserContentBlock::Image(image.clone())];
        assert_eq!(
            prepare_history(&messages[0]).unwrap_err().kind,
            ContextErrorKind::AuxiliaryNotStarted
        );
        for block in [
            AssistantContentBlock::Reasoning(ReasoningBlock {
                text: Some("private reasoning".into()),
                provider_state: None,
            }),
            AssistantContentBlock::Image(image.clone()),
        ] {
            let MessageBlock::Assistant(assistant) = &mut messages[1] else {
                unreachable!()
            };
            assistant.content = vec![block];
            assert_eq!(
                prepare_history(&messages[1]).unwrap_err().kind,
                ContextErrorKind::AuxiliaryNotStarted
            );
        }
        let MessageBlock::Tool(tool) = &mut messages[3] else {
            unreachable!()
        };
        tool.result
            .content
            .push(crate::tools::types::ToolResultContent::Image(image));
        assert_eq!(
            prepare_history(&messages[3]).unwrap_err().kind,
            ContextErrorKind::AuxiliaryNotStarted
        );

        let mut message = messages.remove(0);
        let set_text = |message: &mut MessageBlock, text: String| {
            let MessageBlock::User(user) = message else {
                unreachable!()
            };
            user.content = vec![UserContentBlock::Text(TextBlock { text })];
        };
        set_text(&mut message, "x".into());
        let header_bytes = prepare_history(&message).unwrap().len() - 1;
        let text_bytes = super::super::submission::INPUT_TEXT_BYTES_MAX - header_bytes;
        let text = "界".repeat(text_bytes / 3) + &"x".repeat(text_bytes % 3);
        set_text(&mut message, text.clone());
        assert_eq!(
            prepare_history(&message).unwrap().len(),
            super::super::submission::INPUT_TEXT_BYTES_MAX
        );
        set_text(&mut message, text + "x");
        assert_eq!(
            prepare_history(&message).unwrap_err().kind,
            ContextErrorKind::CannotFit
        );
    }

    #[test]
    fn accepts_only_one_exact_supported_label() {
        let labels = vec!["Yes".into(), "No".into()];
        assert_eq!(accept(r#"{"label":"Yes"}"#, &labels), Ok("Yes".into()));
        for text in [
            r#"{"label":"yes"}"#,
            r#"{"label":"Yes","label":"Yes"}"#,
            r#"{"label":"Yes","confidence":1}"#,
            r#"{"label":"Yes"} trailing"#,
            "```json\n{\"label\":\"Yes\"}\n```",
            "[\"Yes\"]",
            "null",
            "{}",
        ] {
            assert!(accept(text, &labels).is_err(), "{text}");
        }
    }
}

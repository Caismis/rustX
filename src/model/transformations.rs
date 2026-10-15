//! Pure validation of frozen request-local media changes. No mutable resource lookup.
use crate::message::types::{AssistantContentBlock, UserContentBlock};
use crate::model::projected::{ProjectedMessage, SemanticTransformation};
use crate::tools::types::ToolResultContent;

/// An unchanged projection may retain its provenance; changes must append only
/// supported request-time transformations and preserve every non-media value.
#[allow(clippy::too_many_lines)] // One exhaustive pure semantic media transformation validation.
pub(crate) fn valid(
    expected: &ProjectedMessage,
    actual: &ProjectedMessage,
    prior: &[SemanticTransformation],
    transformations: &[SemanticTransformation],
) -> bool {
    if !transformations.starts_with(prior) {
        return false;
    }
    let appended = &transformations[prior.len()..];
    if appended.iter().any(|t| {
        !matches!(
            t,
            SemanticTransformation::ImageDowngrade | SemanticTransformation::UploadRendering
        )
    }) {
        return false;
    }
    if expected == actual {
        return appended.is_empty();
    }
    if let ProjectedMessage::Assistant { content } = expected
        && content.iter().any(|b|matches!(b, AssistantContentBlock::Reasoning(r) if matches!(r.provider_state,Some(crate::runtime::continuation::ProviderContinuationState::OpenAiResponses(crate::runtime::continuation::OpenAiResponsesContinuation::Stateless { .. }))))) {
        // Encrypted Responses replay owns its complete native output unit.
        // A semantic media edit cannot reuse that opaque state.
        return false;
    }
    // Existing projection transforms may also apply to additional media in a
    // request. They never grant permission to change unrelated content.
    let images = transformations.contains(&SemanticTransformation::ImageDowngrade);
    let uploads = transformations.contains(&SemanticTransformation::UploadRendering);
    let mut rendered = expected.clone();
    if uploads
        && let ProjectedMessage::User { content, .. } = &mut rendered
        && content
            .iter()
            .any(|b| matches!(b, UserContentBlock::UploadedFile(_)))
    {
        let ProjectedMessage::User {
            content: observed, ..
        } = actual
        else {
            return false;
        };
        let Some(UserContentBlock::Text(text)) = observed.first() else {
            return false;
        };
        let Some(mut rest) = text.text.strip_prefix("<user_uploaded_files>\n") else {
            return false;
        };
        let mut files = Vec::new();
        for reference in content.iter().filter_map(|b| {
            if let UserContentBlock::UploadedFile(r) = b {
                Some(r)
            } else {
                None
            }
        }) {
            let prefix = format!(
                "  <file name=\"{}\" path=\"",
                super::uploads::escape(&reference.name)
            );
            let Some(after) = rest.strip_prefix(&prefix) else {
                return false;
            };
            let Some((encoded, remaining)) = after.split_once("\" />\n") else {
                return false;
            };
            let path = encoded
                .replace("&#9;", "\t")
                .replace("&#13;", "\r")
                .replace("&#10;", "\n")
                .replace("&apos;", "'")
                .replace("&gt;", ">")
                .replace("&lt;", "<")
                .replace("&quot;", "\"")
                .replace("&amp;", "&");
            if super::uploads::escape(&path) != encoded
                || !std::path::Path::new(&path).is_absolute()
            {
                return false;
            }
            files.push(super::uploads::ResolvedUpload {
                file: reference.clone(),
                path,
            });
            rest = remaining;
        }
        if !rest.starts_with("</user_uploaded_files>\n\n") {
            return false;
        }
        // Render using only paths extracted from the frozen bytes, never a
        // live resolver. Exact equality below also checks the preserved body.
        let mut messages = [super::ModelInputMessage::Derived(
            super::projected::DerivedModelMessage {
                message: rendered.clone(),
                sources: vec![],
                checkpoint_sources: vec![],
                transformations: vec![],
            },
        )];
        if (super::uploads::UploadProjection { files })
            .apply(&mut messages)
            .is_err()
        {
            return false;
        }
        let super::ModelInputMessage::Derived(value) = &messages[0] else {
            unreachable!()
        };
        rendered = value.message.clone();
    }
    if images {
        match (&mut rendered, actual) {
            (
                ProjectedMessage::User { content, .. },
                ProjectedMessage::User {
                    content: observed, ..
                },
            ) if content.len() == observed.len() => {
                for (before, after) in content.iter_mut().zip(observed) {
                    if let UserContentBlock::Image(image) = before
                        && matches!(after, UserContentBlock::Text(t) if t.text == super::images::omission_text(&image.artifact_id))
                    {
                        *before = after.clone();
                    }
                }
            }
            (
                ProjectedMessage::Assistant { content },
                ProjectedMessage::Assistant { content: observed },
            ) if content.len() == observed.len() => {
                for (before, after) in content.iter_mut().zip(observed) {
                    if let AssistantContentBlock::Image(image) = before
                        && matches!(after, AssistantContentBlock::Text(t) if t.text == super::images::omission_text(&image.artifact_id))
                    {
                        *before = after.clone();
                    }
                }
            }
            (
                ProjectedMessage::Tool { result, .. },
                ProjectedMessage::Tool {
                    result: observed, ..
                },
            ) if result.content.len() == observed.content.len() => {
                for (before, after) in result.content.iter_mut().zip(&observed.content) {
                    if let ToolResultContent::Image(image) = before
                        && matches!(after, ToolResultContent::Text(t) if t.text == super::images::omission_text(&image.artifact_id))
                    {
                        *before = after.clone();
                    }
                }
            }
            _ => {}
        }
    }
    &rendered == actual
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{
        content::{TextBlock, UploadedFileRef},
        types::UserSource,
    };
    #[test]
    fn upload_tags_cannot_change_roles_body_or_existing_provenance() {
        let expected = ProjectedMessage::User {
            content: vec![
                UserContentBlock::UploadedFile(UploadedFileRef {
                    batch_id: "b".into(),
                    name: "a&b".into(),
                }),
                UserContentBlock::Text(TextBlock {
                    text: "unaltered".into(),
                }),
            ],
            source: UserSource::Human,
            kind: crate::model::projected::ProjectedUserKind::Message,
            timestamp: None,
        };
        let actual = ProjectedMessage::User { content: vec![UserContentBlock::Text(TextBlock { text: "<user_uploaded_files>\n  <file name=\"a&amp;b\" path=\"/frozen/a\" />\n</user_uploaded_files>\n\nunaltered".into() })], source: UserSource::Human, kind: crate::model::projected::ProjectedUserKind::Message, timestamp: None };
        assert!(valid(
            &expected,
            &actual,
            &[],
            &[SemanticTransformation::UploadRendering]
        ));
        let mut changed = actual.clone();
        let ProjectedMessage::User { content, .. } = &mut changed else {
            unreachable!()
        };
        let UserContentBlock::Text(t) = &mut content[0] else {
            unreachable!()
        };
        t.text.push_str(" invented");
        assert!(!valid(
            &expected,
            &changed,
            &[],
            &[SemanticTransformation::UploadRendering]
        ));
        assert!(!valid(
            &expected,
            &actual,
            &[SemanticTransformation::Summary],
            &[SemanticTransformation::UploadRendering]
        ));
        assert!(!valid(
            &expected,
            &ProjectedMessage::Assistant { content: vec![] },
            &[],
            &[SemanticTransformation::UploadRendering]
        ));
    }
}

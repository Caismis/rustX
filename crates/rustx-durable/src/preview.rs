//! The bounded single-line preview of one canonical user message, persisted
//! beside the ledger row for the `/resume` listing.

use rustx_types::message::types::{UserContentBlock, UserMessageBlock};

/// The maximum rendered preview length persisted with a canonical row.
const SESSION_PREVIEW_LIMIT: usize = 120;

/// Renders one canonical user message as the bounded single line a `/resume`
/// row shows for an unnamed Session. Non-text content contributes nothing:
/// a Session opened with only an image has no first line to show, and saying
/// so with `None` is more honest than inventing one.
pub fn preview_of(message: &UserMessageBlock) -> Option<String> {
    // Only the first normalized 121 characters can affect the exact 120
    // character preview. Stream the native blocks without joining their bodies.
    let mut preview = String::with_capacity(SESSION_PREVIEW_LIMIT * 4);
    let (mut count, mut gap, mut first) = (0_usize, false, true);
    for block in &message.content {
        let UserContentBlock::Text(block) = block else {
            continue;
        };
        if !first {
            gap = !preview.is_empty();
        }
        first = false;
        for character in block.text.chars() {
            if character.is_whitespace() {
                gap = !preview.is_empty();
                continue;
            }
            if gap {
                preview.push(' ');
                count += 1;
                gap = false;
            }
            if count > SESSION_PREVIEW_LIMIT {
                return Some(truncate(preview, SESSION_PREVIEW_LIMIT));
            }
            preview.push(character);
            count += 1;
            if count > SESSION_PREVIEW_LIMIT {
                return Some(truncate(preview, SESSION_PREVIEW_LIMIT));
            }
        }
    }
    (!preview.is_empty()).then(|| truncate(preview, SESSION_PREVIEW_LIMIT))
}

fn truncate(text: String, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text;
    }
    let kept = text
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    format!("{}\u{2026}", kept.trim_end())
}

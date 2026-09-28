//! Typed bounded projection primitives shared by every Trace value.
//!
//! Two rules govern everything in this module.
//!
//! **A bound is explicit.** Every value that can grow — text, JSON, a block
//! list, a message list — carries a `truncated` flag beside it. Truncated
//! content never becomes indistinguishable from complete content, so a
//! reader can always tell "this tool returned nothing" apart from "this tool
//! returned more than Trace will carry".
//!
//! **A bound is not a security boundary.** Nothing here decides *what* may be
//! projected; that decision belongs to the typed allowlists in the
//! projection modules, which name every field they copy. These helpers only
//! decide *how much* of an already-permitted value crosses the boundary. In
//! particular there is no content scanning, no pattern matching for secrets,
//! and no key-name heuristic anywhere in Trace.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Longest one-line preview retained on a pageable summary record.
pub const TRACE_PREVIEW_BYTES: usize = 512;
/// Longest single text field retained in a detail response.
pub const TRACE_DETAIL_TEXT_BYTES: usize = 16 * 1024;
/// Most content blocks retained for one projected message.
pub const TRACE_DETAIL_BLOCKS: usize = 32;
/// Most reconstructed request messages retained in one request detail.
pub const TRACE_DETAIL_MESSAGES: usize = 64;
/// Most historical Tool definitions retained in one request detail.
pub const TRACE_DETAIL_TOOLS: usize = 64;
/// Deepest JSON nesting retained; deeper subtrees become an elision marker.
pub const TRACE_JSON_DEPTH: usize = 12;
/// Most JSON nodes retained across one bounded value.
pub const TRACE_JSON_NODES: usize = 1024;
/// Longest JSON string leaf retained.
pub const TRACE_JSON_STRING_BYTES: usize = 4 * 1024;
/// Encoded-byte ceiling of one pageable summary record.
pub const TRACE_RECORD_BYTES: usize = 8 * 1024;
/// Encoded-byte ceiling of one page's records.
pub const TRACE_PAGE_BYTES: usize = 128 * 1024;
/// Encoded-byte ceiling of one detail response, well inside the 1 MiB frame.
pub const TRACE_DETAIL_BYTES: usize = 512 * 1024;
/// Longest native identity retained; a longer one is omitted, never shortened.
pub const TRACE_IDENTITY_BYTES: usize = 512;
// A certified extension identity is bounded by its own contract, strictly
// below the Trace identity bound, so exact extension provenance always fits
// a summary row whole. Trace reuses that proof instead of adding a second
// bound that could truncate one identity into a different one.
const _: () =
    assert!(crate::runtime::identity::CertifiedExtensionIdentity::MAX_BYTES < TRACE_IDENTITY_BYTES);
/// Most canonical request Context facts carried by one request summary.
pub const TRACE_SUMMARY_CONTEXT: usize = 16;
/// Most artifact references retained beside one summarized Context fact.
pub const TRACE_SUMMARY_CONTEXT_ARTIFACTS: usize = 4;
/// Encoded-byte ceiling of one summary's complete Context presentation list.
///
/// Trace owns this bound. An upstream Context Assembly limit constrains how
/// much context a request may carry; it says nothing about how many encoded
/// bytes that context becomes once identities, previews and artifact
/// references are projected, so it cannot stand in for this ceiling.
pub const TRACE_SUMMARY_CONTEXT_BYTES: usize = 4 * 1024;

/// Bounded text with an explicit completeness statement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceText {
    pub text: String,
    pub truncated: bool,
}

impl TraceText {
    /// Retains at most `limit` UTF-8 bytes, cut on a character boundary.
    #[must_use]
    pub fn bounded(text: &str, limit: usize) -> Self {
        let mut end = text.len().min(limit);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            text: text[..end].to_owned(),
            truncated: end < text.len(),
        }
    }

    /// Retains a detail-sized field.
    #[must_use]
    pub fn detail(text: &str) -> Self {
        Self::bounded(text, TRACE_DETAIL_TEXT_BYTES)
    }

    /// Whether this value carries no characters at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// A one-line summary preview of longer content.
///
/// Whitespace is collapsed so a multi-paragraph message occupies one ledger
/// row. This is presentation shaping of already-permitted content, not a
/// second copy of it: the complete value is reached through detail.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracePreview {
    pub text: String,
    pub truncated: bool,
}

impl TracePreview {
    /// Collapses whitespace and bounds the result to one preview line.
    #[must_use]
    pub fn of(text: &str) -> Self {
        // Bound the scanned source first: collapsing a megabyte of text to
        // produce 512 bytes would make preview cost grow with content size.
        let source = TraceText::bounded(text, TRACE_PREVIEW_BYTES * 8);
        let collapsed = source.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let bounded = TraceText::bounded(&collapsed, TRACE_PREVIEW_BYTES);
        Self {
            text: bounded.text,
            truncated: bounded.truncated || source.truncated,
        }
    }

    /// Serialize only a bounded prefix of canonical JSON. Stop the serializer
    /// at the byte ceiling rather than allocating the complete arguments.
    #[must_use]
    pub fn of_json(value: &serde_json::Value) -> Self {
        struct Prefix(Vec<u8>);
        impl std::io::Write for Prefix {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                let kept = bytes
                    .len()
                    .min(TRACE_PREVIEW_BYTES.saturating_sub(self.0.len()));
                self.0.extend_from_slice(&bytes[..kept]);
                if kept < bytes.len() {
                    return Err(std::io::Error::other("preview bound"));
                }
                Ok(kept)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut prefix = Prefix(Vec::new());
        let truncated = serde_json::to_writer(&mut prefix, value).is_err();
        // Serialization can stop inside a UTF-8 character; retain a valid prefix.
        let valid = match std::str::from_utf8(&prefix.0) {
            Ok(text) => text.len(),
            Err(error) => error.valid_up_to(),
        };
        Self {
            text: String::from_utf8_lossy(&prefix.0[..valid]).into_owned(),
            truncated,
        }
    }

    /// Whether this preview carries no characters at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Bounded structured data with an explicit completeness statement.
///
/// The value stays real JSON rather than a rendered string, so the browser
/// can present it with a structured reader instead of a preformatted dump.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceJson {
    pub value: serde_json::Value,
    pub truncated: bool,
}

impl TraceJson {
    /// Bounds one already-permitted structured value.
    ///
    /// Depth, node count and string length are bounded independently,
    /// because each can blow up a response on its own: a deeply nested
    /// object, a wide array of tiny nodes, and a single enormous string are
    /// three different failure shapes.
    #[must_use]
    pub fn bounded(value: &serde_json::Value) -> Self {
        let mut budget = TRACE_JSON_NODES;
        let mut truncated = false;
        let bounded = bound_value(value, 0, &mut budget, &mut truncated);
        Self {
            value: bounded,
            truncated,
        }
    }
}

/// The marker substituted for a subtree beyond the depth or node budget.
///
/// A distinctive string keeps elision visible in the reader rather than
/// silently presenting a shortened object as the recorded one.
const ELIDED: &str = "[trace: elided]";

fn bound_value(
    value: &serde_json::Value,
    depth: usize,
    budget: &mut usize,
    truncated: &mut bool,
) -> serde_json::Value {
    if *budget == 0 {
        *truncated = true;
        return serde_json::Value::String(ELIDED.to_owned());
    }
    *budget -= 1;
    match value {
        serde_json::Value::String(text) => {
            let bounded = TraceText::bounded(text, TRACE_JSON_STRING_BYTES);
            *truncated |= bounded.truncated;
            serde_json::Value::String(bounded.text)
        }
        serde_json::Value::Array(items) => {
            if depth >= TRACE_JSON_DEPTH {
                *truncated = true;
                return serde_json::Value::String(ELIDED.to_owned());
            }
            let mut bounded = Vec::new();
            for item in items {
                if *budget == 0 {
                    *truncated = true;
                    break;
                }
                bounded.push(bound_value(item, depth + 1, budget, truncated));
            }
            serde_json::Value::Array(bounded)
        }
        serde_json::Value::Object(entries) => {
            if depth >= TRACE_JSON_DEPTH {
                *truncated = true;
                return serde_json::Value::String(ELIDED.to_owned());
            }
            let mut bounded = serde_json::Map::new();
            for (key, entry) in entries {
                if *budget == 0 {
                    *truncated = true;
                    break;
                }
                // Never shorten keys: two long names could collapse into
                // one and silently change the recorded structure.
                *budget -= 1;
                if key.len() > TRACE_JSON_STRING_BYTES {
                    *truncated = true;
                    continue;
                }
                bounded.insert(
                    key.clone(),
                    bound_value(entry, depth + 1, budget, truncated),
                );
            }
            serde_json::Value::Object(bounded)
        }
        other => other.clone(),
    }
}

/// Whether a native identity is short enough to cross the boundary intact.
///
/// An oversized identity is omitted whole. Shortening one would produce a
/// different identity that silently refers to nothing.
#[must_use]
pub fn identity_fits(identity: &str) -> bool {
    identity.len() <= TRACE_IDENTITY_BYTES
}

/// Encoded size of one serializable Trace value.
///
/// Measured after serialization because JSON escaping can multiply a bounded
/// string several times over; a character-count bound alone would not hold.
pub fn encoded_len<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

#[cfg(test)]
mod tests {
    use super::{
        TRACE_JSON_DEPTH, TRACE_JSON_NODES, TRACE_JSON_STRING_BYTES, TraceJson, TracePreview,
        TraceText, identity_fits,
    };

    /// JSON serialization stops at the byte bound and keeps a valid UTF-8 prefix.
    #[test]
    fn argument_preview_bounds_serialization_and_preserves_utf8() {
        for value in [
            serde_json::json!(null),
            serde_json::json!({}),
            serde_json::json!({"command": "ls"}),
        ] {
            let preview = super::TracePreview::of_json(&value);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&preview.text).unwrap(),
                value
            );
            assert!(!preview.truncated);
        }
        for value in [
            serde_json::json!("界".repeat(1000)),
            serde_json::json!("\n".repeat(1000)),
        ] {
            let preview = super::TracePreview::of_json(&value);
            assert!(preview.text.len() <= super::TRACE_PREVIEW_BYTES);
            assert!(preview.truncated);
        }
    }

    /// Text is cut on a character boundary, never inside a code point.
    #[test]
    fn bounded_text_cuts_on_character_boundaries() {
        let text = "é".repeat(10);
        let bounded = TraceText::bounded(&text, 5);
        assert!(bounded.truncated);
        assert_eq!(bounded.text, "éé");
    }

    /// Complete text is not marked truncated.
    #[test]
    fn complete_text_is_not_marked_truncated() {
        let bounded = TraceText::bounded("short", 64);
        assert!(!bounded.truncated);
        assert_eq!(bounded.text, "short");
    }

    /// A preview collapses whitespace into one line.
    #[test]
    fn preview_collapses_whitespace() {
        let preview = TracePreview::of("first\n\n  second\tthird ");
        assert_eq!(preview.text, "first second third");
        assert!(!preview.truncated);
    }

    /// An oversized preview source is reported as truncated.
    #[test]
    fn preview_reports_truncation() {
        let preview = TracePreview::of(&"x".repeat(4096));
        assert!(preview.truncated);
        assert!(preview.text.len() <= super::TRACE_PREVIEW_BYTES);
    }

    /// Nesting past the depth bound becomes a visible elision marker.
    #[test]
    fn json_depth_is_bounded_visibly() {
        let mut value = serde_json::json!("leaf");
        for _ in 0..(TRACE_JSON_DEPTH + 4) {
            value = serde_json::json!({ "next": value });
        }
        let bounded = TraceJson::bounded(&value);
        assert!(bounded.truncated);
        assert!(
            serde_json::to_string(&bounded.value)
                .expect("bounded JSON")
                .contains("[trace: elided]")
        );
    }

    /// A wide value is bounded by node count, not only by depth.
    #[test]
    fn json_node_count_is_bounded() {
        let value = serde_json::Value::Array(
            (0..(TRACE_JSON_NODES * 2))
                .map(|index| serde_json::json!(index))
                .collect(),
        );
        let bounded = TraceJson::bounded(&value);
        assert!(bounded.truncated);
        assert!(bounded.value.as_array().expect("array").len() < TRACE_JSON_NODES);
    }

    #[test]
    fn json_objects_stop_at_budget_and_do_not_alias_long_keys() {
        let value = serde_json::Value::Object(
            (0..TRACE_JSON_NODES * 10)
                .map(|index| (index.to_string(), serde_json::json!([index])))
                .collect(),
        );
        let bounded = TraceJson::bounded(&value);
        assert!(bounded.truncated);
        assert!(bounded.value.as_object().expect("object").len() < TRACE_JSON_NODES);
        let key = "x".repeat(TRACE_JSON_STRING_BYTES + 1);
        let value = serde_json::json!({key: 1, "kept": 2});
        let bounded = TraceJson::bounded(&value);
        assert!(bounded.truncated);
        assert_eq!(bounded.value, serde_json::json!({"kept": 2}));
    }

    /// A single enormous string leaf is bounded on its own.
    #[test]
    fn json_string_leaves_are_bounded() {
        let value = serde_json::json!({ "code": "a".repeat(TRACE_JSON_STRING_BYTES * 2) });
        let bounded = TraceJson::bounded(&value);
        assert!(bounded.truncated);
        let code = bounded.value["code"].as_str().expect("string leaf");
        assert!(code.len() <= TRACE_JSON_STRING_BYTES);
    }

    /// A small structured value crosses intact and unmarked.
    #[test]
    fn small_json_is_complete() {
        let value = serde_json::json!({ "path": "src/main.rs", "limit": 20 });
        let bounded = TraceJson::bounded(&value);
        assert!(!bounded.truncated);
        assert_eq!(bounded.value, value);
    }

    /// Identity admission is a length decision, never a rewriting one.
    #[test]
    fn oversized_identities_are_rejected_not_shortened() {
        assert!(identity_fits("call-1"));
        assert!(!identity_fits(&"c".repeat(super::TRACE_IDENTITY_BYTES + 1)));
    }
}

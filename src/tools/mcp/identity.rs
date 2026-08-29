//! The rustX-owned deterministic cross-process identity of one canonical
//! MCP Tool definition (Issue #145).
//!
//! # Why a rustX-owned identity exists
//!
//! Inside one process an MCP catalog is stabilized by the invalidation
//! **epoch**: preparation snapshots the epoch, lists the catalog, and
//! re-reads the epoch, so a `tools/list_changed` racing discovery is
//! rejected. That mechanism is process-local by construction — the epoch
//! counter lives in one `McpInvalidationState` and means nothing to another
//! OS process.
//!
//! A subagent child is a separate OS process that owns its own MCP
//! transport, its own connection, and its own catalog read. It therefore
//! needs a **semantic** identity it can recompute independently and compare
//! against what the parent froze:
//!
//! ```text
//! parent generation      child process
//!   tools/list             tools/list          (its own connection)
//!   canonical identity ->  canonical identity
//!                    equal ? materialize : fail before ownership commit
//! ```
//!
//! # What the identity covers
//!
//! Exactly the fields that change the model-facing Tool contract or the
//! runtime's invocation semantics of that contract:
//!
//! ```text
//! MCP_TOOL_IDENTITY_V1
//!   + server_id
//!   + canonical tool name
//!   + description
//!   + canonical(input_schema)
//!   + effective execution policy   (execution / concurrency / approval)
//! ```
//!
//! The execution policy participates because it is part of the semantics
//! rustX authorizes: a server whose tools were admitted as
//! approval-required must not silently become approval-free in a child.
//! `ToolId` deliberately does **not** participate: it is derived from
//! `(server_id, name)` and would only restate them.
//!
//! # Canonical JSON is rustX-owned
//!
//! The digest never hashes a Rust in-memory representation and never
//! depends on `serde_json`'s map implementation, its feature flags, or the
//! order keys happened to be inserted in. [`canonical_json`] writes an
//! explicit canonical form:
//!
//! - objects: keys sorted by Unicode scalar order, recursively;
//! - arrays: order preserved (array order is semantic in JSON Schema);
//! - strings: JSON-escaped by one rustX-owned escaper;
//! - numbers: integers as their exact decimal form, non-integers as Rust's
//!   shortest round-trip decimal form;
//! - `true` / `false` / `null` as their literals.
//!
//! The result is a byte string, so a digest computed by a parent build and
//! by a child build of the same rustX version are equal exactly when the
//! Tool contract is equal.

use sha2::Digest;

use crate::runtime::identity::{McpServerId, McpToolIdentity};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolOrigin,
};

/// The versioned domain separator of the canonical MCP Tool identity.
///
/// It is part of the hashed input, so a future revision of the covered
/// field set can never collide with a `V1` digest.
pub const MCP_TOOL_IDENTITY_DOMAIN: &str = "MCP_TOOL_IDENTITY_V1";

/// Derives the deterministic cross-process identity of one canonical MCP
/// Tool definition.
///
/// The inputs are exactly the semantic contract fields; see the module
/// documentation for why each participates.
#[must_use]
pub fn mcp_tool_identity(
    server_id: &McpServerId,
    name: &str,
    description: &str,
    input_schema: &serde_json::Value,
    execution: ToolExecutionPolicy,
    concurrency: ToolConcurrencyPolicy,
    approval: ToolApprovalPolicy,
) -> McpToolIdentity {
    let mut hasher = sha2::Sha256::new();
    // Every component is length-prefixed, so no concatenation of two
    // components can be confused with a different split of the same bytes.
    let mut field = |value: &str| {
        hasher.update(value.len().to_le_bytes());
        hasher.update(value.as_bytes());
    };
    field(MCP_TOOL_IDENTITY_DOMAIN);
    field(server_id.as_str());
    field(name);
    field(description);
    field(&canonical_json(input_schema));
    field(execution_token(execution));
    field(concurrency_token(concurrency));
    field(approval_token(approval));
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(71);
    hex.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    McpToolIdentity::new(hex)
}

/// Derives the canonical identity of an already-composed MCP
/// [`ToolDefinition`].
///
/// Returns `None` for a definition whose origin is not MCP: a non-MCP
/// definition has no server identity and therefore no MCP Tool identity.
#[must_use]
pub fn definition_identity(definition: &ToolDefinition) -> Option<McpToolIdentity> {
    let ToolOrigin::Mcp { server_id } = &definition.origin else {
        return None;
    };
    Some(mcp_tool_identity(
        server_id,
        &definition.name,
        &definition.description,
        &definition.input_schema,
        definition.execution_policy,
        definition.concurrency_policy,
        definition.approval_policy,
    ))
}

const fn execution_token(policy: ToolExecutionPolicy) -> &'static str {
    match policy {
        ToolExecutionPolicy::ForegroundOnly => "execution=foreground_only",
        ToolExecutionPolicy::BackgroundOnly => "execution=background_only",
        ToolExecutionPolicy::ModelSelectable => "execution=model_selectable",
    }
}

const fn concurrency_token(policy: ToolConcurrencyPolicy) -> &'static str {
    match policy {
        ToolConcurrencyPolicy::Sequential => "concurrency=sequential",
        ToolConcurrencyPolicy::Parallel => "concurrency=parallel",
    }
}

const fn approval_token(policy: ToolApprovalPolicy) -> &'static str {
    match policy {
        ToolApprovalPolicy::Never => "approval=never",
        ToolApprovalPolicy::Always => "approval=always",
    }
}

/// Serializes one JSON value into the rustX canonical form.
///
/// See the module documentation for the exact rules. The function is
/// deliberately independent of `serde_json::to_string`: it never consults
/// the map implementation's iteration order and never relies on a crate
/// feature flag remaining unset.
#[must_use]
pub fn canonical_json(value: &serde_json::Value) -> String {
    let mut output = String::new();
    write_canonical(value, &mut output);
    output
}

fn write_canonical(value: &serde_json::Value, output: &mut String) {
    match value {
        serde_json::Value::Null => output.push_str("null"),
        serde_json::Value::Bool(true) => output.push_str("true"),
        serde_json::Value::Bool(false) => output.push_str("false"),
        serde_json::Value::Number(number) => write_number(number, output),
        serde_json::Value::String(text) => write_string(text, output),
        serde_json::Value::Array(items) => {
            // Array order is semantic in JSON Schema (`prefixItems`,
            // `enum`, `required`), so it is preserved exactly.
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical(item, output);
            }
            output.push(']');
        }
        serde_json::Value::Object(entries) => {
            // The one place insertion order is deliberately discarded: keys
            // are sorted here, by this function, rather than trusted to
            // arrive sorted from the map implementation.
            let mut keys: Vec<&String> = entries.keys().collect();
            keys.sort_unstable();
            output.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_string(key, output);
                output.push(':');
                write_canonical(&entries[key], output);
            }
            output.push('}');
        }
    }
}

/// Writes one JSON number canonically: an exact integer keeps its decimal
/// form; every other number uses Rust's shortest round-trip decimal form,
/// which is a deterministic function of the `f64` bit pattern.
fn write_number(number: &serde_json::Number, output: &mut String) {
    use std::fmt::Write as _;
    if let Some(value) = number.as_u64() {
        let _ = write!(output, "{value}");
    } else if let Some(value) = number.as_i64() {
        let _ = write!(output, "{value}");
    } else if let Some(value) = number.as_f64() {
        // `{:?}` is the shortest representation that round-trips; a
        // non-finite value cannot exist in a `serde_json::Number`.
        let _ = write!(output, "{value:?}");
    } else {
        // Unreachable for any value a JSON document can hold; fail closed
        // to a stable token rather than panicking inside a digest.
        output.push_str("\"<unrepresentable-number>\"");
    }
}

/// Writes one JSON string with rustX-owned escaping: the two mandatory
/// escapes, the five short escapes, and `\u00XX` for every other control
/// character. Non-ASCII scalar values are emitted verbatim as UTF-8.
fn write_string(value: &str, output: &mut String) {
    use std::fmt::Write as _;
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{8}' => output.push_str("\\b"),
            '\u{c}' => output.push_str("\\f"),
            control if control < '\u{20}' => {
                let _ = write!(output, "\\u{:04x}", control as u32);
            }
            other => output.push(other),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::{canonical_json, mcp_tool_identity};
    use crate::runtime::identity::McpServerId;
    use crate::tools::types::{ToolApprovalPolicy, ToolConcurrencyPolicy, ToolExecutionPolicy};

    fn identity(schema: &serde_json::Value) -> String {
        mcp_tool_identity(
            &McpServerId::new("github"),
            "get_issue",
            "Reads one issue.",
            schema,
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
            ToolApprovalPolicy::Never,
        )
        .into_string()
    }

    /// The canonical form sorts object keys recursively, so two documents
    /// that differ only in insertion order canonicalize identically.
    #[test]
    fn object_key_insertion_order_does_not_change_the_canonical_form() {
        let mut first = serde_json::Map::new();
        first.insert("zebra".to_owned(), serde_json::json!(1));
        first.insert("alpha".to_owned(), serde_json::json!({"y": 2, "x": 1}));
        let mut second = serde_json::Map::new();
        second.insert("alpha".to_owned(), serde_json::json!({"x": 1, "y": 2}));
        second.insert("zebra".to_owned(), serde_json::json!(1));

        let first = serde_json::Value::Object(first);
        let second = serde_json::Value::Object(second);
        assert_eq!(canonical_json(&first), canonical_json(&second));
        assert_eq!(
            canonical_json(&first),
            r#"{"alpha":{"x":1,"y":2},"zebra":1}"#
        );
        assert_eq!(identity(&first), identity(&second));
    }

    /// Array order is semantic and is never sorted.
    #[test]
    fn array_order_is_preserved_and_changes_the_identity() {
        let ascending = serde_json::json!({"required": ["a", "b"]});
        let descending = serde_json::json!({"required": ["b", "a"]});
        assert_eq!(canonical_json(&ascending), r#"{"required":["a","b"]}"#);
        assert_ne!(identity(&ascending), identity(&descending));
    }

    /// Every semantic field participates: changing any one of them changes
    /// the digest.
    #[test]
    fn every_semantic_field_participates() {
        let schema = serde_json::json!({"type": "object"});
        let base = mcp_tool_identity(
            &McpServerId::new("github"),
            "get_issue",
            "Reads one issue.",
            &schema,
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
            ToolApprovalPolicy::Never,
        );
        let variants = [
            mcp_tool_identity(
                &McpServerId::new("gitlab"),
                "get_issue",
                "Reads one issue.",
                &schema,
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                ToolApprovalPolicy::Never,
            ),
            mcp_tool_identity(
                &McpServerId::new("github"),
                "get_issues",
                "Reads one issue.",
                &schema,
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                ToolApprovalPolicy::Never,
            ),
            mcp_tool_identity(
                &McpServerId::new("github"),
                "get_issue",
                "Reads one issue!",
                &schema,
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                ToolApprovalPolicy::Never,
            ),
            mcp_tool_identity(
                &McpServerId::new("github"),
                "get_issue",
                "Reads one issue.",
                &serde_json::json!({"type": "array"}),
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                ToolApprovalPolicy::Never,
            ),
            mcp_tool_identity(
                &McpServerId::new("github"),
                "get_issue",
                "Reads one issue.",
                &schema,
                ToolExecutionPolicy::BackgroundOnly,
                ToolConcurrencyPolicy::Sequential,
                ToolApprovalPolicy::Never,
            ),
            mcp_tool_identity(
                &McpServerId::new("github"),
                "get_issue",
                "Reads one issue.",
                &schema,
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Parallel,
                ToolApprovalPolicy::Never,
            ),
            mcp_tool_identity(
                &McpServerId::new("github"),
                "get_issue",
                "Reads one issue.",
                &schema,
                ToolExecutionPolicy::ForegroundOnly,
                ToolConcurrencyPolicy::Sequential,
                ToolApprovalPolicy::Always,
            ),
        ];
        for variant in variants {
            assert_ne!(base, variant, "each semantic field must change the digest");
        }
    }

    /// Field boundaries are length-prefixed, so moving a character across a
    /// boundary changes the digest.
    #[test]
    fn field_boundaries_are_unambiguous() {
        let schema = serde_json::json!({});
        let left = mcp_tool_identity(
            &McpServerId::new("ab"),
            "c",
            "",
            &schema,
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
            ToolApprovalPolicy::Never,
        );
        let right = mcp_tool_identity(
            &McpServerId::new("a"),
            "bc",
            "",
            &schema,
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
            ToolApprovalPolicy::Never,
        );
        assert_ne!(left, right);
    }

    /// Strings are escaped by the rustX escaper, and non-ASCII scalars stay
    /// verbatim UTF-8.
    #[test]
    fn strings_are_escaped_canonically() {
        assert_eq!(
            canonical_json(&serde_json::json!("a\"b\\c\nd\te\u{1}f")),
            "\"a\\\"b\\\\c\\nd\\te\\u0001f\""
        );
        assert_eq!(canonical_json(&serde_json::json!("界🙂")), "\"界🙂\"");
    }

    /// Numbers use the exact integer form when the value is an integer and
    /// the shortest round-trip form otherwise.
    #[test]
    fn numbers_are_canonical() {
        assert_eq!(canonical_json(&serde_json::json!(0)), "0");
        assert_eq!(canonical_json(&serde_json::json!(-7)), "-7");
        assert_eq!(
            canonical_json(&serde_json::json!(18_446_744_073_709_551_615u64)),
            "18446744073709551615"
        );
        assert_eq!(canonical_json(&serde_json::json!(1.5)), "1.5");
        assert_eq!(canonical_json(&serde_json::json!(true)), "true");
        assert_eq!(canonical_json(&serde_json::Value::Null), "null");
    }

    /// The canonical form is stable across a serialize/parse round trip:
    /// re-parsing a canonical document canonicalizes to the same bytes.
    #[test]
    fn canonicalization_is_idempotent_across_a_round_trip() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"b": {"type": "string"}, "a": {"type": "number"}},
            "required": ["b", "a"],
            "additionalProperties": false
        });
        let canonical = canonical_json(&schema);
        let reparsed: serde_json::Value = serde_json::from_str(&canonical).expect("canonical JSON");
        assert_eq!(canonical_json(&reparsed), canonical);
    }
}

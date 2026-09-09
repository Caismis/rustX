//! The hand-editable configuration document format.
//!
//! `models.jsonc` and `rustx.jsonc` are JSONC: ordinary JSON plus `//` and
//! `/* */` comments and trailing commas, exactly the dialect editors already
//! understand for `tsconfig.json` and VS Code settings. A human owns these
//! files, so the format has to carry the reasoning behind a value next to the
//! value itself.
//!
//! Nothing else is relaxed. Single-quoted strings, unquoted property names,
//! hexadecimal numbers, unary plus, and missing commas stay rejected: a
//! configuration typo must fail startup loudly rather than parse into
//! something the author did not write. Every schema, default, and
//! `deny_unknown_fields` rule remains serde-owned; this module only chooses
//! the surface syntax the deserializer reads.
//!
//! Runtime-owned generated state under `runtime-root` is deliberately not
//! affected: nothing writes JSONC, and generated documents stay strict JSON.

use jsonc_parser::ParseOptions;
use jsonc_parser::errors::ParseErrorKind;
use serde::de::DeserializeOwned;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
pub(crate) fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    if !std::fs::metadata(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .is_file()
    {
        return Err(format!("{} must be a regular file", path.display()));
    }
    // A regular file replaced by a FIFO between metadata and open must not block.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err(format!("{} must be a regular file", path.display()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(format!("{} exceeds 1 MiB", path.display()));
    }
    Ok(bytes)
}

/// The accepted configuration dialect: JSON, comments, and trailing commas.
const OPTIONS: ParseOptions = ParseOptions {
    allow_comments: true,
    allow_trailing_commas: true,
    allow_loose_object_property_names: false,
    allow_missing_commas: false,
    allow_single_quoted_strings: false,
    allow_hexadecimal_numbers: false,
    allow_unary_plus_numbers: false,
};

/// Deserializes one configuration document from JSONC bytes.
///
/// # Errors
///
/// Returns a human-readable detail for non-UTF-8 input, for a syntax failure
/// — including a rejected relaxation such as a single-quoted string — and for
/// every schema failure serde reports. A syntax detail carries the line and
/// column the failure was detected on; a schema detail carries serde's own
/// message, because the reported position of a schema failure is the
/// enclosing container rather than the offending member.
pub fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    parse_detailed(bytes).map_err(|failure| failure.detail)
}

/// Parser-owned position and classification; callers never parse an error string.
pub struct ParseFailure {
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub syntax: bool,
    detail: String,
}

impl ParseFailure {
    pub(crate) fn into_detail(self) -> String {
        self.detail
    }
}

impl std::fmt::Debug for ParseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParseFailure")
            .field("line", &self.line)
            .field("column", &self.column)
            .field("syntax", &self.syntax)
            .finish_non_exhaustive()
    }
}

/// The same JSONC deserializer with structured, safe parser context.
///
/// # Errors
/// Returns syntax location when reliable, otherwise a shape failure.
pub fn parse_detailed<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ParseFailure> {
    let text = std::str::from_utf8(bytes).map_err(|error| ParseFailure {
        line: None,
        column: None,
        syntax: true,
        detail: format!("not valid UTF-8: {error}"),
    })?;
    jsonc_parser::parse_to_serde_value(text, &OPTIONS).map_err(|error| match error.kind() {
        ParseErrorKind::Custom(message) => ParseFailure {
            line: None,
            column: None,
            syntax: false,
            detail: message.clone(),
        },
        _ => ParseFailure {
            line: Some(error.line_display()),
            column: Some(error.column_display()),
            syntax: true,
            detail: error.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;

    use super::parse;

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Document {
        name: String,
        count: u32,
        #[serde(default)]
        entries: BTreeMap<String, String>,
    }

    #[test]
    fn comments_and_trailing_commas_are_accepted() {
        let document: Document = parse(
            br#"{
              // The line comment form.
              "name": "example",
              /* The block comment form,
                 spanning lines. */
              "count": 7,
              "entries": {
                "a": "1", // trailing comma below
              },
            }"#,
        )
        .expect("JSONC parses");

        assert_eq!(
            document,
            Document {
                name: "example".to_owned(),
                count: 7,
                entries: BTreeMap::from([("a".to_owned(), "1".to_owned())]),
            }
        );
    }

    #[test]
    fn plain_json_remains_valid() {
        let document: Document =
            parse(br#"{"name": "example", "count": 1}"#).expect("JSON is JSONC");
        assert_eq!(document.name, "example");
        assert_eq!(document.count, 1);
        assert!(document.entries.is_empty());
    }

    #[test]
    fn relaxations_beyond_jsonc_stay_rejected() {
        for source in [
            br#"{'name': "example", "count": 1}"#.as_slice(),
            br#"{name: "example", "count": 1}"#.as_slice(),
            br#"{"name": "example" "count": 1}"#.as_slice(),
            br#"{"name": "example", "count": 0x01}"#.as_slice(),
            br#"{"name": "example", "count": +1}"#.as_slice(),
        ] {
            let error = parse::<Document>(source).expect_err("must fail");
            assert!(
                !error.is_empty(),
                "{} must report a syntax failure",
                String::from_utf8_lossy(source)
            );
        }
    }

    #[test]
    fn a_schema_failure_reports_serde_own_message() {
        let error = parse::<Document>(
            br#"{
              "name": "example",
              "count": 1,
              "typo": true
            }"#,
        )
        .expect_err("unknown field must fail");
        assert_eq!(
            error,
            "unknown field `typo`, expected one of `name`, `count`, `entries`"
        );
    }

    #[test]
    fn an_unterminated_comment_reports_its_position() {
        let error = parse::<Document>(
            br#"{
              "name": "example",
              "count": 1
            } /* never closed"#,
        )
        .expect_err("unterminated comment must fail");
        assert!(error.contains("line 4"), "{error}");
    }

    #[test]
    fn non_utf8_input_fails_before_parsing() {
        let error = parse::<Document>(&[0xff, 0xfe, 0x00]).expect_err("must fail");
        assert!(error.contains("UTF-8"), "{error}");
    }
}

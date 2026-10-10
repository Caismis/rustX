//! Typed TOML source parsing, source writing, and structured authoring
//! diagnostics.
//!
//! Source TOML and client-facing JSON share the authoring document types. They
//! differ in exactly one representation: provider-native `request_params`.
//!
//! ```text
//! source TOML      request_params = '{"temperature":0.7}'   (JSON-encoded string)
//! client JSON      "request_params": {"temperature":0.7}    (structured object)
//! ```
//!
//! The source representation exists only inside the source context entered by
//! [`parse_detailed`], [`write`], and [`source_schema`]. Every other
//! (de)serialization of an authoring type — App Server projections, mutations,
//! and generated protocol schemas — uses the structured object. The JSON string
//! is parsed here, once, into the structured
//! [`RequestParams`](crate::model::invocation::RequestParams) map; it never
//! reaches validation, runtime resolution, frozen snapshots, or adapters.

use std::cell::Cell;

use serde::de::{DeserializeOwned, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

use crate::model::invocation::{RequestParams, exact_json_number, numbers_are_exact};

thread_local! {
    static SOURCE: Cell<bool> = const { Cell::new(false) };
}

/// The source context of one parse, write, or schema generation. Restores the
/// previous state on drop, so nested use is well defined.
struct SourceContext(bool);
impl SourceContext {
    fn enter() -> Self {
        Self(SOURCE.replace(true))
    }
}
impl Drop for SourceContext {
    fn drop(&mut self) {
        SOURCE.set(self.0);
    }
}
fn in_source() -> bool {
    SOURCE.get()
}

/// Parse TOML directly into a typed authoring document.
///
/// # Errors
/// Rejects malformed input and any shape rejected by the authoring type.
pub fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    parse_detailed(bytes).map_err(|error| error.detail)
}

/// Serialize an authoring document as canonical source TOML.
///
/// This is the only native source writer: `request_params` fields are emitted
/// as JSON-encoded strings, so every written document parses again through
/// [`parse`] with identical semantics.
///
/// # Errors
/// Returns the TOML serializer failure.
pub fn write<T: Serialize + ?Sized>(document: &T) -> Result<String, toml::ser::Error> {
    let _source = SourceContext::enter();
    toml::to_string_pretty(document)
}

/// The JSON Schema of an authoring document's source TOML representation.
#[must_use]
pub fn source_schema<T: schemars::JsonSchema>() -> schemars::Schema {
    let _source = SourceContext::enter();
    schemars::schema_for!(T)
}

/// Parser-owned location. Debug deliberately excludes authored values.
pub struct ParseFailure {
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub syntax: bool,
    /// The exact authoring path of an invalid `request_params` field; never
    /// authored values.
    pub path: Option<String>,
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

/// Deserialize directly; the syntax-only CST check classifies failures without
/// introducing a dynamic configuration value tree into semantic composition.
///
/// # Errors
/// Returns a safe location and private detailed parser error.
pub fn parse_detailed<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ParseFailure> {
    let text = std::str::from_utf8(bytes).map_err(|error| ParseFailure {
        line: None,
        column: None,
        syntax: true,
        path: None,
        detail: format!("not valid UTF-8: {error}"),
    })?;
    let _source = SourceContext::enter();
    serde_path_to_error::deserialize(toml::Deserializer::new(text)).map_err(|failure| {
        let path = failure.path().to_string();
        let error = failure.into_inner();
        let mut parameter_path = None;
        let message = if let Some(reason) = error.message().strip_prefix(PARAM_ERROR) {
            let path = path.trim_end_matches('.');
            let owner = if path.ends_with("request_params") {
                path.to_owned()
            } else if path.is_empty() {
                "request_params".to_owned()
            } else {
                // Internally tagged summary enums buffer their variant fields;
                // serde_path_to_error then ends at the summary model itself.
                format!("{path}.request_params")
            };
            let message = format!("{owner} {reason}");
            parameter_path = Some(owner);
            message
        } else {
            error.message().to_owned()
        };
        let prefix = error.span().and_then(|span| text.get(..span.start));
        let line = prefix.map(|s| s.bytes().filter(|b| *b == b'\n').count() + 1);
        let column = prefix.map(|s| s.rsplit('\n').next().unwrap_or_default().chars().count() + 1);
        ParseFailure {
            line,
            column,
            syntax: text.parse::<toml_edit::DocumentMut>().is_err(),
            path: parameter_path,
            // Do not include TOML's source excerpt: it may contain credentials
            // or opaque provider parameters beside the malformed token.
            detail: format!(
                "TOML error at line {}, column {}: {message}",
                line.unwrap_or(1),
                column.unwrap_or(1),
            ),
        }
    })
}

/// Marks a `request_params` failure; the parser joins the authoring path.
/// Reasons name structure and locations only, never authored values.
const PARAM_ERROR: &str = "\u{0}request_params: ";

/// Provider-native request parameters of one authoring document field.
///
/// Source TOML authors a JSON-encoded string; every client-facing JSON
/// projection carries the structured object. Both representations hold the
/// same structured [`RequestParams`] value.
#[derive(Clone, Default, PartialEq)]
pub struct AuthoredRequestParams(pub RequestParams);
impl std::fmt::Debug for AuthoredRequestParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthoredRequestParams(<opaque>)")
    }
}
/// Why structured parameters are refused at either representation boundary.
/// Names the domain, never the value or its key.
const INEXACT: &str =
    "has a JSON number that is not exactly representable as an IEEE 754 binary64 value";

impl Serialize for AuthoredRequestParams {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Every representation carries only numbers every hop reads alike, so
        // a native write can never emit a source its parser would refuse.
        if !numbers_are_exact(&self.0) {
            return Err(serde::ser::Error::custom(format!(
                "request_params {INEXACT}"
            )));
        }
        if in_source() {
            let text = serde_json::to_string(&self.0).map_err(serde::ser::Error::custom)?;
            serializer.serialize_str(&text)
        } else {
            self.0.serialize(serializer)
        }
    }
}
impl<'de> Deserialize<'de> for AuthoredRequestParams {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if in_source() {
            let text = deserializer.deserialize_any(JsonText)?;
            parse_request_params_json(&text)
                .map(Self)
                .map_err(|error| serde::de::Error::custom(format!("{PARAM_ERROR}{error}")))
        } else {
            // Client JSON is decoded by `serde_json`, exactly for integers; a
            // number outside the domain is refused here, before any write.
            let params = RequestParams::deserialize(deserializer)?;
            if numbers_are_exact(&params) {
                Ok(Self(params))
            } else {
                Err(serde::de::Error::custom(format!(
                    "request_params {INEXACT}"
                )))
            }
        }
    }
}
impl schemars::JsonSchema for AuthoredRequestParams {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        if in_source() {
            "RequestParamsJson".into()
        } else {
            "RequestParams".into()
        }
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        if in_source() {
            schemars::json_schema!({
                "type": "string",
                "description": "A JSON-encoded object of opaque provider-native request parameters, for example '{\"temperature\":0.7}'. Nested objects, arrays and null are preserved; duplicate keys are rejected, and so is a number that is not exactly an IEEE 754 binary64 value (such as 9007199254740993). Runtime-protected wire keys are checked during model validation."
            })
        } else {
            schemars::json_schema!({
                "type": "object",
                "description": "Opaque provider-native request parameters as a structured JSON object. Nested objects, arrays and null are preserved. Every number is exactly an IEEE 754 binary64 value (I-JSON); another, such as 9007199254740993, is refused. Runtime-protected wire keys are checked during model validation.",
                "additionalProperties": true
            })
        }
    }
}

/// Accepts only a string; every other source shape is rejected without
/// echoing its value.
struct JsonText;
macro_rules! reject_non_string {
    ($($method:ident($($arg:ty)?)),* $(,)?) => {$(
        fn $method<E: serde::de::Error>(self $(, _: $arg)?) -> Result<String, E> {
            Err(E::custom(format!("{PARAM_ERROR}must be a JSON-encoded string")))
        }
    )*};
}
impl<'de> Visitor<'de> for JsonText {
    type Value = String;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON-encoded string")
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
        Ok(value.to_owned())
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<String, E> {
        Ok(value)
    }
    reject_non_string!(
        visit_bool(bool),
        visit_i64(i64),
        visit_u64(u64),
        visit_i128(i128),
        visit_u128(u128),
        visit_f64(f64),
        visit_bytes(&[u8]),
        visit_unit(),
        visit_none(),
    );
    fn visit_seq<A: SeqAccess<'de>>(self, _: A) -> Result<String, A::Error> {
        Err(serde::de::Error::custom(format!(
            "{PARAM_ERROR}must be a JSON-encoded string"
        )))
    }
    fn visit_map<A: MapAccess<'de>>(self, _: A) -> Result<String, A::Error> {
        Err(serde::de::Error::custom(format!(
            "{PARAM_ERROR}must be a JSON-encoded string"
        )))
    }
}

/// Why an authored JSON request-parameter string was rejected.
///
/// Every variant locates the failure by line and column of the JSON text. No
/// variant carries authored text: object keys are as opaque as values, and a
/// key may itself hold a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestParamsJsonError {
    /// The text is not one JSON value.
    Syntax { line: usize, column: usize },
    /// The JSON value is not an object.
    NotObject,
    /// An object, at any depth, repeats a key; located just after the repeat.
    DuplicateKey { line: usize, column: usize },
    /// A number does not survive a binary64 round trip; located at its start.
    /// Columns count bytes, as `serde_json` locates the other variants.
    /// See [`exact_json_number`].
    InexactNumber { line: usize, column: usize },
}
impl std::fmt::Display for RequestParamsJsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let at = |f: &mut std::fmt::Formatter<'_>, line, column| {
            write!(f, " (at line {line}, column {column} of the JSON text)")
        };
        match *self {
            Self::Syntax { line, column } => {
                f.write_str("is not valid JSON")?;
                at(f, line, column)
            }
            Self::NotObject => f.write_str("must encode a JSON object"),
            Self::DuplicateKey { line, column } => {
                f.write_str("repeats a JSON object key")?;
                at(f, line, column)
            }
            Self::InexactNumber { line, column } => {
                f.write_str(INEXACT)?;
                at(f, line, column)
            }
        }
    }
}
impl std::error::Error for RequestParamsJsonError {}

/// Strictly parse one authored JSON request-parameter object.
///
/// Unlike ordinary JSON map deserialization, a repeated key at any depth is an
/// error rather than a silent last-value-wins, and a number some hop would
/// round is an error rather than a silent precision loss.
///
/// # Errors
/// Returns the syntax location, a non-object root, the first duplicate key, or
/// the first inexact number.
pub fn parse_request_params_json(text: &str) -> Result<RequestParams, RequestParamsJsonError> {
    let duplicate = Cell::new(false);
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = StrictValue {
        duplicate: &duplicate,
    }
    .deserialize(&mut deserializer)
    .and_then(|value| deserializer.end().map(|()| value))
    .map_err(|error| {
        let (line, column) = (error.line(), error.column());
        if duplicate.get() {
            RequestParamsJsonError::DuplicateKey { line, column }
        } else {
            RequestParamsJsonError::Syntax { line, column }
        }
    })?;
    let serde_json::Value::Object(map) = value else {
        return Err(RequestParamsJsonError::NotObject);
    };
    if let Some(start) = number_literals(text)
        .find(|&(start, end)| !exact_json_number(&text[start..end]))
        .map(|(start, _)| start)
    {
        let before = &text[..start];
        return Err(RequestParamsJsonError::InexactNumber {
            line: before.matches('\n').count() + 1,
            column: before.len() - before.rfind('\n').map_or(0, |newline| newline + 1) + 1,
        });
    }
    Ok(map)
}

/// The byte ranges of every number literal of already valid JSON text.
fn number_literals(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let bytes = text.as_bytes();
    let mut at = 0;
    std::iter::from_fn(move || {
        while at < bytes.len() {
            match bytes[at] {
                b'"' => {
                    at += 1;
                    while bytes[at] != b'"' {
                        at += if bytes[at] == b'\\' { 2 } else { 1 };
                    }
                    at += 1;
                }
                b'-' | b'0'..=b'9' => {
                    let start = at;
                    while at < bytes.len()
                        && matches!(bytes[at], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                    {
                        at += 1;
                    }
                    return Some((start, at));
                }
                _ => at += 1,
            }
        }
        None
    })
}

/// One JSON value whose objects reject repeated keys. `serde_json` bounds the
/// recursion depth.
struct StrictValue<'a> {
    duplicate: &'a Cell<bool>,
}
impl<'de> DeserializeSeed<'de> for StrictValue<'_> {
    type Value = serde_json::Value;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for StrictValue<'_> {
    type Value = serde_json::Value;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(value.into())
    }
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(value.into())
    }
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(value.into())
    }
    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
        serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(value.into())
    }
    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(value.into())
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(StrictValue {
            duplicate: self.duplicate,
        })? {
            values.push(value);
        }
        Ok(serde_json::Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut object = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                self.duplicate.set(true);
                return Err(serde::de::Error::custom("duplicate key"));
            }
            let value = map.next_value_seed(StrictValue {
                duplicate: self.duplicate,
            })?;
            object.insert(key, value);
        }
        Ok(serde_json::Value::Object(object))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Deserialize, Serialize, Debug, PartialEq, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct Document {
        model: Params,
    }
    #[derive(Deserialize, Serialize, Debug, PartialEq, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct Params {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_params: Option<AuthoredRequestParams>,
    }
    fn params(value: serde_json::Value) -> RequestParams {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn json_strings_parse_to_structured_objects_independent_of_formatting() {
        let expected = params(json!({
            "temperature": 0.7, "flag": false, "text": "exact", "integer": -42,
            "explicit": null,
            "provider": {"order": ["a", "b"], "fallback": null},
            "documents": [{"title": "A", "media": [1, true, "x", null]}, []]
        }));
        for form in [
            r#"request_params = '{"temperature":0.7,"flag":false,"text":"exact","integer":-42,"explicit":null,"provider":{"order":["a","b"],"fallback":null},"documents":[{"title":"A","media":[1,true,"x",null]},[]]}'"#,
            "request_params = '''\n{\n  \"documents\": [{\"media\": [1, true, \"x\", null], \"title\": \"A\"}, []],\n  \"provider\": {\"fallback\": null, \"order\": [\"a\", \"b\"]},\n  \"explicit\": null, \"integer\": -42, \"text\": \"exact\",\n  \"flag\": false, \"temperature\": 0.7\n}\n'''",
            r#"request_params = "{\"integer\":-42,\"text\":\"exact\",\"explicit\":null,\"flag\":false,\"temperature\":0.7,\"provider\":{\"order\":[\"a\",\"b\"],\"fallback\":null},\"documents\":[{\"title\":\"A\",\"media\":[1,true,\"x\",null]},[]]}""#,
        ] {
            let document: Document = parse(format!("[model]\n{form}").as_bytes()).unwrap();
            assert_eq!(document.model.request_params.unwrap().0, expected, "{form}");
        }
    }

    #[test]
    fn source_writes_json_strings_including_null_and_reread_exactly() {
        let document = Document {
            model: Params {
                request_params: Some(AuthoredRequestParams(params(json!({
                    "explicit": null, "nested": {"a": [null, {"b": null}]}, "quote": "it's"
                })))),
            },
        };
        let text = write(&document).unwrap();
        assert!(!text.contains("[model.request_params"), "{text}");
        let table = text.parse::<toml::Table>().unwrap();
        assert!(table["model"]["request_params"].is_str(), "{text}");
        assert_eq!(parse::<Document>(text.as_bytes()).unwrap(), document);
        // An explicitly authored empty object stays present.
        let empty = Document {
            model: Params {
                request_params: Some(AuthoredRequestParams::default()),
            },
        };
        let text = write(&empty).unwrap();
        assert!(text.contains("request_params = \"{}\""), "{text}");
        assert_eq!(parse::<Document>(text.as_bytes()).unwrap(), empty);
    }

    #[test]
    fn client_json_stays_structured_outside_the_source_context() {
        let document = Document {
            model: Params {
                request_params: Some(AuthoredRequestParams(params(
                    json!({"value": null, "nested": {"k": [1]}}),
                ))),
            },
        };
        let value = serde_json::to_value(&document).unwrap();
        assert_eq!(
            value,
            json!({"model": {"request_params": {"value": null, "nested": {"k": [1]}}}})
        );
        assert_eq!(serde_json::from_value::<Document>(value).unwrap(), document);
        // A JSON string is not the client representation.
        assert!(
            serde_json::from_value::<Document>(json!({"model": {"request_params": "{}"}})).is_err()
        );
        // Leaving a source context restores the client representation.
        let _ = write(&document).unwrap();
        assert!(serde_json::to_value(&document).unwrap()["model"]["request_params"].is_object());
    }

    #[test]
    fn invalid_json_strings_fail_with_safe_located_diagnostics() {
        #[derive(Deserialize, Debug)]
        #[allow(dead_code)]
        struct WithSecret {
            secret: String,
            model: Params,
        }
        let dup = "repeats a JSON object key";
        for (json, reason) in [
            (
                "{broken",
                "is not valid JSON (at line 1, column 2 of the JSON text)",
            ),
            (
                "{\"SECRET_PROVIDER_VALUE\": }",
                "is not valid JSON (at line 1, column 27 of the JSON text)",
            ),
            (
                "{} {}",
                "is not valid JSON (at line 1, column 4 of the JSON text)",
            ),
            (
                "",
                "is not valid JSON (at line 1, column 0 of the JSON text)",
            ),
            ("null", "must encode a JSON object"),
            ("[]", "must encode a JSON object"),
            ("42", "must encode a JSON object"),
            ("\"SECRET_PROVIDER_VALUE\"", "must encode a JSON object"),
            // Duplicate keys at every depth are located by position, never by
            // key text: a key is as opaque as a value and may hold a secret.
            (
                "{\"a\":1,\"a\":2}",
                &format!("{dup} (at line 1, column 10 of the JSON text)"),
            ),
            (
                "{\"outer\":{\"SECRET_PROVIDER_VALUE\":[{\"k\":1,\"k\":1}]}}",
                &format!("{dup} (at line 1, column 45 of the JSON text)"),
            ),
            (
                "{\"list\":[{\"k\":1},{\"k\":1,\"k\":2}]}",
                &format!("{dup} (at line 1, column 27 of the JSON text)"),
            ),
            (
                "{\"sk-SECRET_KEY_MARKER\":1,\"sk-SECRET_KEY_MARKER\":2}",
                &format!("{dup} (at line 1, column 48 of the JSON text)"),
            ),
            (
                "{\"SECRET \\\"quoted\\\" [x] y\":{},\"SECRET \\\"quoted\\\" [x] y\":{}}",
                &format!("{dup} (at line 1, column 55 of the JSON text)"),
            ),
            (
                "{\"SECRET_键🔑\\u00e9\":1,\"SECRET_键🔑é\":2}",
                &format!("{dup} (at line 1, column 44 of the JSON text)"),
            ),
        ] {
            let text = format!("secret = 'SECRET_NEIGHBOR'\n[model]\nrequest_params = '{json}'\n");
            let failure = parse_detailed::<toml::Table>(text.as_bytes());
            assert!(
                failure.is_ok(),
                "an untyped table carries no request_params"
            );
            let failure = parse_detailed::<WithSecret>(text.as_bytes()).unwrap_err();
            assert_eq!(failure.path.as_deref(), Some("model.request_params"));
            assert_eq!(
                (failure.line, failure.column),
                (Some(3), Some(18)),
                "{json}"
            );
            assert!(!failure.syntax);
            let detail = failure.into_detail();
            assert!(
                detail.contains(&format!("model.request_params {reason}")),
                "{detail}"
            );
            assert!(!detail.contains("SECRET"), "{detail}");
        }
    }

    #[test]
    fn duplicate_keys_are_strict_at_every_depth_and_never_echo_key_text() {
        for (json, line, column) in [
            ("{\"a\":1,\"a\":2}", 1, 10),
            ("{\"a\":{\"b\":1,\"b\":2}}", 1, 15),
            ("{\"list\":[{\"k\":1},{\"k\":1,\"k\":2}]}", 1, 27),
            ("{\"\\u0061\":1,\"a\":2}", 1, 15),
            (
                "{\n  \"x\": [[{\"SECRET z\": 1,\n  \"SECRET z\": 1}]]\n}",
                3,
                12,
            ),
            (
                "{\"api_key=sk-live-SECRET\":0,\"api_key=sk-live-SECRET\":0}",
                1,
                52,
            ),
        ] {
            let error = parse_request_params_json(json).unwrap_err();
            assert_eq!(
                error,
                RequestParamsJsonError::DuplicateKey { line, column },
                "{json}"
            );
            let shown = format!("{error} {error:?}");
            assert!(!shown.contains("SECRET") && !shown.contains('"'), "{shown}");
        }
        // The same key in sibling objects or as a string value is not a repeat.
        parse_request_params_json(r#"{"a":{"k":1},"b":{"k":1},"c":["a","a"],"d":"a"}"#).unwrap();
        parse_request_params_json(r#"{"s":"{\"a\":1,\"a\":2}"}"#).unwrap();
    }

    #[test]
    fn numbers_some_hop_would_round_are_refused_before_any_rounding() {
        use crate::model::invocation::{
            InexactNumber, RequestParamsLayer, validate_request_params_numbers,
        };
        for (json, column) in [
            (r#"{"seed":9007199254740993}"#, 9),
            (r#"{"seed":-9007199254740993}"#, 9),
            // Exactly a binary64, but printed back as 1152921504606847000.
            (r#"{"seed":1152921504606846976}"#, 9),
            (r#"{"seed":18446744073709551616}"#, 9),
            (r#"{"t":0.12345678901234567890}"#, 6),
            (r#"{"a":{"b":[1,{"c":[2.5,9007199254740993]}]}}"#, 24),
        ] {
            assert_eq!(
                parse_request_params_json(json),
                Err(RequestParamsJsonError::InexactNumber { line: 1, column }),
                "{json}"
            );
        }
        let exact = r#"{"a":9007199254740992,"b":-9007199254740991,"c":0.1,"d":1.50,
            "e":15e-1,"f":1e20,"g":1E+300,"h":5e-324,"i":-0,"j":0.0,"k":10000000000000000000,
            "l":3.141592653589793,"m":[{"n":-2.5e-7}],"s":"9007199254740993",
            "o":5.357830195732913e-76,"p":1.603964615428183e143}"#;
        let params = parse_request_params_json(exact).unwrap();
        assert_eq!(params["a"], json!(9_007_199_254_740_992_u64));
        assert_eq!(params["k"], json!(10_000_000_000_000_000_000_u64));
        assert_eq!(params["s"], json!("9007199254740993"));
        // Correctly rounded parsing: the binary64 a client prints is the one read.
        assert_eq!(params["o"].as_f64(), Some(5.357_830_195_732_913e-76));
        assert_eq!(params["p"].as_f64(), Some(1.603_964_615_428_183e143));
        // A written and reread source carries exactly the same values.
        let written = serde_json::to_string(&params).unwrap();
        assert_eq!(parse_request_params_json(&written).unwrap(), params);
        // Structured client JSON is held to the same domain.
        let layer = RequestParamsLayer::ModelProfile;
        assert_eq!(validate_request_params_numbers(&params, layer), Ok(()));
        for value in [
            json!({"seed": 9_007_199_254_740_993_u64}),
            json!({"x": [{"seed": -9_007_199_254_740_993_i64}]}),
            json!({"seed": 1_152_921_504_606_846_976_u64}),
        ] {
            let serde_json::Value::Object(map) = value else {
                unreachable!()
            };
            assert_eq!(
                validate_request_params_numbers(&map, layer),
                Err(InexactNumber { layer })
            );
        }
    }

    #[test]
    fn toml_tables_and_non_string_values_are_rejected_without_echo() {
        for value in [
            "{ temperature = 0.7 }",
            "{}",
            "[]",
            "42",
            "true",
            "1979-05-27T07:32:00Z",
            "nan",
        ] {
            let text = format!("[model]\nrequest_params = {value}\nsecret = 'SECRET_NEIGHBOR'\n");
            let failure = parse_detailed::<Document>(text.as_bytes()).unwrap_err();
            assert_eq!(failure.path.as_deref(), Some("model.request_params"));
            let detail = failure.into_detail();
            assert!(
                detail.contains("model.request_params must be a JSON-encoded string"),
                "{detail}"
            );
            assert!(!detail.contains("SECRET_NEIGHBOR"), "{detail}");
        }
        let error = parse::<Document>(b"[model.request_params]\ntemperature = 0.7\n").unwrap_err();
        assert!(error.contains("must be a JSON-encoded string"), "{error}");
        let error = parse::<Document>(b"[model]\nrequest_params_json = '{}'").unwrap_err();
        assert!(
            error.contains("unknown field `request_params_json`"),
            "{error}"
        );
    }

    #[test]
    fn malformed_toml_does_not_echo_opaque_values_or_neighboring_credentials() {
        for text in [
            "[model]\nrequest_params = '{\"secret\": \"SECRET_PROVIDER_VALUE\"}' broken",
            "[model]\nrequest_params = '{}' secret = 'SECRET_PROVIDER_VALUE'",
        ] {
            let error = parse::<Document>(text.as_bytes()).unwrap_err();
            assert!(!error.contains("SECRET_PROVIDER_VALUE"), "{error}");
        }
    }

    #[test]
    fn source_and_client_schemas_describe_their_own_representation() {
        let source = serde_json::to_value(source_schema::<Document>()).unwrap();
        let client = serde_json::to_value(schemars::schema_for!(Document)).unwrap();
        assert_eq!(source["$defs"]["RequestParamsJson"]["type"], "string");
        assert_eq!(client["$defs"]["RequestParams"]["type"], "object");
        assert!(source["$defs"].get("RequestParams").is_none());
        assert!(client["$defs"].get("RequestParamsJson").is_none());
    }
}

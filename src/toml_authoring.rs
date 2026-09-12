//! Typed TOML parsing and structured authoring diagnostics.

use serde::de::DeserializeOwned;
/// Parse TOML directly into a typed authoring document.
///
/// # Errors
/// Rejects malformed input and any shape rejected by the authoring type.
pub fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    parse_detailed(bytes).map_err(|error| error.detail)
}

/// Parser-owned location. Debug deliberately excludes authored values.
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
        detail: format!("not valid UTF-8: {error}"),
    })?;
    toml::from_str(text).map_err(|error| {
        let prefix = error.span().and_then(|span| text.get(..span.start));
        ParseFailure {
            line: prefix.map(|s| s.bytes().filter(|b| *b == b'\n').count() + 1),
            column: prefix.map(|s| s.rsplit('\n').next().unwrap_or_default().chars().count() + 1),
            syntax: text.parse::<toml_edit::DocumentMut>().is_err(),
            // Do not include TOML's source excerpt: it may contain credentials
            // or opaque provider parameters beside the malformed token.
            detail: format!(
                "TOML error at line {}, column {}: {}",
                prefix.map_or(1, |s| s.bytes().filter(|b| *b == b'\n').count() + 1),
                prefix.map_or(1, |s| s
                    .rsplit('\n')
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .count()
                    + 1),
                error.message()
            ),
        }
    })
}

/// The sole authored boundary for arbitrary provider-native JSON objects.
/// Runtime overlays retain JSON's full domain, including nested nulls.
#[derive(Clone, Default, PartialEq)]
pub struct RequestParamsJson(pub crate::model::invocation::RequestParams);
impl std::fmt::Debug for RequestParamsJson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RequestParamsJson(<opaque>)")
    }
}
impl<'de> serde::Deserialize<'de> for RequestParamsJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        serde_json::from_str::<crate::model::invocation::RequestParams>(&text)
            .map(Self)
            .map_err(|_| {
                serde::de::Error::custom("request_params_json must contain a valid JSON object")
            })
    }
}
impl serde::Serialize for RequestParamsJson {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer
            .serialize_str(&serde_json::to_string(&self.0).map_err(serde::ser::Error::custom)?)
    }
}
impl schemars::JsonSchema for RequestParamsJson {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RequestParamsJson".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        serde_json::json!({"type":"string", "description":"JSON text with an object at its top level. Arbitrary nested provider-native JSON is retained, including null. Protected wire keys are rejected during model resolution."}).try_into().expect("schema object")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_errors_do_not_echo_opaque_json_or_neighboring_credentials() {
        #[derive(serde::Deserialize, Debug)]
        #[serde(deny_unknown_fields)]
        struct Params {
            #[serde(rename = "request_params_json")]
            _params: RequestParamsJson,
        }
        for text in [
            "request_params_json = '{SECRET_PROVIDER_VALUE broken}'",
            "request_params_json = '{}' secret = 'SECRET_PROVIDER_VALUE'",
        ] {
            let error = parse::<Params>(text.as_bytes()).unwrap_err();
            assert!(!error.contains("SECRET_PROVIDER_VALUE"), "{error}");
        }
    }
}

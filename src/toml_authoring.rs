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
    /// Exact path for unsupported request-parameter values; never authored values.
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
    serde_path_to_error::deserialize(toml::Deserializer::new(text)).map_err(|failure| {
        let path = failure.path().to_string();
        let error = failure.into_inner();
        let mut parameter_path = None;
        let message = if let Some(relative) = error.message().strip_prefix(PARAM_ERROR) {
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
            parameter_path = relative
                .rsplit_once(": ")
                .map(|(suffix, _)| format!("{owner}{suffix}"));
            format!("{PARAM_ERROR}{owner}{relative}")
        } else {
            error.message().to_owned()
        };
        let prefix = error.span().and_then(|span| text.get(..span.start));
        ParseFailure {
            line: prefix.map(|s| s.bytes().filter(|b| *b == b'\n').count() + 1),
            column: prefix.map(|s| s.rsplit('\n').next().unwrap_or_default().chars().count() + 1),
            syntax: text.parse::<toml_edit::DocumentMut>().is_err(),
            path: parameter_path,
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
                message
            ),
        }
    })
}

/// The only provider-parameter TOML boundary. The stored result is already JSON;
/// transient TOML values never enter model resolution or provider adapters.
#[derive(Clone, Default, PartialEq)]
pub struct RequestParamsToml(pub crate::model::invocation::RequestParams);
impl std::fmt::Debug for RequestParamsToml {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RequestParamsToml(<opaque>)")
    }
}
impl<'de> serde::Deserialize<'de> for RequestParamsToml {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = toml::Value::deserialize(deserializer)?;
        if !value.is_table() {
            return Err(serde::de::Error::custom(
                "request_params must be a TOML table",
            ));
        }
        match normalize(value, "").map_err(serde::de::Error::custom)? {
            serde_json::Value::Object(map) => Ok(Self(map)),
            _ => unreachable!("root checked above"),
        }
    }
}

// Paths contain keys and indices only, never values or TOML source excerpts.
// The parser joins this relative path to the enclosing typed authoring path.
const PARAM_ERROR: &str = "unsupported request parameter at ";
fn normalize(value: toml::Value, path: &str) -> Result<serde_json::Value, String> {
    use serde_json::Value as Json;
    Ok(match value {
        toml::Value::String(v) => Json::String(v),
        toml::Value::Integer(v) => Json::Number(v.into()),
        toml::Value::Float(v) => {
            Json::Number(serde_json::Number::from_f64(v).ok_or_else(|| {
                format!("{PARAM_ERROR}{path}: non-finite floats are not JSON-compatible")
            })?)
        }
        toml::Value::Boolean(v) => Json::Bool(v),
        toml::Value::Datetime(_) => {
            return Err(format!(
                "{PARAM_ERROR}{path}: TOML dates, times and datetimes are not JSON-compatible"
            ));
        }
        toml::Value::Array(values) => Json::Array(
            values
                .into_iter()
                .enumerate()
                .map(|(i, v)| normalize(v, &format!("{path}[{i}]")))
                .collect::<Result<_, _>>()?,
        ),
        toml::Value::Table(values) => Json::Object(
            values
                .into_iter()
                .map(|(key, v)| {
                    let segment = if key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                        && !key.is_empty()
                    {
                        format!(".{key}")
                    } else {
                        format!("[{}]", serde_json::to_string(&key).expect("string"))
                    };
                    normalize(v, &format!("{path}{segment}")).map(|v| (key, v))
                })
                .collect::<Result<_, _>>()?,
        ),
    })
}
impl serde::Serialize for RequestParamsToml {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl schemars::JsonSchema for RequestParamsToml {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RequestParamsToml".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        serde_json::json!({
            "type": "object",
            "description": "Opaque provider-native structured TOML. Strings, integers, finite floats, booleans, arrays and tables only; no dates, times, datetimes, non-finite floats or explicit null. Protected wire keys are checked during model resolution.",
            "additionalProperties": {"$ref": "#/$defs/RequestParamsToml/$defs/value"},
            "$defs": {"value": {"anyOf": [
                {"type": "string"}, {"type": "number"}, {"type": "boolean"},
                {"type": "array", "items": {"$ref": "#/$defs/RequestParamsToml/$defs/value"}},
                {"type": "object", "additionalProperties": {"$ref": "#/$defs/RequestParamsToml/$defs/value"}}
            ]}}
        }).try_into().expect("schema object")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(serde::Deserialize, Debug)]
    #[serde(deny_unknown_fields)]
    struct Document {
        model: Params,
    }
    #[derive(serde::Deserialize, Debug)]
    #[serde(deny_unknown_fields)]
    struct Params {
        request_params: RequestParamsToml,
    }

    #[test]
    fn unsupported_values_reject_with_exact_paths_without_values() {
        for value in [
            "1979-05-27",
            "07:32:00",
            "1979-05-27T07:32:00Z",
            "nan",
            "+inf",
            "-inf",
        ] {
            for (body, path) in [
                (
                    format!("provider.started_at = {value}"),
                    "model.request_params.provider.started_at",
                ),
                (
                    format!("items = [{{when = true}}, {{when = {value}}}]"),
                    "model.request_params.items[1].when",
                ),
                (
                    format!("temperature = {value}"),
                    "model.request_params.temperature",
                ),
            ] {
                let text =
                    format!("[model.request_params]\nsecret = 'SECRET_PROVIDER_VALUE'\n{body}");
                let error = parse::<Document>(text.as_bytes()).unwrap_err();
                assert!(error.contains(path), "{error}");
                assert!(!error.contains("SECRET_PROVIDER_VALUE"), "{error}");
            }
        }
    }
    #[test]
    fn native_structures_normalize_exactly() {
        let text = r#"[model.request_params]
text = "exact"
integer = -42
temperature = 0.7
flag = false
chat_template_kwargs.enable_thinking = true
structured_outputs.choice = ["positive", "negative"]
documents = [{title = "A", options = {media = [1, true, "x"]}}, {title = "B"}]
[model.request_params.provider]
order = ["a", "b"]
allow_fallbacks = true
"#;
        let doc = parse::<Document>(text.as_bytes()).unwrap();
        assert_eq!(
            serde_json::Value::Object(doc.model.request_params.0),
            serde_json::json!({
                "text":"exact", "integer":-42, "temperature":0.7, "flag":false,
                "chat_template_kwargs":{"enable_thinking":true},
                "structured_outputs":{"choice":["positive","negative"]},
                "documents":[{"title":"A","options":{"media":[1,true,"x"]}}, {"title":"B"}],
                "provider":{"order":["a","b"],"allow_fallbacks":true}
            })
        );
        let forms = [
            "[model.request_params.provider]\norder = ['a']",
            "[model.request_params]\nprovider.order = ['a']",
            "[model]\nrequest_params = {provider = {order = ['a']}}",
        ];
        for form in forms {
            assert_eq!(
                parse::<Document>(form.as_bytes())
                    .unwrap()
                    .model
                    .request_params
                    .0,
                serde_json::from_value::<crate::model::invocation::RequestParams>(
                    serde_json::json!({"provider":{"order":["a"]}})
                )
                .unwrap()
            );
        }
    }
    #[test]
    fn malformed_toml_does_not_echo_opaque_values_or_neighboring_credentials() {
        for text in [
            "[model]\nrequest_params = {secret = 'SECRET_PROVIDER_VALUE', broken = }",
            "[model]\nrequest_params = {} secret = 'SECRET_PROVIDER_VALUE'",
        ] {
            let error = parse::<Document>(text.as_bytes()).unwrap_err();
            assert!(!error.contains("SECRET_PROVIDER_VALUE"), "{error}");
        }
    }
    #[test]
    fn serialization_preserves_native_tables_and_runtime_null_is_not_a_toml_sentinel() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Authored {
            request_params: RequestParamsToml,
        }
        let params = serde_json::from_value(
            serde_json::json!({"temperature":0.7,"chat_template_kwargs":{"enable_thinking":true}}),
        )
        .unwrap();
        let document = Authored {
            request_params: RequestParamsToml(params),
        };
        let text = toml::to_string_pretty(&document).unwrap();
        assert!(text.contains("[request_params.chat_template_kwargs]"));
        let again: Authored = parse(text.as_bytes()).unwrap();
        assert_eq!(document.request_params, again.request_params);
        let runtime =
            RequestParamsToml(serde_json::from_value(serde_json::json!({"value":null})).unwrap());
        assert_eq!(
            serde_json::to_value(&runtime).unwrap(),
            serde_json::json!({"value":null})
        );
        assert!(
            toml::to_string(&Authored {
                request_params: runtime
            })
            .is_err()
        );
    }
    #[test]
    fn obsolete_field_and_non_object_roots_reject() {
        let error = parse::<Document>(b"[model]\nrequest_params_json = '{}'").unwrap_err();
        assert!(error.contains("unknown field `request_params_json`"));
        for value in ["[]", "42", "true", "'SECRET_PROVIDER_VALUE'"] {
            let error = parse::<Document>(format!("[model]\nrequest_params = {value}").as_bytes())
                .unwrap_err();
            assert!(error.contains("request_params must be a TOML table"));
            assert!(!error.contains("SECRET_PROVIDER_VALUE"));
        }
    }
}

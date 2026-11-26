//! Opaque provider request parameters and their exact-number validation.

use serde::{Deserialize, Serialize};
use std::fmt;

/// An opaque provider request-parameter object.
///
/// Values are preserved exactly as parsed; rustX never rewrites, coerces, or
/// interprets them.
pub type RequestParams = serde_json::Map<String, serde_json::Value>;

/// Which configuration layer a set of request parameters came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestParamsLayer {
    /// The native parameters of a Model without profiles.
    ModelParams,
    /// A declared Model Profile's parameters.
    ModelProfile,
    /// The session's request-parameter overrides.
    SessionOverrides,
    /// Shared auxiliary model request-parameter overrides.
    AuxiliaryOverrides,
    /// The effective parameters at final wire construction.
    EffectiveRequest,
}

impl fmt::Display for RequestParamsLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ModelParams => "model request parameters",
            Self::ModelProfile => "model profile request parameters",
            Self::SessionOverrides => "session request-parameter overrides",
            Self::AuxiliaryOverrides => "auxiliary request-parameter overrides",
            Self::EffectiveRequest => "effective request parameters",
        })
    }
}

/// One configured collision with a runtime-owned protected wire key.
///
/// One configured request-parameter number that does not survive a binary64
/// round trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InexactNumber {
    /// The configuration layer that declared it.
    pub layer: RequestParamsLayer,
}

impl fmt::Display for InexactNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} carry a JSON number that is not exactly representable as an IEEE 754 binary64 value",
            self.layer
        )
    }
}

impl std::error::Error for InexactNumber {}

/// Whether one JSON number literal means the same value at every hop.
///
/// Authored request parameters travel through rustX, through binary64 JSON
/// clients such as the browser, and back into native sources. Following the
/// I-JSON interoperability rule (RFC 7493 §2.2), a number is accepted only when
/// the value rustX holds and emits is the value its binary64 reading prints:
/// `9007199254740993` (above 2^53), `2^60` and a decimal with more significant
/// digits than binary64 carries are refused instead of being silently rounded
/// by some later hop. The decision compares exact decimal values, so spelling
/// (`1.50`, `15e-1`, `1.5`) is irrelevant.
#[must_use]
pub fn exact_json_number(literal: &str) -> bool {
    // What rustX holds and emits for this literal.
    let Ok(number) = serde_json::from_str::<serde_json::Number>(literal) else {
        return false;
    };
    let emitted = number.to_string();
    // What a binary64 reader holds, printed as its shortest round trip.
    let Ok(binary) = emitted.parse::<f64>() else {
        return false;
    };
    let value = decimal_value(literal);
    binary.is_finite()
        && value.is_some()
        && value == decimal_value(&emitted)
        && value == decimal_value(&format!("{binary:e}"))
}

/// The exact value of a decimal literal as `(negative, significant digits,
/// exponent)`, with zero normalized to one value.
fn decimal_value(literal: &str) -> Option<(bool, String, i64)> {
    let (negative, unsigned) = literal
        .strip_prefix('-')
        .map_or((false, literal), |rest| (true, rest));
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
        None => (unsigned, 0),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{integer}{fraction}");
    let leading = digits.trim_start_matches('0');
    let significant = leading.trim_end_matches('0');
    if significant.is_empty() {
        return Some((false, String::new(), 0));
    }
    let exponent = exponent
        .checked_sub(i64::try_from(fraction.len()).ok()?)?
        .checked_add(i64::try_from(leading.len() - significant.len()).ok()?)?;
    Some((negative, significant.to_owned(), exponent))
}

/// Validates that every number of one configuration layer, at any depth, is
/// an [`exact_json_number`].
///
/// # Errors
///
/// Returns [`InexactNumber`] naming the layer; never the value or its key.
pub fn validate_request_params_numbers(
    params: &RequestParams,
    layer: RequestParamsLayer,
) -> Result<(), InexactNumber> {
    if numbers_are_exact(params) {
        Ok(())
    } else {
        Err(InexactNumber { layer })
    }
}

/// Whether every number of `params`, at any depth, is an
/// [`exact_json_number`].
#[must_use]
pub fn numbers_are_exact(params: &RequestParams) -> bool {
    fn exact(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Number(number) => exact_json_number(&number.to_string()),
            serde_json::Value::Array(values) => values.iter().all(exact),
            serde_json::Value::Object(map) => map.values().all(exact),
            _ => true,
        }
    }
    params.values().all(exact)
}

/// Applies one top-level shallow overlay.
///
/// A nested object or array value replaces the previous value atomically;
/// rustX never deep-merges configuration.
pub fn overlay_shallow(base: &mut RequestParams, overlay: &RequestParams) {
    for (key, value) in overlay {
        base.insert(key.clone(), value.clone());
    }
}

pub fn append_content_modalities<'a>(
    messages: impl IntoIterator<Item = &'a crate::model::input::ModelInputMessage>,
    required: &mut Vec<crate::model::capabilities::Modality>,
) {
    use crate::message::types::{AssistantContentBlock, UserContentBlock};
    use crate::model::projected::ModelMessageView;
    use crate::tools::types::ToolResultContent;

    let mut require = |modality: crate::model::capabilities::Modality| {
        if !required.contains(&modality) {
            required.push(modality);
        }
    };
    for message in messages {
        match message.view() {
            ModelMessageView::User(user) => {
                for content in user {
                    match content {
                        UserContentBlock::Text(_) | UserContentBlock::UploadedFile(_) => {
                            require(crate::model::capabilities::Modality::Text);
                        }
                        UserContentBlock::Image(_) => {
                            require(crate::model::capabilities::Modality::Image);
                        }
                        UserContentBlock::File(_) => {
                            require(crate::model::capabilities::Modality::File);
                        }
                    }
                }
            }
            ModelMessageView::Assistant(assistant) => {
                for content in assistant {
                    match content {
                        AssistantContentBlock::Image(_) => {
                            require(crate::model::capabilities::Modality::Image);
                        }
                        AssistantContentBlock::Text(_)
                        | AssistantContentBlock::Refusal(_)
                        | AssistantContentBlock::Reasoning(_)
                        | AssistantContentBlock::ToolCall(_) => {
                            require(crate::model::capabilities::Modality::Text);
                        }
                    }
                }
            }
            ModelMessageView::Tool(_, result) => {
                for content in &result.content {
                    match content {
                        ToolResultContent::Image(_) => {
                            require(crate::model::capabilities::Modality::Image);
                        }
                        ToolResultContent::File(_) => {
                            require(crate::model::capabilities::Modality::File);
                        }
                        ToolResultContent::Text(_) | ToolResultContent::Json { .. } => {
                            require(crate::model::capabilities::Modality::Text);
                        }
                    }
                }
            }
            // Every request-only context item is a runtime-authored textual
            // narration. It never carries image/file semantics or a
            // canonical identity.
            ModelMessageView::RequestOnly(_) => require(crate::model::capabilities::Modality::Text),
        }
    }
}

//! Closed provider-neutral request option policy shared by historical projections.

/// The closed allowlist of provider-neutral request options historical inspection exposes.
///
/// `RequestParams` is an arbitrary operator-supplied JSON map that rustX
/// forwards to a provider without interpreting it. Projecting it whole would
/// mean projecting whatever an operator put there, so inspection instead names
/// every key it is willing to show. These are the provider-neutral sampling
/// and decoding controls that explain a generation's behaviour; everything
/// else is counted and omitted.
///
/// This is a typed allowlist, not a secret scanner: a key is shown because
/// it is on this list, never because its value looked harmless.
pub(crate) const REQUEST_OPTION_ALLOWLIST: &[&str] = &[
    "frequency_penalty",
    "logit_bias",
    "logprobs",
    "min_p",
    "n",
    "presence_penalty",
    "repetition_penalty",
    "response_format",
    "seed",
    "stop",
    "temperature",
    "top_k",
    "top_logprobs",
    "top_p",
];

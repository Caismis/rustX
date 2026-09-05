//! Recognition of the Qwen XML in-band tool dialect.
//!
//! Some model families served over the `OpenAI` Chat Completions protocol do
//! not emit tool calls only as structured `tool_calls`. They emit a reserved
//! XML region into the generated token stream that the serving stack
//! (vLLM and compatible stacks) is expected to parse out. When that parse
//! fails, the reserved region survives into ordinary `content` or
//! `reasoning` and the request terminates as though the model had simply
//! answered — a malformed tool proposal wearing the shape of a normal
//! completion.
//!
//! The real emission contract is a nested, line-oriented region:
//!
//! ```text
//! <tool_call>
//! <function=write_file>
//! <parameter=path>
//! notes.txt
//! </parameter>
//! </function>
//! </tool_call>
//! ```
//!
//! # Why this is not a substring rule
//!
//! An assistant that is *asked about* this dialect will legitimately write
//! the exact reserved bytes:
//!
//! ```text
//! Qwen's tool-call syntax can look like:
//!
//! <tool_call>
//! ...
//! </tool_call>
//!
//! A parameter is encoded as <parameter=path>...</parameter>.
//! ```
//!
//! `contains(open) && contains(close)` cannot tell those apart, so it
//! misclassifies a correct answer as malformed tool intent. This module
//! instead recognizes the *emission shape*, using evidence a discussion of
//! the syntax does not produce:
//!
//! - **Standalone region.** A reserved tag is evidence only when it owns its
//!   whole line. Prose embeds the tag in a sentence
//!   (`... as <parameter=path>...</parameter>.`); an emission puts it on a
//!   line by itself.
//! - **Quotation is not emission.** Tags inside a fenced code block are
//!   quoted syntax — how a model shows the reader what the dialect looks
//!   like — and are skipped.
//! - **Real envelope structure.** An opener must be matched by its own
//!   closer, in order, and the opener's payload must be a plausible
//!   function/parameter identifier. An illustrative `<function=...>` or a
//!   `<tool_call>` wrapping a literal `...` is not a protocol region.
//!
//! Recognition is deliberately one-directional: it proves a generation
//! *leaked protocol*, and nothing here ever reconstructs a `ToolCall` from
//! the leaked text. The outcome is one refused proposal, never invented
//! model intent.

/// The reserved envelope whose complete emission was recognized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QwenReservedEnvelope {
    /// A complete `<function=NAME>` … `</function>` region.
    Function,
    /// A complete `<parameter=KEY>` … `</parameter>` region.
    Parameter,
}

impl QwenReservedEnvelope {
    /// The envelope's shape, for the runtime diagnostic. It names the
    /// dialect construct rather than echoing the model's own bytes.
    pub(crate) const fn shape(self) -> &'static str {
        match self {
            Self::Function => "<function=\u{2026}>\u{2026}</function>",
            Self::Parameter => "<parameter=\u{2026}>\u{2026}</parameter>",
        }
    }
}

const FUNCTION_OPEN_PREFIX: &str = "<function=";
const FUNCTION_CLOSE: &str = "</function>";
const PARAMETER_OPEN_PREFIX: &str = "<parameter=";
const PARAMETER_CLOSE: &str = "</parameter>";

/// The longest payload accepted inside a reserved opener. A tool name or a
/// parameter key is an identifier; anything longer is prose that happens to
/// begin with the reserved prefix, and bounding it keeps recognition
/// constant-work per line.
const MAX_RESERVED_NAME_BYTES: usize = 128;

/// What one line of generated output is, in dialect terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReservedLine {
    FunctionOpen,
    FunctionClose,
    ParameterOpen,
    ParameterClose,
    /// Anything else, including a tag embedded in a sentence.
    Ordinary,
}

/// Whether a reserved opener's payload is a plausible function name or
/// parameter key.
///
/// This is what separates an emitted `<function=write_file>` from an
/// illustrative `<function=...>`: the first names a tool, the second is an
/// ellipsis a human wrote to mean "your function here".
fn is_reserved_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_RESERVED_NAME_BYTES {
        return false;
    }
    let mut characters = name.chars();
    let leading = characters
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    leading && characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Classifies one line, which is a reserved tag only when it owns the whole
/// line. A tag quoted inside a sentence is `Ordinary` by construction.
fn classify(line: &str) -> ReservedLine {
    let line = line.trim();
    if line == FUNCTION_CLOSE {
        return ReservedLine::FunctionClose;
    }
    if line == PARAMETER_CLOSE {
        return ReservedLine::ParameterClose;
    }
    for (prefix, opened) in [
        (FUNCTION_OPEN_PREFIX, ReservedLine::FunctionOpen),
        (PARAMETER_OPEN_PREFIX, ReservedLine::ParameterOpen),
    ] {
        if let Some(rest) = line.strip_prefix(prefix)
            && let Some(name) = rest.strip_suffix('>')
            && is_reserved_name(name)
        {
            return opened;
        }
    }
    ReservedLine::Ordinary
}

/// Whether a line opens or closes a Markdown code fence.
fn is_code_fence(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

/// Recognizes an actual Qwen tool-protocol emission in generated output.
///
/// Returns the reserved envelope whose complete standalone region was found,
/// or `None` when the output merely mentions, quotes, or discusses the
/// dialect. The scan is a single bounded pass with constant state: it never
/// backtracks and never materializes the candidate regions.
pub(crate) fn tool_protocol_emission(output: &str) -> Option<QwenReservedEnvelope> {
    let mut inside_fence = false;
    let mut function_open = false;
    let mut parameter_open = false;
    for line in output.lines() {
        if is_code_fence(line) {
            inside_fence = !inside_fence;
            continue;
        }
        if inside_fence {
            continue;
        }
        match classify(line) {
            // A second opener replaces the first: the protocol never nests an
            // envelope inside itself, so the nearest opener is the one a
            // closer can complete.
            ReservedLine::FunctionOpen => function_open = true,
            ReservedLine::ParameterOpen => parameter_open = true,
            ReservedLine::FunctionClose if function_open => {
                return Some(QwenReservedEnvelope::Function);
            }
            ReservedLine::ParameterClose if parameter_open => {
                return Some(QwenReservedEnvelope::Parameter);
            }
            ReservedLine::FunctionClose | ReservedLine::ParameterClose | ReservedLine::Ordinary => {
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{QwenReservedEnvelope, tool_protocol_emission};

    /// The real vLLM/Qwen emission contract, complete and nested, is
    /// recognized.
    #[test]
    fn a_complete_nested_emission_is_recognized() {
        let output = "<tool_call>\n<function=write_file>\n<parameter=path>\nnotes.txt\n\
                      </parameter>\n</function>\n</tool_call>";
        assert!(tool_protocol_emission(output).is_some());
    }

    /// A partially consumed emission — the serving stack stripped the outer
    /// envelope and left the parameter region behind — is still an emission.
    #[test]
    fn a_residual_parameter_region_is_recognized() {
        let output = "<parameter=path>\nnotes.txt\n</parameter>\n";
        assert_eq!(
            tool_protocol_emission(output),
            Some(QwenReservedEnvelope::Parameter)
        );
    }

    /// A parameterless call still leaks a complete function region.
    #[test]
    fn a_residual_function_region_is_recognized() {
        let output = "<tool_call>\n<function=list_directory>\n</function>\n</tool_call>";
        assert_eq!(
            tool_protocol_emission(output),
            Some(QwenReservedEnvelope::Function)
        );
    }

    /// Exact reserved syntax quoted inside a sentence is prose. This is the
    /// case a `contains(open) && contains(close)` rule gets wrong.
    #[test]
    fn reserved_syntax_inside_a_sentence_is_prose() {
        let output = "A parameter is encoded as <parameter=path>...</parameter>, and the \
                      function as <function=write_file>...</function> inside it.";
        assert_eq!(tool_protocol_emission(output), None);
    }

    /// A standalone `<tool_call>` block whose body is an ellipsis is an
    /// illustration, not a call: it contains no function or parameter
    /// region.
    #[test]
    fn an_illustrative_tool_call_block_is_prose() {
        let output = "Qwen's tool-call syntax can look like:\n\n<tool_call>\n...\n</tool_call>\n";
        assert_eq!(tool_protocol_emission(output), None);
        let placeholders = "<tool_call>\n<function=...>\n<parameter=...>\nvalue\n</parameter>\n\
                            </function>\n</tool_call>";
        assert_eq!(tool_protocol_emission(placeholders), None);
    }

    /// A fenced code block is quoted syntax, which is how a model shows the
    /// reader the dialect. Quotation is not emission.
    #[test]
    fn a_fenced_example_is_quoted_not_emitted() {
        let output = "Here is the shape:\n\n```xml\n<tool_call>\n<function=write_file>\n\
                      <parameter=path>\nnotes.txt\n</parameter>\n</function>\n</tool_call>\n```\n\
                      Use it exactly like that.";
        assert_eq!(tool_protocol_emission(output), None);
    }

    /// An unmatched opener is not a region: ordering is required, not just
    /// co-occurrence of tokens.
    #[test]
    fn an_unmatched_or_misordered_tag_is_not_a_region() {
        assert_eq!(
            tool_protocol_emission("<parameter=path>\nnotes.txt\n"),
            None
        );
        assert_eq!(
            tool_protocol_emission("</parameter>\nnotes.txt\n<parameter=path>"),
            None
        );
    }

    /// Ordinary output that names no reserved tag at all is never inspected
    /// into a false positive.
    #[test]
    fn ordinary_output_is_never_a_region() {
        assert_eq!(tool_protocol_emission("The file has been written."), None);
        assert_eq!(tool_protocol_emission(""), None);
    }
}

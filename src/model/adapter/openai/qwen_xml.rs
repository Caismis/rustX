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
//! # The grammar this module depends on
//!
//! Upstream (`vllm/parser/qwen3.py`) recognizes the dialect with a terminal
//! driven state machine over reserved markers — `<tool_call>`,
//! `</tool_call>`, `<function=`, `</function>`, `<parameter=`,
//! `</parameter>` — and extracts parameters with a `re.DOTALL` pattern
//! whose value group is `(.*?)`. Two consequences matter here:
//!
//! - **Newline placement is not protocol.** The template pretty-prints an
//!   emission, and upstream trims *at most one* wrapping newline from a
//!   parameter value, which is exactly what a decoration rather than a
//!   delimiter looks like. `<parameter=path>notes.txt</parameter>` and a
//!   fully compact
//!   `<tool_call><function=write_file><parameter=path>notes.txt</parameter></function></tool_call>`
//!   are the same emission as the pretty-printed form, and upstream's own
//!   fixtures use the inline shape.
//! - **A residual region is still an emission.** Upstream transitions into a
//!   tool call on a bare `<function=` with no preceding `<tool_call>`, so a
//!   partially consumed region that survives into ordinary output is a leak
//!   and not decorative text.
//!
//! So recognition here follows the reserved grammar, never a pretty-printed
//! layout. A line break is used only as the boundary of a *sentence*, which
//! is what a line break means in natural language; it is never treated as a
//! protocol delimiter, and every reserved region is recognized identically
//! whether it arrives compact or pretty-printed, in one provider chunk or
//! many. The scan runs on the fully assembled generated output, so chunk
//! boundaries cannot matter.
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
//! instead recognizes the *emission structure*, using evidence a discussion
//! of the syntax does not produce:
//!
//! - **Reserved markup, not sentence material.** A reserved opener is
//!   evidence only when what immediately precedes it within the sentence is
//!   reserved markup rather than ordinary words. Prose introduces the tag
//!   (`... is encoded as <parameter=path>`); an emission reaches it from the
//!   envelope around it, or from nothing at all.
//! - **Quotation is not emission.** Tags inside a fenced code block are
//!   quoted syntax — how a model shows the reader what the dialect looks
//!   like — and are skipped.
//! - **Real envelope structure.** An opener must be matched by its own
//!   closer, in order, and the opener's payload must be a plausible
//!   function/parameter identifier. An illustrative `<function=...>` or a
//!   `<tool_call>` wrapping a literal `...` is not a protocol region.
//!
//! The recognizer is deliberately conservative: it would rather miss a
//! speculative shape than reclassify a correct answer, and it is bounded to
//! one forward pass with constant state — no backtracking, no materialized
//! regions, no general XML parsing.
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

/// The reserved terminals of the dialect, named exactly as upstream names
/// them. `<tool_call>` and `</tool_call>` carry no envelope of their own —
/// they wrap one — but they are still reserved markup rather than words,
/// which is what lets a compact emission be recognized inside them.
const TOOL_CALL_OPEN: &str = "<tool_call>";
const TOOL_CALL_CLOSE: &str = "</tool_call>";
const FUNCTION_OPEN_PREFIX: &str = "<function=";
const FUNCTION_CLOSE: &str = "</function>";
const PARAMETER_OPEN_PREFIX: &str = "<parameter=";
const PARAMETER_CLOSE: &str = "</parameter>";

/// The longest payload accepted inside a reserved opener. A tool name or a
/// parameter key is an identifier; anything longer is prose that happens to
/// begin with the reserved prefix, and bounding it keeps the opener probe
/// constant work.
const MAX_RESERVED_NAME_BYTES: usize = 128;

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

/// Matches a reserved opener at the head of `rest`, returning what follows
/// it. The payload search is bounded by [`MAX_RESERVED_NAME_BYTES`], so an
/// unterminated `<parameter=` in prose costs a bounded probe rather than a
/// scan to the end of the output.
fn reserved_opener<'a>(rest: &'a str, prefix: &str) -> Option<&'a str> {
    let after = rest.strip_prefix(prefix)?;
    let close = after
        .char_indices()
        .take_while(|(offset, _)| *offset <= MAX_RESERVED_NAME_BYTES)
        .find(|(_, character)| *character == '>')
        .map(|(offset, _)| offset)?;
    is_reserved_name(&after[..close]).then(|| &after[close + '>'.len_utf8()..])
}

/// Whether a line opens or closes a Markdown code fence.
fn is_code_fence(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

/// The bounded recognizer state: which reserved envelopes are open, and
/// whether ordinary sentence text stands between the current sentence
/// boundary and here.
#[derive(Debug, Default)]
struct EmissionScan {
    function_open: bool,
    parameter_open: bool,
    /// Set by ordinary words, cleared at a sentence boundary, and left
    /// untouched by reserved markup. An opener preceded by words is a tag
    /// quoted in a sentence; an opener preceded only by reserved markup (or
    /// by nothing) is emitted structure.
    words_precede: bool,
}

impl EmissionScan {
    /// Consumes one line of unfenced output, carrying envelope state across
    /// the call so a region may span lines or sit entirely within one.
    ///
    /// Returns the envelope as soon as a reserved opener is matched by its
    /// own closer.
    fn consume(&mut self, line: &str) -> Option<QwenReservedEnvelope> {
        // A line break ends a sentence, so words on the previous line do not
        // make the next line's markup a quotation. This is the only thing
        // layout decides; the reserved grammar below never consults it.
        self.words_precede = false;
        let mut rest = line;
        while let Some(character) = rest.chars().next() {
            if let Some(tail) = rest
                .strip_prefix(TOOL_CALL_OPEN)
                .or_else(|| rest.strip_prefix(TOOL_CALL_CLOSE))
            {
                rest = tail;
                continue;
            }
            if let Some(tail) = rest.strip_prefix(FUNCTION_CLOSE) {
                rest = tail;
                if std::mem::take(&mut self.function_open) {
                    return Some(QwenReservedEnvelope::Function);
                }
                continue;
            }
            if let Some(tail) = rest.strip_prefix(PARAMETER_CLOSE) {
                rest = tail;
                if std::mem::take(&mut self.parameter_open) {
                    return Some(QwenReservedEnvelope::Parameter);
                }
                continue;
            }
            // An opener is structure only where words do not introduce it.
            // Where they do, the tag is left to be consumed as the sentence
            // material it is.
            if !self.words_precede {
                if let Some(tail) = reserved_opener(rest, FUNCTION_OPEN_PREFIX) {
                    rest = tail;
                    self.function_open = true;
                    continue;
                }
                if let Some(tail) = reserved_opener(rest, PARAMETER_OPEN_PREFIX) {
                    rest = tail;
                    self.parameter_open = true;
                    continue;
                }
            }
            rest = &rest[character.len_utf8()..];
            if !character.is_whitespace() {
                self.words_precede = true;
            }
        }
        None
    }

    /// Forgets any open envelope. A region never straddles quoted syntax, so
    /// a fence boundary cannot complete one.
    fn reset_envelopes(&mut self) {
        self.function_open = false;
        self.parameter_open = false;
    }
}

/// Recognizes an actual Qwen tool-protocol emission in generated output.
///
/// Returns the reserved envelope whose complete region was found, or `None`
/// when the output merely mentions, quotes, or discusses the dialect. The
/// scan is a single bounded forward pass with constant state: it never
/// backtracks and never materializes the candidate regions.
pub(crate) fn tool_protocol_emission(output: &str) -> Option<QwenReservedEnvelope> {
    let mut inside_fence = false;
    let mut scan = EmissionScan::default();
    for line in output.lines() {
        if is_code_fence(line) {
            inside_fence = !inside_fence;
            scan.reset_envelopes();
            continue;
        }
        if inside_fence {
            continue;
        }
        if let Some(envelope) = scan.consume(line) {
            return Some(envelope);
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

    /// The same emission written compactly — every reserved tag on one line,
    /// which is the shape upstream's own parser fixtures use — is the same
    /// emission. Newline placement is decoration, not protocol.
    #[test]
    fn a_compact_nested_emission_is_recognized() {
        let output = "<tool_call><function=write_file><parameter=path>notes.txt</parameter>\
                      </function></tool_call>";
        assert!(tool_protocol_emission(output).is_some());
    }

    /// A partially consumed emission — the serving stack stripped the outer
    /// envelope and left the parameter region behind — is still an emission,
    /// pretty-printed or inline.
    #[test]
    fn a_residual_parameter_region_is_recognized() {
        for output in [
            "<parameter=path>\nnotes.txt\n</parameter>\n",
            "<parameter=path>notes.txt</parameter>",
        ] {
            assert_eq!(
                tool_protocol_emission(output),
                Some(QwenReservedEnvelope::Parameter),
                "not recognized: {output:?}"
            );
        }
    }

    /// A parameterless call still leaks a complete function region, in
    /// either layout.
    #[test]
    fn a_residual_function_region_is_recognized() {
        for output in [
            "<tool_call>\n<function=list_directory>\n</function>\n</tool_call>",
            "<tool_call><function=list_directory></function></tool_call>",
        ] {
            assert_eq!(
                tool_protocol_emission(output),
                Some(QwenReservedEnvelope::Function),
                "not recognized: {output:?}"
            );
        }
    }

    /// Layout is not evidence in either direction: an emission wrapped
    /// across lines at arbitrary points is the same region.
    #[test]
    fn layout_does_not_decide_recognition() {
        let compact = "<tool_call><function=write_file><parameter=path>notes.txt</parameter>\
                       </function></tool_call>";
        let split = "<tool_call><function=write_file>\n<parameter=path>notes.txt\
                     </parameter></function>\n</tool_call>";
        assert_eq!(
            tool_protocol_emission(compact),
            tool_protocol_emission(split)
        );
        assert!(tool_protocol_emission(split).is_some());
    }

    /// Exact reserved syntax quoted inside a sentence is prose. This is the
    /// case a `contains(open) && contains(close)` rule gets wrong.
    #[test]
    fn reserved_syntax_inside_a_sentence_is_prose() {
        let output = "A parameter is encoded as <parameter=path>...</parameter>, and the \
                      function as <function=write_file>...</function> inside it.";
        assert_eq!(tool_protocol_emission(output), None);
    }

    /// The same sentence with a real-looking value rather than an ellipsis
    /// is still a sentence: words introduce the tag, so it is quotation.
    #[test]
    fn reserved_syntax_with_a_real_value_inside_a_sentence_is_prose() {
        let output = "A parameter is encoded as <parameter=path>notes.txt</parameter> in the \
                      body.";
        assert_eq!(tool_protocol_emission(output), None);
        let function = "Qwen uses <function=write_file>...</function> for function calls.";
        assert_eq!(tool_protocol_emission(function), None);
    }

    /// A sentence that quotes the whole compact envelope is still a
    /// sentence: `<tool_call>` is reserved markup, but it does not erase the
    /// words that introduced it.
    #[test]
    fn a_compact_envelope_quoted_in_a_sentence_is_prose() {
        let output = "Wrap it in <tool_call><function=write_file><parameter=path>notes.txt\
                      </parameter></function></tool_call> to call the tool.";
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
        let compact_placeholders = "<tool_call><function=...><parameter=...>value</parameter>\
                                    </function></tool_call>";
        assert_eq!(tool_protocol_emission(compact_placeholders), None);
    }

    /// A fenced code block is quoted syntax, which is how a model shows the
    /// reader the dialect. Quotation is not emission — in either layout.
    #[test]
    fn a_fenced_example_is_quoted_not_emitted() {
        let output = "Here is the shape:\n\n```xml\n<tool_call>\n<function=write_file>\n\
                      <parameter=path>\nnotes.txt\n</parameter>\n</function>\n</tool_call>\n```\n\
                      Use it exactly like that.";
        assert_eq!(tool_protocol_emission(output), None);
        let compact = "Here is the shape:\n\n```xml\n<tool_call><function=write_file>\
                       <parameter=path>notes.txt</parameter></function></tool_call>\n```\n";
        assert_eq!(tool_protocol_emission(compact), None);
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
        assert_eq!(
            tool_protocol_emission("</parameter><parameter=path>notes.txt"),
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

    /// An unterminated reserved prefix in prose costs a bounded probe and
    /// never becomes an opener.
    #[test]
    fn an_unterminated_reserved_prefix_is_not_an_opener() {
        let long = format!("<parameter={}", "a".repeat(4096));
        assert_eq!(tool_protocol_emission(&long), None);
        let overlong_name = format!("<parameter={}>value</parameter>", "a".repeat(200));
        assert_eq!(tool_protocol_emission(&overlong_name), None);
    }
}

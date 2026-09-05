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
//! layout. Every reserved region is recognized identically whether it
//! arrives compact or pretty-printed, in one provider chunk or many. The
//! scan runs on the fully assembled generated output, so chunk boundaries
//! cannot matter.
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
//! misclassifies a correct answer as malformed tool intent.
//!
//! # Ownership, not layout
//!
//! What this module recognizes instead is *who owns the reserved bytes*.
//!
//! A generation that leaks the dialect is a generation the serving stack was
//! supposed to consume whole: its output **is** the reserved region. A
//! generation that explains the dialect is writing a document, and the
//! reserved bytes appear inside that document as material it introduces,
//! quotes, and talks about.
//!
//! So the scan walks the generated output from its start and stays in the
//! protocol only while everything it has passed is reserved markup or the
//! payload of an open reserved envelope. The first thing that is *not* —
//! ordinary words outside any envelope, or a Markdown code fence — hands
//! ownership to the document, and the scan stops. Reserved bytes after that
//! point are the document's material: introduced, quoted, discussed.
//!
//! That ownership is a property of the output, not of its formatting. It
//! does not begin again at a line break. A line break between explanatory
//! prose and the syntax that prose introduces is presentation:
//!
//! ```text
//! The exact parameter syntax is: <parameter=path>notes.txt</parameter>
//! ```
//!
//! and
//!
//! ```text
//! The exact parameter syntax is:
//! <parameter=path>notes.txt</parameter>
//! ```
//!
//! are the same answer, and classify the same way. Only an explicit
//! structural marker carries meaning here, and the one this module honours
//! is the code fence, because a fence is the author *declaring* the block
//! quoted. Lines exist for the scan as the unit a fence is recognized on and
//! as the place a pretty-printed emission puts its tags; they are never
//! evidence that prose stopped.
//!
//! Within the protocol the region must still be real structure: an opener is
//! matched by its own closer, in order, and the opener's payload must be a
//! plausible function/parameter identifier, so an illustrative
//! `<function=...>` or a `<tool_call>` wrapping a literal `...` is not a
//! region.
//!
//! The consequence is deliberate. Reserved bytes a model produces after it
//! has begun writing a document are not classified as a leak — there is no
//! structural evidence that separates "here is the syntax:" from a leak
//! following a sentence, and enumerating English phrasings would be a
//! vocabulary rule, not a grammar. The recognizer would rather miss that
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

/// Whether a line opens or closes a Markdown code fence. A fence is the one
/// piece of layout that is an explicit declaration rather than presentation:
/// the author is marking the block as quoted.
fn is_code_fence(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

/// What one line of output settled about ownership.
enum LineOutcome {
    /// Everything so far is reserved markup or envelope payload, and no
    /// region has closed yet. The scan continues into the next line with the
    /// open envelopes it already has.
    StillProtocol,
    /// A reserved opener was matched by its own closer: a complete emitted
    /// region.
    Emission(QwenReservedEnvelope),
    /// The generation is writing a document — ordinary words outside every
    /// envelope, or an explicit code fence. Ownership has passed to the
    /// document and cannot pass back.
    Document,
}

/// The bounded recognizer state: which reserved envelopes are currently
/// open.
///
/// There is no "was there prose recently" flag, because prose ownership is
/// not something the scan re-decides. Once a line hands ownership to the
/// document the scan is over; while it is still running, everything behind
/// it is reserved markup or envelope payload by construction.
#[derive(Debug, Default)]
struct EmissionScan {
    function_open: bool,
    parameter_open: bool,
}

impl EmissionScan {
    /// Whether the scan currently stands inside a reserved envelope, where
    /// ordinary characters are the envelope's payload rather than the
    /// document's words.
    const fn inside_envelope(&self) -> bool {
        self.function_open || self.parameter_open
    }

    /// Consumes one line of output, carrying envelope state across the call
    /// so a region may span lines or sit entirely within one.
    fn consume_line(&mut self, line: &str) -> LineOutcome {
        if is_code_fence(line) {
            return LineOutcome::Document;
        }
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
                    return LineOutcome::Emission(QwenReservedEnvelope::Function);
                }
                continue;
            }
            if let Some(tail) = rest.strip_prefix(PARAMETER_CLOSE) {
                rest = tail;
                if std::mem::take(&mut self.parameter_open) {
                    return LineOutcome::Emission(QwenReservedEnvelope::Parameter);
                }
                continue;
            }
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
            // Not reserved markup. Inside an envelope this is the payload
            // upstream's `(.*?)` group would have captured; outside every
            // envelope it is the document's own words, and the document owns
            // everything from here on.
            if !character.is_whitespace() && !self.inside_envelope() {
                return LineOutcome::Document;
            }
            rest = &rest[character.len_utf8()..];
        }
        LineOutcome::StillProtocol
    }
}

/// Recognizes an actual Qwen tool-protocol emission in generated output.
///
/// Returns the reserved envelope whose complete region was found, or `None`
/// when the output merely mentions, quotes, or discusses the dialect. The
/// scan is a single bounded forward pass with constant state: it never
/// backtracks and never materializes the candidate regions.
pub(crate) fn tool_protocol_emission(output: &str) -> Option<QwenReservedEnvelope> {
    let mut scan = EmissionScan::default();
    for line in output.lines() {
        match scan.consume_line(line) {
            LineOutcome::Emission(envelope) => return Some(envelope),
            LineOutcome::Document => return None,
            LineOutcome::StillProtocol => {}
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
    /// either layout, wrapped or bare.
    #[test]
    fn a_residual_function_region_is_recognized() {
        for output in [
            "<tool_call>\n<function=list_directory>\n</function>\n</tool_call>",
            "<tool_call><function=list_directory></function></tool_call>",
            "<function=list_directory></function>",
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
    /// is still a document: prose owns the reserved bytes it introduces.
    #[test]
    fn reserved_syntax_with_a_real_value_inside_a_sentence_is_prose() {
        let output = "A parameter is encoded as <parameter=path>notes.txt</parameter> in the \
                      body.";
        assert_eq!(tool_protocol_emission(output), None);
        let function = "Qwen uses <function=write_file>...</function> for function calls.";
        assert_eq!(tool_protocol_emission(function), None);
    }

    /// A sentence that quotes the whole compact envelope is still a
    /// document: `<tool_call>` is reserved markup, but it does not take
    /// ownership back from the words that introduced it.
    #[test]
    fn a_compact_envelope_quoted_in_a_sentence_is_prose() {
        let output = "Wrap it in <tool_call><function=write_file><parameter=path>notes.txt\
                      </parameter></function></tool_call> to call the tool.";
        assert_eq!(tool_protocol_emission(output), None);
    }

    /// The invariant this recognizer is built around: a line break between
    /// explanatory prose and the syntax it introduces is presentation, so
    /// the two spellings of one answer classify identically. Every pairing
    /// here is the same text with the separating space swapped for a
    /// newline.
    #[test]
    fn a_line_break_does_not_change_classification() {
        for (inline, broken) in [
            (
                "The exact parameter syntax is: <parameter=path>notes.txt</parameter>",
                "The exact parameter syntax is:\n<parameter=path>notes.txt</parameter>",
            ),
            (
                "The function form is: <function=write_file>...</function>",
                "The function form is:\n<function=write_file>...</function>",
            ),
            (
                "The complete compact example is: <tool_call><function=write_file>\
                 <parameter=path>notes.txt</parameter></function></tool_call>",
                "The complete compact example is:\n<tool_call><function=write_file>\
                 <parameter=path>notes.txt</parameter></function></tool_call>",
            ),
            (
                "The pretty-printed form is: <tool_call> <function=write_file> \
                 <parameter=path> notes.txt </parameter> </function> </tool_call>",
                "The pretty-printed form is:\n<tool_call>\n<function=write_file>\n\
                 <parameter=path>\nnotes.txt\n</parameter>\n</function>\n</tool_call>",
            ),
        ] {
            assert_eq!(
                tool_protocol_emission(inline),
                tool_protocol_emission(broken),
                "layout changed classification: {inline:?} vs {broken:?}"
            );
            assert_eq!(tool_protocol_emission(broken), None, "{broken:?}");
        }
    }

    /// Prose introduces an opener on one line and its closer only arrives
    /// several lines later, inside more prose. No envelope may be left open
    /// across the explanation and completed by the sentence that describes
    /// the closing tag.
    #[test]
    fn an_explanation_split_across_lines_never_completes_an_envelope() {
        let output = "The opening tag is:\n<parameter=path>\nThe closing tag is </parameter>.";
        assert_eq!(tool_protocol_emission(output), None);
        let function = "The opening tag is:\n<function=write_file>\nand it ends at </function>.";
        assert_eq!(tool_protocol_emission(function), None);
    }

    /// Once the generation is writing a document, reserved bytes further
    /// down are that document's material however they are laid out —
    /// including a blank line or a Markdown list between the prose and the
    /// example.
    #[test]
    fn a_document_keeps_ownership_of_later_reserved_bytes() {
        let paragraph = "Here is the syntax.\n\n<parameter=path>notes.txt</parameter>";
        assert_eq!(tool_protocol_emission(paragraph), None);
        let list = "The pieces are:\n\n- <function=write_file></function>\n\
                    - <parameter=path>notes.txt</parameter>\n";
        assert_eq!(tool_protocol_emission(list), None);
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
    /// reader the dialect. Quotation is not emission — in either layout, and
    /// whether or not a sentence introduces the fence.
    #[test]
    fn a_fenced_example_is_quoted_not_emitted() {
        let output = "Here is the shape:\n\n```xml\n<tool_call>\n<function=write_file>\n\
                      <parameter=path>\nnotes.txt\n</parameter>\n</function>\n</tool_call>\n```\n\
                      Use it exactly like that.";
        assert_eq!(tool_protocol_emission(output), None);
        let compact = "Here is the shape:\n\n```xml\n<tool_call><function=write_file>\
                       <parameter=path>notes.txt</parameter></function></tool_call>\n```\n";
        assert_eq!(tool_protocol_emission(compact), None);
        let bare_fence = "```xml\n<tool_call><function=write_file><parameter=path>notes.txt\
                          </parameter></function></tool_call>\n```\n";
        assert_eq!(tool_protocol_emission(bare_fence), None);
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

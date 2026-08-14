//! Native Grep tool (M5).
//!
//! Searches the workspace for lines matching a pattern. There is no `grep`
//! or `rg` subprocess: the ripgrep crates are linked directly, and the
//! ownership split between them and rustX is explicit.
//!
//! ```text
//! Grep contract/executor          <- rustX: what may be searched, what is returned
//!         |
//!         v
//! shared native-search traversal  <- rustX: the file universe (see search/)
//!         |
//!         v
//! grep-regex / grep-searcher      <- how matching happens inside one file
//! ```
//!
//! The shared substrate decides *which files exist*; the ripgrep engine only
//! decides *how a match is found inside a file it is handed*. `grep-searcher`
//! never traverses the workspace, and its defaults never become rustX
//! semantics.
//!
//! # Result semantics
//!
//! - **Eligibility.** Grep searches UTF-8 text files. A file whose bytes are
//!   not valid UTF-8 is not searched and contributes no matches; binary
//!   content is never fabricated as text.
//! - **Ordering.** Matches are reported in relative path order, then line
//!   number, then the byte column of the match within its line. Several
//!   matches on one line are reported separately, in column order.
//! - **Context.** `context = N` also returns the `N` lines before and after
//!   each matching line. The merge policy is a set union: every source line
//!   appears exactly once. A line that contains a match is reported as a
//!   match (once per match on it) and never additionally as a context line,
//!   so overlapping and adjacent context windows collapse into one run of
//!   distinct lines instead of duplicating them.
//! - **Bounds.** At most `limit` matches (default
//!   [`DEFAULT_GREP_MATCHES`](crate::tools::limits::DEFAULT_GREP_MATCHES),
//!   hard cap [`MAX_GREP_MATCHES`](crate::tools::limits::MAX_GREP_MATCHES))
//!   are returned, and the payload is bounded by
//!   [`MAX_MODEL_TOOL_RESULT_BYTES`]. Reaching either bound sets the
//!   explicit truncation state; nothing is dropped silently.
//! - **Long lines.** A reported line longer than [`MAX_GREP_LINE_BYTES`] is
//!   shortened with an explicit truncation marker. `column` always refers to
//!   the original, untruncated line.
//!
//! The model-facing argument contract is the typed [`GrepInput`]; the
//! canonical schema is generated from it.

mod input;

use futures_util::future::BoxFuture;
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{Searcher, SearcherBuilder, SinkContext, SinkContextKind, SinkMatch};

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{
    MAX_GREP_LINE_BYTES, MAX_MODEL_TOOL_RESULT_BYTES, bounded_text_preview,
};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::search::{SearchFile, SearchRoot};
use crate::tools::native::support::{failed_result, success_json_with};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation, TruncationState};

use input::GrepInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "grep";

/// The serialized cost one reported line adds beyond its own text (the JSON
/// keys, the path, and the numbers). The byte cap is a deterministic bound
/// on the model-facing payload, not an exact serialization measure.
const LINE_ENVELOPE_BYTES: usize = 48;

/// The tool-owned registration of the native Grep tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<GrepInput>(
            "tool-grep",
            NAME,
            "Search workspace files for lines matching a pattern. Returns matches ordered by \
             path, line, and column. Hidden files are included and ignore files such as \
             .gitignore are not applied.",
            policy,
        ),
        std::sync::Arc::new(GrepTool),
    )
}

/// The native Grep executor.
pub struct GrepTool;

impl ToolExecutor for GrepTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move { run_grep(&invocation, &context) })
    }
}

/// One reported match, carrying its deterministic ordering fields.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct Match {
    /// The path relative to the search root.
    path: String,
    /// The 1-based line number of the matching line.
    line: u64,
    /// The 1-based byte column of the match inside the original line.
    column: u64,
    /// The matching line, bounded by [`MAX_GREP_LINE_BYTES`].
    text: String,
}

/// One reported context line.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct ContextLine {
    /// The path relative to the search root.
    path: String,
    /// The 1-based line number of the context line.
    line: u64,
    /// The context line, bounded by [`MAX_GREP_LINE_BYTES`].
    text: String,
}

#[allow(clippy::too_many_lines)] // one coherent compile/traverse/search pipeline
fn run_grep(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let input = match GrepInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let pattern = input.pattern.as_str();
    let matcher = match build_matcher(pattern, input.literal(), input.ignore_case()) {
        Ok(matcher) => matcher,
        Err(error) => return failed_result(format!("invalid pattern {pattern:?}: {error}")),
    };
    let file_filter = match input.glob.as_deref() {
        None => None,
        Some(glob) => match globset::GlobBuilder::new(glob)
            .literal_separator(true)
            .build()
            .map(|compiled| compiled.compile_matcher())
        {
            Ok(compiled) => Some(compiled),
            Err(error) => return failed_result(format!("invalid glob {glob:?}: {error}")),
        },
    };
    let root = match SearchRoot::resolve(context.workspace, input.path.as_deref()) {
        Ok(root) => root,
        Err(error) => return failed_result(error),
    };
    let files = match root.files() {
        Ok(files) => files,
        Err(error) => return failed_result(error),
    };

    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        // One match report per line: rustX owns the line-addressable
        // contract, so multi-line matching is deliberately never enabled.
        .multi_line(false)
        // Eligibility is decided by rustX below (valid UTF-8 or not
        // searched at all), so the engine applies no detection of its own.
        .binary_detection(grep_searcher::BinaryDetection::none())
        .before_context(input.context())
        .after_context(input.context())
        .build();

    let mut collector = Collector::new(input.limit());
    for file in files {
        if file_filter
            .as_ref()
            .is_some_and(|filter| !filter.is_match(&file.relative))
        {
            continue;
        }
        if collector.exhausted() {
            break;
        }
        if let Err(error) = search_file(&mut searcher, &matcher, &file, &mut collector) {
            return failed_result(error);
        }
    }

    let Collector {
        matches,
        context: context_lines,
        truncated,
        ..
    } = collector;
    success_json_with(
        serde_json::json!({
            "matches": matches,
            "context": context_lines,
            "truncated": truncated,
        }),
        truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: None,
        }),
        Vec::new(),
    )
}

/// Compiles the model's pattern into the ripgrep matcher.
///
/// `literal = true` searches the pattern as fixed text, so the model never
/// has to escape regex metacharacters itself.
fn build_matcher(
    pattern: &str,
    literal: bool,
    ignore_case: bool,
) -> Result<RegexMatcher, grep_regex::Error> {
    RegexMatcherBuilder::new()
        .fixed_strings(literal)
        .case_insensitive(ignore_case)
        .multi_line(false)
        .build(pattern)
}

/// Searches one eligible file and feeds its matches and context lines to the
/// collector.
///
/// A file whose bytes are not valid UTF-8 is skipped: it is not searched, it
/// produces no matches, and it is not an error.
fn search_file(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    file: &SearchFile,
    collector: &mut Collector,
) -> Result<(), String> {
    let Ok(bytes) = std::fs::read(&file.absolute) else {
        // An unreadable workspace file narrows the universe rather than
        // failing the whole search; the traversal already proved it exists.
        return Ok(());
    };
    if std::str::from_utf8(&bytes).is_err() {
        return Ok(());
    }
    let mut sink = FileSink {
        path: file.relative.as_str(),
        matcher,
        collector,
        error: None,
    };
    searcher
        .search_slice(matcher, &bytes, &mut sink)
        .map_err(|error| format!("cannot search {}: {error}", file.relative))?;
    match sink.error.take() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// The bounded, deterministically ordered accumulation of one Grep run.
struct Collector {
    /// The reported matches, in path/line/column order.
    matches: Vec<Match>,
    /// The reported context lines, in path/line order.
    context: Vec<ContextLine>,
    /// Whether a bound cut the result short.
    truncated: bool,
    /// The maximum number of reported matches.
    limit: usize,
    /// The accumulated model-facing payload estimate.
    bytes: usize,
}

impl Collector {
    fn new(limit: usize) -> Self {
        Self {
            matches: Vec::new(),
            context: Vec::new(),
            truncated: false,
            limit,
            bytes: 0,
        }
    }

    /// Whether no further line may be accepted.
    fn exhausted(&self) -> bool {
        self.truncated
    }

    /// Accounts one reported line against the byte cap.
    ///
    /// Returns `false` once the cap is reached, which marks the result
    /// truncated and stops the search.
    fn admit(&mut self, text: &str) -> bool {
        let projected = self.bytes + text.len() + LINE_ENVELOPE_BYTES;
        if projected > MAX_MODEL_TOOL_RESULT_BYTES {
            self.truncated = true;
            return false;
        }
        self.bytes = projected;
        true
    }

    /// Records one match, or reports that the match bound was reached.
    ///
    /// Returns `false` when the search must stop.
    fn push_match(&mut self, path: &str, line: u64, column: u64, text: &str) -> bool {
        if self.matches.len() >= self.limit {
            self.truncated = true;
            return false;
        }
        if !self.admit(text) {
            return false;
        }
        self.matches.push(Match {
            path: path.to_owned(),
            line,
            column,
            text: text.to_owned(),
        });
        true
    }

    /// Records one context line.
    ///
    /// Returns `false` when the search must stop.
    fn push_context(&mut self, path: &str, line: u64, text: &str) -> bool {
        if !self.admit(text) {
            return false;
        }
        self.context.push(ContextLine {
            path: path.to_owned(),
            line,
            text: text.to_owned(),
        });
        true
    }
}

/// The `grep-searcher` sink of one file.
///
/// `grep-searcher` delivers matching lines through `matched` and context
/// lines through `context`, both in ascending line order and each source
/// line at most once — that is exactly the merge policy this tool
/// documents, so overlapping context windows need no repair here.
struct FileSink<'a> {
    /// The path reported with every line of this file.
    path: &'a str,
    /// The matcher, reused to locate each match inside a matching line.
    matcher: &'a RegexMatcher,
    /// The run-wide bounded accumulation.
    collector: &'a mut Collector,
    /// A deferred within-line matching failure.
    error: Option<String>,
}

impl grep_searcher::Sink for FileSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let Some(line_number) = mat.line_number() else {
            return Ok(true);
        };
        let raw = trim_line_terminator(mat.bytes());
        let Ok(line) = std::str::from_utf8(raw) else {
            return Ok(true);
        };
        let text = bounded_line(line);
        // Every match on the line is reported separately, in column order.
        let mut columns = Vec::new();
        if let Err(error) = self.matcher.find_iter(raw, |found| {
            columns.push(found.start() as u64 + 1);
            true
        }) {
            self.error = Some(format!("cannot locate matches in {}: {error}", self.path));
            return Ok(false);
        }
        for column in columns {
            if !self
                .collector
                .push_match(self.path, line_number, column, &text)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        context: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        // Both sides of a context window are reported identically: the
        // model gets the surrounding lines, not a before/after taxonomy.
        match context.kind() {
            SinkContextKind::Before | SinkContextKind::After => {}
            SinkContextKind::Other => return Ok(true),
        }
        let Some(line_number) = context.line_number() else {
            return Ok(true);
        };
        let Ok(line) = std::str::from_utf8(trim_line_terminator(context.bytes())) else {
            return Ok(true);
        };
        let text = bounded_line(line);
        Ok(self.collector.push_context(self.path, line_number, &text))
    }
}

/// Strips the trailing line terminator `grep-searcher` includes in the
/// reported bytes, so a reported line never carries `\n` or `\r\n`.
fn trim_line_terminator(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

/// Bounds one reported line, with an explicit marker when it is shortened.
fn bounded_line(line: &str) -> String {
    let (bounded, _) = bounded_text_preview(line.as_bytes(), MAX_GREP_LINE_BYTES);
    bounded
}

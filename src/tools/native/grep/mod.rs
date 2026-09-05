//! Native Grep tool.
//!
//! Grep keeps rustX's in-process ripgrep-crate implementation. The shared
//! traversal owns the filesystem universe; `grep-regex` and `grep-searcher`
//! only match content in files handed to them. Model-facing output is plain
//! text with a deterministic 50KB complete-line budget.

mod input;

use futures_util::future::BoxFuture;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{Searcher, SearcherBuilder, SinkContext, SinkContextKind, SinkMatch};

use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{MAX_GREP_LINE_CHARS, NATIVE_FILE_TOOL_MAX_BYTES};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::search::{SearchFile, SearchRoot};
use crate::tools::native::support::{failed_result, success_text};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation, TruncationState};

use input::GrepInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "grep";

/// The tool-owned registration of the native Grep tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<GrepInput>(
            "tool-grep",
            NAME,
            "Search UTF-8 text files for matching lines using an in-process search engine. Resolve a relative path from the execution cwd; absolute paths are used as host filesystem paths. The optional limit defaults to 100 and may be larger. Results are plain text; long lines are shortened to 500 characters and bounded results include instructions for continuing or refining the search. Hidden files are included and .gitignore behavior is unchanged.",
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

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

#[allow(clippy::too_many_lines)]
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
    let root = match SearchRoot::resolve(context.workspace.root(), input.path.as_deref()) {
        Ok(root) => root,
        Err(error) => return failed_result(error),
    };
    let files = match root.files() {
        Ok(files) => files,
        Err(error) => return failed_result(error),
    };

    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .multi_line(false)
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

    let text = collector.render();
    if text == "No matches found" {
        return success_text(text, None);
    }
    let truncated = collector.is_truncated();
    success_text(
        text,
        truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: None,
        }),
    )
}

/// Compiles the model pattern into the in-process ripgrep matcher.
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

fn search_file(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    file: &SearchFile,
    collector: &mut Collector,
) -> Result<(), String> {
    let bytes = std::fs::read(&file.absolute)
        .map_err(|error| format!("cannot read {}: {error}", file.relative))?;
    if std::str::from_utf8(&bytes).is_err() {
        return Ok(());
    }
    let mut sink = FileSink {
        path: file.relative.as_str(),
        collector,
    };
    searcher
        .search_slice(matcher, &bytes, &mut sink)
        .map_err(|error| format!("cannot search {}: {error}", file.relative))?;
    Ok(())
}

/// The bounded plain-text accumulation of one Grep run.
struct Collector {
    lines: Vec<String>,
    matches: u64,
    limit: u64,
    bytes: usize,
    byte_limit_reached: bool,
    match_limit_reached: bool,
    lines_truncated: bool,
}

impl Collector {
    fn new(limit: u64) -> Self {
        Self {
            lines: Vec::new(),
            matches: 0,
            limit,
            bytes: 0,
            byte_limit_reached: false,
            match_limit_reached: false,
            lines_truncated: false,
        }
    }

    fn exhausted(&self) -> bool {
        self.byte_limit_reached || self.match_limit_reached
    }

    fn is_truncated(&self) -> bool {
        self.byte_limit_reached || self.match_limit_reached || self.lines_truncated
    }

    fn push_match(&mut self, path: &str, line: u64, text: &str) -> bool {
        if self.matches >= self.limit {
            self.match_limit_reached = true;
            return false;
        }
        if !self.push_line(format_line(path, line, text, true)) {
            return false;
        }
        self.matches = self.matches.saturating_add(1);
        true
    }

    fn push_context(&mut self, path: &str, line: u64, text: &str) -> bool {
        self.push_line(format_line(path, line, text, false))
    }

    fn push_line(&mut self, line: String) -> bool {
        let cost = line
            .len()
            .saturating_add(usize::from(!self.lines.is_empty()));
        if self.bytes.saturating_add(cost) > NATIVE_FILE_TOOL_MAX_BYTES {
            self.byte_limit_reached = true;
            return false;
        }
        self.bytes = self.bytes.saturating_add(cost);
        self.lines.push(line);
        true
    }

    fn render(&self) -> String {
        if self.matches == 0 && self.lines.is_empty() {
            return "No matches found".to_owned();
        }
        let mut output = self.lines.join("\n");
        let mut notices = Vec::new();
        if self.match_limit_reached {
            let suggested = self.limit.saturating_mul(2);
            notices.push(format!(
                "{} matches limit reached. Use limit={} for more, or refine pattern",
                self.limit, suggested
            ));
        }
        if self.byte_limit_reached {
            notices.push("50KB limit reached".to_owned());
        }
        if self.lines_truncated {
            notices.push(
                "Some lines truncated to 500 chars. Use read tool to see full lines".to_owned(),
            );
        }
        if !notices.is_empty() {
            output.push_str("\n\n[");
            output.push_str(&notices.join(". "));
            output.push(']');
        }
        output
    }
}

struct FileSink<'a> {
    path: &'a str,
    collector: &'a mut Collector,
}

impl grep_searcher::Sink for FileSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let Some(line_number) = mat.line_number() else {
            return Ok(true);
        };
        let Ok(line) = std::str::from_utf8(trim_line_terminator(mat.bytes())) else {
            return Ok(true);
        };
        let (text, shortened) = bounded_line(line);
        self.collector.lines_truncated |= shortened;
        Ok(self.collector.push_match(self.path, line_number, &text))
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        context: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
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
        let (text, shortened) = bounded_line(line);
        self.collector.lines_truncated |= shortened;
        Ok(self.collector.push_context(self.path, line_number, &text))
    }
}

fn format_line(path: &str, line: u64, text: &str, matched: bool) -> String {
    if matched {
        format!("{path}:{line}: {text}")
    } else {
        format!("{path}-{line}- {text}")
    }
}

fn trim_line_terminator(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

fn bounded_line(line: &str) -> (String, bool) {
    let mut chars = line.chars();
    let bounded: String = chars.by_ref().take(MAX_GREP_LINE_CHARS).collect();
    if chars.next().is_some() {
        (format!("{bounded}... [truncated]"), true)
    } else {
        (bounded, false)
    }
}

#[cfg(test)]
mod tests {
    use super::{Collector, SearchFile, build_matcher, search_file};
    use grep_searcher::SearcherBuilder;

    #[test]
    fn an_enumerated_file_that_cannot_be_read_fails_explicitly() {
        let directory = tempfile::tempdir().expect("temp dir");
        let missing_path = directory.path().join("gone.txt");
        let mut searcher = SearcherBuilder::new().line_number(true).build();
        let matcher = build_matcher("hit", false, false).expect("valid pattern");
        let mut collector = Collector::new(10);
        let missing = SearchFile {
            relative: "gone.txt".to_owned(),
            absolute: missing_path,
        };
        let error = search_file(&mut searcher, &matcher, &missing, &mut collector)
            .expect_err("an unreadable enumerated file is an execution failure");
        assert!(error.contains("gone.txt"));
        assert!(collector.lines.is_empty());
        assert!(!collector.is_truncated());
    }

    #[test]
    fn non_utf8_content_is_skipped_without_failing() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("binary.bin");
        std::fs::write(&path, b"\xff\xfehit\0").expect("write binary fixture");
        let mut searcher = SearcherBuilder::new().line_number(true).build();
        let matcher = build_matcher("hit", false, false).expect("valid pattern");
        let mut collector = Collector::new(10);
        let file = SearchFile {
            relative: "binary.bin".to_owned(),
            absolute: path,
        };
        search_file(&mut searcher, &matcher, &file, &mut collector)
            .expect("non-UTF-8 content is skipped");
        assert!(collector.lines.is_empty());
    }
}

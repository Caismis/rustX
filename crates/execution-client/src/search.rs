//! The bounded workspace search behind `ClientRequest::Search`.
//!
//! One shared file universe — identical for the `Glob` and `Grep` spec
//! variants — is enumerated descriptor-relative below the retained
//! workspace [`PackageDirectory`]: every component opens no-follow, hidden
//! and ignored files are ordinary members, symlinked and special entries
//! are never part of the universe, and a symlinked root is refused
//! outright. Traversal is fail-closed under the wire budgets — depth,
//! visited entries, retained path bytes, and per-directory names — so a
//! partial universe is never reported as complete.
//!
//! The universe is a point-in-time enumeration, not an atomic snapshot:
//! `Glob` reports the names it observed — a file created mid-traversal
//! may or may not appear, and a removed one may still be listed — while
//! `Grep` pins every selected file's enumeration stamp and reopens only
//! through `root.file` on the shared authorized prefix plus the retained
//! suffix, so a path whose target changed since enumeration is a
//! conflict, never a silently different source. A searched file is
//! re-verified descriptor-wise and name-wise before its records publish;
//! no unselected file body ever crosses the wire.
//!
//! `Glob` selects the universe files whose root-relative path matches the
//! pattern; `Grep` streams each selected file — whole-source UTF-8
//! validated first, exactly like the text scanner, so a file invalid
//! after its last match still contributes nothing — through the bounded
//! `grep-searcher` engine. Records arrive pre-projected: the Server owns
//! every notice, never the display bytes.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use rustx_confined_fs::{FileMetadata, PackageDirectory};
use rustx_execution_protocol::{
    ClientReply, DecimalU64, MAX_DIRECTORY_NAMES, MAX_SEARCH_BUFFER_BYTES,
    MAX_SEARCH_CONTEXT_LINES, MAX_SEARCH_DEPTH, MAX_SEARCH_ENTRIES, MAX_SEARCH_PATH_BYTES,
    MAX_SEARCH_PATTERN_BYTES, MAX_TEXT_BYTES, ReadError, SEARCH_PREVIEW_SCALARS, SearchSpec,
};

use crate::{read_error, relative_entry};

/// One file of the shared search universe.
struct SearchFile {
    /// The normalized forward-slash path relative to the search root —
    /// the display and matching form, lossy like the tool contract.
    relative: String,
    /// The search-root-relative path the file reopens under — joined to
    /// the one shared authorized root prefix, never a per-file copy of
    /// that prefix.
    suffix: PathBuf,
    /// The enumeration-time stamp the reopened descriptor must still
    /// name — a file replaced between enumeration and reopen is a
    /// conflict, never silently searched.
    metadata: FileMetadata,
}

/// The fail-closed traversal accounting: every visited directory entry
/// and every retained path — in both owned forms — counts against the
/// wire budgets.
struct Universe {
    files: Vec<SearchFile>,
    visited: u64,
    path_bytes: usize,
}

/// Retains one enumerated file: both owned path forms — the raw suffix
/// bytes and the lossy display string, which can expand to three times
/// the raw length for non-UTF-8 names — are charged before retention so
/// the bound cannot be under-measured.
fn retain_file(
    universe: &mut Universe,
    suffix: PathBuf,
    display: String,
    metadata: FileMetadata,
) -> Result<(), ReadError> {
    universe.path_bytes = universe
        .path_bytes
        .saturating_add(suffix.as_os_str().len())
        .saturating_add(display.len());
    if universe.path_bytes > MAX_SEARCH_PATH_BYTES {
        return Err(ReadError::SearchBudgetExceeded);
    }
    universe.files.push(SearchFile {
        relative: display,
        suffix,
        metadata,
    });
    Ok(())
}

/// A reader that refuses every physical read past `deadline`. It bounds
/// the work *between* kernel calls — a read already blocked inside the
/// kernel is not claimed to be interruptible.
struct DeadlineReader<R> {
    inner: R,
    deadline: Instant,
}

impl<R: Read> Read for DeadlineReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if Instant::now() >= self.deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "search deadline elapsed",
            ));
        }
        self.inner.read(buf)
    }
}

/// `Search`: validates the wire spec, resolves `relative_root` against
/// the retained workspace, enumerates the shared universe, and selects
/// the spec's bounded records.
pub fn search(
    root: &PackageDirectory,
    relative_root: &Path,
    spec: &SearchSpec,
    deadline: Instant,
) -> ClientReply {
    match run(root, relative_root, spec, deadline) {
        Ok((body, matches, hit_limit, byte_limit, short_lines)) => ClientReply::SearchPage {
            body,
            matches: DecimalU64::new(matches),
            hit_limit,
            byte_limit,
            short_lines,
        },
        Err(kind) => ClientReply::Error { kind },
    }
}

fn run(
    root: &PackageDirectory,
    relative_root: &Path,
    spec: &SearchSpec,
    deadline: Instant,
) -> Result<(String, u64, bool, bool, bool), ReadError> {
    // The lexical gate and the spec's own bounds run before any physical
    // work; the Server validates the same spelling on its side, so a
    // violation here means the peer sent a request it never checked.
    let relative = relative_entry(relative_root)?;
    let invalid = match spec {
        SearchSpec::Glob { pattern, limit } => {
            pattern.len() > MAX_SEARCH_PATTERN_BYTES || limit.get() == 0
        }
        SearchSpec::Grep {
            pattern,
            glob,
            context,
            limit,
            ..
        } => {
            pattern.len() > MAX_SEARCH_PATTERN_BYTES
                || glob
                    .as_ref()
                    .is_some_and(|glob| glob.len() > MAX_SEARCH_PATTERN_BYTES)
                || *context > MAX_SEARCH_CONTEXT_LINES
                || limit.get() == 0
        }
    };
    if invalid {
        return Err(ReadError::Invalid);
    }
    // The root is stamped without following: a symlink root is refused,
    // a file root is a whole universe for grep only, and anything that
    // is neither file nor directory is absent.
    let metadata = root
        .entry_metadata(relative)
        .map_err(|error| read_error(&error))?;
    if metadata.is_symlink {
        return Err(ReadError::Denied);
    }
    let mut universe = Universe {
        files: Vec::new(),
        visited: 0,
        path_bytes: 0,
    };
    // `root_prefix` is the single authorized workspace-relative prefix
    // every retained file reopens beneath — the directory itself for a
    // directory root, the parent for a single-file root — so no file
    // stores a second copy of it.
    let root_prefix: PathBuf;
    if metadata.is_file {
        let SearchSpec::Grep { .. } = spec else {
            return Err(ReadError::SearchNeedsDirectory);
        };
        let name = relative
            .file_name()
            .map_or_else(|| relative.as_os_str(), |name| name);
        root_prefix = relative.parent().unwrap_or(Path::new("")).to_path_buf();
        retain_file(
            &mut universe,
            PathBuf::from(name),
            name.to_string_lossy().into_owned(),
            metadata.clone(),
        )?;
    } else if metadata.is_directory {
        root_prefix = relative.to_path_buf();
        traverse(
            &root.descend(relative).map_err(|error| read_error(&error))?,
            Path::new(""),
            0,
            &mut universe,
            deadline,
        )?;
        universe
            .files
            .sort_by(|left, right| left.relative.cmp(&right.relative));
    } else {
        return Err(ReadError::NotFound);
    }
    match spec {
        SearchSpec::Glob { pattern, limit } => {
            glob(&universe.files, pattern, limit.get(), deadline)
        }
        SearchSpec::Grep {
            pattern,
            glob,
            literal,
            ignore_case,
            context,
            limit,
        } => grep(
            root,
            &root_prefix,
            &universe.files,
            pattern,
            glob.as_deref(),
            *literal,
            *ignore_case,
            *context,
            limit.get(),
            deadline,
        ),
    }
}

/// The shared bounded traversal: `dir` is the already-descended search
/// root and `prefix` the display path relative to it. Every entry is
/// stamped no-follow; directories recurse no-follow through
/// [`PackageDirectory::child`], regular files join the universe, and
/// everything else is skipped. Every budget is fail-closed — the wire
/// never publishes a partial universe.
fn traverse(
    dir: &PackageDirectory,
    prefix: &Path,
    depth: u32,
    universe: &mut Universe,
    deadline: Instant,
) -> Result<(), ReadError> {
    if depth > MAX_SEARCH_DEPTH {
        return Err(ReadError::SearchBudgetExceeded);
    }
    if Instant::now() >= deadline {
        return Err(ReadError::Unavailable);
    }
    let names = dir.names(MAX_DIRECTORY_NAMES).map_err(|error| {
        // `names` fails closed past the directory-entry bound with a
        // constructed error — an OS read or a replaced chain carries a
        // raw error or a distinct kind.
        if error.raw_os_error().is_none() && error.kind() == std::io::ErrorKind::Other {
            ReadError::SearchBudgetExceeded
        } else {
            read_error(&error)
        }
    })?;
    for name in names {
        if Instant::now() >= deadline {
            return Err(ReadError::Unavailable);
        }
        universe.visited += 1;
        if universe.visited > MAX_SEARCH_ENTRIES {
            return Err(ReadError::SearchBudgetExceeded);
        }
        let metadata = dir
            .entry_metadata(&name)
            .map_err(|error| read_error(&error))?;
        let relative = prefix.join(&name);
        if metadata.is_directory {
            traverse(
                &dir.child(name.as_os_str())
                    .map_err(|error| read_error(&error))?,
                &relative,
                depth + 1,
                universe,
                deadline,
            )?;
        } else if metadata.is_file {
            let suffix = relative.clone();
            retain_file(universe, suffix, display_relative(&relative), metadata)?;
        }
        // Symlinked and special entries are never part of the universe.
    }
    Ok(())
}

/// The normalized forward-slash display form of a relative path — the
/// same per-component lossy rendering the pre-migration tools reported.
fn display_relative(relative: &Path) -> String {
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// `Glob`: the matching relative paths, in the universe's global
/// bytewise lexical order, under the selected-record and joined-body
/// budgets. `hit_limit` is a proof: it is set only when a further match
/// was observed and excluded.
fn glob(
    files: &[SearchFile],
    pattern: &str,
    limit: u64,
    deadline: Instant,
) -> Result<(String, u64, bool, bool, bool), ReadError> {
    let matcher = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|_| ReadError::Invalid)?
        .compile_matcher();
    let mut records = Vec::new();
    let mut bytes = 0_usize;
    let mut hit_limit = false;
    let mut byte_limit = false;
    for file in files {
        if Instant::now() >= deadline {
            return Err(ReadError::Unavailable);
        }
        if !matcher.is_match(&file.relative) {
            continue;
        }
        if records.len() as u64 >= limit {
            hit_limit = true;
            break;
        }
        let cost = file
            .relative
            .len()
            .saturating_add(usize::from(!records.is_empty()));
        if bytes.saturating_add(cost) > MAX_TEXT_BYTES {
            byte_limit = true;
            break;
        }
        bytes = bytes.saturating_add(cost);
        records.push(file.relative.clone());
    }
    Ok((
        records.join("\n"),
        records.len() as u64,
        hit_limit,
        byte_limit,
        false,
    ))
}

/// `Grep`: every selected file — narrowed by `glob` — is whole-source
/// UTF-8 validated, then streamed through the bounded searcher under
/// the stamped-size reader; a file that changed across the pass is a
/// conflict, and a file that cannot fit the wire working budgets is a
/// typed budget error, never silent partial output.
#[allow(clippy::too_many_arguments)] // one search's full context
fn grep(
    root: &PackageDirectory,
    root_prefix: &Path,
    files: &[SearchFile],
    pattern: &str,
    glob: Option<&str>,
    literal: bool,
    ignore_case: bool,
    context: u32,
    limit: u64,
    deadline: Instant,
) -> Result<(String, u64, bool, bool, bool), ReadError> {
    let matcher = grep_regex::RegexMatcherBuilder::new()
        .fixed_strings(literal)
        .case_insensitive(ignore_case)
        .multi_line(false)
        // The compiled and determinization budgets share the wire's grep
        // working bound: a regex that cannot fit fails at compile, never
        // mid-file.
        .size_limit(MAX_SEARCH_BUFFER_BYTES)
        .dfa_size_limit(MAX_SEARCH_BUFFER_BYTES)
        .build(pattern)
        .map_err(|_| ReadError::Invalid)?;
    let filter = glob
        .map(|glob| {
            globset::GlobBuilder::new(glob)
                .literal_separator(true)
                .build()
                .map(|compiled| compiled.compile_matcher())
        })
        .transpose()
        .map_err(|_| ReadError::Invalid)?;
    let mut searcher = grep_searcher::SearcherBuilder::new()
        .line_number(true)
        .multi_line(false)
        .binary_detection(grep_searcher::BinaryDetection::none())
        .before_context(usize::try_from(context).unwrap_or(usize::MAX))
        .after_context(usize::try_from(context).unwrap_or(usize::MAX))
        .heap_limit(Some(MAX_SEARCH_BUFFER_BYTES))
        .build();
    let mut collector = Collector::new(limit);
    for file in files {
        if Instant::now() >= deadline {
            return Err(ReadError::Unavailable);
        }
        if filter
            .as_ref()
            .is_some_and(|filter| !filter.is_match(&file.relative))
        {
            continue;
        }
        if collector.exhausted() {
            break;
        }
        search_file(
            root,
            root_prefix,
            &mut searcher,
            &matcher,
            file,
            &mut collector,
            deadline,
        )?;
    }
    Ok((
        collector.lines.join("\n"),
        collector.matches,
        collector.match_limit_reached,
        collector.byte_limit_reached,
        collector.lines_truncated,
    ))
}

/// One selected file's grep pass: reopened only through `root.file` on
/// the shared authorized prefix joined to the retained suffix, then
/// pinned to the enumeration stamp — a path whose named target changed
/// since enumeration is a conflict, not a silent new source. The pinned
/// descriptor is whole-source UTF-8 validated on the stamped-size
/// deadline-bounded reader, rewound on the same descriptor, then matched
/// under the bounded searcher. The retained descriptor and a freshly
/// opened path must still name the same file afterwards — a mid-search
/// change is a conflict, never unverified records. A file invalid
/// anywhere is skipped entirely.
fn search_file(
    root: &PackageDirectory,
    root_prefix: &Path,
    searcher: &mut grep_searcher::Searcher,
    matcher: &grep_regex::RegexMatcher,
    file: &SearchFile,
    collector: &mut Collector,
    deadline: Instant,
) -> Result<(), ReadError> {
    let real = root_prefix.join(&file.suffix);
    let handle = root.file(&real).map_err(|error| read_error(&error))?;
    let before = FileMetadata::of(&handle).map_err(|_| ReadError::Unavailable)?;
    if before != file.metadata {
        return Err(ReadError::Conflict);
    }
    let validating = DeadlineReader {
        inner: (&handle).take(before.size),
        deadline,
    };
    match rustx_confined_fs::validate_utf8(validating) {
        Ok(observed) if observed == before.size => {}
        Ok(_) => return Err(ReadError::Conflict),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            return recheck(root, &handle, &before, &real);
        }
        Err(error) => return Err(read_error(&error)),
    }
    // The same descriptor rewinds: the match reads the bytes the
    // validation pass proved, bounded again at the stamped size.
    (&handle)
        .seek(SeekFrom::Start(0))
        .map_err(|error| read_error(&error))?;
    let mut sink = FileSink {
        path: file.relative.as_str(),
        collector,
    };
    let matching = DeadlineReader {
        inner: (&handle).take(before.size),
        deadline,
    };
    searcher
        .search_reader(matcher, matching, &mut sink)
        .map_err(|error| {
            // A deadline refusal surfaces as its own kind; the searcher
            // routes its own internal failures — a line the heap bound
            // cannot stage, a pattern budget the match engine exceeds —
            // through the sink error without an OS cause; the wire
            // reports them as the typed budget refusal they are.
            if error.kind() == std::io::ErrorKind::TimedOut {
                ReadError::Unavailable
            } else if error.raw_os_error().is_none() {
                ReadError::SearchBudgetExceeded
            } else {
                read_error(&error)
            }
        })?;
    recheck(root, &handle, &before, &real)
}

/// The descriptor and named-reopen stamp recheck every selected file —
/// searched or skipped — answers to before its records publish.
fn recheck(
    root: &PackageDirectory,
    handle: &std::fs::File,
    before: &FileMetadata,
    relative: &Path,
) -> Result<(), ReadError> {
    let unchanged = FileMetadata::of(handle).is_ok_and(|after| after == *before)
        && root
            .file(relative)
            .and_then(|reopened| FileMetadata::of(&reopened))
            .is_ok_and(|reopened| reopened == *before);
    if unchanged {
        Ok(())
    } else {
        Err(ReadError::Conflict)
    }
}

/// The bounded record accumulation of one search — the same accounting
/// the Server verifies on the reply: `match_limit_reached` is set only
/// when a further match was observed past the bound, and the joined
/// body never exceeds [`MAX_TEXT_BYTES`].
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
        if self.bytes.saturating_add(cost) > MAX_TEXT_BYTES {
            self.byte_limit_reached = true;
            return false;
        }
        self.bytes = self.bytes.saturating_add(cost);
        self.lines.push(line);
        true
    }
}

struct FileSink<'a> {
    path: &'a str,
    collector: &'a mut Collector,
}

impl grep_searcher::Sink for FileSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        mat: &grep_searcher::SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
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
        _searcher: &grep_searcher::Searcher,
        context: &grep_searcher::SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        match context.kind() {
            grep_searcher::SinkContextKind::Before | grep_searcher::SinkContextKind::After => {}
            grep_searcher::SinkContextKind::Other => return Ok(true),
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
    let bounded: String = chars.by_ref().take(SEARCH_PREVIEW_SCALARS).collect();
    if chars.next().is_some() {
        (format!("{bounded}... [truncated]"), true)
    } else {
        (bounded, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (tempfile::TempDir, PackageDirectory) {
        let dir = tempfile::tempdir().expect("temp workspace");
        let root = PackageDirectory::open(dir.path()).expect("workspace opens");
        (dir, root)
    }

    fn later() -> Instant {
        Instant::now() + std::time::Duration::from_secs(60)
    }

    fn page(reply: ClientReply) -> (String, u64, bool, bool, bool) {
        let ClientReply::SearchPage {
            body,
            matches,
            hit_limit,
            byte_limit,
            short_lines,
        } = reply
        else {
            panic!("expected a search page: {reply:?}");
        };
        (body, matches.get(), hit_limit, byte_limit, short_lines)
    }

    fn glob_spec(pattern: &str, limit: u64) -> SearchSpec {
        SearchSpec::Glob {
            pattern: pattern.to_owned(),
            limit: DecimalU64::new(limit),
        }
    }

    fn grep_spec(pattern: &str, limit: u64) -> SearchSpec {
        SearchSpec::Grep {
            pattern: pattern.to_owned(),
            glob: None,
            literal: false,
            ignore_case: false,
            context: 0,
            limit: DecimalU64::new(limit),
        }
    }

    #[test]
    fn glob_enumerates_the_shared_universe_in_lexical_order() {
        let (dir, root) = workspace();
        std::fs::create_dir_all(dir.path().join("z/sub")).unwrap();
        std::fs::write(dir.path().join("z/sub/b.txt"), "b").unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join(".hidden.txt"), "h").unwrap();
        let (body, matches, hit, byte, short) = page(search(
            &root,
            Path::new(""),
            &glob_spec("**/*.txt", 100),
            later(),
        ));
        assert_eq!(body, ".hidden.txt\na.txt\nz/sub/b.txt");
        assert_eq!((matches, hit, byte, short), (3, false, false, false));
    }

    #[test]
    fn glob_under_a_subdirectory_root_reports_root_relative_names() {
        let (dir, root) = workspace();
        std::fs::create_dir_all(dir.path().join("inside/nested")).unwrap();
        std::fs::write(dir.path().join("inside/inside.txt"), "i").unwrap();
        std::fs::write(dir.path().join("inside/nested/deep.txt"), "d").unwrap();
        std::fs::write(dir.path().join("outside.txt"), "o").unwrap();
        let (body, ..) = page(search(
            &root,
            Path::new("inside"),
            &glob_spec("**/*.txt", 100),
            later(),
        ));
        assert_eq!(body, "inside.txt\nnested/deep.txt");
    }

    #[test]
    fn grep_under_a_subdirectory_root_reports_root_relative_lines() {
        let (dir, root) = workspace();
        std::fs::create_dir_all(dir.path().join("inside")).unwrap();
        std::fs::write(dir.path().join("inside/a.txt"), "hit\n").unwrap();
        std::fs::write(dir.path().join("other.txt"), "hit\n").unwrap();
        let (body, ..) = page(search(
            &root,
            Path::new("inside"),
            &grep_spec("hit", 10),
            later(),
        ));
        assert_eq!(body, "a.txt:1: hit");
    }

    #[test]
    fn glob_never_follows_symlinks() {
        let (dir, root) = workspace();
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("escape.txt"), "escape").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("escape.txt"),
            dir.path().join("linked.txt"),
        )
        .unwrap();
        std::fs::write(dir.path().join("real.txt"), "r").unwrap();
        let (body, ..) = page(search(
            &root,
            Path::new(""),
            &glob_spec("**/*.txt", 100),
            later(),
        ));
        assert_eq!(body, "real.txt");
    }

    #[test]
    fn a_symlinked_search_root_is_refused() {
        let (dir, root) = workspace();
        let outside = tempfile::tempdir().expect("outside");
        std::fs::create_dir_all(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let reply = search(&root, Path::new("link"), &glob_spec("*", 10), later());
        assert_eq!(
            reply,
            ClientReply::Error {
                kind: ReadError::Denied
            }
        );
    }

    #[test]
    fn glob_on_a_file_root_needs_a_directory() {
        let (dir, root) = workspace();
        std::fs::write(dir.path().join("one.txt"), "one").unwrap();
        let reply = search(&root, Path::new("one.txt"), &glob_spec("*", 10), later());
        assert_eq!(
            reply,
            ClientReply::Error {
                kind: ReadError::SearchNeedsDirectory
            }
        );
    }

    #[test]
    fn a_missing_root_is_not_found() {
        let (_dir, root) = workspace();
        let reply = search(&root, Path::new("missing"), &grep_spec("x", 10), later());
        assert_eq!(
            reply,
            ClientReply::Error {
                kind: ReadError::NotFound
            }
        );
    }

    #[test]
    fn grep_matches_with_context_and_limit_proof() {
        let (dir, root) = workspace();
        std::fs::write(dir.path().join("a.txt"), "before\nhit\nafter\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "hit\n").unwrap();
        let spec = SearchSpec::Grep {
            pattern: "hit".to_owned(),
            glob: None,
            literal: false,
            ignore_case: false,
            context: 1,
            limit: DecimalU64::new(1),
        };
        let (body, matches, hit, byte, _short) = page(search(&root, Path::new(""), &spec, later()));
        // The limit is a proof: one further match was seen and excluded.
        assert!(body.contains("a.txt-1- before"));
        assert!(body.contains("a.txt:2: hit"));
        assert!(body.contains("a.txt-3- after"));
        assert!(!body.contains("b.txt"));
        assert_eq!(matches, 1);
        assert!(hit);
        assert!(!byte);
    }

    #[test]
    fn a_single_file_root_is_the_whole_grep_universe() {
        let (dir, root) = workspace();
        std::fs::write(dir.path().join("one.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("two.txt"), "needle\n").unwrap();
        let (body, ..) = page(search(
            &root,
            Path::new("one.txt"),
            &grep_spec("needle", 10),
            later(),
        ));
        assert_eq!(body, "one.txt:1: needle");
    }

    #[test]
    fn an_invalid_utf8_tail_excludes_the_whole_file() {
        let (dir, root) = workspace();
        // The invalid byte sits after an apparent early match: the file
        // still contributes nothing.
        std::fs::write(dir.path().join("tail.bin"), b"hit\nvalid\n\xff\xfe").unwrap();
        std::fs::write(dir.path().join("clean.txt"), "hit\n").unwrap();
        let (body, ..) = page(search(&root, Path::new(""), &grep_spec("hit", 10), later()));
        assert_eq!(body, "clean.txt:1: hit");
    }

    #[test]
    fn an_unreadable_selected_file_fails_explicitly() {
        let (dir, root) = workspace();
        let file = dir.path().join("gone.txt");
        std::fs::write(&file, "hit\n").unwrap();
        // Enumerate, then remove the file: a later grep on the stale
        // universe fails rather than fabricating output. (The traversal
        // itself is a snapshot under the same verified descriptors.)
        let spec = grep_spec("hit", 10);
        std::fs::remove_file(&file).unwrap();
        let reply = search(&root, Path::new("gone.txt"), &spec, later());
        assert!(matches!(reply, ClientReply::Error { .. }));
    }

    #[test]
    fn grep_short_lines_report_truncation() {
        let (dir, root) = workspace();
        std::fs::write(
            dir.path().join("long.txt"),
            format!("{}hit{}", "a".repeat(600), "b".repeat(600)),
        )
        .unwrap();
        let (body, _, _, _, short) =
            page(search(&root, Path::new(""), &grep_spec("hit", 10), later()));
        assert!(body.contains("... [truncated]"));
        assert!(short);
    }

    #[test]
    fn before_context_can_overflow_the_body_with_zero_shown_matches() {
        // Deterministic: a ~600-byte display name prefixes 20 context
        // rows of 500 four-byte scalars each — the context alone crosses
        // MAX_TEXT_BYTES, so the matching line's own record never fits.
        // The honest page is a nonempty body of context rows, zero
        // matches, and byte_limit — never a fabricated "no matches".
        let (dir, root) = workspace();
        let nested = dir.path().join("x".repeat(250)).join("y".repeat(250));
        std::fs::create_dir_all(&nested).unwrap();
        let long_line = "\u{1F600}".repeat(500);
        let mut file = String::new();
        for _ in 0..20 {
            file.push_str(&long_line);
            file.push('\n');
        }
        file.push_str("needle\n");
        std::fs::write(nested.join(format!("{}.txt", "z".repeat(80))), file).unwrap();
        let spec = SearchSpec::Grep {
            pattern: "needle".to_owned(),
            glob: None,
            literal: false,
            ignore_case: false,
            context: 20,
            limit: DecimalU64::new(100),
        };
        let (body, matches, hit, byte, _short) = page(search(&root, Path::new(""), &spec, later()));
        assert_ne!(body, "");
        assert!(body.len() <= MAX_TEXT_BYTES);
        assert!(body.contains('-'), "context rows use the path-line- form");
        assert_eq!(matches, 0);
        assert!(!hit);
        assert!(byte);
    }

    #[test]
    fn pattern_and_context_bounds_are_enforced_before_traversal() {
        let (_dir, root) = workspace();
        let big = "x".repeat(MAX_SEARCH_PATTERN_BYTES + 1);
        let reply = search(&root, Path::new(""), &glob_spec(&big, 10), later());
        assert_eq!(
            reply,
            ClientReply::Error {
                kind: ReadError::Invalid
            }
        );
        let mut spec = grep_spec("x", 10);
        let SearchSpec::Grep { context, .. } = &mut spec else {
            unreachable!()
        };
        *context = MAX_SEARCH_CONTEXT_LINES + 1;
        let reply = search(&root, Path::new(""), &spec, later());
        assert_eq!(
            reply,
            ClientReply::Error {
                kind: ReadError::Invalid
            }
        );
    }

    #[test]
    fn a_line_beyond_the_working_buffer_is_a_budget_error_not_an_oom() {
        let (dir, root) = workspace();
        // A single line larger than the grep working buffer: the typed
        // budget refusal, never an allocation explosion.
        let mut file = Vec::with_capacity(MAX_SEARCH_BUFFER_BYTES + 16);
        file.extend_from_slice(&vec![b'x'; MAX_SEARCH_BUFFER_BYTES + 8]);
        file.extend_from_slice(b"needle\n");
        std::fs::write(dir.path().join("huge.txt"), file).unwrap();
        let reply = search(&root, Path::new(""), &grep_spec("needle", 10), later());
        assert_eq!(
            reply,
            ClientReply::Error {
                kind: ReadError::SearchBudgetExceeded
            }
        );
    }
}

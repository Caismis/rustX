//! M5 native filesystem tool tests.
//!
//! Every test runs the native tool plane against an isolated temporary
//! workspace — never the developer machine's repository.
//!
//! Read/Write/Edit exercise the workspace boundary (absolute paths, `..`
//! escapes, symlink escapes), the deterministic `offset`/`limit` line
//! window, the atomic commit, and bounded output. Edit additionally proves
//! the atomic multi-edit invariant: every replacement is resolved against
//! one original file snapshot, the whole range set is validated before any
//! mutation, and a rejected edit set never touches the file.
//!
//! Glob/Grep prove that they observe **one** shared file universe, that the
//! universe policy (hidden files visible, ignore files not applied, symlinks
//! never followed, search-root containment) is explicit, and that ordering,
//! bounds, and truncation are rustX-owned semantics rather than accidents of
//! the underlying ripgrep crates.
//!
//! No test sleeps, races, or depends on anything installed on the machine
//! running it — in particular, no test depends on an `rg` executable.

#![allow(clippy::similar_names)] // scripted fixture names are intentionally similar

mod common;

use common::{NativeFixture, native_fixture, run_tool};
use rustx::tools::limits::{MAX_GLOB_RESULTS, MAX_GREP_LINE_BYTES, MAX_MODEL_TOOL_RESULT_BYTES};
use rustx::tools::types::ToolExecutionStatus;

/// Runs a tool call and asserts it failed.
fn assert_failed(result: &rustx::tools::types::ToolExecutionResult) {
    assert!(
        matches!(result.status, ToolExecutionStatus::Failed { .. }),
        "expected a failed result, got {:?}",
        result.status
    );
}

fn json_content(result: &rustx::tools::types::ToolExecutionResult) -> serde_json::Value {
    for content in &result.content {
        if let rustx::tools::types::ToolResultContent::Json { value } = content {
            return value.clone();
        }
    }
    panic!("expected a JSON result content block");
}

fn text_content(result: &rustx::tools::types::ToolExecutionResult) -> String {
    for content in &result.content {
        if let rustx::tools::types::ToolResultContent::Text(text) = content {
            return text.text.clone();
        }
    }
    panic!("expected a text result content block");
}

/// Executes one native tool **without** the registry preflight, so the
/// tool's own typed input boundary is what rejects the arguments.
///
/// The registry rejects the same arguments against the generated schema
/// (proved in the native tool contract suite); this path proves the tool
/// enforces its semantic rules itself, so a direct executor call can never
/// slip past them.
async fn run_tool_unchecked(
    fixture: &NativeFixture,
    name: &str,
    arguments: serde_json::Value,
) -> rustx::tools::types::ToolExecutionResult {
    use rustx::runtime::identity::ToolCallId;
    use rustx::tools::executor::ToolExecutionContext;
    use rustx::tools::types::{ToolInvocation, ToolInvocationMode};
    let definition = fixture
        .registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == name)
        .expect("tool registered");
    let executor = fixture.registry.executor(&definition.id);
    let reporter = common::NoopProgress;
    let context = ToolExecutionContext {
        conversation_id: fixture.runtime.conversation_id(),
        execution_id: None,
        cancellation: rustx::runtime::ExecutionCancellation::detached(
            rustx::runtime::CancellationSignal::new(),
            rustx::runtime::types::CancellationReason::UserRequested,
        ),
        workspace: fixture.runtime.workspace(),
        progress: &reporter,
        artifacts: fixture.runtime.artifacts(),
        tool_output: fixture.runtime.tool_output(),
        environment: fixture.runtime.environment(),
    };
    executor
        .execute(
            ToolInvocation {
                call_id: ToolCallId::new("call-unchecked"),
                tool_id: definition.id.clone(),
                tool_name: name.to_owned(),
                mode: ToolInvocationMode::Foreground,
                arguments,
            },
            context,
        )
        .await
}

/// Whether any temporary file of the native atomic commit is left behind
/// anywhere below the workspace root.
fn stray_temp_files(fixture: &NativeFixture) -> Vec<String> {
    fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".rustx-") {
                found.push(name);
            }
            if entry.path().is_dir() {
                walk(&entry.path(), found);
            }
        }
    }
    let mut found = Vec::new();
    walk(fixture.runtime.workspace().root(), &mut found);
    found
}

/// The absolute model-facing locator of a workspace-relative fixture path.
fn abs(fixture: &NativeFixture, relative: &str) -> String {
    fixture
        .runtime
        .workspace()
        .root()
        .join(relative)
        .to_str()
        .expect("utf8 workspace path")
        .to_owned()
}

// ---------------------------------------------------------------------------
// Read: the 1-based offset/limit line window
// ---------------------------------------------------------------------------

/// An omitted window means the documented defaults: start at line 1 and
/// return at most 200 lines.
#[tokio::test]
async fn read_defaults_to_the_documented_line_window() {
    let fixture = native_fixture();
    let lines: Vec<String> = (1..=250).map(|index| format!("line-{index:03}")).collect();
    std::fs::write(
        fixture.runtime.workspace().root().join("many.txt"),
        format!("{}\n", lines.join("\n")),
    )
    .expect("write sample");
    let result = run_tool(
        &fixture,
        "read",
        serde_json::json!({"file_path": abs(&fixture, "many.txt")}),
    )
    .await;
    let returned: Vec<String> = text_content(&result).lines().map(str::to_owned).collect();
    assert_eq!(returned.len(), 200, "the default limit is 200 lines");
    assert_eq!(returned[0], "line-001", "the default offset is line 1");
    assert_eq!(returned[199], "line-200");
}

/// `offset` is 1-based and `limit` bounds the window; the two compose into
/// exactly one deterministic slice of the file.
#[tokio::test]
async fn read_offset_is_one_based_and_limit_bounds_the_window() {
    let fixture = native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("sample.txt"),
        "one\ntwo\nthree\nfour\nfive\n",
    )
    .expect("write sample");
    let full = run_tool(
        &fixture,
        "read",
        serde_json::json!({"file_path": abs(&fixture, "sample.txt")}),
    )
    .await;
    assert_eq!(text_content(&full), "one\ntwo\nthree\nfour\nfive");

    let first = run_tool(
        &fixture,
        "read",
        serde_json::json!({"file_path": abs(&fixture, "sample.txt"), "offset": 1, "limit": 1}),
    )
    .await;
    assert_eq!(
        text_content(&first),
        "one",
        "offset 1 is the first line of the file, never the second"
    );

    let middle = run_tool(
        &fixture,
        "read",
        serde_json::json!({"file_path": abs(&fixture, "sample.txt"), "offset": 2, "limit": 2}),
    )
    .await;
    assert_eq!(text_content(&middle), "two\nthree");

    let to_eof = run_tool(
        &fixture,
        "read",
        serde_json::json!({"file_path": abs(&fixture, "sample.txt"), "offset": 4, "limit": 1000}),
    )
    .await;
    assert_eq!(
        text_content(&to_eof),
        "four\nfive",
        "a limit reaching past EOF stops at the last line instead of failing"
    );
}

/// An offset past the end of the file is an empty window, not an error.
#[tokio::test]
async fn read_offset_past_the_end_is_an_empty_window() {
    let fixture = native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("sample.txt"),
        "one\ntwo\n",
    )
    .expect("write sample");
    let past_end = run_tool(
        &fixture,
        "read",
        serde_json::json!({"file_path": abs(&fixture, "sample.txt"), "offset": 99}),
    )
    .await;
    assert_eq!(past_end.status, ToolExecutionStatus::Success);
    assert_eq!(text_content(&past_end), "");
}

/// A zero `offset` or `limit` is rejected by the tool itself, not only by
/// the registry preflight: a direct executor call can never read from line
/// zero or ask for zero lines.
#[tokio::test]
async fn read_rejects_a_zero_offset_or_limit_at_the_executor_boundary() {
    let fixture = native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("sample.txt"),
        "one\n",
    )
    .expect("write sample");
    for arguments in [
        serde_json::json!({"file_path": abs(&fixture, "sample.txt"), "offset": 0}),
        serde_json::json!({"file_path": abs(&fixture, "sample.txt"), "limit": 0}),
    ] {
        assert_failed(&run_tool_unchecked(&fixture, "read", arguments).await);
    }
}

#[tokio::test]
async fn read_output_is_bounded_and_truncated() {
    let fixture = native_fixture();
    let file = fixture.runtime.workspace().root().join("big.txt");
    let content = "x".repeat(200 * 1024);
    std::fs::write(&file, &content).expect("write big file");
    let result = run_tool(
        &fixture,
        "read",
        serde_json::json!({"file_path": abs(&fixture, "big.txt"), "limit": 1000}),
    )
    .await;
    assert!(
        text_content(&result).len() <= MAX_MODEL_TOOL_RESULT_BYTES,
        "model-facing output stays bounded"
    );
    let truncation = result.truncation.expect("truncation state");
    assert!(truncation.truncated);
    assert_eq!(
        truncation.original_bytes,
        Some(content.len() as u64),
        "the original byte count is reported"
    );
}

#[tokio::test]
async fn read_rejects_binary_input() {
    let fixture = native_fixture();
    let file = fixture.runtime.workspace().root().join("binary.bin");
    std::fs::write(&file, [0xff, 0xfe, 0x00, 0x01]).expect("write binary");
    assert_failed(
        &run_tool(
            &fixture,
            "read",
            serde_json::json!({"file_path": abs(&fixture, "binary.bin")}),
        )
        .await,
    );
}

/// A relative locator is rejected outright, and an absolute locator is not
/// authority: an absolute host path outside every authorized root is
/// rejected as well.
#[tokio::test]
async fn read_rejects_relative_and_unauthorized_locators() {
    let fixture = native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("sample.txt"),
        "one\n",
    )
    .expect("write sample");
    assert_failed(
        &run_tool(
            &fixture,
            "read",
            serde_json::json!({"file_path": "sample.txt"}),
        )
        .await,
    );
    assert_failed(
        &run_tool(
            &fixture,
            "read",
            serde_json::json!({"file_path": "/etc/hostname"}),
        )
        .await,
    );
    // The enclosing runtime-private root of the managed tool-output root is
    // not implicitly readable either.
    let private = fixture
        .runtime
        .tool_output()
        .root()
        .parent()
        .expect("the managed root has a parent")
        .join("conversation.sqlite");
    assert!(
        private.exists(),
        "the fixture composes the durable store next to the managed root"
    );
    assert_failed(
        &run_tool(
            &fixture,
            "read",
            serde_json::json!({"file_path": private.to_str().expect("utf8")}),
        )
        .await,
    );
}

#[cfg(unix)]
#[tokio::test]
async fn read_rejects_symlinks_escaping_the_workspace() {
    let fixture = native_fixture();
    let outside = fixture.dir().path().join("outside-secret.txt");
    std::fs::write(&outside, "secret").expect("write outside");
    std::os::unix::fs::symlink(
        &outside,
        fixture.runtime.workspace().root().join("linked.txt"),
    )
    .expect("symlink");
    assert_failed(
        &run_tool(
            &fixture,
            "read",
            serde_json::json!({"file_path": abs(&fixture, "linked.txt")}),
        )
        .await,
    );
}

// ---------------------------------------------------------------------------
// Write: absolute `file_path` + `content` mutation guarantees
// ---------------------------------------------------------------------------

#[tokio::test]
async fn write_creates_and_replaces_atomically() {
    let fixture = native_fixture();
    std::fs::create_dir_all(fixture.runtime.workspace().root().join("dir")).expect("dir");
    let created = run_tool(
        &fixture,
        "write",
        serde_json::json!({"file_path": abs(&fixture, "dir/file.txt"), "content": "hello"}),
    )
    .await;
    assert_eq!(created.status, ToolExecutionStatus::Success);
    assert_eq!(json_content(&created)["bytes_written"], 5);
    assert_eq!(
        std::fs::read_to_string(fixture.runtime.workspace().root().join("dir/file.txt"))
            .expect("read back"),
        "hello"
    );
    let replaced = run_tool(
        &fixture,
        "write",
        serde_json::json!({"file_path": abs(&fixture, "dir/file.txt"), "content": "world"}),
    )
    .await;
    assert_eq!(replaced.status, ToolExecutionStatus::Success);
    assert_eq!(
        std::fs::read_to_string(fixture.runtime.workspace().root().join("dir/file.txt"))
            .expect("read back"),
        "world"
    );
    // The atomic commit leaves no temporary files behind.
    assert!(
        stray_temp_files(&fixture).is_empty(),
        "no temp files remain: {:?}",
        stray_temp_files(&fixture)
    );
}

#[tokio::test]
async fn write_requires_an_existing_parent_directory() {
    let fixture = native_fixture();
    let result = run_tool(
        &fixture,
        "write",
        serde_json::json!({"file_path": abs(&fixture, "missing/deep/file.txt"), "content": "x"}),
    )
    .await;
    assert_failed(&result);
    assert!(
        !fixture.runtime.workspace().root().join("missing").exists(),
        "no implicit recursive directory creation"
    );
}

/// Relative locators, absolute host paths outside the workspace, and the
/// read-only managed tool-output root are all rejected for mutation.
#[tokio::test]
async fn write_rejects_relative_unauthorized_and_managed_output_paths() {
    let fixture = native_fixture();
    let managed = fixture.runtime.tool_output().root().join("blocked.txt");
    let managed = managed.to_str().expect("utf8").to_owned();
    for path in [
        "../escape.txt".to_owned(),
        "/tmp/rustx-escape.txt".to_owned(),
        managed,
    ] {
        assert_failed(
            &run_tool(
                &fixture,
                "write",
                serde_json::json!({"file_path": path, "content": "x"}),
            )
            .await,
        );
    }
}

// ---------------------------------------------------------------------------
// Edit: one atomic transformation from one original snapshot
// ---------------------------------------------------------------------------

/// Writes `content` to `edit.txt` and returns its workspace path.
fn edit_fixture(fixture: &NativeFixture, content: &str) -> std::path::PathBuf {
    let path = fixture.runtime.workspace().root().join("edit.txt");
    std::fs::write(&path, content).expect("write edit fixture");
    path
}

/// One `edits` entry.
fn replacement(old_text: &str, new_text: &str) -> serde_json::Value {
    serde_json::json!({"oldText": old_text, "newText": new_text})
}

/// Asserts that a failed Edit left the file byte-for-byte unchanged and
/// committed nothing at all.
fn assert_unchanged(fixture: &NativeFixture, path: &std::path::Path, original: &str) {
    assert_eq!(
        std::fs::read_to_string(path).expect("read back"),
        original,
        "a rejected edit set never mutates the file"
    );
    assert!(
        stray_temp_files(fixture).is_empty(),
        "a rejected edit set never even starts a commit"
    );
}

/// A single replacement must identify exactly one place in the file.
#[tokio::test]
async fn edit_applies_a_single_unambiguous_replacement() {
    let fixture = native_fixture();
    let path = edit_fixture(&fixture, "alpha beta alpha");
    let applied = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"file_path": abs(&fixture, "edit.txt"), "edits": [replacement("beta", "GAMMA")]}),
    )
    .await;
    assert_eq!(applied.status, ToolExecutionStatus::Success);
    assert_eq!(json_content(&applied)["replacements"], 1);
    assert_eq!(
        std::fs::read_to_string(&path).expect("read back"),
        "alpha GAMMA alpha"
    );
    assert!(stray_temp_files(&fixture).is_empty());
}

/// An empty edit set describes no transformation, and an empty `oldText`
/// has no exact-match semantics. Both are rejected by the tool itself, on
/// the direct executor path that bypasses the registry preflight.
#[tokio::test]
async fn edit_rejects_an_empty_edit_set_and_an_empty_old_text() {
    let fixture = native_fixture();
    let path = edit_fixture(&fixture, "original");

    let empty_set = run_tool_unchecked(
        &fixture,
        "edit",
        serde_json::json!({"file_path": abs(&fixture, "edit.txt"), "edits": []}),
    )
    .await;
    assert_failed(&empty_set);
    assert_unchanged(&fixture, &path, "original");

    let empty_anchor = run_tool_unchecked(
        &fixture,
        "edit",
        serde_json::json!({"file_path": abs(&fixture, "edit.txt"), "edits": [replacement("", "x")]}),
    )
    .await;
    assert_failed(&empty_anchor);
    assert_unchanged(&fixture, &path, "original");

    let empty_anchor_among_valid = run_tool_unchecked(
        &fixture,
        "edit",
        serde_json::json!({
            "file_path": abs(&fixture, "edit.txt"),
            "edits": [replacement("orig", "X"), replacement("", "y")]
        }),
    )
    .await;
    assert_failed(&empty_anchor_among_valid);
    assert_unchanged(&fixture, &path, "original");
}

/// A `oldText` that matches nothing is a deterministic failure that mutates
/// nothing — including when every other edit of the set would have applied.
#[tokio::test]
async fn edit_fails_on_zero_matches_without_mutating() {
    let fixture = native_fixture();
    let path = edit_fixture(&fixture, "alpha beta");

    let only = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"file_path": abs(&fixture, "edit.txt"), "edits": [replacement("absent", "x")]}),
    )
    .await;
    assert_failed(&only);
    assert_unchanged(&fixture, &path, "alpha beta");

    let mixed = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "file_path": abs(&fixture, "edit.txt"),
            "edits": [replacement("alpha", "A"), replacement("absent", "x")]
        }),
    )
    .await;
    assert_failed(&mixed);
    assert_unchanged(&fixture, &path, "alpha beta");
}

/// A `oldText` that matches more than once is ambiguous: the tool never
/// guesses which occurrence was meant, and never replaces all of them.
#[tokio::test]
async fn edit_fails_on_an_ambiguous_match_without_mutating() {
    let fixture = native_fixture();
    let path = edit_fixture(&fixture, "a a a");
    let ambiguous = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"file_path": abs(&fixture, "edit.txt"), "edits": [replacement("a", "z")]}),
    )
    .await;
    assert_failed(&ambiguous);
    assert_unchanged(&fixture, &path, "a a a");
}

/// The core invariant: every `oldText` is matched against the **original**
/// snapshot, never against the result of an earlier edit of the same call.
///
/// With sequential semantics, replacing `A` with `B` first would make the
/// second edit's `B` anchor ambiguous and the call would fail. With
/// snapshot semantics the two anchors are disjoint regions of the original
/// file and the transformation is exactly `AB -> BC`.
#[tokio::test]
async fn edit_matches_every_replacement_against_the_original_snapshot() {
    let fixture = native_fixture();
    let path = edit_fixture(&fixture, "AB");
    let applied = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "file_path": abs(&fixture, "edit.txt"),
            "edits": [replacement("A", "B"), replacement("B", "C")]
        }),
    )
    .await;
    assert_eq!(applied.status, ToolExecutionStatus::Success);
    assert_eq!(
        std::fs::read_to_string(&path).expect("read back"),
        "BC",
        "the second anchor matched the original snapshot, not the edited file"
    );
}

/// Disjoint edits are applied together as one final snapshot, and the
/// result does not depend on the order the model listed them in.
#[tokio::test]
async fn edit_applies_disjoint_edits_independently_of_input_order() {
    let original = "one two three four";
    let expected = "1 two 3 four";
    let forward = [replacement("one", "1"), replacement("three", "3")];
    let reverse = [replacement("three", "3"), replacement("one", "1")];
    for edits in [forward, reverse] {
        let fixture = native_fixture();
        let path = edit_fixture(&fixture, original);
        let applied = run_tool(
            &fixture,
            "edit",
            serde_json::json!({"file_path": abs(&fixture, "edit.txt"), "edits": edits}),
        )
        .await;
        assert_eq!(applied.status, ToolExecutionStatus::Success);
        assert_eq!(json_content(&applied)["replacements"], 2);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            expected,
            "input ordering never changes the final snapshot"
        );
        assert!(
            stray_temp_files(&fixture).is_empty(),
            "the whole edit set commits exactly once"
        );
    }
}

/// Overlapping, nested, and duplicate replacement ranges are all the same
/// conflict, and all of them leave the file untouched.
#[tokio::test]
async fn edit_rejects_conflicting_replacement_ranges() {
    // (case, original content, edits)
    let cases: [(&str, &str, [serde_json::Value; 2]); 3] = [
        (
            "overlapping",
            "abcdef",
            [replacement("abc", "X"), replacement("cde", "Y")],
        ),
        (
            "nested",
            "abcdef",
            [replacement("abcd", "X"), replacement("bc", "Y")],
        ),
        (
            "duplicate",
            "xyz",
            [replacement("y", "1"), replacement("y", "2")],
        ),
    ];
    for (case, original, edits) in cases {
        let fixture = native_fixture();
        let path = edit_fixture(&fixture, original);
        let rejected = run_tool(
            &fixture,
            "edit",
            serde_json::json!({"file_path": abs(&fixture, "edit.txt"), "edits": edits}),
        )
        .await;
        assert!(
            matches!(rejected.status, ToolExecutionStatus::Failed { .. }),
            "{case} ranges must be rejected, got {:?}",
            rejected.status
        );
        assert_unchanged(&fixture, &path, original);
    }
}

/// An anchor whose candidate placements **overlap each other** identifies no
/// single target and must be rejected as ambiguous.
///
/// This is the case a non-overlapping match iterator silently gets wrong:
/// `"aa"` in `"aaa"` reports one match and would quietly replace `0..2`,
/// even though `1..3` is an equally valid reading of the same request. The
/// contract is "exactly one possible target", not "exactly one match
/// according to a convenience iterator".
#[tokio::test]
async fn edit_rejects_an_anchor_with_overlapping_candidate_placements() {
    // (original content, anchor, the two placements that make it ambiguous)
    //
    // The multibyte case also proves the scan advances by whole characters:
    // each `α` is two bytes, so a byte-wise cursor would slice through a code
    // point and panic.
    let cases: [(&str, &str, &str); 3] = [
        ("aaa", "aa", "0..2 and 1..3"),
        ("ababa", "aba", "0..3 and 2..5"),
        ("ααα", "αα", "0..4 and 2..6"),
    ];
    for (original, anchor, placements) in cases {
        let fixture = native_fixture();
        let path = edit_fixture(&fixture, original);
        let rejected = run_tool(
            &fixture,
            "edit",
            serde_json::json!({"file_path": abs(&fixture, "edit.txt"), "edits": [replacement(anchor, "REPLACED")]}),
        )
        .await;
        assert!(
            matches!(rejected.status, ToolExecutionStatus::Failed { .. }),
            "{anchor:?} in {original:?} can be placed at {placements}, so it must be rejected \
             as ambiguous; got {:?}",
            rejected.status
        );
        assert_unchanged(&fixture, &path, original);
    }
}

/// An overlapping-candidate anchor anywhere in the edit set fails the whole
/// invocation: no other edit of the same call may partially commit.
#[tokio::test]
async fn an_ambiguous_overlapping_anchor_blocks_every_edit_of_the_invocation() {
    let fixture = native_fixture();
    let original = "aaa and unique";
    let path = edit_fixture(&fixture, original);
    let rejected = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "file_path": abs(&fixture, "edit.txt"),
            // The first edit is perfectly valid on its own; the second one
            // can be placed at two overlapping ranges.
            "edits": [replacement("unique", "UNIQUE"), replacement("aa", "X")]
        }),
    )
    .await;
    assert!(
        matches!(rejected.status, ToolExecutionStatus::Failed { .. }),
        "one ambiguous anchor fails the whole invocation, got {:?}",
        rejected.status
    );
    assert_unchanged(&fixture, &path, original);
}

/// Overlapping *candidate placements of one anchor* are ambiguous, but an
/// anchor that occurs exactly once is still unique even when it repeats a
/// character that could overlap in a longer file.
#[tokio::test]
async fn edit_still_accepts_an_anchor_with_one_possible_placement() {
    // (original, anchor, replacement, expected result)
    let cases: [(&str, &str, &str, &str); 2] = [
        ("aab", "aa", "X", "Xb"),
        // The multibyte counterpart: exactly one placement, and the byte
        // range it resolves to must land on character boundaries.
        ("ααβ", "αα", "X", "Xβ"),
    ];
    for (original, anchor, new_text, expected) in cases {
        let fixture = native_fixture();
        let path = edit_fixture(&fixture, original);
        let applied = run_tool(
            &fixture,
            "edit",
            serde_json::json!({"file_path": abs(&fixture, "edit.txt"), "edits": [replacement(anchor, new_text)]}),
        )
        .await;
        assert_eq!(
            applied.status,
            ToolExecutionStatus::Success,
            "{anchor:?} has exactly one possible placement in {original:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).expect("read back"), expected);
    }
}

/// Adjacent — but not overlapping — replacements are legal: the rule is
/// that a range may start exactly where the previous one ended.
#[tokio::test]
async fn edit_accepts_adjacent_replacement_ranges() {
    let fixture = native_fixture();
    let path = edit_fixture(&fixture, "abcdef");
    let applied = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "file_path": abs(&fixture, "edit.txt"),
            "edits": [replacement("abc", "X"), replacement("def", "Y")]
        }),
    )
    .await;
    assert_eq!(applied.status, ToolExecutionStatus::Success);
    assert_eq!(std::fs::read_to_string(&path).expect("read back"), "XY");
}

#[tokio::test]
async fn edit_rejects_absolute_and_escaping_paths() {
    let fixture = native_fixture();
    for path in ["/etc/hostname", "../escape.txt"] {
        assert_failed(
            &run_tool(
                &fixture,
                "edit",
                serde_json::json!({"file_path": path, "edits": [replacement("a", "b")]}),
            )
            .await,
        );
    }
}

#[tokio::test]
async fn edit_rejects_binary_input() {
    let fixture = native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("binary.bin"),
        [0xff, 0xfe, 0x00, 0x01],
    )
    .expect("write binary");
    assert_failed(
        &run_tool(
            &fixture,
            "edit",
            serde_json::json!({"file_path": abs(&fixture, "binary.bin"), "edits": [replacement("a", "b")]}),
        )
        .await,
    );
}

// ---------------------------------------------------------------------------
// The shared Glob/Grep file universe
// ---------------------------------------------------------------------------

/// Builds one controlled tree covering every visibility decision of the
/// shared search policy, and returns the paths Glob and Grep must both see.
///
/// Every file below contains the token `needle`, so the *set of paths* Grep
/// reports is directly comparable to the set of paths Glob reports.
#[cfg(unix)]
fn search_universe_fixture(fixture: &NativeFixture) -> Vec<&'static str> {
    let root = fixture.runtime.workspace().root().to_path_buf();
    let outside = fixture.dir().path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside dir");
    std::fs::write(outside.join("secret.txt"), "secret needle").expect("outside file");

    std::fs::create_dir_all(root.join("nested")).expect("nested dir");
    std::fs::create_dir_all(root.join("ignored-dir")).expect("ignored dir");
    // A real .gitignore that would hide two of the files below if ignore
    // semantics were applied — they are deliberately not applied.
    std::fs::write(
        root.join(".gitignore"),
        "ignored.txt\nignored-dir/\n# needle\n",
    )
    .expect("gitignore");
    std::fs::write(root.join(".hidden.txt"), "hidden needle").expect("hidden file");
    std::fs::write(root.join("visible.txt"), "visible needle").expect("visible file");
    std::fs::write(root.join("ignored.txt"), "ignored needle").expect("ignored file");
    std::fs::write(root.join("ignored-dir/inside.txt"), "inside needle").expect("ignored nested");
    std::fs::write(root.join("nested/deep.txt"), "deep needle").expect("nested file");
    // A file symlink and a directory symlink, both pointing outside the
    // workspace. Neither may contribute anything to the universe.
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt"))
        .expect("file symlink");
    std::os::unix::fs::symlink(&outside, root.join("linkdir")).expect("directory symlink");

    vec![
        ".gitignore",
        ".hidden.txt",
        "ignored-dir/inside.txt",
        "ignored.txt",
        "nested/deep.txt",
        "visible.txt",
    ]
}

/// The sorted distinct paths of a Grep result.
fn grep_paths(result: &rustx::tools::types::ToolExecutionResult) -> Vec<String> {
    let content = json_content(result);
    let mut paths: Vec<String> = content["matches"]
        .as_array()
        .expect("matches array")
        .iter()
        .map(|entry| entry["path"].as_str().expect("path").to_owned())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// The results of a Glob call.
fn glob_results(result: &rustx::tools::types::ToolExecutionResult) -> Vec<String> {
    json_content(result)["results"]
        .as_array()
        .expect("results array")
        .iter()
        .map(|entry| entry.as_str().expect("path").to_owned())
        .collect()
}

/// The central shared-universe invariant: with no caller filter narrowing
/// either of them, Glob and Grep observe exactly the same set of files.
///
/// The one policy both share is asserted explicitly: hidden files are
/// visible, `.gitignore` is not applied, and symlinks (file or directory)
/// are not part of the universe.
#[cfg(unix)]
#[tokio::test]
async fn glob_and_grep_observe_one_shared_file_universe() {
    let fixture = native_fixture();
    let expected = search_universe_fixture(&fixture);

    let glob = run_tool(&fixture, "glob", serde_json::json!({"pattern": "**/*"})).await;
    assert_eq!(glob.status, ToolExecutionStatus::Success);
    assert_eq!(
        glob_results(&glob),
        expected,
        "Glob sees hidden files, ignores .gitignore, and never follows symlinks"
    );

    let grep = run_tool(&fixture, "grep", serde_json::json!({"pattern": "needle"})).await;
    assert_eq!(grep.status, ToolExecutionStatus::Success);
    assert_eq!(
        grep_paths(&grep),
        expected,
        "Grep observes exactly the same universe as Glob"
    );
}

/// Neither tool follows a directory symlink (which would recurse outside
/// the workspace) or a file symlink (whose target lives outside it).
#[cfg(unix)]
#[tokio::test]
async fn search_tools_never_follow_symlinks() {
    let fixture = native_fixture();
    search_universe_fixture(&fixture);

    let glob = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"pattern": "**/secret.txt"}),
    )
    .await;
    assert_eq!(
        glob_results(&glob),
        Vec::<String>::new(),
        "a directory symlink is never traversed"
    );
    let glob_link = run_tool(&fixture, "glob", serde_json::json!({"pattern": "link.txt"})).await;
    assert_eq!(
        glob_results(&glob_link),
        Vec::<String>::new(),
        "a file symlink is not part of the universe"
    );
    let grep = run_tool(&fixture, "grep", serde_json::json!({"pattern": "secret"})).await;
    assert_eq!(
        grep_paths(&grep),
        Vec::<String>::new(),
        "no symlinked content is searchable"
    );
}

/// An explicit `path` narrows both tools to the same sub-root, and every
/// reported path is relative to that requested root.
#[cfg(unix)]
#[tokio::test]
async fn search_tools_share_one_explicit_search_root() {
    let fixture = native_fixture();
    search_universe_fixture(&fixture);

    let glob = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"pattern": "**/*", "path": abs(&fixture, "nested")}),
    )
    .await;
    assert_eq!(
        glob_results(&glob),
        vec!["deep.txt".to_owned()],
        "paths are relative to the requested search root"
    );
    let grep = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "needle", "path": abs(&fixture, "nested")}),
    )
    .await;
    assert_eq!(grep_paths(&grep), vec!["deep.txt".to_owned()]);
}

/// The search root contract of both tools: an omitted root means the
/// workspace, a supplied root must be absolute and contained in the
/// workspace root or the read-only managed tool-output root, and a
/// single-file root is a Grep-only spelling (Glob searches directories).
#[tokio::test]
async fn search_root_locator_contract_is_enforced_for_both_tools() {
    let fixture = native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("file.txt"),
        "content hit",
    )
    .expect("write");
    // Relative locators and absolute paths outside every authorized root
    // are rejected by both tools.
    for path in ["..", "missing-directory", "file.txt", "/etc"] {
        assert_failed(
            &run_tool(
                &fixture,
                "glob",
                serde_json::json!({"pattern": "**/*", "path": path}),
            )
            .await,
        );
        assert_failed(
            &run_tool(
                &fixture,
                "grep",
                serde_json::json!({"pattern": "x", "path": path}),
            )
            .await,
        );
    }
    // A single absolute file is a legal Grep root but not a Glob root.
    let file = abs(&fixture, "file.txt");
    let grep = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "hit", "path": file}),
    )
    .await;
    assert_eq!(grep.status, ToolExecutionStatus::Success);
    assert_eq!(grep_paths(&grep), vec!["file.txt".to_owned()]);
    assert_failed(
        &run_tool(
            &fixture,
            "glob",
            serde_json::json!({"pattern": "**/*", "path": abs(&fixture, "file.txt")}),
        )
        .await,
    );
}

/// Neither search tool ever spawns a process: there is no `rg` executable
/// dependency to be satisfied by the machine running the tests.
///
/// This is a source-level guard rather than a runtime probe, so it cannot
/// pass merely because `rg` happens to be installed (or absent) here.
#[test]
fn the_search_implementation_never_spawns_an_external_process() {
    let mut checked = 0usize;
    for relative in [
        "src/tools/native/search/mod.rs",
        "src/tools/native/search/traversal.rs",
        "src/tools/native/glob/mod.rs",
        "src/tools/native/glob/input.rs",
        "src/tools/native/grep/mod.rs",
        "src/tools/native/grep/input.rs",
    ] {
        let source = std::fs::read_to_string(relative)
            .unwrap_or_else(|error| panic!("{relative} is readable: {error}"));
        for forbidden in ["Command", "std::process", "\"rg\"", "ripgrep\""] {
            assert!(
                !source.contains(forbidden),
                "{relative} must not reference {forbidden}: search is linked, never spawned"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 6, "every search source file was inspected");
}

// ---------------------------------------------------------------------------
// Glob
// ---------------------------------------------------------------------------

#[tokio::test]
async fn glob_is_lexically_ordered_and_root_relative() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    // Creation order is deliberately reversed relative to lexical order.
    std::fs::create_dir_all(root.join("sub")).expect("subdir");
    std::fs::write(root.join("zebra.rs"), "z").expect("write");
    std::fs::write(root.join("alpha.rs"), "a").expect("write");
    std::fs::write(root.join("sub/middle.rs"), "m").expect("write");
    std::fs::write(root.join("skipped.txt"), "t").expect("write");
    let result = run_tool(&fixture, "glob", serde_json::json!({"pattern": "**/*.rs"})).await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert_eq!(
        glob_results(&result),
        vec!["alpha.rs", "sub/middle.rs", "zebra.rs"],
        "results are lexically sorted paths relative to the search root"
    );
    assert_eq!(json_content(&result)["truncated"], false);
    assert!(result.truncation.is_none());
}

#[tokio::test]
async fn glob_truncates_at_the_result_limit() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    std::fs::create_dir_all(root.join("many")).expect("dir");
    for index in 0..(MAX_GLOB_RESULTS + 50) {
        std::fs::write(root.join("many").join(format!("f{index:05}.txt")), "x").expect("write");
    }
    let result = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"pattern": "many/*.txt"}),
    )
    .await;
    assert_eq!(glob_results(&result).len(), MAX_GLOB_RESULTS);
    assert_eq!(json_content(&result)["truncated"], true);
    assert!(result.truncation.expect("truncation state").truncated);
}

/// The exact number of bytes the model-facing JSON payload of a result
/// serializes to.
///
/// This — not the sum of the returned string lengths — is what the hard cap
/// has to bound: JSON escaping, field names, punctuation, and numeric widths
/// are all part of what the model actually receives.
fn serialized_payload_bytes(result: &rustx::tools::types::ToolExecutionResult) -> usize {
    serde_json::to_vec(&json_content(result))
        .expect("a native tool payload is serializable")
        .len()
}

#[tokio::test]
async fn glob_truncates_at_the_byte_limit() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    std::fs::create_dir_all(root.join("long")).expect("dir");
    // Far fewer files than the result cap, but long enough names that the
    // payload cap is what stops the enumeration.
    let files = 500usize;
    for index in 0..files {
        let name = format!("{index:04}-{}.txt", "n".repeat(190));
        std::fs::write(root.join("long").join(name), "x").expect("write");
    }
    let result = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"pattern": "long/*.txt"}),
    )
    .await;
    let returned = glob_results(&result);
    assert!(
        returned.len() < files && returned.len() < MAX_GLOB_RESULTS,
        "the byte cap stopped the enumeration before the result cap: {}",
        returned.len()
    );
    assert!(
        serialized_payload_bytes(&result) <= MAX_MODEL_TOOL_RESULT_BYTES,
        "the delivered JSON payload stays within the hard cap: {} bytes",
        serialized_payload_bytes(&result)
    );
    assert_eq!(json_content(&result)["truncated"], true);
    assert!(result.truncation.expect("truncation state").truncated);
}

/// The hard payload cap bounds the **serialized** document, so filenames
/// full of characters that expand under JSON encoding cannot push the
/// delivered payload past it.
///
/// Each fixture name below carries quotes, backslashes, and a control
/// character, every one of which serializes to more bytes than it occupies
/// on disk (`"` → `\"`, `\` → `\\`, `\t` → `\t`, `\x07` → ``). An
/// accounting based on `path.len()` undercounts every one of them.
#[cfg(unix)]
#[tokio::test]
async fn glob_bounds_the_serialized_payload_of_json_hostile_paths() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    std::fs::create_dir_all(root.join("hostile")).expect("dir");
    // Every one of these bytes is legal in a POSIX filename and every one of
    // them grows during JSON encoding.
    let expanding = "\"\\\t\u{7}".repeat(40);
    let files = 400usize;
    for index in 0..files {
        let name = format!("{index:04}{expanding}.txt");
        std::fs::write(root.join("hostile").join(name), "x").expect("write");
    }
    let result = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"pattern": "hostile/*.txt"}),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);

    let returned = glob_results(&result);
    let raw = returned.iter().map(String::len).sum::<usize>();
    let encoded = serialized_payload_bytes(&result);
    assert!(
        encoded > raw,
        "the fixture must actually expand under JSON encoding: raw {raw}, encoded {encoded}"
    );
    assert!(
        encoded <= MAX_MODEL_TOOL_RESULT_BYTES,
        "the delivered JSON payload stays within the hard cap: {encoded} bytes"
    );
    assert!(
        returned.len() < files,
        "the payload cap actually engaged: {} of {files}",
        returned.len()
    );
    assert_eq!(json_content(&result)["truncated"], true);
    assert!(result.truncation.expect("truncation state").truncated);

    // Truncation keeps the deterministic lexical prefix: the paths that
    // survive are exactly the first ones in order.
    let mut ordered = returned.clone();
    ordered.sort();
    assert_eq!(returned, ordered, "the surviving prefix stays ordered");
}

// ---------------------------------------------------------------------------
// Grep
// ---------------------------------------------------------------------------

/// The ordered `(path, line, column, text)` tuples of a Grep result.
fn grep_matches(
    result: &rustx::tools::types::ToolExecutionResult,
) -> Vec<(String, u64, u64, String)> {
    json_content(result)["matches"]
        .as_array()
        .expect("matches array")
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().expect("path").to_owned(),
                entry["line"].as_u64().expect("line"),
                entry["column"].as_u64().expect("column"),
                entry["text"].as_str().expect("text").to_owned(),
            )
        })
        .collect()
}

/// The ordered `(path, line, text)` tuples of a Grep result's context.
fn grep_context(result: &rustx::tools::types::ToolExecutionResult) -> Vec<(String, u64, String)> {
    json_content(result)["context"]
        .as_array()
        .expect("context array")
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().expect("path").to_owned(),
                entry["line"].as_u64().expect("line"),
                entry["text"].as_str().expect("text").to_owned(),
            )
        })
        .collect()
}

/// Matches are ordered by path, then line, then the column of the match
/// inside the line — and several matches on one line are reported
/// separately, in column order.
#[tokio::test]
async fn grep_orders_matches_by_path_line_and_column() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    std::fs::create_dir_all(root.join("b")).expect("dir b");
    std::fs::create_dir_all(root.join("a")).expect("dir a");
    // Written in reverse lexical order on purpose.
    std::fs::write(root.join("b/late.rs"), "hit\nhit and hit\n").expect("write");
    std::fs::write(root.join("a/early.rs"), "hit hit\n").expect("write");
    let result = run_tool(&fixture, "grep", serde_json::json!({"pattern": "hit"})).await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert_eq!(
        grep_matches(&result),
        vec![
            ("a/early.rs".to_owned(), 1, 1, "hit hit".to_owned()),
            ("a/early.rs".to_owned(), 1, 5, "hit hit".to_owned()),
            ("b/late.rs".to_owned(), 1, 1, "hit".to_owned()),
            ("b/late.rs".to_owned(), 2, 1, "hit and hit".to_owned()),
            ("b/late.rs".to_owned(), 2, 9, "hit and hit".to_owned()),
        ],
        "matches are ordered by path, then line, then column"
    );
}

/// The pattern is a regular expression by default; `literal = true` matches
/// the pattern as text, so the model never has to escape metacharacters.
#[tokio::test]
async fn grep_regex_is_the_default_and_literal_opts_out() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    std::fs::write(root.join("f.txt"), "value a.c here\nvalue abc here\n").expect("write");

    let regex = run_tool(&fixture, "grep", serde_json::json!({"pattern": "a.c"})).await;
    assert_eq!(
        grep_matches(&regex)
            .iter()
            .map(|entry| entry.1)
            .collect::<Vec<_>>(),
        vec![1, 2],
        "the default regex `a.c` matches both `a.c` and `abc`"
    );

    let literal = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "a.c", "literal": true}),
    )
    .await;
    assert_eq!(
        grep_matches(&literal)
            .iter()
            .map(|entry| entry.1)
            .collect::<Vec<_>>(),
        vec![1],
        "a literal search matches only the literal text"
    );

    // A pattern that is not a valid regex at all is still a legal literal.
    let unescaped = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "([unclosed", "literal": true}),
    )
    .await;
    assert_eq!(unescaped.status, ToolExecutionStatus::Success);
    assert!(grep_matches(&unescaped).is_empty());
}

/// The search is case-sensitive unless `ignoreCase` explicitly opts in.
#[tokio::test]
async fn grep_is_case_sensitive_unless_ignore_case_is_requested() {
    let fixture = native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("f.txt"),
        "Needle\nneedle\n",
    )
    .expect("write");

    let sensitive = run_tool(&fixture, "grep", serde_json::json!({"pattern": "needle"})).await;
    assert_eq!(
        grep_matches(&sensitive)
            .iter()
            .map(|entry| entry.1)
            .collect::<Vec<_>>(),
        vec![2],
        "the default is a case-sensitive search"
    );

    let insensitive = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "needle", "ignoreCase": true}),
    )
    .await;
    assert_eq!(
        grep_matches(&insensitive)
            .iter()
            .map(|entry| entry.1)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

/// The optional `glob` narrows the shared universe and nothing else.
#[cfg(unix)]
#[tokio::test]
async fn grep_glob_only_narrows_the_shared_universe() {
    let fixture = native_fixture();
    search_universe_fixture(&fixture);
    let filtered = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "needle", "glob": "nested/**"}),
    )
    .await;
    assert_eq!(grep_paths(&filtered), vec!["nested/deep.txt".to_owned()]);

    let hidden_only = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "needle", "glob": ".hidden.txt"}),
    )
    .await;
    assert_eq!(grep_paths(&hidden_only), vec![".hidden.txt".to_owned()]);
}

/// `context = 0` (the default) returns matches only. `context = N` adds the
/// surrounding lines, and overlapping or adjacent windows merge into one
/// run of distinct lines: every source line is reported exactly once, and a
/// matching line is never also reported as context.
#[tokio::test]
async fn grep_context_windows_merge_without_duplicating_lines() {
    let fixture = native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("f.txt"),
        "l1\nl2\nhit one\nl4\nhit two\nl6\nl7\nl8\nhit three\nl10\n",
    )
    .expect("write");

    let no_context = run_tool(&fixture, "grep", serde_json::json!({"pattern": "hit"})).await;
    assert_eq!(
        grep_matches(&no_context)
            .iter()
            .map(|entry| entry.1)
            .collect::<Vec<_>>(),
        vec![3, 5, 9]
    );
    assert!(
        grep_context(&no_context).is_empty(),
        "context defaults to 0"
    );

    let with_context = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "hit", "context": 1}),
    )
    .await;
    assert_eq!(
        grep_matches(&with_context)
            .iter()
            .map(|entry| entry.1)
            .collect::<Vec<_>>(),
        vec![3, 5, 9],
        "matching lines stay in matches, never in context"
    );
    assert_eq!(
        grep_context(&with_context)
            .iter()
            .map(|entry| (entry.1, entry.2.clone()))
            .collect::<Vec<_>>(),
        vec![
            (2, "l2".to_owned()),
            (4, "l4".to_owned()),
            (6, "l6".to_owned()),
            (8, "l8".to_owned()),
            (10, "l10".to_owned()),
        ],
        "line 4 belongs to two overlapping windows and is reported exactly once"
    );
}

/// `limit` bounds the number of returned matches and truncation is
/// explicit; the surplus is never dropped silently.
#[tokio::test]
async fn grep_limit_bounds_the_matches_with_explicit_truncation() {
    let fixture = native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("f.txt"),
        "hit\n".repeat(10),
    )
    .expect("write");

    let all = run_tool(&fixture, "grep", serde_json::json!({"pattern": "hit"})).await;
    assert_eq!(grep_matches(&all).len(), 10);
    assert_eq!(json_content(&all)["truncated"], false);
    assert!(all.truncation.is_none());

    let bounded = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "hit", "limit": 4}),
    )
    .await;
    assert_eq!(grep_matches(&bounded).len(), 4);
    assert_eq!(json_content(&bounded)["truncated"], true);
    assert!(bounded.truncation.expect("truncation state").truncated);
}

/// The payload byte cap is a hard bound that stops the search even before
/// the match limit is reached, with explicit truncation.
#[tokio::test]
async fn grep_truncates_at_the_byte_limit() {
    let fixture = native_fixture();
    let line = format!("hit {}\n", "p".repeat(300));
    std::fs::write(
        fixture.runtime.workspace().root().join("wide.txt"),
        line.repeat(300),
    )
    .expect("write");
    let result = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "hit", "limit": 300}),
    )
    .await;
    let matches = grep_matches(&result);
    assert!(
        matches.len() < 300,
        "the byte cap stopped the search before the match limit: {}",
        matches.len()
    );
    assert!(
        serialized_payload_bytes(&result) <= MAX_MODEL_TOOL_RESULT_BYTES,
        "the delivered JSON payload stays within the hard cap: {} bytes",
        serialized_payload_bytes(&result)
    );
    assert_eq!(json_content(&result)["truncated"], true);
    assert!(result.truncation.expect("truncation state").truncated);
}

/// The hard payload cap bounds the **serialized** document for Grep too, and
/// `matches` and `context` share that one budget.
///
/// The fixture's matching lines are full of characters that grow under JSON
/// encoding (`"`, `\`, tab, and a bare control byte), so an accounting based
/// on `text.len()` undercounts every reported line. Context is requested as
/// well, so both arrays are charged against the same cap.
#[tokio::test]
async fn grep_bounds_the_serialized_payload_of_json_hostile_content() {
    let fixture = native_fixture();
    // Each of these bytes serializes to more than one byte of JSON.
    let expanding = "\"\\\t\u{1}".repeat(60);
    let mut body = String::new();
    for index in 0..400 {
        use std::fmt::Write as _;
        writeln!(body, "hit {index} {expanding}").expect("in-memory fixture");
    }
    std::fs::write(fixture.runtime.workspace().root().join("hostile.txt"), body).expect("write");

    let result = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "hit", "limit": 400, "context": 2}),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);

    let matches = grep_matches(&result);
    let raw: usize = matches
        .iter()
        .map(|entry| entry.3.len())
        .chain(grep_context(&result).iter().map(|entry| entry.2.len()))
        .sum();
    let encoded = serialized_payload_bytes(&result);
    assert!(
        encoded > raw,
        "the fixture must actually expand under JSON encoding: raw {raw}, encoded {encoded}"
    );
    assert!(
        encoded <= MAX_MODEL_TOOL_RESULT_BYTES,
        "the delivered JSON payload stays within the hard cap: {encoded} bytes"
    );
    assert!(
        matches.len() < 400,
        "the payload cap actually engaged: {} of 400",
        matches.len()
    );
    assert_eq!(json_content(&result)["truncated"], true);
    assert!(result.truncation.expect("truncation state").truncated);

    // Ordering survives truncation: the surviving matches are still the
    // deterministic path/line/column prefix.
    let mut ordered = matches.clone();
    ordered.sort_by(|left, right| (&left.0, left.1, left.2).cmp(&(&right.0, right.1, right.2)));
    assert_eq!(matches, ordered, "the surviving prefix stays ordered");
}

/// A very long line is reported with an explicit truncation marker, and the
/// reported column still refers to the original, untruncated line.
#[tokio::test]
async fn grep_bounds_long_lines_explicitly() {
    let fixture = native_fixture();
    let prefix = "q".repeat(2_000);
    std::fs::write(
        fixture.runtime.workspace().root().join("long.txt"),
        format!("{prefix}hit{}\n", "z".repeat(2_000)),
    )
    .expect("write");
    let result = run_tool(&fixture, "grep", serde_json::json!({"pattern": "hit"})).await;
    let matches = grep_matches(&result);
    assert_eq!(matches.len(), 1);
    let (_, line, column, text) = &matches[0];
    assert_eq!(*line, 1);
    assert_eq!(
        *column,
        prefix.len() as u64 + 1,
        "the column refers to the original line, not the shortened one"
    );
    assert!(
        text.len() <= MAX_GREP_LINE_BYTES,
        "the reported line is bounded: {} bytes",
        text.len()
    );
    assert!(
        text.contains("truncated"),
        "the shortening is explicit, never silent: {text}"
    );
}

/// A file whose bytes are not valid UTF-8 is not searched: binary content
/// is never fabricated as text, and it is not an error either.
#[tokio::test]
async fn grep_skips_non_utf8_files() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    std::fs::write(root.join("text.txt"), "hit here\n").expect("write text");
    // Invalid UTF-8 that nonetheless contains the ASCII pattern bytes.
    std::fs::write(root.join("binary.bin"), b"\xff\xfehit\x00\x01\xfe").expect("write binary");
    let result = run_tool(&fixture, "grep", serde_json::json!({"pattern": "hit"})).await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert_eq!(
        grep_paths(&result),
        vec!["text.txt".to_owned()],
        "the non-UTF-8 file contributes no matches"
    );
}

/// An unparsable regular expression is an explicit business failure of the
/// tool, not a runtime failure.
#[tokio::test]
async fn grep_rejects_an_invalid_regex() {
    let fixture = native_fixture();
    std::fs::write(fixture.runtime.workspace().root().join("f.txt"), "text").expect("write");
    assert_failed(
        &run_tool(
            &fixture,
            "grep",
            serde_json::json!({"pattern": "([unclosed"}),
        )
        .await,
    );
    assert_failed(
        &run_tool(
            &fixture,
            "grep",
            serde_json::json!({"pattern": "x", "glob": "["}),
        )
        .await,
    );
}

// ---------------------------------------------------------------------------
// Execution-policy configurability
// ---------------------------------------------------------------------------

/// Every ordinary native tool can be registered under any legal execution
/// policy; `background_task` stays fixed foreground-only sequential.
#[test]
#[allow(clippy::too_many_lines)] // one coherent policy matrix across three policies
fn native_tools_register_under_every_legal_execution_policy() {
    use rustx::runtime::identity::{ConversationId, ToolCallId, ToolId};
    use rustx::tools::executor::{PreflightOutcome, ToolRegistry};
    use rustx::tools::native::{NativeToolPolicies, NativeToolResources, register_native_tools};
    use rustx::tools::runtime::ConversationToolRuntime;
    use rustx::tools::types::{
        ToolCall, ToolConcurrencyPolicy, ToolExecutionPolicy, ToolInvocationMode,
        ToolInvocationPolicy,
    };

    for execution in [
        ToolExecutionPolicy::ForegroundOnly,
        ToolExecutionPolicy::BackgroundOnly,
        ToolExecutionPolicy::ModelSelectable,
    ] {
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let runtime = ConversationToolRuntime::new(
            ConversationId::new("conv-policy"),
            &workspace_root,
            dir.path().join("artifacts"),
        )
        .expect("runtime");
        let mut registry = ToolRegistry::new();
        register_native_tools(
            &mut registry,
            NativeToolResources {
                background: runtime.background().clone(),
                subagents: None,
            },
            NativeToolPolicies::uniform(ToolInvocationPolicy {
                execution,
                concurrency: ToolConcurrencyPolicy::Sequential,
            }),
        )
        .expect("every legal policy registers every ordinary native tool");

        // An ordinary filesystem tool (read) preflights under the policy.
        let read_call = |arguments| ToolCall {
            id: ToolCallId::new("call-read"),
            tool_id: ToolId::new("tool-read"),
            name: "read".to_owned(),
            arguments,
        };
        let read_mode = match execution {
            ToolExecutionPolicy::ForegroundOnly => {
                let outcome = registry
                    .preflight(&read_call(serde_json::json!({"file_path": "a.txt"})))
                    .expect("preflight");
                let PreflightOutcome::Ready(prepared) = outcome else {
                    panic!("foreground-only read must preflight as ready");
                };
                prepared.invocation.mode
            }
            ToolExecutionPolicy::BackgroundOnly => {
                let outcome = registry
                    .preflight(&read_call(serde_json::json!({"file_path": "a.txt"})))
                    .expect("preflight");
                let PreflightOutcome::Ready(prepared) = outcome else {
                    panic!("background-only read must preflight as ready");
                };
                prepared.invocation.mode
            }
            ToolExecutionPolicy::ModelSelectable => {
                let outcome = registry
                    .preflight(&read_call(serde_json::json!({
                        "__rustx_execution": "foreground",
                        "file_path": "a.txt"
                    })))
                    .expect("preflight");
                let PreflightOutcome::Ready(prepared) = outcome else {
                    panic!("model-selectable read must preflight as ready");
                };
                prepared.invocation.mode
            }
        };
        let expected_read_mode = if execution == ToolExecutionPolicy::BackgroundOnly {
            ToolInvocationMode::Background
        } else {
            ToolInvocationMode::Foreground
        };
        assert_eq!(read_mode, expected_read_mode);

        // Bash preflights under the policy.
        let bash_call = |arguments| ToolCall {
            id: ToolCallId::new("call-bash"),
            tool_id: ToolId::new("tool-bash"),
            name: "bash".to_owned(),
            arguments,
        };
        let bash_arguments = match execution {
            ToolExecutionPolicy::ForegroundOnly | ToolExecutionPolicy::BackgroundOnly => {
                serde_json::json!({"command": "echo hi"})
            }
            ToolExecutionPolicy::ModelSelectable => {
                serde_json::json!({"__rustx_execution": "background", "command": "echo hi"})
            }
        };
        let outcome = registry
            .preflight(&bash_call(bash_arguments))
            .expect("preflight");
        let PreflightOutcome::Ready(prepared) = outcome else {
            panic!("bash must preflight as ready under {execution:?}");
        };
        assert_eq!(
            prepared.invocation.mode,
            if execution == ToolExecutionPolicy::ForegroundOnly {
                ToolInvocationMode::Foreground
            } else {
                ToolInvocationMode::Background
            }
        );

        // The model-selectable decoration stays provider-neutral: only the
        // compiled definitions carry the synthetic field.
        let compiled = registry
            .model_definitions()
            .into_iter()
            .find(|definition| definition.name == "bash")
            .expect("bash compiled definition");
        if execution == ToolExecutionPolicy::ModelSelectable {
            assert!(compiled.input_schema["properties"]["__rustx_execution"].is_object());
        } else {
            assert!(compiled.input_schema["properties"]["__rustx_execution"].is_null());
        }

        // `background_task` cannot be configured away from its fixed
        // foreground-only sequential policy, regardless of the policy used
        // for the bulk registration.
        let definitions = registry.definitions();
        let intrinsic = definitions
            .iter()
            .find(|definition| definition.name == "background_task")
            .expect("background_task registered");
        assert_eq!(
            intrinsic.execution_policy,
            ToolExecutionPolicy::ForegroundOnly,
            "background_task stays foreground-only"
        );
        assert_eq!(
            intrinsic.concurrency_policy,
            ToolConcurrencyPolicy::Sequential,
            "background_task stays sequential"
        );
    }
}

/// The concrete per-tool configuration lets ordinary native tools select
/// independent execution/concurrency policies in one registry; each
/// definition preflights under exactly its own policy.
#[test]
#[allow(clippy::too_many_lines)] // one coherent mixed-policy matrix
fn mixed_native_policies_coexist_and_preflight_independently() {
    use rustx::runtime::identity::{ConversationId, ToolCallId, ToolId};
    use rustx::tools::executor::{PreflightOutcome, ToolRegistry};
    use rustx::tools::native::{NativeToolPolicies, NativeToolResources, register_native_tools};
    use rustx::tools::runtime::ConversationToolRuntime;
    use rustx::tools::types::{
        ToolCall, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolInvocationMode,
        ToolInvocationPolicy,
    };

    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let runtime = ConversationToolRuntime::new(
        ConversationId::new("conv-mixed"),
        &workspace_root,
        dir.path().join("artifacts"),
    )
    .expect("runtime");
    let mut registry = ToolRegistry::new();
    register_native_tools(
        &mut registry,
        NativeToolResources {
            background: runtime.background().clone(),
            subagents: None,
        },
        NativeToolPolicies {
            read: ToolInvocationPolicy {
                execution: ToolExecutionPolicy::ForegroundOnly,
                concurrency: ToolConcurrencyPolicy::Sequential,
            },
            write: ToolInvocationPolicy {
                execution: ToolExecutionPolicy::BackgroundOnly,
                concurrency: ToolConcurrencyPolicy::Sequential,
            },
            edit: ToolInvocationPolicy {
                execution: ToolExecutionPolicy::ForegroundOnly,
                concurrency: ToolConcurrencyPolicy::Sequential,
            },
            glob: ToolInvocationPolicy {
                execution: ToolExecutionPolicy::ForegroundOnly,
                concurrency: ToolConcurrencyPolicy::Parallel,
            },
            grep: ToolInvocationPolicy {
                execution: ToolExecutionPolicy::ModelSelectable,
                concurrency: ToolConcurrencyPolicy::Parallel,
            },
            bash: ToolInvocationPolicy {
                execution: ToolExecutionPolicy::ModelSelectable,
                concurrency: ToolConcurrencyPolicy::Sequential,
            },
        },
    )
    .expect("a mixed policy matrix registers");

    let definitions = registry.definitions();
    let policy_of = |name: &str| {
        let definition: &ToolDefinition = definitions
            .iter()
            .find(|definition| definition.name == name)
            .expect("registered tool");
        (definition.execution_policy, definition.concurrency_policy)
    };
    assert_eq!(
        policy_of("read"),
        (
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential
        )
    );
    assert_eq!(
        policy_of("write"),
        (
            ToolExecutionPolicy::BackgroundOnly,
            ToolConcurrencyPolicy::Sequential
        )
    );
    assert_eq!(
        policy_of("edit"),
        (
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential
        )
    );
    assert_eq!(
        policy_of("glob"),
        (
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel
        )
    );
    assert_eq!(
        policy_of("grep"),
        (
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Parallel
        )
    );
    assert_eq!(
        policy_of("bash"),
        (
            ToolExecutionPolicy::ModelSelectable,
            ToolConcurrencyPolicy::Sequential
        )
    );
    assert_eq!(
        policy_of("background_task"),
        (
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential
        ),
        "background_task remains outside the configurable set"
    );

    let call = |id: &str, name: &str, tool_id: &str, arguments: serde_json::Value| ToolCall {
        id: ToolCallId::new(id),
        tool_id: ToolId::new(tool_id),
        name: name.to_owned(),
        arguments,
    };
    let preflight_mode = |call: ToolCall| {
        let outcome = registry.preflight(&call).expect("preflight");
        let PreflightOutcome::Ready(prepared) = outcome else {
            panic!("every mixed-policy native call preflights as ready");
        };
        prepared.invocation.mode
    };

    // Read is foreground-only: no execution field, resolves foreground.
    assert_eq!(
        preflight_mode(call(
            "call-read",
            "read",
            "tool-read",
            serde_json::json!({"file_path": "a.txt"}),
        )),
        ToolInvocationMode::Foreground
    );
    // Write is background-only: resolves background with no execution field.
    assert_eq!(
        preflight_mode(call(
            "call-write",
            "write",
            "tool-write",
            serde_json::json!({"file_path": "a.txt", "content": "x"}),
        )),
        ToolInvocationMode::Background
    );
    // Grep and Bash are model-selectable: only the compiled model-facing
    // schema carries the reserved execution field (the canonical
    // tool-owned schema is never decorated), and the explicit choice is
    // resolved by preflight.
    for name in ["grep", "bash"] {
        let definition = definitions
            .iter()
            .find(|definition| definition.name == name)
            .expect("definition");
        assert!(
            definition.input_schema["properties"]["__rustx_execution"].is_null(),
            "the canonical tool-owned schema of {name} must never carry the reserved field"
        );
        let compiled = registry
            .model_definitions()
            .into_iter()
            .find(|compiled| compiled.name == name)
            .expect("compiled definition");
        assert!(
            compiled.input_schema["properties"]["__rustx_execution"].is_object(),
            "the compiled model-facing schema of {name} carries the execution field"
        );
    }
    let grep_mode = preflight_mode(call(
        "call-grep",
        "grep",
        "tool-grep",
        serde_json::json!({"__rustx_execution": "background", "pattern": "x"}),
    ));
    assert_eq!(grep_mode, ToolInvocationMode::Background);
    let bash_mode = preflight_mode(call(
        "call-bash",
        "bash",
        "tool-bash",
        serde_json::json!({"__rustx_execution": "foreground", "command": "echo hi"}),
    ));
    assert_eq!(bash_mode, ToolInvocationMode::Foreground);
}

/// The default per-tool configuration is conservative: every ordinary
/// native tool is foreground-only sequential.
#[test]
fn default_native_policies_are_conservative_for_every_ordinary_tool() {
    use rustx::runtime::identity::ConversationId;
    use rustx::tools::executor::ToolRegistry;
    use rustx::tools::native::{NativeToolPolicies, NativeToolResources, register_native_tools};
    use rustx::tools::runtime::ConversationToolRuntime;
    use rustx::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolInvocationPolicy};

    let defaults = NativeToolPolicies::default();
    assert_eq!(
        defaults,
        NativeToolPolicies::uniform(ToolInvocationPolicy::default())
    );
    for policy in [
        defaults.read,
        defaults.write,
        defaults.edit,
        defaults.glob,
        defaults.grep,
        defaults.bash,
    ] {
        assert_eq!(policy.execution, ToolExecutionPolicy::ForegroundOnly);
        assert_eq!(policy.concurrency, ToolConcurrencyPolicy::Sequential);
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).expect("workspace");
    let runtime = ConversationToolRuntime::new(
        ConversationId::new("conv-default-policy"),
        &workspace_root,
        dir.path().join("artifacts"),
    )
    .expect("runtime");
    let mut registry = ToolRegistry::new();
    register_native_tools(
        &mut registry,
        NativeToolResources {
            background: runtime.background().clone(),
            subagents: None,
        },
        defaults,
    )
    .expect("default policies register");
    for definition in registry.definitions() {
        if definition.name == "background_task" {
            continue;
        }
        assert_eq!(
            definition.execution_policy,
            ToolExecutionPolicy::ForegroundOnly
        );
        assert_eq!(
            definition.concurrency_policy,
            ToolConcurrencyPolicy::Sequential
        );
    }
}

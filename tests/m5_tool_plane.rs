//! Deterministic Issue #91 coverage for the native Read/Write/Edit/Grep/Glob
//! contract convergence.
//!
//! Each test uses an isolated temporary runtime. The five tools resolve
//! relative paths from the execution cwd, accept absolute host paths, own
//! their finite model-facing text projections, and leave mutations unchanged
//! when validation or atomic commit fails.

mod common;

use common::{NativeFixture, native_fixture, run_tool};
use rustx::runtime::identity::ToolExecutionId;
use rustx::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

fn text_content(result: &ToolExecutionResult) -> String {
    result
        .content
        .iter()
        .find_map(|content| match content {
            ToolResultContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("native file-tool result is plain text")
}

fn error_content(result: &ToolExecutionResult) -> String {
    match &result.status {
        ToolExecutionStatus::Failed { error } => error.clone(),
        status => panic!("expected failure, got {status:?}"),
    }
}

fn assert_failed_contains(result: &ToolExecutionResult, expected: &str) {
    let error = error_content(result);
    assert!(
        error.contains(expected),
        "{error:?} does not contain {expected:?}"
    );
}

fn workspace_path(fixture: &NativeFixture, relative: &str) -> std::path::PathBuf {
    fixture.runtime.workspace().root().join(relative)
}

fn edit(old_text: &str, new_text: &str) -> serde_json::Value {
    serde_json::json!({"oldText": old_text, "newText": new_text})
}

fn assert_no_stray_temps(fixture: &NativeFixture) {
    fn walk(path: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".rustx-tool-tmp-")
            {
                found.push(entry_path.clone());
            }
            if entry_path.is_dir() {
                walk(&entry_path, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(fixture.runtime.workspace().root(), &mut found);
    assert!(found.is_empty(), "temporary commit files remain: {found:?}");
}

/// Executes a native executor without registry preflight. This keeps direct
/// typed-input validation covered separately from the canonical registry
/// boundary and proves invalid arguments cannot cause filesystem side effects.
async fn run_tool_unchecked(
    fixture: &NativeFixture,
    name: &str,
    arguments: serde_json::Value,
) -> ToolExecutionResult {
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
    executor
        .execute(
            ToolInvocation {
                call_id: ToolCallId::new("call-unchecked"),
                tool_id: definition.id,
                tool_name: name.to_owned(),
                mode: ToolInvocationMode::Foreground,
                arguments,
            },
            ToolExecutionContext::new(
                fixture.runtime.conversation_id(),
                None,
                rustx::runtime::ExecutionCancellation::detached(
                    rustx::runtime::CancellationSignal::new(),
                    rustx::runtime::types::CancellationReason::UserRequested,
                ),
                fixture.runtime.workspace(),
                &reporter,
                fixture.runtime.artifacts(),
                fixture.runtime.tool_output(),
                fixture.runtime.environment(),
                None,
            ),
        )
        .await
}

#[tokio::test]
async fn direct_executor_validation_rejects_invalid_inputs_without_side_effects() {
    let fixture = native_fixture();
    let existing = workspace_path(&fixture, "unchanged.txt");
    std::fs::write(&existing, "original").expect("fixture");

    let invalid_write = run_tool_unchecked(
        &fixture,
        "write",
        serde_json::json!({"path": "new.txt", "content": 7}),
    )
    .await;
    assert_failed_contains(&invalid_write, "invalid write arguments");
    assert!(!workspace_path(&fixture, "new.txt").exists());

    let invalid_edit = run_tool_unchecked(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "unchanged.txt",
            "edits": [{"oldText": "original"}]
        }),
    )
    .await;
    assert_failed_contains(&invalid_edit, "invalid edit arguments");
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "original");

    let invalid_read = run_tool_unchecked(&fixture, "read", serde_json::json!({"path": 7})).await;
    assert_failed_contains(&invalid_read, "invalid read arguments");
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_is_cwd_oriented_and_has_no_implicit_200_line_window() {
    let fixture = native_fixture();
    let lines: Vec<String> = (1..=250).map(|index| format!("line-{index:03}")).collect();
    std::fs::write(workspace_path(&fixture, "many.txt"), lines.join("\n")).expect("fixture");

    let result = run_tool(&fixture, "read", serde_json::json!({"path": "many.txt"})).await;
    let output = text_content(&result);
    assert_eq!(output.lines().count(), 250);
    assert!(output.starts_with("line-001\n"));
    assert!(output.ends_with("line-250"));
}

#[tokio::test]
async fn read_line_accounting_handles_empty_and_final_newline_cases() {
    let fixture = native_fixture();
    let empty = workspace_path(&fixture, "empty.txt");
    std::fs::write(&empty, b"").expect("empty fixture");
    let result = run_tool(&fixture, "read", serde_json::json!({"path": "empty.txt"})).await;
    assert_eq!(text_content(&result), "");
    let beyond = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": "empty.txt", "offset": 2}),
    )
    .await;
    assert_failed_contains(&beyond, "Offset 2 is beyond end of file (1 lines total)");

    let no_final = workspace_path(&fixture, "no-final.txt");
    std::fs::write(&no_final, "one").expect("no-final fixture");
    let beyond = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": "no-final.txt", "offset": 2}),
    )
    .await;
    assert_failed_contains(&beyond, "(1 lines total)");

    let final_newline = workspace_path(&fixture, "final.txt");
    std::fs::write(&final_newline, "one\n").expect("final fixture");
    let trailing = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": "final.txt", "offset": 2}),
    )
    .await;
    assert_eq!(text_content(&trailing), "");
}

#[tokio::test]
async fn read_normalizes_zero_offset_and_reports_user_page_continuation() {
    let fixture = native_fixture();
    std::fs::write(
        workspace_path(&fixture, "sample.txt"),
        "one\ntwo\nthree\nfour",
    )
    .expect("fixture");
    let zero = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": "sample.txt", "offset": 0, "limit": 1}),
    )
    .await;
    assert_eq!(
        text_content(&zero),
        "one\n\n[3 more lines in file. Use offset=2 to continue.]"
    );

    let page = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": "sample.txt", "offset": 2, "limit": 2}),
    )
    .await;
    assert_eq!(
        text_content(&page),
        "two\nthree\n\n[1 more lines in file. Use offset=4 to continue.]"
    );
}

#[tokio::test]
async fn native_tools_interpret_dot_and_dotdot_lexically_before_filesystem_access() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root();

    let write = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": "missing/../target.txt",
            "content": "initial"
        }),
    )
    .await;
    assert!(matches!(write.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(root.join("target.txt")).unwrap(),
        "initial"
    );
    assert!(!root.join("missing").exists());

    let read = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": "./missing/../target.txt"}),
    )
    .await;
    assert_eq!(text_content(&read), "initial");

    let edit_result = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "./sub/../target.txt",
            "edits": [edit("initial", "edited")]
        }),
    )
    .await;
    assert!(matches!(edit_result.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(root.join("target.txt")).unwrap(),
        "edited"
    );
    assert!(!root.join("sub").exists());

    let absolute = fixture
        .dir()
        .path()
        .join("missing-absolute/../absolute.txt");
    let absolute_result = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": absolute.to_str().expect("utf8 absolute path"),
            "content": "absolute"
        }),
    )
    .await;
    assert!(matches!(
        absolute_result.status,
        ToolExecutionStatus::Success
    ));
    assert_eq!(
        std::fs::read_to_string(fixture.dir().path().join("absolute.txt")).unwrap(),
        "absolute"
    );
    assert!(!fixture.dir().path().join("missing-absolute").exists());

    let search_root = root.join("search");
    std::fs::create_dir_all(&search_root).expect("search root");
    std::fs::write(search_root.join("hit.txt"), "needle\n").expect("search fixture");
    let grep = run_tool(
        &fixture,
        "grep",
        serde_json::json!({
            "pattern": "needle",
            "path": "missing/../search"
        }),
    )
    .await;
    assert_eq!(text_content(&grep), "hit.txt:1: needle");
    let glob = run_tool(
        &fixture,
        "glob",
        serde_json::json!({
            "pattern": "*.txt",
            "path": "./missing/../search"
        }),
    )
    .await;
    assert_eq!(text_content(&glob), "hit.txt");
}

#[tokio::test]
async fn read_uses_contiguous_complete_line_head_for_line_and_byte_limits() {
    let fixture = native_fixture();
    let exact_lines: Vec<String> = (1..=2000).map(|index| format!("exact-{index}")).collect();
    std::fs::write(
        workspace_path(&fixture, "exact-lines.txt"),
        exact_lines.join("\n"),
    )
    .expect("exact line fixture");
    let exact = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": "exact-lines.txt"}),
    )
    .await;
    assert_eq!(text_content(&exact).lines().count(), 2000);
    assert!(!text_content(&exact).contains("Use offset="));

    let exact_with_final_newline = exact_lines.join("\n") + "\n";
    std::fs::write(
        workspace_path(&fixture, "exact-lines-final-newline.txt"),
        exact_with_final_newline,
    )
    .expect("final-newline line fixture");
    let exact_final = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": "exact-lines-final-newline.txt"}),
    )
    .await;
    assert_eq!(text_content(&exact_final).lines().count(), 2000);
    assert!(text_content(&exact_final).ends_with('\n'));
    assert!(!text_content(&exact_final).contains("Use offset="));

    let lines: Vec<String> = (1..=2001).map(|index| format!("line-{index}")).collect();
    std::fs::write(workspace_path(&fixture, "many.txt"), lines.join("\n")).expect("fixture");
    let result = run_tool(&fixture, "read", serde_json::json!({"path": "many.txt"})).await;
    let output = text_content(&result);
    assert!(output.contains("[Showing lines 1-2000 of 2001. Use offset=2001 to continue.]"));
    assert!(!output.contains("...[truncated"));
    assert!(!output.contains("line-2001\n"));

    let byte_lines = format!("{}\nnext", "é".repeat(25_600));
    std::fs::write(workspace_path(&fixture, "bytes.txt"), byte_lines).expect("fixture");
    let result = run_tool(&fixture, "read", serde_json::json!({"path": "bytes.txt"})).await;
    let output = text_content(&result);
    assert!(output.contains("(50KB limit). Use offset=2 to continue."));
    assert!(!output.contains("next"));
    assert!(
        output
            .lines()
            .next()
            .is_some_and(|line| line.len() == 51_200)
    );

    let oversized = format!("{}\nrest", "x".repeat(51_201));
    std::fs::write(workspace_path(&fixture, "oversized.txt"), oversized).expect("fixture");
    let result = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": "oversized.txt"}),
    )
    .await;
    let output = text_content(&result);
    assert!(output.contains("Line 1 is 50.0KB, exceeds 50.0KB limit"));
    assert!(!output.contains(&"x".repeat(100)));
}

// ---------------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------------

#[tokio::test]
async fn write_creates_parents_accepts_absolute_paths_and_reports_utf8_bytes() {
    let fixture = native_fixture();
    let relative = run_tool(
        &fixture,
        "write",
        serde_json::json!({"path": "deep/nested/file.txt", "content": "héllo"}),
    )
    .await;
    assert_eq!(
        text_content(&relative),
        "Successfully wrote 6 bytes to deep/nested/file.txt"
    );
    assert_eq!(
        std::fs::read_to_string(workspace_path(&fixture, "deep/nested/file.txt")).unwrap(),
        "héllo"
    );

    let outside = fixture.dir().path().join("outside").join("file.txt");
    let absolute = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": outside.to_str().expect("utf8"),
            "content": "outside"
        }),
    )
    .await;
    assert_eq!(
        text_content(&absolute),
        format!("Successfully wrote 7 bytes to {}", outside.display())
    );
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "outside");
}

#[tokio::test]
async fn write_overwrites_empty_content_and_failed_commit_leaves_target_unchanged() {
    let fixture = native_fixture();
    let path = workspace_path(&fixture, "file.txt");
    std::fs::write(&path, "old").expect("fixture");
    run_tool(
        &fixture,
        "write",
        serde_json::json!({"path": "file.txt", "content": ""}),
    )
    .await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");

    let directory = workspace_path(&fixture, "directory");
    std::fs::create_dir(&directory).expect("directory");
    let failed = run_tool(
        &fixture,
        "write",
        serde_json::json!({"path": "directory", "content": "replacement"}),
    )
    .await;
    assert_failed_contains(&failed, "cannot persist");
    assert!(directory.is_dir());
    assert_no_stray_temps(&fixture);
}

#[cfg(unix)]
#[tokio::test]
async fn write_and_edit_follow_final_component_symlinks_without_replacing_the_link() {
    use std::os::unix::fs::symlink;

    let fixture = native_fixture();
    let target = fixture.dir().path().join("symlink-target.txt");
    let link = workspace_path(&fixture, "link.txt");
    std::fs::write(&target, "old").expect("target");
    symlink(&target, &link).expect("symlink");
    let write = run_tool(
        &fixture,
        "write",
        serde_json::json!({"path": "link.txt", "content": "new"}),
    )
    .await;
    assert!(matches!(write.status, ToolExecutionStatus::Success));
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");

    let edit = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"path": "link.txt", "edits": [edit("new", "edited")]}),
    )
    .await;
    assert!(matches!(edit.status, ToolExecutionStatus::Success));
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "edited");
}

#[tokio::test]
async fn managed_output_is_readable_but_model_mutation_cannot_replace_a_live_inode() {
    let fixture = native_fixture();
    let execution_id = ToolExecutionId::background(91);
    let advertised = fixture
        .runtime
        .tool_output()
        .allocate_background_output(&execution_id)
        .expect("allocate managed live output");
    let mut sink = fixture
        .runtime
        .tool_output()
        .open_background_output_sink(&execution_id)
        .expect("open managed append sink");
    sink.append("A").expect("append A");

    let write = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": advertised.to_str().expect("utf8 advertised path"),
            "content": "replacement"
        }),
    )
    .await;
    assert_failed_contains(&write, "managed tool-output root");

    let edit = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": advertised.to_str().expect("utf8 advertised path"),
            "edits": [edit("A", "replacement")]
        }),
    )
    .await;
    assert_failed_contains(&edit, "managed tool-output root");

    sink.append("B").expect("append B through original inode");
    let read = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": advertised.to_str().expect("utf8 advertised path")}),
    )
    .await;
    assert_eq!(text_content(&read), "AB");
    assert_eq!(std::fs::read_to_string(&advertised).unwrap(), "AB");
}

#[tokio::test]
async fn managed_output_descendants_are_rejected_before_parent_creation() {
    let fixture = native_fixture();
    let root = fixture.runtime.tool_output().root();
    let missing = root.join("tasks").join("new/deep/file.txt");
    let write = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": missing.to_str().expect("utf8 managed path"),
            "content": "blocked"
        }),
    )
    .await;
    assert_failed_contains(&write, "managed tool-output root");
    assert!(!root.join("tasks/new").exists());

    let existing = root.join("results/existing.txt");
    std::fs::write(&existing, "original").expect("managed fixture");
    let edit_result = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": existing.to_str().expect("utf8 managed path"),
            "edits": [edit("original", "blocked")]
        }),
    )
    .await;
    assert_failed_contains(&edit_result, "managed tool-output root");
    assert_eq!(std::fs::read_to_string(existing).unwrap(), "original");

    let lexical = root.join("tasks/../results/lexical.txt");
    let lexical_result = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": lexical.to_str().expect("utf8 lexical managed path"),
            "content": "blocked"
        }),
    )
    .await;
    assert_failed_contains(&lexical_result, "managed tool-output root");
    assert!(!root.join("results/lexical.txt").exists());

    let lexical_missing = root.join("tasks/missing/../results/lexical-missing.txt");
    let lexical_missing_result = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": lexical_missing.to_str().expect("utf8 lexical managed path"),
            "content": "blocked"
        }),
    )
    .await;
    assert_failed_contains(&lexical_missing_result, "managed tool-output root");
    assert!(!root.join("tasks/missing").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn managed_output_symlink_mutations_are_rejected_without_side_effects() {
    use std::os::unix::fs::symlink;

    let fixture = native_fixture();
    let managed_root = fixture.runtime.tool_output().root();
    let managed_file = managed_root.join("tasks/managed.txt");
    std::fs::write(&managed_file, "original").expect("managed fixture");

    let final_link = workspace_path(&fixture, "managed-final-link.txt");
    symlink(&managed_file, &final_link).expect("final symlink");
    let final_result = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": "managed-final-link.txt",
            "content": "blocked"
        }),
    )
    .await;
    assert_failed_contains(&final_result, "managed tool-output root");
    assert_eq!(std::fs::read_to_string(&managed_file).unwrap(), "original");
    assert!(
        std::fs::symlink_metadata(&final_link)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let ancestor_link = workspace_path(&fixture, "managed-ancestor");
    symlink(managed_root.join("tasks"), &ancestor_link).expect("ancestor symlink");
    let ancestor_target = ancestor_link.join("new.txt");
    let ancestor_result = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": ancestor_target.to_str().expect("utf8 ancestor path"),
            "content": "blocked"
        }),
    )
    .await;
    assert_failed_contains(&ancestor_result, "managed tool-output root");
    assert!(!managed_root.join("tasks/new.txt").exists());

    let dangling_target = managed_root.join("tasks/dangling.txt");
    let dangling_link = workspace_path(&fixture, "managed-dangling-link.txt");
    symlink(&dangling_target, &dangling_link).expect("dangling final symlink");
    let dangling_result = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": "managed-dangling-link.txt",
            "content": "blocked"
        }),
    )
    .await;
    assert_failed_contains(&dangling_result, "managed tool-output root");
    assert!(!dangling_target.exists());

    let lexical_link = workspace_path(&fixture, "managed-lexical-link");
    symlink(managed_root.join("tasks"), &lexical_link).expect("lexical ancestor symlink");
    let lexical_target = lexical_link.join("../managed-lexical-link/lexical-managed.txt");
    let lexical_result = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": lexical_target.to_str().expect("utf8 lexical symlink path"),
            "content": "blocked"
        }),
    )
    .await;
    assert_failed_contains(&lexical_result, "managed tool-output root");
    assert!(!managed_root.join("tasks/lexical-managed.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn write_creates_parents_for_a_dangling_symlink_destination() {
    use std::os::unix::fs::symlink;

    let fixture = native_fixture();
    let destination = fixture.dir().path().join("outside/deep/file.txt");
    let link = workspace_path(&fixture, "outside-link.txt");
    symlink(&destination, &link).expect("dangling destination link");
    let result = run_tool(
        &fixture,
        "write",
        serde_json::json!({"path": "outside-link.txt", "content": "written"}),
    )
    .await;
    assert!(matches!(result.status, ToolExecutionStatus::Success));
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "written");
    assert!(
        std::fs::symlink_metadata(link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn dangling_symlink_destination_with_lexical_parent_is_written_through() {
    use std::os::unix::fs::symlink;

    let fixture = native_fixture();
    let existing_parent = fixture.dir().path().join("symlink-parent");
    std::fs::create_dir_all(&existing_parent).expect("symlink parent");
    let destination = existing_parent.join("../symlink-destination/deep/file.txt");
    let link = workspace_path(&fixture, "lexical-dangling-link.txt");
    symlink(&destination, &link).expect("dangling lexical destination link");

    let result = run_tool(
        &fixture,
        "write",
        serde_json::json!({
            "path": "lexical-dangling-link.txt",
            "content": "written"
        }),
    )
    .await;
    assert!(matches!(result.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(
            fixture
                .dir()
                .path()
                .join("symlink-destination/deep/file.txt")
        )
        .unwrap(),
        "written"
    );
    assert_eq!(std::fs::read_to_string(link).unwrap(), "written");
}

// ---------------------------------------------------------------------------
// Edit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_applies_disjoint_replacements_against_one_original_snapshot() {
    let fixture = native_fixture();
    let path = workspace_path(&fixture, "edit.txt");
    std::fs::write(&path, "alpha\nbeta\ngamma").expect("fixture");
    let result = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "edit.txt",
            "edits": [edit("alpha", "A"), edit("gamma", "G")]
        }),
    )
    .await;
    assert_eq!(
        text_content(&result),
        "Successfully replaced 2 block(s) in edit.txt."
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "A\nbeta\nG");
}

#[tokio::test]
async fn edit_preserves_bom_and_original_line_ending_style() {
    let fixture = native_fixture();
    for (name, original) in [
        ("lf.txt", "\u{FEFF}one\ntwo\n"),
        ("crlf.txt", "\u{FEFF}one\r\ntwo\r\n"),
        ("cr.txt", "\u{FEFF}one\rtwo\r"),
    ] {
        let path = workspace_path(&fixture, name);
        std::fs::write(&path, original.as_bytes()).expect("fixture");
        let result = run_tool(
            &fixture,
            "edit",
            serde_json::json!({"path": name, "edits": [edit("two", "TWO")]}),
        )
        .await;
        assert!(matches!(result.status, ToolExecutionStatus::Success));
        let updated = std::fs::read(&path).expect("updated");
        assert!(updated.starts_with("\u{FEFF}".as_bytes()));
        assert!(String::from_utf8(updated).unwrap().contains("TWO"));
        if original.contains("\r\n") {
            assert_eq!(
                std::fs::read(&path)
                    .unwrap()
                    .windows(2)
                    .filter(|w| *w == b"\r\n")
                    .count(),
                2
            );
        } else if original.contains('\r') {
            assert!(
                !std::fs::read(&path)
                    .unwrap()
                    .windows(2)
                    .any(|w| w == b"\r\n")
            );
        } else {
            assert!(
                !std::fs::read(&path)
                    .unwrap()
                    .windows(2)
                    .any(|w| w == b"\r\n")
            );
        }
    }
}

#[tokio::test]
async fn edit_fuzzy_matching_normalizes_quotes_dashes_spaces_and_trailing_whitespace() {
    let fixture = native_fixture();
    let path = workspace_path(&fixture, "fuzzy.txt");
    std::fs::write(&path, "‘quote’ — value\u{00A0}  \nnext").expect("fixture");
    let result = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "fuzzy.txt",
            "edits": [edit("'quote' - value  ", "changed")]
        }),
    )
    .await;
    assert!(matches!(result.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "changed\u{00A0}  \nnext",
        "fuzzy matching must not normalize source text outside the matched range"
    );

    // Exact matching wins even when the fuzzy normalization would make the
    // same oldText ambiguous.
    let exact_path = workspace_path(&fixture, "exact-before-fuzzy.txt");
    std::fs::write(&exact_path, "‘same’\n'same'").expect("exact fixture");
    let exact = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "exact-before-fuzzy.txt",
            "edits": [edit("'same'", "changed")]
        }),
    )
    .await;
    assert!(matches!(exact.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(exact_path).unwrap(),
        "‘same’\nchanged"
    );
}

#[tokio::test]
async fn edit_resolves_exact_and_fuzzy_matches_independently_per_edit() {
    let fixture = native_fixture();
    let original = "‘same’\n'same'\n‘other’";
    let path = workspace_path(&fixture, "mixed.txt");
    std::fs::write(&path, original).expect("fixture");
    let result = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "mixed.txt",
            "edits": [edit("'same'", "EXACT"), edit("'other'", "FUZZY")]
        }),
    )
    .await;
    assert!(matches!(result.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "‘same’\nEXACT\nFUZZY"
    );

    let reverse_path = workspace_path(&fixture, "mixed-reverse.txt");
    std::fs::write(&reverse_path, original).expect("reverse fixture");
    let reverse = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "mixed-reverse.txt",
            "edits": [edit("'other'", "FUZZY"), edit("'same'", "EXACT")]
        }),
    )
    .await;
    assert!(matches!(reverse.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(reverse_path).unwrap(),
        "‘same’\nEXACT\nFUZZY"
    );
}

#[tokio::test]
async fn edit_fuzzy_mapping_preserves_unrelated_source_representation() {
    let fixture = native_fixture();
    let path = workspace_path(&fixture, "fuzzy-collateral.txt");
    let original = "‘target’ — café and Δ\nuntouched “text”";
    std::fs::write(&path, original).expect("fixture");
    let result = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "fuzzy-collateral.txt",
            "edits": [edit("'target' -", "changed")]
        }),
    )
    .await;
    assert!(matches!(result.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "changed café and Δ\nuntouched “text”"
    );

    let expansion_path = workspace_path(&fixture, "fuzzy-expansion.txt");
    std::fs::write(&expansion_path, "before ﬃ after").expect("compatibility fixture");
    let expansion = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "fuzzy-expansion.txt",
            "edits": [edit("ffi", "X")]
        }),
    )
    .await;
    assert!(matches!(expansion.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(expansion_path).unwrap(),
        "before X after"
    );

    let composition_path = workspace_path(&fixture, "fuzzy-composition.txt");
    std::fs::write(&composition_path, "before e\u{301} after").expect("composition fixture");
    let composition = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "fuzzy-composition.txt",
            "edits": [edit("é", "X")]
        }),
    )
    .await;
    assert!(matches!(composition.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(composition_path).unwrap(),
        "before X after"
    );

    let hangul_path = workspace_path(&fixture, "fuzzy-hangul.txt");
    std::fs::write(&hangul_path, "before \u{1100}\u{1161} after").expect("Hangul fixture");
    let hangul = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "fuzzy-hangul.txt",
            "edits": [edit("가", "X")]
        }),
    )
    .await;
    assert!(matches!(hangul.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(hangul_path).unwrap(),
        "before X after"
    );
}

#[tokio::test]
async fn edit_rejects_unsafe_partial_normalization_ranges_without_mutation() {
    let fixture = native_fixture();

    let expansion_path = workspace_path(&fixture, "unsafe-expansion.txt");
    let expansion_original = "before ﬃ after";
    std::fs::write(&expansion_path, expansion_original).expect("expansion fixture");
    let partial = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "unsafe-expansion.txt",
            "edits": [edit("ff", "X")]
        }),
    )
    .await;
    assert_failed_contains(&partial, "safely map");
    assert_eq!(
        std::fs::read_to_string(&expansion_path).unwrap(),
        expansion_original
    );

    let ambiguous_partial = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "unsafe-expansion.txt",
            "edits": [edit("f", "X")]
        }),
    )
    .await;
    assert!(matches!(
        ambiguous_partial.status,
        ToolExecutionStatus::Failed { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(&expansion_path).unwrap(),
        expansion_original
    );

    let combining_path = workspace_path(&fixture, "unsafe-combining.txt");
    let combining_original = "before a\u{301}\u{323} after";
    std::fs::write(&combining_path, combining_original).expect("combining fixture");
    let partial = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "unsafe-combining.txt",
            "edits": [edit("ạ", "X")]
        }),
    )
    .await;
    assert_failed_contains(&partial, "safely map");
    assert_eq!(
        std::fs::read_to_string(&combining_path).unwrap(),
        combining_original
    );
}

#[tokio::test]
async fn edit_accepts_safe_full_normalized_ranges_but_rejects_unsafe_mixed_edits_atomically() {
    let fixture = native_fixture();

    let safe_path = workspace_path(&fixture, "safe-combining.txt");
    std::fs::write(&safe_path, "before a\u{301}\u{323} after").expect("safe fixture");
    let safe = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "safe-combining.txt",
            "edits": [edit("ạ\u{301}", "X")]
        }),
    )
    .await;
    assert!(matches!(safe.status, ToolExecutionStatus::Success));
    assert_eq!(
        std::fs::read_to_string(safe_path).unwrap(),
        "before X after"
    );

    let mixed_path = workspace_path(&fixture, "unsafe-mixed.txt");
    let mixed_original = "exact\nbefore ﬃ after";
    std::fs::write(&mixed_path, mixed_original).expect("mixed fixture");
    let mixed = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "unsafe-mixed.txt",
            "edits": [edit("exact", "changed"), edit("ff", "X")]
        }),
    )
    .await;
    assert_failed_contains(&mixed, "safely map");
    assert_eq!(std::fs::read_to_string(mixed_path).unwrap(), mixed_original);
}

#[tokio::test]
async fn edit_rejects_overlap_between_exact_and_fuzzy_ranges_atomically() {
    let fixture = native_fixture();
    let path = workspace_path(&fixture, "mixed-overlap.txt");
    let original = "‘same’";
    std::fs::write(&path, original).expect("fixture");
    let result = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "mixed-overlap.txt",
            "edits": [edit("‘same’", "EXACT"), edit("'same'", "FUZZY")]
        }),
    )
    .await;
    assert_failed_contains(&result, "overlap");
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
}

#[tokio::test]
async fn edit_fuzzy_empty_anchors_fail_without_panicking_or_mutating() {
    let fixture = native_fixture();
    for (name, old_text) in [("ascii-space.txt", "   "), ("nbsp.txt", "\u{00a0}")] {
        let path = workspace_path(&fixture, name);
        std::fs::write(&path, "unchanged\n").expect("fixture");
        let result = run_tool(
            &fixture,
            "edit",
            serde_json::json!({
                "path": name,
                "edits": [edit(old_text, "replacement")]
            }),
        )
        .await;
        assert_failed_contains(&result, "Could not find");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "unchanged\n");
    }
}

#[tokio::test]
async fn edit_non_overlapping_occurrences_are_used_and_ambiguous_fuzzy_matches_fail() {
    let fixture = native_fixture();
    let path = workspace_path(&fixture, "occurrences.txt");
    std::fs::write(&path, "aaa").expect("fixture");
    let result = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"path": "occurrences.txt", "edits": [edit("aa", "X")]}),
    )
    .await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Xa");
    assert!(matches!(result.status, ToolExecutionStatus::Success));

    std::fs::write(&path, "‘same’\n‘same’").expect("fixture");
    let failed = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"path": "occurrences.txt", "edits": [edit("'same'", "x")]}),
    )
    .await;
    assert_failed_contains(&failed, "Found 2 occurrences");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "‘same’\n‘same’");

    std::fs::write(&path, "alpha alpha").expect("exact ambiguity fixture");
    let exact_ambiguous = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"path": "occurrences.txt", "edits": [edit("alpha", "x")]}),
    )
    .await;
    assert_failed_contains(&exact_ambiguous, "Found 2 occurrences");
    assert_eq!(std::fs::read_to_string(path).unwrap(), "alpha alpha");
}

#[tokio::test]
async fn edit_rejects_missing_later_edits_overlap_and_noop_without_partial_commit() {
    let fixture = native_fixture();
    let path = workspace_path(&fixture, "atomic-edit.txt");
    std::fs::write(&path, "alpha beta gamma").expect("fixture");

    let missing = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "atomic-edit.txt",
            "edits": [edit("alpha", "A"), edit("missing", "M")]
        }),
    )
    .await;
    assert_failed_contains(&missing, "Could not find edits[1]");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha beta gamma");

    let overlap = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "atomic-edit.txt",
            "edits": [edit("alpha beta", "x"), edit("beta gamma", "y")]
        }),
    )
    .await;
    assert_failed_contains(&overlap, "overlap");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha beta gamma");

    let noop = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"path": "atomic-edit.txt", "edits": [edit("alpha", "alpha")]}),
    )
    .await;
    assert_failed_contains(&noop, "No changes made");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha beta gamma");
}

#[tokio::test]
async fn edit_accepts_known_model_argument_variants_but_keeps_one_canonical_execution_shape() {
    let fixture = native_fixture();
    let variants = [
        serde_json::json!({"path": "one.txt", "edits": [edit("a", "b")]}),
        serde_json::json!({"path": "two.txt", "edits": serde_json::to_string(&vec![edit("a", "b")]).unwrap()}),
        serde_json::json!({"path": "three.txt", "edits": edit("a", "b")}),
        serde_json::json!({"path": "four.txt", "oldText": "a", "newText": "b"}),
    ];
    for (index, arguments) in variants.into_iter().enumerate() {
        std::fs::write(
            workspace_path(
                &fixture,
                &format!("{}.txt", ["one", "two", "three", "four"][index]),
            ),
            "a",
        )
        .expect("fixture");
        let result = run_tool(&fixture, "edit", arguments).await;
        assert!(matches!(result.status, ToolExecutionStatus::Success));
    }
    for name in ["one", "two", "three", "four"] {
        assert_eq!(
            std::fs::read_to_string(workspace_path(&fixture, &format!("{name}.txt"))).unwrap(),
            "b"
        );
    }

    let outside = fixture.dir().path().join("absolute-edit.txt");
    std::fs::write(&outside, "outside").expect("absolute fixture");
    let result = run_tool(
        &fixture,
        "edit",
        serde_json::json!({
            "path": outside.to_str().expect("utf8"),
            "edits": [edit("outside", "edited")]
        }),
    )
    .await;
    assert!(matches!(result.status, ToolExecutionStatus::Success));
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "edited");
}

// ---------------------------------------------------------------------------
// Grep
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grep_is_in_process_text_output_with_cwd_absolute_single_file_and_context() {
    let fixture = native_fixture();
    std::fs::create_dir_all(workspace_path(&fixture, "src")).expect("src");
    std::fs::write(
        workspace_path(&fixture, "src/a.txt"),
        "before\nhit\nafter\n",
    )
    .expect("fixture");
    let result = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "hit", "path": "src", "context": 1}),
    )
    .await;
    let output = text_content(&result);
    assert!(output.contains("a.txt-1- before"));
    assert!(output.contains("a.txt:2: hit"));
    assert!(output.contains("a.txt-3- after"));

    let outside = fixture.dir().path().join("outside.txt");
    std::fs::write(&outside, "outside-hit").expect("outside");
    let result = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "hit", "path": outside.to_str().unwrap()}),
    )
    .await;
    assert_eq!(text_content(&result), "outside.txt:1: outside-hit");
}

#[tokio::test]
async fn grep_default_and_large_limits_are_actionable_and_safe() {
    let fixture = native_fixture();
    for index in 0..101 {
        std::fs::write(
            workspace_path(&fixture, &format!("file-{index:03}.txt")),
            "hit\n",
        )
        .expect("fixture");
    }
    let limited = run_tool(&fixture, "grep", serde_json::json!({"pattern": "hit"})).await;
    let limited_text = text_content(&limited);
    assert!(
        limited_text
            .contains("100 matches limit reached. Use limit=200 for more, or refine pattern")
    );
    assert!(!limited_text.contains("file-100.txt:1:"));

    let large = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "hit", "limit": 2001}),
    )
    .await;
    assert!(matches!(large.status, ToolExecutionStatus::Success));
    assert!(!text_content(&large).contains("matches limit reached"));

    let huge = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "hit", "limit": u64::MAX}),
    )
    .await;
    assert!(matches!(huge.status, ToolExecutionStatus::Success));
}

#[tokio::test]
async fn grep_bounds_long_utf8_lines_and_the_50kb_projection() {
    let fixture = native_fixture();
    let long = format!("{}hit{}", "a".repeat(600), "é".repeat(600));
    std::fs::write(workspace_path(&fixture, "long.txt"), &long).expect("fixture");
    let result = run_tool(&fixture, "grep", serde_json::json!({"pattern": "hit"})).await;
    let output = text_content(&result);
    assert!(output.contains("Some lines truncated to 500 chars. Use read tool to see full lines"));
    assert!(output.contains("... [truncated]"));
    assert!(output.chars().count() > 500);

    for index in 0..100 {
        std::fs::write(
            workspace_path(&fixture, &format!("large-{index:03}.txt")),
            format!("{} hit", "x".repeat(900)),
        )
        .expect("fixture");
    }
    let result = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "hit", "limit": u64::MAX}),
    )
    .await;
    assert!(text_content(&result).contains("50KB limit reached"));
}

#[tokio::test]
async fn grep_preserves_regex_literal_case_context_and_binary_skip_semantics() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root();
    std::fs::write(
        root.join("patterns.txt"),
        "value a.c here\nvalue abc here\nNeedle\nneedle\nbefore\nhit\nafter\n",
    )
    .expect("pattern fixture");

    let regex = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "a.c", "path": "patterns.txt"}),
    )
    .await;
    let regex_text = text_content(&regex);
    assert!(regex_text.contains("patterns.txt:1: value a.c here"));
    assert!(regex_text.contains("patterns.txt:2: value abc here"));

    let literal = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "a.c", "path": "patterns.txt", "literal": true}),
    )
    .await;
    let literal_text = text_content(&literal);
    assert!(literal_text.contains("patterns.txt:1: value a.c here"));
    assert!(!literal_text.contains("patterns.txt:2:"));

    let sensitive = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "needle", "path": "patterns.txt"}),
    )
    .await;
    assert_eq!(text_content(&sensitive), "patterns.txt:4: needle");
    let insensitive = run_tool(
        &fixture,
        "grep",
        serde_json::json!({
            "pattern": "needle",
            "path": "patterns.txt",
            "ignoreCase": true
        }),
    )
    .await;
    let insensitive_text = text_content(&insensitive);
    assert!(insensitive_text.contains("patterns.txt:3: Needle"));
    assert!(insensitive_text.contains("patterns.txt:4: needle"));

    let context = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "hit", "path": "patterns.txt", "context": 1}),
    )
    .await;
    let context_text = text_content(&context);
    assert!(context_text.contains("patterns.txt-5- before"));
    assert!(context_text.contains("patterns.txt:6: hit"));
    assert!(context_text.contains("patterns.txt-7- after"));

    let invalid_regex = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "([unclosed", "path": "patterns.txt"}),
    )
    .await;
    assert_failed_contains(&invalid_regex, "regex");
    let invalid_glob = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"pattern": "needle", "glob": "["}),
    )
    .await;
    assert_failed_contains(&invalid_glob, "glob");

    std::fs::write(root.join("binary.bin"), b"\xff\xfehit\x00\x01").expect("binary fixture");
    let binary = run_tool(&fixture, "grep", serde_json::json!({"pattern": "hit"})).await;
    assert!(!text_content(&binary).contains("binary.bin"));
}

#[cfg(unix)]
#[tokio::test]
async fn grep_and_glob_keep_hidden_and_ignore_file_behavior_and_never_follow_symlinks() {
    use std::os::unix::fs::symlink;

    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root();
    std::fs::write(root.join(".gitignore"), "ignored.txt\n").expect("ignore fixture");
    std::fs::write(root.join(".hidden.txt"), "needle\n").expect("hidden fixture");
    std::fs::write(root.join("ignored.txt"), "needle\n").expect("ignored fixture");
    let outside = fixture.dir().path().join("outside-search.txt");
    std::fs::write(&outside, "needle\n").expect("outside fixture");
    symlink(&outside, root.join("linked.txt")).expect("search symlink");

    let grep = run_tool(&fixture, "grep", serde_json::json!({"pattern": "needle"})).await;
    let grep_text = text_content(&grep);
    assert!(grep_text.contains(".hidden.txt:1: needle"));
    assert!(grep_text.contains("ignored.txt:1: needle"));
    assert!(!grep_text.contains("linked.txt"));

    let glob = run_tool(&fixture, "glob", serde_json::json!({"pattern": "**/*"})).await;
    let glob_text = text_content(&glob);
    assert!(glob_text.contains(".hidden.txt"));
    assert!(glob_text.contains("ignored.txt"));
    assert!(!glob_text.contains("linked.txt"));
}

#[tokio::test]
async fn grep_no_matches_is_exact_plain_text() {
    let fixture = native_fixture();
    std::fs::write(workspace_path(&fixture, "none.txt"), "nothing").expect("fixture");
    let result = run_tool(&fixture, "grep", serde_json::json!({"pattern": "missing"})).await;
    assert_eq!(text_content(&result), "No matches found");
}

// ---------------------------------------------------------------------------
// Glob
// ---------------------------------------------------------------------------

#[tokio::test]
async fn glob_is_cwd_oriented_sorted_posix_text_and_supports_absolute_roots() {
    let fixture = native_fixture();
    std::fs::create_dir_all(workspace_path(&fixture, "z/sub")).expect("directories");
    std::fs::write(workspace_path(&fixture, "z/sub/b.txt"), "b").expect("fixture");
    std::fs::write(workspace_path(&fixture, "a.txt"), "a").expect("fixture");
    let result = run_tool(&fixture, "glob", serde_json::json!({"pattern": "**/*.txt"})).await;
    assert_eq!(text_content(&result), "a.txt\nz/sub/b.txt");

    let outside = fixture.dir().path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside");
    std::fs::write(outside.join("outside.txt"), "outside").expect("outside fixture");
    let result = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"pattern": "*.txt", "path": outside.to_str().unwrap()}),
    )
    .await;
    assert_eq!(text_content(&result), "outside.txt");
}

#[tokio::test]
async fn glob_default_and_large_limits_have_continuation_guidance() {
    let fixture = native_fixture();
    for index in 0..1001 {
        std::fs::write(
            workspace_path(&fixture, &format!("file-{index:04}.txt")),
            "x",
        )
        .expect("fixture");
    }
    let limited = run_tool(&fixture, "glob", serde_json::json!({"pattern": "*.txt"})).await;
    let output = text_content(&limited);
    assert!(
        output.contains("1000 results limit reached. Use limit=2000 for more, or refine pattern")
    );
    assert!(!output.contains("file-1000.txt\n"));

    let huge = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"pattern": "*.txt", "limit": u64::MAX}),
    )
    .await;
    assert!(matches!(huge.status, ToolExecutionStatus::Success));
}

#[tokio::test]
async fn glob_byte_limit_and_no_match_contract_are_plain_text() {
    let fixture = native_fixture();
    for index in 0..200 {
        let directory = workspace_path(&fixture, &format!("dir-{index:03}"));
        std::fs::create_dir_all(&directory).expect("directory");
        let name = format!("{}.txt", "x".repeat(245));
        std::fs::write(directory.join(name), "x").expect("fixture");
    }
    let result = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"pattern": "**/*.txt", "limit": u64::MAX}),
    )
    .await;
    assert!(text_content(&result).contains("50KB limit reached"));

    let none = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"pattern": "does-not-exist"}),
    )
    .await;
    assert_eq!(text_content(&none), "No files found matching pattern");
}

#[cfg(unix)]
#[tokio::test]
async fn glob_does_not_follow_symlinks_and_keeps_hidden_files_visible() {
    use std::os::unix::fs::symlink;

    let fixture = native_fixture();
    let hidden = workspace_path(&fixture, ".hidden.txt");
    std::fs::write(&hidden, "hidden").expect("hidden");
    std::fs::write(workspace_path(&fixture, ".gitignore"), "ignored.txt\n").expect("ignore file");
    std::fs::write(workspace_path(&fixture, "ignored.txt"), "ignored").expect("ignored file");
    let outside = fixture.dir().path().join("outside.txt");
    std::fs::write(&outside, "outside").expect("outside");
    symlink(&outside, workspace_path(&fixture, "link.txt")).expect("symlink");
    let result = run_tool(&fixture, "glob", serde_json::json!({"pattern": "**/*"})).await;
    let output = text_content(&result);
    assert!(output.contains(".hidden.txt"));
    assert!(output.contains("ignored.txt"));
    assert!(!output.contains("link.txt"));
}

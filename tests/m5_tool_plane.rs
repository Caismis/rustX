//! M5 native filesystem tool tests.
//!
//! Every test runs the native tool plane against an isolated temporary
//! workspace — never the developer machine's repository. Read/Write/Edit
//! exercise the workspace boundary (absolute paths, `..` escapes, symlink
//! escapes), deterministic line slicing, atomic writeback, and bounded
//! output; Glob/Grep prove deterministic ordering, truncation at the
//! configured limits, and the directory-symlink traversal policy.

#![allow(clippy::similar_names)] // scripted fixture names are intentionally similar

mod common;

use common::{native_fixture, run_tool};
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

#[tokio::test]
async fn read_slices_lines_deterministically() {
    let fixture = native_fixture();
    let file = fixture.runtime.workspace().root().join("sample.txt");
    std::fs::write(&file, "one\ntwo\nthree\nfour\nfive\n").expect("write sample");
    let full = run_tool(
        &fixture,
        "read",
        serde_json::json!({"__rustx_execution": "foreground", "path": "sample.txt"}),
    )
    .await;
    assert_eq!(text_content(&full), "one\ntwo\nthree\nfour\nfive");
    let middle = run_tool(
        &fixture,
        "read",
        serde_json::json!({"__rustx_execution": "foreground", "path": "sample.txt", "start_line": 2, "line_count": 2}),
    )
    .await;
    assert_eq!(text_content(&middle), "two\nthree");
    let past_end = run_tool(
        &fixture,
        "read",
        serde_json::json!({"__rustx_execution": "foreground", "path": "sample.txt", "start_line": 99}),
    )
    .await;
    assert_eq!(text_content(&past_end), "");
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
        serde_json::json!({"__rustx_execution": "foreground", "path": "big.txt", "line_count": 1_000_000}),
    )
    .await;
    assert!(
        text_content(&result).len() <= rustx::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES,
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
            serde_json::json!({"__rustx_execution": "foreground", "path": "binary.bin"}),
        )
        .await,
    );
}

#[tokio::test]
async fn read_rejects_absolute_and_escaping_paths() {
    let fixture = native_fixture();
    assert_failed(
        &run_tool(
            &fixture,
            "read",
            serde_json::json!({"__rustx_execution": "foreground", "path": "/etc/hostname"}),
        )
        .await,
    );
    assert_failed(
        &run_tool(
            &fixture,
            "read",
            serde_json::json!({"__rustx_execution": "foreground", "path": "../escape.txt"}),
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
            serde_json::json!({"__rustx_execution": "foreground", "path": "linked.txt"}),
        )
        .await,
    );
}

#[tokio::test]
async fn write_creates_and_replaces_atomically() {
    let fixture = native_fixture();
    std::fs::create_dir_all(fixture.runtime.workspace().root().join("dir")).expect("dir");
    let created = run_tool(
        &fixture,
        "write",
        serde_json::json!({"__rustx_execution": "foreground", "path": "dir/file.txt", "content": "hello"}),
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
        serde_json::json!({"__rustx_execution": "foreground", "path": "dir/file.txt", "content": "world"}),
    )
    .await;
    assert_eq!(replaced.status, ToolExecutionStatus::Success);
    assert_eq!(
        std::fs::read_to_string(fixture.runtime.workspace().root().join("dir/file.txt"))
            .expect("read back"),
        "world"
    );
    // Atomic writeback leaves no temporary files behind.
    let leftovers: Vec<_> = std::fs::read_dir(fixture.runtime.workspace().root().join("dir"))
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".rustx-"))
        .collect();
    assert!(leftovers.is_empty(), "no temp files remain: {leftovers:?}");
}

#[tokio::test]
async fn write_requires_an_existing_parent_directory() {
    let fixture = native_fixture();
    let result = run_tool(
        &fixture,
        "write",
        serde_json::json!({"__rustx_execution": "foreground", "path": "missing/deep/file.txt", "content": "x"}),
    )
    .await;
    assert_failed(&result);
    assert!(
        !fixture.runtime.workspace().root().join("missing").exists(),
        "no implicit recursive directory creation"
    );
}

#[tokio::test]
async fn edit_requires_exactly_one_match() {
    let fixture = native_fixture();
    std::fs::write(
        fixture.runtime.workspace().root().join("edit.txt"),
        "alpha beta alpha",
    )
    .expect("write");
    let single = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"__rustx_execution": "foreground", "path": "edit.txt", "old_text": "beta", "new_text": "GAMMA"}),
    )
    .await;
    assert_eq!(single.status, ToolExecutionStatus::Success);
    assert_eq!(json_content(&single)["replacements"], 1);
    assert_eq!(
        std::fs::read_to_string(fixture.runtime.workspace().root().join("edit.txt"))
            .expect("read back"),
        "alpha GAMMA alpha"
    );
    let duplicate = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"__rustx_execution": "foreground", "path": "edit.txt", "old_text": "alpha", "new_text": "x"}),
    )
    .await;
    assert_failed(&duplicate);
    let zero = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"__rustx_execution": "foreground", "path": "edit.txt", "old_text": "absent", "new_text": "x"}),
    )
    .await;
    assert_failed(&zero);
}

#[tokio::test]
async fn edit_replace_all_replaces_every_match_but_fails_on_zero() {
    let fixture = native_fixture();
    std::fs::write(fixture.runtime.workspace().root().join("edit.txt"), "a a a").expect("write");
    let replaced = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"__rustx_execution": "foreground", "path": "edit.txt", "old_text": "a", "new_text": "z", "replace_all": true}),
    )
    .await;
    assert_eq!(replaced.status, ToolExecutionStatus::Success);
    assert_eq!(json_content(&replaced)["replacements"], 3);
    assert_eq!(
        std::fs::read_to_string(fixture.runtime.workspace().root().join("edit.txt"))
            .expect("read back"),
        "z z z"
    );
    let zero = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"__rustx_execution": "foreground", "path": "edit.txt", "old_text": "missing", "new_text": "z", "replace_all": true}),
    )
    .await;
    assert_failed(&zero);
}

#[tokio::test]
async fn glob_is_lexically_ordered_and_workspace_relative() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    // Creation order is deliberately reversed relative to lexical order.
    std::fs::create_dir_all(root.join("sub")).expect("subdir");
    std::fs::write(root.join("zebra.rs"), "z").expect("write");
    std::fs::write(root.join("alpha.rs"), "a").expect("write");
    std::fs::write(root.join("sub/middle.rs"), "m").expect("write");
    let result = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"__rustx_execution": "foreground", "pattern": "**/*.rs"}),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    assert_eq!(
        json_content(&result)["results"],
        serde_json::json!(["alpha.rs", "sub/middle.rs", "zebra.rs"]),
        "results are lexically sorted workspace-relative paths"
    );
}

#[tokio::test]
async fn glob_truncates_at_the_result_limit() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    std::fs::create_dir_all(root.join("many")).expect("dir");
    for index in 0..(rustx::tools::limits::MAX_GLOB_RESULTS + 50) {
        std::fs::write(root.join("many").join(format!("f{index:05}.txt")), "x").expect("write");
    }
    let result = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"__rustx_execution": "foreground", "pattern": "many/*.txt"}),
    )
    .await;
    let results = json_content(&result)["results"]
        .as_array()
        .expect("results array")
        .len();
    assert_eq!(results, rustx::tools::limits::MAX_GLOB_RESULTS);
    assert!(result.truncation.expect("truncation state").truncated);
}

#[tokio::test]
async fn grep_orders_matches_deterministically() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    std::fs::create_dir_all(root.join("b")).expect("dir b");
    std::fs::create_dir_all(root.join("a")).expect("dir a");
    std::fs::write(root.join("b/late.rs"), "fn x() {}\nfn y() {}\n").expect("write");
    std::fs::write(root.join("a/early.rs"), "fn y() {}\nfn x() {}\n").expect("write");
    let result = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"__rustx_execution": "foreground", "pattern": "fn [xy]", "glob": "**/*.rs"}),
    )
    .await;
    assert_eq!(result.status, ToolExecutionStatus::Success);
    let content = json_content(&result);
    let matches = content["matches"].as_array().expect("matches");
    let ordered: Vec<(String, u64, u64)> = matches
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().expect("path").to_owned(),
                entry["line_number"].as_u64().expect("line"),
                entry["column"].as_u64().expect("column"),
            )
        })
        .collect();
    assert_eq!(
        ordered,
        vec![
            ("a/early.rs".to_owned(), 1, 1),
            ("a/early.rs".to_owned(), 2, 1),
            ("b/late.rs".to_owned(), 1, 1),
            ("b/late.rs".to_owned(), 2, 1),
        ],
        "matches are ordered by relative path, then line, then column"
    );
}

#[tokio::test]
async fn grep_rejects_invalid_regex() {
    let fixture = native_fixture();
    std::fs::write(fixture.runtime.workspace().root().join("f.txt"), "text").expect("write");
    let result = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"__rustx_execution": "foreground", "pattern": "([unclosed"}),
    )
    .await;
    assert_failed(&result);
}

#[tokio::test]
async fn grep_truncates_at_the_match_limit() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    let content = "match\n".repeat(rustx::tools::limits::MAX_GREP_MATCHES + 100);
    std::fs::write(root.join("huge.txt"), content).expect("write");
    let result = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"__rustx_execution": "foreground", "pattern": "match"}),
    )
    .await;
    let matches = json_content(&result)["matches"]
        .as_array()
        .expect("matches")
        .len();
    assert_eq!(matches, rustx::tools::limits::MAX_GREP_MATCHES);
    assert!(result.truncation.expect("truncation").truncated);
}

#[cfg(unix)]
#[tokio::test]
async fn traversal_tools_do_not_follow_directory_symlinks() {
    let fixture = native_fixture();
    let root = fixture.runtime.workspace().root().to_path_buf();
    let outside = fixture.dir().path().join("outside-dir");
    std::fs::create_dir_all(&outside).expect("outside dir");
    std::fs::write(outside.join("secret.rs"), "fn secret() {}").expect("write outside");
    std::os::unix::fs::symlink(&outside, root.join("linked")).expect("symlink");
    let glob = run_tool(
        &fixture,
        "glob",
        serde_json::json!({"__rustx_execution": "foreground", "pattern": "**/*.rs"}),
    )
    .await;
    assert_eq!(
        json_content(&glob)["results"],
        serde_json::json!([]),
        "directory symlinks are never traversed"
    );
    let grep = run_tool(
        &fixture,
        "grep",
        serde_json::json!({"__rustx_execution": "foreground", "pattern": "secret"}),
    )
    .await;
    assert_eq!(
        json_content(&grep)["matches"]
            .as_array()
            .expect("matches")
            .len(),
        0,
        "grep never descends into directory symlinks"
    );
}

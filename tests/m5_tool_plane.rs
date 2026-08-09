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
    let full = run_tool(&fixture, "read", serde_json::json!({"path": "sample.txt"})).await;
    assert_eq!(text_content(&full), "one\ntwo\nthree\nfour\nfive");
    let middle = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": "sample.txt", "start_line": 2, "line_count": 2}),
    )
    .await;
    assert_eq!(text_content(&middle), "two\nthree");
    let past_end = run_tool(
        &fixture,
        "read",
        serde_json::json!({"path": "sample.txt", "start_line": 99}),
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
        serde_json::json!({"path": "big.txt", "line_count": 1_000_000}),
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
    assert_failed(&run_tool(&fixture, "read", serde_json::json!({"path": "binary.bin"})).await);
}

#[tokio::test]
async fn read_rejects_absolute_and_escaping_paths() {
    let fixture = native_fixture();
    assert_failed(
        &run_tool(
            &fixture,
            "read",
            serde_json::json!({"path": "/etc/hostname"}),
        )
        .await,
    );
    assert_failed(
        &run_tool(
            &fixture,
            "read",
            serde_json::json!({"path": "../escape.txt"}),
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
    assert_failed(&run_tool(&fixture, "read", serde_json::json!({"path": "linked.txt"})).await);
}

#[tokio::test]
async fn write_creates_and_replaces_atomically() {
    let fixture = native_fixture();
    std::fs::create_dir_all(fixture.runtime.workspace().root().join("dir")).expect("dir");
    let created = run_tool(
        &fixture,
        "write",
        serde_json::json!({"path": "dir/file.txt", "content": "hello"}),
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
        serde_json::json!({"path": "dir/file.txt", "content": "world"}),
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
        serde_json::json!({"path": "missing/deep/file.txt", "content": "x"}),
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
        serde_json::json!({"path": "edit.txt", "old_text": "beta", "new_text": "GAMMA"}),
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
        serde_json::json!({"path": "edit.txt", "old_text": "alpha", "new_text": "x"}),
    )
    .await;
    assert_failed(&duplicate);
    let zero = run_tool(
        &fixture,
        "edit",
        serde_json::json!({"path": "edit.txt", "old_text": "absent", "new_text": "x"}),
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
        serde_json::json!({"path": "edit.txt", "old_text": "a", "new_text": "z", "replace_all": true}),
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
        serde_json::json!({"path": "edit.txt", "old_text": "missing", "new_text": "z", "replace_all": true}),
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
    let result = run_tool(&fixture, "glob", serde_json::json!({"pattern": "**/*.rs"})).await;
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
        serde_json::json!({"pattern": "many/*.txt"}),
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
        serde_json::json!({"pattern": "fn [xy]", "glob": "**/*.rs"}),
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
        serde_json::json!({"pattern": "([unclosed"}),
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
    let result = run_tool(&fixture, "grep", serde_json::json!({"pattern": "match"})).await;
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
    let glob = run_tool(&fixture, "glob", serde_json::json!({"pattern": "**/*.rs"})).await;
    assert_eq!(
        json_content(&glob)["results"],
        serde_json::json!([]),
        "directory symlinks are never traversed"
    );
    let grep = run_tool(&fixture, "grep", serde_json::json!({"pattern": "secret"})).await;
    assert_eq!(
        json_content(&grep)["matches"]
            .as_array()
            .expect("matches")
            .len(),
        0,
        "grep never descends into directory symlinks"
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
    use rustx::tools::native::{
        NativeToolPolicies, NativeToolPolicy, NativeToolResources, register_native_tools,
    };
    use rustx::tools::runtime::ConversationToolRuntime;
    use rustx::tools::types::{
        ToolCall, ToolConcurrencyPolicy, ToolExecutionPolicy, ToolInvocationMode,
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
            },
            NativeToolPolicies::uniform(NativeToolPolicy {
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
                    .preflight(&read_call(serde_json::json!({"path": "a.txt"})))
                    .expect("preflight");
                let PreflightOutcome::Ready(prepared) = outcome else {
                    panic!("foreground-only read must preflight as ready");
                };
                prepared.invocation.mode
            }
            ToolExecutionPolicy::BackgroundOnly => {
                let outcome = registry
                    .preflight(&read_call(serde_json::json!({"path": "a.txt"})))
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
                        "path": "a.txt"
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
    use rustx::tools::native::{
        NativeToolPolicies, NativeToolPolicy, NativeToolResources, register_native_tools,
    };
    use rustx::tools::runtime::ConversationToolRuntime;
    use rustx::tools::types::{
        ToolCall, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolInvocationMode,
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
        },
        NativeToolPolicies {
            read: NativeToolPolicy {
                execution: ToolExecutionPolicy::ForegroundOnly,
                concurrency: ToolConcurrencyPolicy::Sequential,
            },
            write: NativeToolPolicy {
                execution: ToolExecutionPolicy::BackgroundOnly,
                concurrency: ToolConcurrencyPolicy::Sequential,
            },
            edit: NativeToolPolicy {
                execution: ToolExecutionPolicy::ForegroundOnly,
                concurrency: ToolConcurrencyPolicy::Sequential,
            },
            glob: NativeToolPolicy {
                execution: ToolExecutionPolicy::ForegroundOnly,
                concurrency: ToolConcurrencyPolicy::Parallel,
            },
            grep: NativeToolPolicy {
                execution: ToolExecutionPolicy::ModelSelectable,
                concurrency: ToolConcurrencyPolicy::Parallel,
            },
            bash: NativeToolPolicy {
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
            serde_json::json!({"path": "a.txt"}),
        )),
        ToolInvocationMode::Foreground
    );
    // Write is background-only: resolves background with no execution field.
    assert_eq!(
        preflight_mode(call(
            "call-write",
            "write",
            "tool-write",
            serde_json::json!({"path": "a.txt", "content": "x"}),
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
    use rustx::tools::native::{
        NativeToolPolicies, NativeToolPolicy, NativeToolResources, register_native_tools,
    };
    use rustx::tools::runtime::ConversationToolRuntime;
    use rustx::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy};

    let defaults = NativeToolPolicies::default();
    assert_eq!(
        defaults,
        NativeToolPolicies::uniform(NativeToolPolicy::default())
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

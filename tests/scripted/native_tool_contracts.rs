//! Canonical schema and registry-boundary regressions for Issue #91.
//!
//! These tests deliberately exercise the model-facing schema and the actual
//! preflight path. Edit's tolerated malformed model spellings are normalized
//! by its registration before canonical schema validation; no provider or
//! Agent Loop branch knows about those spellings.

use super::common;
use rustx::runtime::identity::ToolCallId;
use rustx::tools::executor::PreflightOutcome;
use rustx::tools::types::ToolCall;

const NATIVE_TOOL_NAMES: [&str; 7] = [
    "read",
    "write",
    "edit",
    "glob",
    "grep",
    "bash",
    "background_task",
];

fn definition(fixture: &common::NativeFixture, name: &str) -> rustx::tools::types::ToolDefinition {
    fixture
        .registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == name)
        .unwrap_or_else(|| panic!("{name} is registered"))
}

fn required(schema: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = schema["required"]
        .as_array()
        .expect("required")
        .iter()
        .map(|value| value.as_str().expect("required string").to_owned())
        .collect();
    names.sort();
    names
}

fn properties(schema: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

fn preflight(
    fixture: &common::NativeFixture,
    name: &str,
    arguments: serde_json::Value,
) -> PreflightOutcome {
    let definition = definition(fixture, name);
    fixture
        .registry
        .preflight(&ToolCall {
            id: ToolCallId::new("call-preflight"),
            tool_id: definition.id,
            name: name.to_owned(),
            arguments,
        })
        .expect("identity resolves")
}

#[test]
fn all_native_schemas_are_canonical_and_have_no_file_path_contract() {
    let fixture = common::native_fixture();
    for name in NATIVE_TOOL_NAMES {
        let schema = definition(&fixture, name).input_schema;
        rustx::tools::schema::validate_canonical_schema(&schema)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert!(!schema.to_string().contains("$ref"));
        assert!(!schema.to_string().contains("file_path"));
    }
}

#[test]
fn read_write_edit_schemas_are_path_oriented_and_read_accepts_zero_offset() {
    let fixture = common::native_fixture();
    let read = definition(&fixture, "read").input_schema;
    assert_eq!(required(&read), ["path"]);
    assert_eq!(properties(&read), ["limit", "offset", "path"]);
    assert_eq!(read["properties"]["offset"]["minimum"], 0);
    assert_eq!(read["properties"]["limit"]["minimum"], 1);
    assert!(read["properties"]["limit"]["maximum"].is_null());

    for name in ["write", "edit"] {
        let schema = definition(&fixture, name).input_schema;
        assert!(required(&schema).contains(&"path".to_owned()));
        assert!(properties(&schema).contains(&"path".to_owned()));
        assert!(!properties(&schema).contains(&"file_path".to_owned()));
    }
    let edit = definition(&fixture, "edit").input_schema;
    assert_eq!(edit["properties"]["edits"]["minItems"], 1);
    assert_eq!(
        edit["properties"]["edits"]["items"]["properties"]["oldText"]["minLength"],
        1
    );
}

#[test]
fn grep_and_glob_expose_unbounded_model_configurable_limits() {
    let fixture = common::native_fixture();
    let grep = definition(&fixture, "grep").input_schema;
    assert_eq!(grep["properties"]["limit"]["minimum"], 1);
    assert!(grep["properties"]["limit"]["maximum"].is_null());
    let glob = definition(&fixture, "glob").input_schema;
    assert_eq!(properties(&glob), ["limit", "path", "pattern"]);
    assert_eq!(glob["properties"]["limit"]["minimum"], 1);
    assert!(glob["properties"]["limit"]["maximum"].is_null());
}

#[test]
fn old_file_path_and_invalid_business_arguments_are_rejected() {
    let fixture = common::native_fixture();
    for name in ["read", "write", "edit"] {
        let result = preflight(
            &fixture,
            name,
            serde_json::json!({"file_path": "/tmp/file.txt", "content": "x", "edits": []}),
        );
        assert!(
            matches!(result, PreflightOutcome::Rejected { .. }),
            "{name}"
        );
    }
    for (name, arguments) in [
        ("read", serde_json::json!({"path": "/tmp/file", "limit": 0})),
        ("write", serde_json::json!({"path": "/tmp/file"})),
        ("edit", serde_json::json!({"path": "/tmp/file", "edits": 7})),
        ("grep", serde_json::json!({"pattern": "x", "limit": 0})),
        ("glob", serde_json::json!({"pattern": "*", "limit": 0})),
    ] {
        assert!(matches!(
            preflight(&fixture, name, arguments),
            PreflightOutcome::Rejected { .. }
        ));
    }
    assert!(matches!(
        preflight(
            &fixture,
            "read",
            serde_json::json!({"path": "relative.txt", "offset": 0})
        ),
        PreflightOutcome::Ready(_)
    ));
}

#[test]
fn edit_model_variants_normalize_to_the_same_canonical_invocation() {
    let fixture = common::native_fixture();
    let canonical_edits = serde_json::json!([{"oldText": "a", "newText": "b"}]);
    let variants = [
        serde_json::json!({"path": "file.txt", "edits": canonical_edits}),
        serde_json::json!({
            "path": "file.txt",
            "edits": serde_json::to_string(&canonical_edits).expect("encoded edits")
        }),
        serde_json::json!({"path": "file.txt", "edits": {"oldText": "a", "newText": "b"}}),
        serde_json::json!({"path": "file.txt", "oldText": "a", "newText": "b"}),
    ];
    let mut canonical = None;
    for variant in variants {
        let PreflightOutcome::Ready(prepared) = preflight(&fixture, "edit", variant) else {
            panic!("supported Edit variant was rejected");
        };
        if let Some(expected) = &canonical {
            assert_eq!(&prepared.invocation.arguments, expected);
        } else {
            canonical = Some(prepared.invocation.arguments);
        }
    }
    assert_eq!(
        canonical.expect("canonical invocation"),
        serde_json::json!({"path": "file.txt", "edits": [{"oldText": "a", "newText": "b"}]})
    );
}

#[test]
fn edit_normalization_cannot_consume_reserved_or_unrelated_fields() {
    let fixture = common::native_fixture();
    let reserved = preflight(
        &fixture,
        "edit",
        serde_json::json!({
            "path": "file.txt",
            "oldText": "a",
            "newText": "b",
            "__rustx_forged": "value"
        }),
    );
    assert!(matches!(reserved, PreflightOutcome::Rejected { .. }));
    let unrelated = preflight(
        &fixture,
        "edit",
        serde_json::json!({"path": "file.txt", "edits": 42}),
    );
    assert!(matches!(unrelated, PreflightOutcome::Rejected { .. }));
}

use super::*;
use crate::runtime::workflow::{WorkflowId, WorkflowProgram};
use serde_json::json;

fn file() -> WorkflowFileDocument {
    serde_yaml::from_str(include_str!(
        "../../examples/local-runtime/.agents/workflows/parallel_review.yaml"
    ))
    .unwrap()
}
fn annotated() -> WorkflowFileDocument {
    let mut document = file();
    document.editor=Some(serde_json::from_value(json!({
        "nodes":[{"address":{"block":[],"node":"reviews"},"title":"Review lanes","description":"Author-only guidance","position":{"x":0.5,"y":-40.0},"dimensions":{"width":900.0,"height":600.0}},
            {"address":{"block":[{"node":"reviews","branch":"quality"}],"node":"done"},"position":{"x":200.0,"y":300.0}}],
        "entries":[{"block":[],"position":{"x":-200.0,"y":0.0}}],
        "branches":[{"address":{"block":[],"node":"reviews"},"branch":"quality","position":{"x":0.0,"y":0.0},"dimensions":{"width":450.0,"height":600.0}}],
        "notes":[{"id":"note_1","block":[{"node":"reviews","branch":"quality"}],"text":"# Review\n\n**Markdown** and 中文 🦀\n","theme":"blue","position":{"x":-100.0,"y":200.0},"dimensions":{"width":240.0,"height":180.0}}],
        "orders":[{"target":{"kind":"branches","address":{"block":[],"node":"reviews"}},"fields":["security","quality"]}]
    })).unwrap());
    document
}
#[test]
fn complete_file_round_trip_preserves_nested_metadata_markdown_order_and_numbers() {
    let document = annotated();
    let bytes = document.canonical_bytes().unwrap();
    assert_eq!(document, serde_yaml::from_slice(&bytes).unwrap());
    assert_eq!(
        bytes,
        serde_yaml::from_slice::<WorkflowFileDocument>(&bytes)
            .unwrap()
            .canonical_bytes()
            .unwrap()
    );
    let mut value = serde_json::to_value(&document).unwrap();
    value["block"]["nodes"]["done"] = json!({"type":"return","output":{"type":"literal","value":{"large":9_007_199_254_740_993_u64,"fraction":0.25}}});
    let numbers: WorkflowFileDocument = serde_json::from_value(value).unwrap();
    assert_eq!(
        numbers,
        serde_yaml::from_slice(&numbers.canonical_bytes().unwrap()).unwrap()
    );
}

#[test]
fn presentation_changes_source_bytes_but_not_program_digest_graph_or_model_contracts() {
    let original = file();
    let presented = annotated();
    let id = WorkflowId::parse("review").unwrap();
    let frozen = WorkflowProgram::compile(id.clone(), original.definition.clone()).unwrap();
    let program = WorkflowProgram::compile(id, presented.definition.clone()).unwrap();
    assert_ne!(
        original.canonical_bytes().unwrap(),
        presented.canonical_bytes().unwrap()
    );
    assert_eq!(frozen.tool_identity(), program.tool_identity());
    assert_eq!(frozen.input_schema(), program.input_schema());
    assert_eq!(
        crate::tools::native::workflow_definition(&frozen),
        crate::tools::native::workflow_definition(&program)
    );
    assert_eq!(frozen.node_outputs(), program.node_outputs());
    assert_eq!(frozen.inspect(), program.inspect());
    assert!(
        !serde_json::to_string(&program.inspect())
            .unwrap()
            .contains("Author-only")
    );
    let mut changed = presented;
    changed.editor.as_mut().unwrap().notes[0].text = "Later note".into();
    changed.definition.block.input["description"] = json!("Model-visible input contract");
    let updated =
        WorkflowProgram::compile(WorkflowId::parse("review").unwrap(), changed.definition).unwrap();
    assert_ne!(frozen.tool_identity(), updated.tool_identity());
    assert_eq!(
        updated.input_schema()["description"],
        "Model-visible input contract"
    );
    assert_eq!(
        frozen.input_schema().get("description"),
        None,
        "admitted program stays frozen"
    );
}

#[test]
fn missing_metadata_is_optional_and_unknown_or_duplicate_fields_are_rejected() {
    let document = file();
    assert!(document.editor.is_none());
    let mut value = serde_json::to_value(&document).unwrap();
    value["unknown_execution_field"] = json!(true);
    assert!(serde_json::from_value::<WorkflowFileDocument>(value).is_err());
    let mut text = String::from_utf8(document.canonical_bytes().unwrap()).unwrap();
    text.push_str("editor: {}\neditor: {}\n");
    assert!(serde_yaml::from_str::<WorkflowFileDocument>(&text).is_err());
    assert_eq!(document.timeout_ms, 600_000);
}

#[test]
fn metadata_bounds_and_stale_references_have_distinct_non_destructive_outcomes() {
    let mut document = annotated();
    document.editor.as_mut().unwrap().nodes[0].address.node = "deleted".into();
    let before = document.clone();
    assert!(document.validate().is_ok());
    assert!(
        document
            .presentation_diagnostics()
            .iter()
            .any(|d| d.category == "presentation_reference")
    );
    assert_eq!(document, before);
    assert!(document.canonical_bytes().is_ok());
    for bad in [f64::NAN, f64::INFINITY, 1_000_001.0] {
        let mut invalid = before.clone();
        invalid.editor.as_mut().unwrap().notes[0].position.x = bad;
        assert_eq!(invalid.validate().unwrap_err().category, "presentation");
    }
    let mut invalid = before.clone();
    invalid.editor.as_mut().unwrap().notes[0].text = "界".repeat(6000);
    assert!(invalid.validate().is_err(), "UTF-8 byte bound");
    let mut invalid = before.clone();
    let note = invalid.editor.as_ref().unwrap().notes[0].clone();
    invalid.editor.as_mut().unwrap().notes.push(note);
    assert!(
        invalid.validate().is_err(),
        "stable note identities are unique"
    );
    let mut invalid = before.clone();
    invalid.editor.as_mut().unwrap().notes[0].dimensions.width = 0.0;
    assert!(invalid.validate().is_err());
}

#[test]
fn annotations_are_bounded_and_do_not_change_structural_compatibility() {
    use crate::runtime::workflow::authoring::analyze;
    let mut document = file();
    document.block.input["description"] = json!("Input description");
    document.block.input["properties"]["proposal"]["description"] = json!("Proposal to review");
    WorkflowProgram::compile(
        WorkflowId::parse("review").unwrap(),
        document.definition.clone(),
    )
    .unwrap();
    assert!(
        analyze(
            &document.definition,
            None,
            &std::collections::BTreeSet::default()
        )
        .is_ok()
    );
    document.block.input["description"] = json!("界".repeat(1366));
    assert!(
        WorkflowProgram::compile(WorkflowId::parse("review").unwrap(), document.definition)
            .is_err()
    );
}

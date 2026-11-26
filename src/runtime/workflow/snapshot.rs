//! Workflow run snapshot vocabulary lives in `rustx_durable::workflow::snapshot`.

pub use rustx_durable::workflow::snapshot::*;

#[cfg(test)]
mod tests {
    #[test]
    fn inspection_bounds_fixture_is_admissible() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../../web-console/test/fixtures/workflow-run-bounds.json"
        ))
        .unwrap();
        for key in ["maximum", "long_history"] {
            let document: rustx_workflow::workflow_file::WorkflowFileDocument =
                serde_json::from_value(fixtures[key].clone()).unwrap();
            let program = crate::runtime::workflow::WorkflowProgram::compile_document(
                rustx_types::runtime::workflow::WorkflowId::parse(key).unwrap(),
                "sha256:bounds-fixture",
                document,
            )
            .unwrap_or_else(|error| panic!("{key}: {error}"));
            assert!(
                program.inspect().conservative_steps
                    <= rustx_types::runtime::workflow::MAX_WORKFLOW_STEPS
            );
        }
    }
}

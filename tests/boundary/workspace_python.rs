//! Workspace interpreter-discovery contracts against the installed uv executable.

#[tokio::test]
async fn ancestor_python_version_does_not_bind_workspace_discovery() {
    crate::tools::workspace_python::tests::ancestor_python_version_does_not_bind_workspace_discovery().await;
}

#[tokio::test]
async fn relative_interpreter_pin_is_bound_to_the_workspace() {
    crate::tools::workspace_python::tests::relative_interpreter_pin_is_bound_to_the_workspace()
        .await;
}

#[tokio::test]
async fn repaired_path_pin_recovers_without_rewriting_root_inputs() {
    crate::tools::workspace_python::tests::repaired_path_pin_recovers_without_rewriting_root_inputs().await;
}

#[tokio::test]
async fn replaced_root_while_create_waits_for_permit_is_not_written() {
    crate::tools::workspace_python::tests::replaced_root_while_create_waits_for_permit_is_not_written().await;
}

#[tokio::test]
async fn shutdown_between_check_and_signal_cannot_start_a_program() {
    crate::tools::workspace_python::tests::shutdown_between_check_and_signal_cannot_start_a_program().await;
}

#[tokio::test]
async fn real_uv_missing_interpreter_obeys_download_policy() {
    crate::tools::workspace_python::tests::real_uv_missing_interpreter_obeys_download_policy()
        .await;
}

#[tokio::test]
async fn real_uv_does_not_seed_or_select_an_external_project() {
    crate::tools::workspace_python::tests::real_uv_does_not_seed_or_select_an_external_project()
        .await;
}

#[tokio::test]
async fn real_dual_venv_launcher_variables_cannot_change_prefix() {
    crate::tools::workspace_python::tests::real_dual_venv_launcher_variables_cannot_change_prefix()
        .await;
}

#[tokio::test]
async fn targets_added_while_creation_waits_for_capacity_are_never_initialized() {
    crate::tools::workspace_python::tests::targets_added_while_creation_waits_for_capacity_are_never_initialized().await;
}

#[tokio::test]
async fn real_multi_interpreter_selection_satisfies_both_root_inputs() {
    crate::tools::workspace_python::tests::real_multi_interpreter_selection_satisfies_both_root_inputs().await;
}

#[tokio::test]
async fn launcher_selection_change_between_probes_cannot_publish_ready() {
    crate::tools::workspace_python::tests::launcher_selection_change_between_probes_cannot_publish_ready().await;
}

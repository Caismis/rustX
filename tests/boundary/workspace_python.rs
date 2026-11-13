//! Workspace interpreter-discovery contracts against the installed uv executable.

#[test]
fn ancestor_python_version_does_not_bind_workspace_discovery() {
    crate::tools::workspace_python::tests::ancestor_python_version_does_not_bind_workspace_discovery();
}

#[test]
fn relative_interpreter_pin_is_bound_to_the_workspace() {
    crate::tools::workspace_python::tests::relative_interpreter_pin_is_bound_to_the_workspace();
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

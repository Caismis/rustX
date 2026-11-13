//! Workspace interpreter-discovery contracts against the installed uv executable.

#[test]
fn ancestor_python_version_does_not_bind_workspace_discovery() {
    crate::tools::workspace_python::tests::ancestor_python_version_does_not_bind_workspace_discovery();
}

#[test]
fn relative_interpreter_pin_is_bound_to_the_workspace() {
    crate::tools::workspace_python::tests::relative_interpreter_pin_is_bound_to_the_workspace();
}

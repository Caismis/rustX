//! Real child-process inheritance of an admitted parent's frozen invocation.
//! Native Session selection and provider traffic use the existing loopback fixture.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn children_created_after_selection_inherit_the_parent_and_running_children_allow_selection()
{
    Box::pin(
        crate::local_runtime::session_runtime_manager::tests::protocol::live_model_child_inheritance_boundary_scenario(),
    )
    .await;
}

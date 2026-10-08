//! Real host-only WebSocket admission and descriptor-open revocation.
//! Reuse the existing native Session/provider fixture without duplicating it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_present_reads_exact_native_scope_through_current_authorized_attachment() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::committed_present_read_boundary_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transport_granted_delivery_access_reads_and_locates_only_through_own_attachment() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::delivery_access_boundary_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delivery_cancellation_is_request_scoped_at_every_native_interleaving() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::delivery_cancellation_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delivery_revocation_before_publication_commit_suppresses_sensitive_responses() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::delivery_publication_revocation_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delivery_cancellation_is_admitted_as_the_sixteenth_in_flight_request() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::delivery_cancellation_capacity_scenario()).await;
}

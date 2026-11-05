//! Cached Python catalog verification over a real prepared environment and stdio EOF.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t12_cached_python_catalog_disconnect_retains_worker_and_listing_permit() {
    if super::common::provider_emulator::required_uv("cached Python catalog verification").is_none()
    {
        return;
    }
    Box::pin(crate::local_runtime::session_runtime_manager::tests::transports::cached_python_catalog_disconnect_scenario()).await;
}

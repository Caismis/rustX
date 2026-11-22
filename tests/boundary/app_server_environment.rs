//! Actual standalone process, PTY and filesystem protocol boundaries.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_watch_retirement_is_the_last_visible_event_of_its_incarnation() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::concurrency::watch_terminal_order()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_before_native_start_never_executes_an_independent_command() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::concurrency::revocation_before_start()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn poisoned_trace_variable_is_never_authority() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::concurrency::poisoned_trace_variable_is_never_authority()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detached_operations_release_without_touching_siblings() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::review::detached_operations_release_without_touching_siblings()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_during_data_and_control_backpressure() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::detach_during_data_and_control_backpressure()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_retries_prewire_busy_then_releases() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::review::release_retries_prewire_busy_then_releases()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unwatched_callback_cannot_interfere_with_an_immediate_identity_successor() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::concurrency::watch_immediate_successor()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_signals_preserve_program_handlers_and_settle_default_or_lost_owners() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::review::terminal_signals()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn watch_rechecks_atomic_writes_and_stops_while_the_data_lane_is_held() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::review::watch_recheck()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn continuous_pipe_and_pty_output_remains_cancellable_after_capture_cap() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::review::output_after_cap()).await;
}
#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_catalog_redacts_literal_and_reference_http_and_stdio_credentials() {
    use crate::tools::mcp::fixture;
    if fixture::serve_if_fixture_mode(fixture::FixtureServer::from_env()).await {
        return;
    }
    // Inject host credentials at process creation, without mutating the
    // multithreaded test process's global environment.
    if std::env::var("RUSTX_CATALOG_REGRESSION_KEY")
        .ok()
        .as_deref()
        != Some("catalog-secret-sentinel-533")
        || std::env::var("RUSTX_CATALOG_BEARER_REGRESSION_KEY")
            .ok()
            .as_deref()
            != Some("bEaReR catalog-secret-sentinel-533")
    {
        let child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(fixture::fixture_spawn_args("boundary_suites::app_server_environment::mcp_catalog_redacts_literal_and_reference_http_and_stdio_credentials"))
            .env("RUSTX_CATALOG_REGRESSION_KEY", "catalog-secret-sentinel-533")
            .env("RUSTX_CATALOG_BEARER_REGRESSION_KEY", "bEaReR catalog-secret-sentinel-533")
            .kill_on_drop(true).output().await.unwrap();
        assert!(
            child.status.success(),
            "credential regression child failed: {} {}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        return;
    }
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::review::catalog_credentials()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn buffered_argv_environment_binary_exit_and_no_agent() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::buffered_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_local_stdin_terminate_disconnect_and_process_tree() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::connection_process_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_input_resize_and_output_before_final() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::terminal_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canonical_terminal_eof_flushes_partial_line_and_honors_configured_character() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::terminal_eof_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_endpoints_are_not_inherited_by_own_or_other_commands() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::terminal_descriptor_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn filesystem_atomic_conflicts_authority_copy_watch_and_retirement() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::filesystem_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn watcher_registration_retains_authorized_inode_across_symlink_and_directory_aba() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::watch_registration_scenario()).await;
}
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn macos_descriptor_watch_covers_children_refresh_capacity_and_physical_teardown() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::macos_watch_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn output_caps_timeout_and_slow_client_cleanup_preserve_other_clients() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::output_capacity_and_timeout_scenario()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_drain_cancels_commands_before_waiting_for_request_settlement() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::host_drain_scenario()).await;
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnected_mcp_management_retains_host_admission_until_native_close() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::mcp_admission_scenario()).await;
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_management_capacity_belongs_to_each_native_instance() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::mcp_capacity_scenario()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_terminal_drains_output_while_stdin_is_backpressured() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::duplex_terminal_scenario()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skill_native_discovery_retains_admission_after_disconnect() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::skill_admission_scenario()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn incomplete_stdin_after_child_exit_reports_unknown_partial_effects() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::partial_stdin_scenario()).await;
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_root_watch_filters_native_access_and_releases_physical_capacity() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::idle_root_watch_scenario()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skill_large_metadata_and_body_use_bounded_native_ranges() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::skill_large_resources_scenario()).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skill_catalog_pages_trim_by_encoded_bytes_and_resume_at_exact_offset() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::skill_catalog_page_scenario()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_returns_the_client_cancelled_snapshot_without_retiring_the_binding() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::review::start_returns_the_client_cancelled_snapshot()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_reports_an_admitted_client_with_lost_control_as_unconfirmed() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::lost_control_is_unconfirmed_in_host_drain()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refused_register_does_not_allocate_a_cancellation_tombstone() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::refused_register_does_not_allocate_a_cancellation_tombstone()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_revoke_does_not_pin_residency_or_forget_failure() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::delete_revoke_does_not_pin_residency_or_forget_failure()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_capture_discard_hands_off_to_binding_revocation() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::queued_capture_discard_hands_off_to_binding_revocation()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_preparation_hands_off_to_binding_revocation() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::cancelled_preparation_hands_off_to_binding_revocation()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_workspace_release_hands_off_to_binding_revocation() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::queued_workspace_release_hands_off_to_binding_revocation()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn released_output_preserves_the_unclaimed_input_receipt() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::released_output_preserves_the_unclaimed_input_receipt()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_input_acceptance_reports_unknown_and_retires_the_binding() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::lost_input_acceptance_reports_unknown_and_retires_the_binding()).await;
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_sigkill_keeps_other_sessions_and_history_available() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::client_sigkill_keeps_other_sessions_and_history_available()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_input_cleanup_retires_an_open_binding() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::expired_input_cleanup_settles_the_binding(false)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_input_cleanup_preserves_the_terminal_revoke_receipt() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::expired_input_cleanup_settles_the_binding(true)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_input_receipt_after_poll_expiry_preserves_the_binding() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::failed_input_receipt_after_poll_expiry_preserves_the_binding()).await;
}

#[cfg(feature = "mcp-fixture")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconcile_waiting_for_client_does_not_retain_the_unloaded_writer() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::reconcile_waiting_for_client_does_not_retain_the_unloaded_writer()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_mutation_cancel_hands_off_to_binding_revocation() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::unknown_mutation_cancel_hands_off_to_binding_revocation()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_relay_joins_the_physical_binding_revocation() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::child_relay_joins_the_physical_binding_revocation()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_cleanup_requires_physical_settlement_before_and_after_journal_release() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::child_cleanup_requires_physical_settlement_before_and_after_journal_release()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_delegated_resource_receipts_keep_physical_cleanup_unconfirmed() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::lost_delegated_resource_receipts_keep_physical_cleanup_unconfirmed()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_command_settlement_preserves_the_binding_and_output() {
    Box::pin(crate::local_runtime::session_runtime_manager::tests::protocol::environment::detach::unknown_command_settlement_preserves_the_binding_and_output()).await;
}

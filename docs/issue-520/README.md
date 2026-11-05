# Temporary managed tool output

## Contract

Managed tool output is disposable auxiliary text. Canonical History and the
Event Journal persist committed previews, statuses and absolute locators; they
do not guarantee retention or reconstruction of complete oversized output.

The Tool Plane allocates private `rx-<16 hex>` directories beneath
`std::env::temp_dir()`, with `r-<16 hex>.txt` foreground spills and
`e-<16 hex>.output` background streams. Directory and file creation are exclusive,
collision retries are bounded, and POSIX modes are 0700 and 0600. Internal
Session, Conversation and execution identities remain unchanged.

Each Conversation keeps trusted local namespace bindings in its private resource
allocation. These bindings record the canonical directory, filesystem identity,
and ownership token. They are allocation/deletion authority; model-authored
paths and inherited history are never authority. No global spill registry or
temp-root scan exists. Each reconstructed allocator lazily starts a fresh
namespace on its first output, retaining old bindings for reads and deletion.
Within an allocator, issued names are never reused even after external removal.
Missing directories or lost ownership proof retire the current namespace for
future allocations. Foreground and background output then use a fresh namespace,
without repairing the old marker or recreating an old published locator. A temp
root inside the workspace is rejected explicitly because managed namespaces
must remain disjoint from workspace read and mutation roots.

Foreground allocation remains lazy at the existing preview threshold and streams
the retained prefix followed by subsequent decoded UTF-8 text. Background output
is allocated before dispatch acceptance. Its execution identity is bound to its
allocated file, rather than encoded in the filename. Accepted, live and terminal
references share that file; pre-commit rollback removes only its allocation.
Capture and write failures remain explicit; Partial never becomes Complete.

Published files survive tool/Attempt completion, Session switching, runtime drop
and normal exit. Restart preserves original locators. OS removal is ordinary
unavailability: reads fail, committed history remains intact, and subsequent
output uses new names. No rerun, preview reconstruction, or empty-file repair is
performed.

Session deletion cleans only recorded namespaces under the existing logical
commit, Conversation exclusion and cleanup-pending protocol. Open writers retain
ConversationAccess even after path removal. Missing namespaces are already clean;
other cleanup errors remain pending. A public directory without a valid ownership
marker remains pending even when empty; an inode alone never authorizes it.

Cleanup durably records a reserved retirement slot before moving a namespace
with atomic no-replace rename into a product-owned private arena on the same
filesystem. It verifies the moved object before durably confirming retirement
and deleting any content. Final removal is relative to the private arena's
validated descriptor, never the published path. A replaced public directory is
preserved. Confirmed retirement can finish a markerless slot after process death;
the arena's own ownership marker remains intact throughout Session deletion.
Slots use the permanently reserved Conversation identity plus namespace ordinal.

`<ProductRoot>/output-retirement/` stores one directly addressed arena binding per
canonical temp parent, with a publication lock. It contains no output index or
retrieval mapping, and cleanup performs no temp-root scan. Empty arena scaffolding
and these small product control records are shared infrastructure outside the
Session cascade; they are intentionally retained, without reference counting or
garbage collection. A later externally lost arena is never repaired. Native
filesystem permissions remain the trust boundary: this private control area is
not a sandbox against arbitrary same-user host filesystem mutation.
Descriptor-relative operations reject symlink redirection. The system temporary root is
never an authorized read root or a model-mutation read-only root.

Forks and clones retain literal source references without taking ownership of
source namespaces. Each child Conversation owns its own namespaces. Deleting a
destination cannot remove source output. Archives continue to omit auxiliary
output, whether present or missing. Native filesystem tools and Bash retain their
existing host-path permissions: private namespaces are not a cross-Session
security sandbox.

## Deterministic evidence map

| Contract | Owning regressions |
| --- | --- |
| Short real paths, spaces, canonical temp-root aliases, POSIX modes, streamed UTF-8 | `tools::managed_output::tests::{short_flat_private_files_retain_streamed_utf8_after_drop,a_symlinked_system_temp_root_is_canonicalized_legitimately}` |
| Exclusive bounded directory/file collisions, no overwrite or reuse after deletion | `tools::managed_output::storage::tests::directory_collisions_are_bounded_exclusive_and_never_reuse_recorded_names`; `bounded_file_collisions_and_deleted_names_never_overwrite_or_alias` |
| Lazy threshold, complete prefix, decoder fragments and failure semantics | `tools::output::tests`; Native Bash capture/executor tests; `tests/tools/mcp.rs` foreground collective normalization |
| Pre-acceptance allocation, rollback, live reads, cancellation/timeout and one terminal locator | `tools::background::tests`; `boundary_suites::background::text_spill`; `tests/tools/mcp.rs` background normalization |
| Planted symlinks, FIFO/regular replacements and allocation-handle settlement | `planted_leaf_symlink_is_a_collision_and_never_written`; `fifo_replacements_fail_without_waiting_for_a_peer`; `regular_background_replacement_is_rejected_and_settlement_releases_handle` |
| Runtime drop, real cold process restart, vanished files/directories and unavailable current temp root | `genuine_cold_restart_keeps_the_original_locator`; `cold_reconstruction_retains_valid_references_and_changes_allocation_namespace`; `external_removal_rotates_namespace_without_repairing_old_locators`; `recovery_does_not_require_the_current_temporary_root` |
| Marker loss rotates allocation; temp/workspace overlap has a clear diagnostic | `partial_cleanup_retires_the_current_namespace_without_repairing_it`; Native Bash `a_replaced_namespace_rotates_without_losing_foreground_output`; `a_temporary_root_inside_the_workspace_reports_configuration_conflict` |
| Crash after marker unlink and deterministic public-path substitution | `storage::retirement::tests::{process_death_after_rename_or_marker_unlink_preserves_public_replacement,public_substitution_after_validation_is_preserved_and_original_remains_owned,replacement_at_public_path_before_final_rmdir_is_untouched,a_foreign_replacement_in_a_retired_slot_never_grants_authority}` |
| Retirement infrastructure loss preserves or renews only proven authority | `storage::retirement::tests::{missing_arena_marker_keeps_confirmed_cleanup_pending,losing_an_unconfirmed_arena_retries_only_with_valid_original_authority}` |
| Concurrent allocation and cleanup isolation across owners/products | `concurrent_conversations_and_products_do_not_mix_or_delete_each_other`; `foreign_or_traversing_metadata_never_grants_cleanup_authority`; `markerless_empty_directory_is_not_cleanup_authority` |
| External unlink preserves detached writer exclusion; Session deletion removes only owned namespaces | `local_runtime::session::tests::deletion_tests::lifecycle::temporary_outputs_keep_writer_exclusion_and_cleanup_only_the_owned_session` |
| Committed deletion retains cleanup-pending and retries missing namespaces idempotently | `temporary_cleanup_failure_stays_pending_and_missing_namespace_is_idempotent` |
| Clone keeps literal source locator, grants no source ownership, missing spill preserves canonical history | `cloned_history_keeps_source_locator_without_transferring_namespace_ownership`; `boundary_suites::durable::process_death::conformance::{a_forked_lineage_cuts_the_history_and_inherits_no_durable_ownership,a_branched_lineage_cuts_the_history_and_inherits_no_durable_ownership}`; existing child ownership and archive projection suites |
| Trace preserves typed capture facts and original locator | `runtime_client::trace::tests::managed_storage_failure_diagnostics_preserve_native_execution_facts`; archive managed-output projection test |

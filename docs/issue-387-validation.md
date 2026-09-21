# Issue #387 validation: bounded Conversation identity reservation

This document records the acceptance matrix, the measurement environment, and
the reproducible before/after evidence for [Issue
#387](https://github.com/Caismis/rustX/issues/387). It is evidence, not a
substitute for the code and tests it describes.

## Base and measured trees

- Dependency base: `0083f64df3f16d2270e21eae0b90e13ad668d5b9`
  (merge of PR #389, which implements Issue #386). Verified to contain #386's
  persisted Session display projection by ancestry, not merely by issue state.
- Implementation branch: `issue-387-bounded-conversation-reservation`,
  worktree `../rustX-issue-387`.
- The final PR head SHA is the pushed commit of that branch; see the PR.

## Environment

| Fact | Value |
| --- | --- |
| OS / kernel | Fedora Linux 44, `7.1.13-200.fc44.x86_64` |
| CPU | Intel(R) Core(TM) Ultra X7 358H, 16 logical CPUs |
| Memory | 32,343,792 kB total |
| Filesystem | btrfs (`/dev/nvme0n1p3[/home]`), `compress=zstd:1`, SSD |
| Rust | `rustc 1.95.0`, `cargo 1.95.0` (stable) |
| Benchmark profile | `--release` |
| Test profile | debug (unit/bootstrap) |
| CPU source | `/proc/self/stat` utime+stime over `getconf CLK_TCK` |
| RSS source | `/proc/self/statm` resident pages times `getconf PAGE_SIZE` (point-in-time `VmRSS`) |

## Reproducible commands

```text
# List/search regression (#386) with preview seeding enabled on both sides
cargo run --release --example session_list_benchmark -- \
    --root <fresh-root> --sessions 256 --messages 10 --reps 50

# Create scaling and the 150-create resource workload
cargo run --release --example session_create_benchmark -- \
    --root <fresh-root> --existing <1|10|100|1000> --creates 30
cargo run --release --example session_create_benchmark -- \
    --root <fresh-root> --existing 1 --creates 150

# Instrumented exclusive stage profile (test-only, separate run)
RUSTX_387_PROFILE_EXISTING=100 cargo test --lib --all-features \
    'stage_profile_real_create_pipeline' -- --ignored --nocapture
```

The create benchmark compiles against both revisions. Its one
revision-specific body is `reservation_counters`: the head reads the
storage-owner counters, the base copy returns `(None, None)`. Workload,
durability and publication semantics are identical on both sides.

## Create scaling (30 creates per point, fresh root, release)

Wall and CPU microseconds per Session create, and reservation-specific
counters. Base is `0083f64d`; head is the implementation branch.

| Existing Sessions | Base wall µs | Head wall µs | Base CPU µs | Head CPU µs | Head reservation |
| --- | --- | --- | --- | --- | --- |
| 1 | 3,579 | 3,633 | 3,333 | 3,333 | 30 reserved, 0 layout probes |
| 10 | 3,483 | 3,136 | 3,333 | 3,000 | 30 reserved, 0 layout probes |
| 100 | 4,594 | 4,223 | 4,333 | 4,000 | 30 reserved, 0 layout probes |
| 1000 | 26,506 | 21,517 | 26,333 | 21,333 | 30 reserved, 0 layout probes |

Every create opens exactly one `ConversationStore` (to seed the new
Conversation) on both sides. The head performs **zero** legacy-layout probes
during reservation at every point, including 1000 existing Sessions: the
reservation path never inspects an existing Session tree.

150-create resource workload (1 pre-existing Session): base 3,803 µs/create,
head 3,617 µs/create; 150 store opens on each side; head reserves 150
identities with 0 layout probes.

Logical Catalog payload (sum of `catalog.json` lengths before each create) is
identical across revisions (e.g. ~25.0 MB over 30 creates at 1000 existing
Sessions), because the Catalog write is not part of this issue. This is the
measured remaining bottleneck.

## Instrumented exclusive stage profile (debug test build, 50 creates)

Microseconds per create. Inclusive stages (`prepare_total`, `publish_total`)
are not the sum of their children; the difference is unattributed work
(`CatalogDocument` clone, `validate_document`, session-directory creation,
ID generation, line generation, publication-lock acquisition).

| Stage | 0 existing | 100 existing |
| --- | --- | --- |
| reserve identity | 15.7 | 17.6 |
| allocation directory | 1,317 | 6,411 |
| `SQLite` open | 3,533 | 3,413 |
| schema + lineage seed | 165 | 162 |
| prepare total (inclusive) | 6,350 | 16,392 |
| catalog serialize | 485 | 2,277 |
| temp write | 17 | 41 |
| file fsync | ~0.4 | ~0.4 |
| rename | 8.9 | 13.3 |
| directory fsync | 8.7 | 9.0 |
| publish total (inclusive) | 1,971 | 9,276 |

Reservation is flat (≈16–18 µs) across catalog sizes. Allocation-directory
creation, Catalog serialization and publication grow with the catalog because
`create_conversation_allocation` consults the catalog's retired/deleted
identities and because the whole Catalog document is cloned, serialized and
rewritten. These are measured, not hypotheses.

## List/search regression (#386 preserved)

Head, 256 Sessions, 50 reps per workload: **0** `ConversationStore` opens for
first/middle/last page, Session-id search, name search, and preview search.
Prepared clone/fork publication still carries the frozen-seed preview
(`r13_list_and_search_open_zero_stores_and_clone_keeps_preview`).

## R01-R14 mapping

| ID | Test(s) | File |
| --- | --- | --- |
| R01 | `r01_reservation_inspects_zero_existing_session_directories` | `src/local_runtime/session/tests/cfg3_reservation.rs` |
| R02 | `r02_duplicate_identity_rejects_without_overwriting`; `cfg3_identity_tests::injected_collisions_retry_and_publication_order_does_not_follow_uuid_order` | `cfg3_reservation.rs`, `cfg3_identity.rs` |
| R03 | `r03_concurrent_same_identity_has_one_winner`; `cfg3_identity_tests::same_conversation_uuid_in_different_sessions_has_exactly_one_reservation_winner` (barrier) | `cfg3_reservation.rs`, `cfg3_identity.rs` |
| R04 | `session_controller::tests::create_and_rename_visibility_follow_catalog_rename_not_durability_barrier`; `session_controller::tests::retained_allocations_and_committed_delete_have_one_winner` | `src/local_runtime/session_controller.rs` |
| R05 | `runtime::local_storage::tests::r05_kill_after_reservation_never_reissues_consumed_identity` (real `SIGKILL` child, restart) | `src/runtime/local_storage.rs` |
| R06 | `r06_kill_after_preparation_before_publication_leaves_inert_orphan` (real `SIGKILL`); `r06_prepared_unpublished_is_inert_across_reopen` | `cfg3_reservation.rs` |
| R07 | `r07_kill_after_publication_leaves_complete_valid_conversation` (real `SIGKILL`) | `cfg3_reservation.rs` |
| R08 | `r08_all_allocation_paths_share_the_reservation_contract` (new, clone, fork, branch) | `cfg3_reservation.rs` |
| R09 | `r09_marker_and_bytes_alone_never_claim_ownership` | `cfg3_reservation.rs` |
| R10 | `r10_old_populated_layout_is_rejected_before_allocation` (root, controller, preserved bytes) | `cfg3_reservation.rs` |
| R11 | `r11_reopen_preserves_consumed_identities` | `cfg3_reservation.rs` |
| R12 | `r12_pre_visibility_fault_never_reports_success`; `session_controller::tests::create_and_rename_visibility_follow_catalog_rename_not_durability_barrier` | `cfg3_reservation.rs`, `session_controller.rs` |
| R13 | `r13_list_and_search_open_zero_stores_and_clone_keeps_preview`; `session::tests::the_first_list_page_reads_persisted_projections_without_opening_a_store` | `cfg3_reservation.rs`, `session.rs` |
| R14 | `session_controller::tests::settings_cas_has_one_winner_without_touching_other_metadata`; `session::tests::new_and_name_publish_metadata_without_mutating_old_history`; `session::tests::tree_nodes_switch_between_independent_lineages_without_rewind` | `session_controller.rs`, `session.rs` |

Synchronization gates are barriers (R03), a token rendezvous plus real
`SIGKILL` and restart (R05-R07), and the catalog write fault seam (R12). No
test depends on a sleep to establish an interleaving. The R01 counter
instruments the storage owner's one legacy-layout probe, not a deleted helper.

## Remaining bottleneck and recommendation

After removing the application-level identity scan, the measured remaining
size-dependent costs are (a) the `CatalogDocument` clone and validation and
(b) whole-document serialization plus whole-file write/fsync/rename, both
proportional to Catalog size, and (c) `create_conversation_allocation`'s
catalog read. These dominate create cost at 1000 existing Sessions. A later
issue to replace whole-file Catalog publication with bounded transactional
persistence is therefore **supported by measurement**. This delivery does not
implement it, does not preselect Catalog `SQLite`, and keeps the current
`catalog.json` + one `conversation.sqlite` topology.

# Issue #387 validation: bounded Conversation identity reservation

This document records the acceptance matrix, the measurement environment, and
the reproducible before/after evidence for [Issue
#387](https://github.com/Caismis/rustX/issues/387). It is evidence, not a
substitute for the code and tests it describes. It distinguishes what is
directly measured from what is inferred from source structure.

## Base and measured trees

- Dependency base: `0083f64df3f16d2270e21eae0b90e13ad668d5b9`
  (merge of PR #389, which implements Issue #386). Verified to contain #386's
  persisted Session display projection by ancestry, not merely by issue state.
- Implementation branch: `issue-387-bounded-conversation-reservation`,
  worktree `../rustX-issue-387`.
- The final PR head SHA is the pushed commit of that branch; see PR #390.
- Base measurements below were taken from an isolated worktree detached at the
  exact base SHA with the *same* corrected benchmark source (only the two
  marked revision-specific counter bodies stubbed), so the workload, durability
  semantics and harness are identical.

## Environment

| Fact | Value |
| --- | --- |
| OS / kernel | Fedora Linux 44, `7.1.13-200.fc44.x86_64` |
| CPU | Intel(R) Core(TM) Ultra X7 358H, 16 logical CPUs |
| Memory | 32,343,792 kB total |
| Filesystem | btrfs (`/dev/nvme0n1p3[/home]`), `compress=zstd:1`, SSD |
| Timed roots | On that btrfs filesystem under the measured worktree's `target/bench-roots/` |
| Rust | `rustc 1.95.0`, `cargo 1.95.0` (stable) |
| Benchmark profile | `--release` |
| Test profile | debug (unit/bootstrap) |
| CPU source | `/proc/self/stat` utime+stime over `getconf CLK_TCK` |
| RSS source | `/proc/self/statm` resident pages times `getconf PAGE_SIZE` (point-in-time `VmRSS`, not peak) |

**Durability model matters to the numbers.** `SessionCatalog`'s catalog commit
fsyncs the catalog's parent directory and every ancestor
(`sync_directory_ancestry`), so each create performs roughly one directory sync
request per path component. On btrfs these are real and dominate wall time; on
a shallow `tmpfs` root they are effectively free. The numbers below are
measured on btrfs so the directory-sync and Catalog-byte evidence is physical.
Earlier measurements taken against a shallow or memory-backed root are
superseded and are not reproduced here.

## Three evidence layers that must not be conflated

The report separates three layers; no table mixes them:

1. **rustX logical owner operations.** The `logical_*` counters below. Each is
   incremented **once per logical operation requested by a named rustX owner**
   at its call site. They are *not* syscall counts: `create_dir_all` may create
   several directories or none, and a `write_all` may issue several `write(2)`
   calls. They are not physical device I/O.
2. **SQLite / library-internal filesystem work.** Deliberately **excluded**.
   `SqliteConversationStore::open` performs connection open, connection
   configuration, schema creation-or-validation, and Conversation identity
   binding, plus its own journal, page and fsync work. The only thing counted
   is one rustX `SqliteConversationStore::open` request
   (`logical_sqlite_store_open_request`); none of SQLite's internal syscalls
   are observed.
3. **Physical device I/O.** Deliberately **excluded**. Page cache, btrfs
   compression, journaling and device scheduling are invisible to these
   counters; measuring them needs an external process or block-device boundary
   (e.g. a block trace), which this repair does not add.

Additional rustX owners are intentionally not instrumented and are reported as
excluded rather than inferred from a neighbouring counter: the once-per-root
`sessions/` directory setup and the catalog-root `create_dir_all` no-op
requests.

## Reproducible commands

```text
# Create scaling and the 150-create resource workload (absolute roots required)
cargo build --release --example session_create_benchmark
./target/release/examples/session_create_benchmark \
    --root <abs>/create-<1|10|100|1000> --existing <n> --creates 30 \
    --json /tmp/create-<n>.json
./target/release/examples/session_create_benchmark \
    --root <abs>/res150 --existing 1 --creates 150 --json /tmp/res150.json

# List/search regression (#386), zero ConversationStore opens
cargo build --release --example session_list_benchmark
./target/release/examples/session_list_benchmark \
    --root <abs>/list --sessions 256 --messages 10 --reps 50 \
    --json /tmp/list.json

# Instrumented exclusive stage profile of the real controller (test-only)
RUSTX_387_PROFILE_EXISTING=100 cargo test --lib --all-features \
    'stage_profile_real_create_pipeline' -- --ignored --nocapture
```

The create benchmark compiles against both revisions. Its two
revision-specific bodies are `reservation_counters` (head reads the
storage-owner counters; base returns `(None, None)`) and
`logical_operation_counters` (head reads the logical-operation counters; base
returns `None`, so a base run reports every `logical_*` field as `null`, never
as a measured zero). Workload, durability and publication semantics are
identical on both sides.

## Create scaling (30 creates per point, fresh root, release, btrfs)

Each point was measured **three times**; the table reports the median, with the
observed range in parentheses. The batch grows its own fixture, so the timed
operations run against `start, start+1, ... end`, not a fixed population, and
the authoritative observed population is paginated to exhaustion (never a page
length capped at 32).

| Existing (`start..end`) | Base wall µs (range) | Head wall µs (range) | Base CPU µs (range) | Head CPU µs (range) |
| --- | --- | --- | --- | --- |
| 1 (`1..31`) | 86,311 (81,912–87,824) | 110,794 (108,610–160,559) | 25,333 (23,333–26,333) | 40,000 (38,333–44,000) |
| 10 (`10..40`) | 89,083 (83,373–89,421) | 109,697 (105,749–173,670) | 26,333 (25,333–27,000) | 40,667 (39,333–44,000) |
| 100 (`100..130`) | 90,546 (88,901–118,760) | 104,040 (96,911–114,956) | 28,667 (27,667–30,333) | 38,667 (37,667–40,000) |
| 1000 (`1000..1030`) | 124,474 (120,676–129,935) | 120,098 (113,594–200,143) | 59,667 (58,000–61,000) | 50,000 (48,333–52,333) |

Every create opens exactly one `ConversationStore` (to seed the new
Conversation) on both sides; the head reserves exactly 30 identities with
**zero** legacy-layout probes at every point. The head's per-create CPU is
consistently higher at low population by roughly the added reservation
durability barrier (the reservation-specific stage is flat; see the stage
profile), and at 1000 the removed allocation scan makes head marginally faster
than base. The individual run range shows the host is noisy; only the
1000-point ordering is stable across all three runs.

### Head rustX logical owner operations (deterministic; 30-create batch)

These are logical requests, **not** syscall counts, and **not** physical device
I/O. Values are identical across the three repetitions because every operation
is deterministic; `logical_catalog_bytes_written` is the only field that varies
with Catalog content.

| Logical operation | 30-create total | Per create |
| --- | --- | --- |
| reservation marker exclusive-create request | 30 | 1 |
| reservation marker payload-write request | 30 | 1 |
| reservation marker file-sync request | 30 | 1 |
| reservation namespace mkdir request (already initialized) | 0 | 0 |
| reservation namespace directory-sync requests | 60 | 2 |
| Session allocation mkdir request | 30 | 1 |
| Conversation allocation mkdir requests (parent chain + exclusive leaf) | 60 | 2 |
| `SqliteConversationStore::open` request | 30 | 1 |
| catalog temp-file open request | 30 | 1 |
| catalog payload-write request | 30 | 1 |
| catalog file-sync request | 30 | 1 |
| catalog rename request | 30 | 1 |
| catalog directory-sync requests (one per path ancestor) | 300 | 10 |
| catalog logical bytes written | grows with Catalog | see below |

`logical_catalog_bytes_written` per 30-create batch, by population:
≈429,785 (1), ≈661,095 (10), ≈2,974,140 (100), ≈26,171,505 (1000). That is the
only per-create payload proportional to Catalog size (≈14 KB → ≈872 KB per
create from 1 to 1000 existing Sessions). The reservation path's own logical
operations are constants independent of the population, and the removed
Session-directory scan emits no counter at all.

The catalog directory-sync request count is path-depth dependent (10 ancestors
for the measured roots); it is a count of ancestors visited by
`sync_directory_ancestry`, not a claim about `fsync(2)` calls or device
flushes.

## 150-create resource workload (1 pre-existing Session, release, btrfs)

Median of five runs.

| Metric | Base | Head |
| --- | --- | --- |
| Wall µs / create | 88,735 | 116,032 |
| Process CPU µs / create | 26,933 | 43,000 |
| `ConversationStore` opens | 150 | 150 |
| Reservation count | not instrumented | 150 |
| Legacy-layout probes | not instrumented | 0 |
| Logical Catalog bytes written | not instrumented | ≈9.86 MB |
| Reservation marker create / write / file sync | not instrumented | 150 / 150 / 150 |
| Session / Conversation allocation mkdir requests | not instrumented | 150 / 300 |
| SQLite open requests | not instrumented | 150 |
| Catalog directory-sync requests | not instrumented | 1,500 |
| RSS before / after (point-in-time, median) | 6,934,528 / 11,153,408 | 6,852,608 / 11,124,736 |

RSS is a point-in-time sample, not peak RSS. The base's `logical_*` fields are
`null` because the base has no such counters; they are not measured zeros.

## List/search regression (#386 preserved)

Head, 256 Sessions, 50 reps per workload: **0** `ConversationStore` opens for
first/middle/last page, Session-id search, name search, and preview search.
Head wall ms per workload: 4.98 / 5.06 / 5.06 / 5.92 / 6.19 / 5.17.
Prepared clone/fork publication still carries the frozen-seed preview
(`r13_list_and_search_open_zero_stores_and_clone_keeps_preview`).

## Instrumented exclusive stage profile (debug test build, 50 creates, tmpfs)

This is a **separate debug/tmpfs tracing experiment** of the real
`SessionController::create_session` pipeline, not a timing-threshold test and
not release/btrfs evidence. It prints exclusive leaf-stage times, the measured
total, and the unattributed remainder. Inclusive parent operations
(`prepare_session`, `publish_session`) are never summed with their children. It
runs on a `tmpfs` `tempdir`, so `fsync` stages are effectively free here and
must not be read as disk-persistence cost; the release logical-operation
evidence above is authoritative for I/O.

The stage names track the real implementation boundaries.
`sqlite_bootstrap_ns` is the whole `SqliteConversationStore::open` call
(connection open, connection configuration, schema creation-or-validation and
Conversation identity binding), and `lineage_initialize_ns` is only the
separate `initialize_lineage(seed)` call. They are not "connection open" and
"schema + seed".

Microseconds per create:

| Stage (exclusive) | 0 existing | 100 existing |
| --- | --- | --- |
| Catalog snapshot clone (controller) | 15.2 | 71.7 |
| reserve identity (incl. root barrier) | 39.0 | 41.1 |
| allocation directory (incl. Catalog read) | 1,485.3 | 7,002.3 |
| `SQLite` bootstrap/open (conn+config+schema+identity bind) | 4,624.7 | 4,656.1 |
| lineage initialization/seed (`initialize_lineage`) | 203.7 | 210.6 |
| Catalog document clone (publication) | 17.0 | 81.4 |
| catalog serialize | 590.6 | 2,816.6 |
| temp write | 19.0 | 36.7 |
| file sync | 0.4 | 0.6 |
| rename | 9.5 | 15.3 |
| directory sync (ancestry) | 9.7 | 12.6 |
| **measured total** | **10,210.9** | **29,787.4** |
| **exclusive sum** | **7,014.2** | **14,944.9** |
| **unattributed remainder** | **3,196.7** | **14,842.4** |

The unattributed remainder is dominated by Catalog work that is not a named
leaf stage: `create_conversation_allocation`'s `check_allocation_live` reads
and parses the whole `catalog.json` under the allocation guard, `commit`
re-reads the current Catalog (`read_under_guard`), and `validate_document`
walks every Session. Those are inferred from the source structure and from the
growth of the measured `allocation directory` stage; they are not separately
instrumented, so they are reported as the remainder rather than claimed as
measured leaf stages.

## Namespace-initialization durability regressions

`ensure_reservation_namespace` no longer treats an existing
`conversation-reservations/` directory as proof that initialization durability
completed. Visibility is not durability: every path that observes the namespace
re-establishes the product-root parent-entry barrier. `ProductRoot::create`
also persists the product root's own directory entries (and any ancestors
`create_dir_all` created) before the namespace barrier. The fresh-root format
decision is linearized on the exclusive namespace creation, so a concurrent
initializer that creates the namespace (and then the `sessions` tree) cannot
make a parked observer reject the valid new-format root as a legacy layout.

Deterministic regressions (no sleeps; explicit gates and injected faults):

| Test | Proves |
| --- | --- |
| `runtime::local_storage::tests::namespace_visibility_is_not_initialization_durability` | A namespace built without the barrier (residue) still forces a later reservation to perform the root barrier |
| `runtime::local_storage::tests::failed_initialization_barrier_residue_is_completed_by_retry` | A failed barrier leaves visible residue, reports no success, and a retry completes it |
| `runtime::local_storage::tests::concurrent_fresh_root_initialization_is_not_rejected_as_legacy` | A parked observer accepts a concurrently initialized new-format root (gate rendezvous) |
| `runtime::local_storage::tests::repeated_open_of_initialized_root_is_valid` | Repeated open preserves consumed identities |

## R01-R14 mapping

| ID | Test(s) | File |
| --- | --- | --- |
| R01 | `r01_reservation_inspects_zero_existing_session_directories` | `cfg3_reservation.rs` |
| R02 | `r02_duplicate_identity_rejects_without_overwriting`; `cfg3_identity_tests::injected_collisions_retry_and_publication_order_does_not_follow_uuid_order` | `cfg3_reservation.rs`, `cfg3_identity.rs` |
| R03 | `r03_concurrent_same_identity_has_one_winner` (barrier); `cfg3_identity_tests::same_conversation_uuid_in_different_sessions_has_exactly_one_reservation_winner` | `cfg3_reservation.rs`, `cfg3_identity.rs` |
| R04 | `session_controller::tests::r04_concurrent_distinct_session_creates_preserve_both_publications` (deterministic preparation-wait signal; real destinations); `session_controller::tests::r04_pre_publication_snapshots_preserve_both_publications` (two pre-publication snapshots both publish) | `session_controller.rs` |
| R05 | `runtime::local_storage::tests::r05_kill_after_reservation_never_reissues_consumed_identity` (real `SIGKILL` child, restart) | `local_storage.rs` |
| R06 | `r06_kill_after_preparation_before_publication_leaves_inert_orphan` (real `SIGKILL`); `r06_prepared_unpublished_is_inert_across_reopen` | `cfg3_reservation.rs` |
| R07 | `r07_kill_after_publication_leaves_complete_valid_conversation` (real `SIGKILL`) | `cfg3_reservation.rs` |
| R08 | `r08_all_allocation_paths_share_the_reservation_contract` (new, clone, fork, branch); `runtime::subagent::process::tests::r08_child_allocation_uses_the_shared_reservation_contract` (real `allocate_child_runtime_root` -> `PhysicalChildRuntimeRoot::allocate`) | `cfg3_reservation.rs`, `subagent/process.rs` |
| R09 | `r09_marker_and_bytes_alone_never_claim_ownership` | `cfg3_reservation.rs` |
| R10 | `r10_old_populated_layout_is_rejected_before_allocation` (root, controller, preserved bytes); `concurrent_fresh_root_initialization_is_not_rejected_as_legacy` | `cfg3_reservation.rs`, `local_storage.rs` |
| R11 | `r11_reopen_preserves_consumed_identities`; `repeated_open_of_initialized_root_is_valid`; `namespace_visibility_is_not_initialization_durability` | `cfg3_reservation.rs`, `local_storage.rs` |
| R12 | `r12_initialization_failure_after_reservation_is_inert`; `r12_destination_validation_failure_after_initialization_is_inert`; `r12_pre_visibility_fault_never_reports_success` | `cfg3_reservation.rs` |
| R13 | `r13_list_and_search_open_zero_stores_and_clone_keeps_preview`; `session::tests::the_first_list_page_reads_persisted_projections_without_opening_a_store` | `cfg3_reservation.rs`, `session.rs` |
| R14 | `session_controller::tests::settings_cas_has_one_winner_without_touching_other_metadata`; `session::tests::new_and_name_publish_metadata_without_mutating_old_history`; `session::tests::tree_nodes_switch_between_independent_lineages_without_rewind` | `session_controller.rs`, `session.rs` |

### How R01 detects real enumeration

R01 builds its 1- and 1000-Session fixtures as **published** Sessions through
the production owners (real `prepare_session` allocations plus the catalog's
`commit` publication owner), then measures a reservation against an OS-level
fixture:

- the fixture's `sessions/` tree is chmod'd execute-only (`0o111`), so directory
  enumeration is denied while direct known-path access still works;
- on Linux an inotify watch on `sessions/` observes `IN_ACCESS`/`IN_OPEN`, the
  kernel boundary a repeated `readdir` would cross.

The assertion is that reservation succeeds while the probe records zero
directory accesses and (for non-root) enumeration fails. This detects an
alternate traversal path, not merely "the deleted helper was not called". The
fixture is constructed outside the measured reservation, and the only counter
read is the operation result (exactly one reservation of a fresh identity
succeeds); no process-global before/after delta is asserted.

### R04 overlap (deterministic)

The controller's preparation owner serializes preparation by design, so R04
proves the contract at the actual allowed overlap/publication boundary. The
test parks the first request **inside** preparation while it holds the
preparation reservation, then waits on a test-only notification that fires only
when the second request's preparation-lock future has been polled and returned
`Pending` (see `SessionController::acquire_preparation`). That makes the second
request a registered waiter on the very mutex the first holds — not merely a
spawned task that a scheduler might run after the first completes. Only after
that signal is the first released; both then publish through the
generation-checked Catalog owner, leaving two distinct complete destinations.
No sleep and no `yield_now()` participates. The companion test prepares two
lineages from pre-publication whole-document snapshots and publishes both, so a
stale whole-document replacement would drop one Session.

### R08 child/subagent proof

`r08_child_allocation_uses_the_shared_reservation_contract` drives the real
`SubagentSpawnPlan::allocate_child_runtime_root` ->
`PhysicalChildRuntimeRoot::allocate` path and asserts the storage-owner
`conversation-reservations/<ConversationId>` marker exists, the identity is not
reusable, and the semantic child destination directory was materialized. The
child path contains no Session-directory enumeration: it reserves the identity,
rejects existing known-path residues, and creates the allocation directory.

### R12 initialization and validation faults

- initialization fault: reservation succeeds, then the real
  `initialize_database` seam fails. No Session is visible, no success is
  reported, the reserved identity stays consumed, the prepared directory is
  inert across reopen, and a retry selects a fresh identity.
- destination-validation fault: reservation and initialization succeed, then
  publication-time destination validation fails (the database file is removed).
  Same invariants.
- catalog pre-visibility fault: the existing tested write seam fails before the
  visibility rename; no Session is visible.

## Synchronization gates

Barriers (R03), a `Gate` rendezvous (concurrent init), a deterministic
preparation-wait notification (R04), a token rendezvous plus real `SIGKILL` and
restart (R05-R07), narrow thread-local fault seams (initialization barrier,
database initialization, catalog write), and the OS enumeration probe (R01). No
test depends on a sleep to establish an interleaving, and no correctness
assertion depends on a process-global counter that unrelated parallel tests can
perturb.

## Remaining bottleneck and recommendation

Directly measured on the real release pipeline:

- the reservation primitive is flat and small (≈40 µs per create in the debug
  profile, and 30 reservations with 0 layout probes at every scale);
- Catalog-size-dependent work grows: the logical Catalog payload written per
  create rises from ≈14 KB to ≈872 KB from 1 to 1000 existing Sessions, and
  head CPU per create rises from ≈40 ms to ≈50 ms over that range;
- in the debug stage profile, `allocation directory` (which includes
  `check_allocation_live`'s whole-`catalog.json` read), `catalog serialize`,
  the controller Catalog snapshot clone and the publication-time
  `CatalogDocument` clone all grow with Catalog size.

Inferred from source structure (not separately instrumented): the unattributed
remainder (≈3.2 ms at 0, ≈14.8 ms at 100 in debug) is dominated by the
`commit`-path current-Catalog re-read and `validate_document`'s whole-Catalog
walk.

Conclusion: the corrected evidence **does** show that Catalog-size-dependent
whole-document work (read, clone, serialize, validate, write/rename/sync)
dominates create cost at scale, while the reservation primitive no longer does.
The reservation's added durability barrier keeps head CPU per create higher
than base at low population, and the removed Session-directory scan makes head
marginally faster than base only at 1000. A later, separately scoped issue to
replace whole-file Catalog publication with bounded transactional persistence
is therefore **supported by measurement**. This delivery does not implement it,
does not preselect Catalog `SQLite`, and keeps the current `catalog.json` + one
`conversation.sqlite` topology. The physical directory-sync cost of the
durability contract is also visible and would be addressed by such an issue
only if a shallow-root or batched-ancestry strategy is part of its scope.

## Limitations

- The host is noisy; scaling wall/CPU are medians of three runs with the range
  shown, and the per-run range at 1000 spans ≈114–200 ms per create. The
  deterministic logical-operation counts and the Catalog logical bytes are the
  stable evidence; the wall/CPU tables are indicative.
- The stage profile is debug/tmpfs and excludes release/device effects by
  construction; it attributes exclusive leaf stages only and leaves the
  `commit` re-read and `validate_document` walk in the unattributed remainder.
- Physical device I/O and SQLite-internal filesystem work are not measured by
  any counter in this delivery.

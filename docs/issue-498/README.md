# Parallel source preparation evidence

The coordinator admits a finite set of source identities before preparation.
Independent package builds and MCP connections share four preparation slots per
coordinator. A managed source retains its slot across package preparation and
the following MCP connection; a ready package can connect while a sibling is
still building. Completed results return to captured identity order before
tool selection and atomic publication. Root source failures remain
optional-source availability facts; frozen required child failures reject the
complete child candidate. The existing Python fingerprint store remains the
only build owner.

## Preserved contracts and deterministic coverage

| Contract | Executable coverage |
| --- | --- |
| Independent handshakes overlap; the bound is four; unselected sources remain inert | `parallel_tests::mcp_handshakes_overlap_with_a_bound_of_four_and_undemanded_sources_stay_inert` |
| Simultaneous candidates share the coordinator's MCP preparation bound | `parallel_tests::simultaneous_candidates_share_the_coordinators_source_preparation_bound` |
| Reverse completion yields the same ordered tools, availability and diagnostics; optional failure remains isolated | `parallel_tests::reverse_mcp_completion_keeps_identity_order_and_isolates_optional_failure` |
| A failed required child publishes no partial capability and settles successful siblings | `parallel_tests::selected_required_failure_settles_every_parallel_source_before_returning` |
| Dropping a preparation waiter cancels owned work and settles every owner | `parallel_tests::dropped_parallel_preparation_cancels_lists_and_settles_owners`; `mcp_race_tests::dropped_mcp_preparation_still_owes_physical_settlement` |
| Cancellation during catalog reconnection retains ownership until the replacement physically settles | `parallel_tests::cancelled_catalog_reconnect_retains_ownership_until_physical_settlement` |
| A physical close failure remains observable while every sibling is still settled | `mcp_race_tests::one_failed_mcp_close_never_abandons_a_sibling_runtime` |
| Supersession and cancellation before spawn or commit cannot publish obsolete source admission | `mcp_race_tests::cfg233_stale_preparation_cannot_publish_old_enablement_over_disable`; `tests::runtime_drain_wins_capability_commit_boundary`; child preparation cases `attempt_cancellation_settles_a_long_preparation`, `parent_loss_settles_a_long_preparation`, `a_step_completing_after_cancellation_is_not_publishable` |
| Invalidation rejects stale candidates while preserving the last published snapshot | `parallel_tests::invalidated_parallel_candidate_preserves_the_last_good_snapshot`; `mcp_race_tests::mcp_notification_wins_prepared_candidate_is_stale_and_active_snapshot_unchanged`; `mcp_race_tests::mcp_commit_wins_later_notification_belongs_to_the_next_refresh`; `mcp_race_tests::mcp_real_list_changed_notification_rejects_the_prepared_candidate` |
| Attempt leases pin their complete immutable snapshot; quiescence controls publication | `mcp_race_tests::mcp_active_attempt_lease_keeps_commit_busy_and_pins_the_old_registry`; `mcp_race_tests::an_old_lease_keeps_serving_its_generation_while_future_leases_resolve_to_the_new_one` |
| Independent Python packages overlap; a ready package continues before a slow sibling finishes; results return in source order | `python_preparation_tests::distinct_python_packages_overlap_and_return_in_source_order` |
| Managed Python state remains keyed and coalesced by the existing fingerprint owner | `python_preparation_tests::overlapping_python_preparations_coalesce_identical_fingerprints`; `tools::python::tests::concurrent_preparations_coalesce_onto_one_physical_build`; `scripted_suites::capability::python_tools` |
| Python build slots are shared across candidates and retained until physical cancellation settles | `python_preparation_tests::python_build_slots_are_shared_and_retained_until_cancellation_settles` |
| HTTP and stdio use the same generic capability and settlement owners | `mcp` in the external `tools` target; `boundary_suites::mcp_recovery` |
| Modern and legacy lifecycle negotiation, no replay after an ambiguous effect, progress, MRTR and Tasks stay unchanged | `mcp_managed::a_real_managed_child_negotiates_the_modern_mcp_revision` in the external `tools` target; `boundary_suites::mcp_recovery`; `boundary_suites::mcp_mrtr`; `boundary_suites::mcp_tasks` |

The new MCP cases live in
[`parallel.rs`](../../src/capabilities/coordinator/tests/parallel.rs).
Existing ownership, notification and lease cases remain in
[`coordinator.rs`](../../src/capabilities/coordinator.rs). Protocol and execution
contracts remain in their existing boundary suites; initialization does not
change dispatched `tools/call` behavior. The child cancellation and parent-loss
cases remain in
[`subagent_child/tests.rs`](../../src/local_runtime/composition/subagent_child/tests.rs).

## Controlled-delay measurement

[`performance.rs`](../../src/capabilities/coordinator/tests/performance.rs) is an
ignored measurement fixture, independent of correctness coverage. Both revisions
run the same test source through the actual coordinator's preparation and commit
methods. Every row uses a new coordinator, empty store, new source bindings and
fresh loopback HTTP listeners. Listener construction is outside the timer; the
timer spans preparation and commit. Cleanup is awaited after the timer.

For 1, 4 and 8 MCP sources, an official-rmcp HTTP server handler waits 100 ms
during `server/discover` and 100 ms during `tools/list`. The recorded handshake
and list durations are server-handler residence times, including the controlled
wait; they are not network round-trip estimates. The evidence retains every
phase duration, its exact request count (also the corresponding array length), and
the maximum simultaneous handler phases across all servers. There is no list
pagination, subscription, tool call, warm transport or catalog cache in this
measurement. Each source must publish exactly one ready tool.

Managed Python is reported separately. Its deterministic process backend waits
100 ms for each `uv lock` and `uv sync`, returns fixed runtime identities, and
writes the same non-executable interpreter stub used by the existing package
accounting fixtures. The real Python store still freezes source, fingerprints,
coalesces, builds and publishes prepared state. Synthesized MCP launches then
fail at exec. The per-phase durations and peak report build work separately from
connections. `controlled_work_span_ms` spans the first build phase through the
last completed build phase, including scheduling gaps while earlier packages
attempt their connections; `composition_ms` includes the complete candidate and
its typed unavailable source results. These are controlled build-overlap
measurements, not dependency download or real FastMCP startup timings.

Run the head fixture explicitly:

```bash
mkdir -p docs/issue-498/evidence
cargo build --bins
RUSTX_SOURCE_TIMING_JSON="$PWD/docs/issue-498/evidence/after.json" \
  cargo test --lib --all-features \
  capabilities::coordinator::performance_tests::source_preparation_timing \
  -- --ignored --exact --nocapture
```

The baseline is `d730966ce54ba330529bffb0279d6e0c4a44fd17`. To reproduce it,
export that revision to a separate directory, copy the unchanged
`performance.rs` from this PR into the same relative path, and append only its
test module declaration to the baseline coordinator:

```rust
#[cfg(all(test, feature = "mcp-fixture"))]
#[path = "coordinator/tests/performance.rs"]
mod performance_tests;
```

Run the identical command from that directory with `RUSTX_SOURCE_TIMING_JSON`
pointing to `before.json`. There is no production baseline patch. If sharing a
`CARGO_TARGET_DIR`, run base and head builds sequentially. Wall-clock values may
vary; overlapping phases and unchanged request counts are the evidence.
All rows in one run share a process, so process-wide initialization can make
the first row more expensive. Recorded samples use tmpfs source stores on a
shared host with other Rust builds active; composition values include host
scheduling and synthesized process-start costs.

## Recorded timing results

One controlled run per revision used the same fixture bytes and 100 ms delay
per phase. Paired phase means and peaks read **before → after**. Request counts
are identical in both revisions. Every individual duration remains in
[`before.json`](evidence/before.json) and [`after.json`](evidence/after.json);
source hashes, baseline isolation checks and exact commands are in
[`manifest.json`](evidence/manifest.json).

| MCP sources | Composition before ms | Composition after ms | Handshake mean ms | List mean ms | Peak phases | Discover / list requests per revision |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 264.5 | 268.9 | 101.3 → 100.3 | 101.3 → 101.3 | 1 → 1 | 1 / 1 |
| 4 | 910.0 | 233.5 | 101.0 → 100.4 | 101.0 → 100.8 | 1 → 4 | 4 / 4 |
| 8 | 1829.4 | 473.1 | 101.2 → 101.3 | 101.2 → 101.3 | 1 → 4 | 8 / 8 |

| Python packages | Build span before ms | Build span after ms | Composition before ms | Composition after ms | Lock mean ms | Sync mean ms | Peak build phases | Lock / sync requests per revision |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 201.8 | 201.7 | 741.0 | 539.3 | 101.4 → 100.3 | 100.3 → 101.3 | 1 → 1 | 1 / 1 |
| 4 | 811.5 | 202.8 | 1779.3 | 367.2 | 100.9 → 101.4 | 101.4 → 101.3 | 1 → 4 | 4 / 4 |
| 8 | 1625.8 | 570.0 | 3409.5 | 734.7 | 101.4 → 101.4 | 101.2 → 101.5 | 1 → 4 | 8 / 8 |

The per-request waits remain approximately 100 ms while four sources overlap.
Eight sources stay bounded at four concurrent phases. Python composition also
includes intentionally failed synthesized launches; the phase means report only
scripted build work and the build span includes intervening connection scheduling.

## Validation prerequisites

The current CI pins Rust 1.98.1. `cargo build --bins` must precede library tests
that execute the native supervisors. Real managed-Python and provider boundaries
require uv and Python 3.12; prepare the shared provider emulator with
`uv sync --frozen` in `test-support/fake-provider`. Set
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1` when running those boundaries, as CI does.
The controlled timing fixture itself uses no external package registry.

The issue's final Rust check set is:

```bash
git diff --check
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --bins
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features
```

The repository's split FULL lanes and their prerequisites are documented in
[`tests/README.md`](../../tests/README.md#running) and enforced by
`.github/workflows/ci.yml`. Coordinator production changes require FULL
verification on the exact PR head; apply the `full-verification` label to select
that profile.

## Local validation environment

Local checks used Rust 1.98.1 on Linux x86_64 (32 logical CPUs), Python 3.12.3
and uv 0.12.10. The full Rust test command used
`CARGO_TARGET_DIR=/tmp/rustx-issue-498-target`,
`TMPDIR=/run/user/1000/rx.R85W` (exec-capable tmpfs),
`RUST_TEST_THREADS=4` and `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. The selected
test corpus and deadlines were retained; tmpfs and bounded test scheduling
isolate source preparation from shared-host disk contention.

The full all-targets run passed 5,325 tests across 20 harnesses, with 15 ignored
measurements/live tests and no failures. This includes 4,625 library tests,
135 external tool tests and 47 external subagent tests. The CI-controller
Python suite (`.github/scripts`) passed 64 tests, and the benchmark-controller
Python suite (`benchmarks/runtime`) passed 37 tests. Per-harness results and
the exact environment are retained in
[`validation.json`](evidence/validation.json).

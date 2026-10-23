# Issue #499: bounded MCP startup with inert catalog hints

This change removes slow optional MCP discovery from the unbounded initial
Root-composition path. A process-local catalog hint can supply immutable tool
declarations immediately. Cache misses share one one-second grace period, after
which initial composition reports `connecting { cached: false }` and omits those
tools. The existing four-source preparation bound still applies. Selected Workflow
dependencies remain required; child admission keeps its complete frozen-source
contract.

A catalog hint is not a live connection or a permission grant. Each Conversation
owns its transport and process. Before `tools/call`, the owning connection waits
for its counted initializer, obtains a complete live catalog, and validates the
exact tool name, description and full input schema captured by the Attempt. A
changed declaration fails before dispatch. A replacement transport repeats live
validation. The rmcp response cache remains disabled, including positive server
TTLs and failed refreshes.

The cache retains at most 32 entries for 30 minutes. Identity includes Workspace,
server ID, effective transport, executable/cwd/environment or HTTP headers,
captured credentials, invocation policy, protocol negotiation and advertised
client capabilities. Secret values only feed the opaque digest. Unknown identity
is a miss. Only successful complete discovery at a stable source epoch can
publish; fetch ordering, expiry, LRU eviction and invalidation revoke stale
captures. Failed discovery never masquerades as successful stale discovery.

Late readiness signals the existing native configuration owner. Ordinary
preparation/publication and Session adoption govern the next usable snapshot;
active Attempts remain immutable. Headless hosts explicitly call
`adopt_ready_capabilities` at their existing safe adoption boundary. An explicit
configuration rescan/reconcile or headless adoption can also recover an
invalidated or Unavailable MCP catalog without editing configuration. It uses
normal full preparation and adoption; startup notifications never retry failed
discovery. Managed Python admission remains strict. App Server
v56 and Runtime Client v76 expose the new closed state vocabulary; WebUI and TUI
show connecting/cached status without claiming that a source is ready.

## Reference pattern and scope

The reference was OpenAI Codex at immutable commit
[`d6817454d76885e9d0e118bbf82e507aed6f715a`](https://github.com/openai/codex/tree/d6817454d76885e9d0e118bbf82e507aed6f715a),
particularly
[`tool_catalog_cache.rs`](https://github.com/openai/codex/blob/d6817454d76885e9d0e118bbf82e507aed6f715a/codex-rs/codex-mcp/src/tool_catalog_cache.rs)
and
[`connection_manager/tool_catalog.rs`](https://github.com/openai/codex/blob/d6817454d76885e9d0e118bbf82e507aed6f715a/codex-rs/codex-mcp/src/connection_manager/tool_catalog.rs).
The useful patterns are bounded metadata retention, exact identity, ordered
fetch publication, invalidatable captures and one optional-startup deadline.
rustX retains its own counted lifecycle, immutable capability snapshots,
required-source admission and configuration adoption boundaries. No transport
pool, disk cache, configuration flag or second Session adoption path is added.

## Correctness evidence

These are deterministic behavior tests, separate from the timing fixture:

| Invariant | Tests |
| --- | --- |
| Hint identity, expiry, bounded LRU, fetch ordering and invalidation | `tools::mcp::catalog_cache::tests` |
| Hint has no transport authority; first use waits for live schema | `cache_hint_is_not_a_connection_and_first_use_waits_for_live_schema` |
| Same name/description with changed schema has zero dispatch | `same_name_description_changed_schema_never_dispatches_cached_tool` |
| Invalidation during listing and after validation cannot cross the effect frontier | `invalidation_during_listing_rejects_capture_and_sdk_ttl_never_hides_failure`, `invalidation_after_validation_before_enqueue_refuses_tools_call` |
| Replacement transport revalidates and concurrent discovery coalesces | `replacement_transport_revalidates_schema_and_coalesces_concurrent_fetches` |
| A newer catalog cannot authorize an older capture, and duplicate names never authorize a declaration | `newer_catalog_cannot_authorize_an_older_validation_capture`, `duplicate_live_names_never_authorize_a_frozen_declaration` |
| Cancellation settles live listing, a discovery-lock waiter and connection acquisition before dispatch | `cancellation_during_live_listing_settles_without_dispatching_a_tool`, `cancellation_while_waiting_for_discovery_lock_starts_no_request`, `cancellation_while_acquiring_connection_starts_no_request` |
| Terminal startup failure is reused for that generation/epoch; invalidation permits fresh discovery; one caller’s cancellation cannot poison another | `terminal_startup_failure_is_not_retried_by_cached_first_use`, `invalidation_reopens_a_failed_generation_catalog_for_fresh_discovery`, `cancelled_validation_does_not_poison_the_same_connection_for_another_caller` |
| An older failed acquisition cannot evict a newer Conversation publication | `older_failed_acquisition_preserves_newer_conversation_publication` |
| Queued first use cannot bypass the source bound; cancellation has zero dispatch | `cancelled_first_use_never_bypasses_queued_startup_owner` |
| Cold grace and warm hints preserve admitted snapshots | `cold_optional_grace_keeps_the_admitted_snapshot_immutable`, `cached_optional_catalog_is_frozen_before_transport_readiness` |
| Required Workflow admission still waits for complete source preparation | `required_workflow_source_cannot_fall_through_optional_grace` |
| Optional MCP handling does not relax selected Python source admission | `selected_invalid_python_refuses_composition_without_publication`, `python_shadowing_is_inert_and_a_malformed_higher_file_blocks_selected_admission` |
| Cancellation/drain waits for physical initializer settlement | `pending_initializer_stays_counted_until_drain_settles_the_process` |
| Resident resource ownership permits idle adoption but still blocks quiescence; retained candidates and late registrations settle | `resident_resources_allow_idle_adoption_but_still_block_quiescence`, `drain_settles_retained_candidate_and_late_generation_registration` |
| Another Conversation invalidates a captured hint; bounded retry settles old work | `another_conversation_invalidates_the_hint_before_commit_and_retry_settles` |
| Failed refresh keeps the previous snapshot and revokes changed-source hints | `failed_refresh_keeps_last_good_snapshot_and_revokes_changed_source_hint`, `failed_refresh_preserves_a_ready_empty_catalog_and_its_owner` |
| Each ready source publishes while a sibling remains held | `each_late_source_can_be_adopted_while_a_sibling_remains_held` |
| Startup failure and invalidation become unavailable through native adoption; live results retain their captured epoch | `warm_startup_failure_becomes_unavailable_through_native_adoption`, `invalidated_startup_listing_becomes_unavailable_after_native_adoption`, `live_startup_catalog_cannot_commit_after_invalidation`, `catalog_observation_retains_capture_epoch_and_terminal_invalidation_failure` |
| Actual provider requests and native Session adoption retain each Attempt's schema | `cold_readiness_and_warm_schema_change_use_native_adoption_without_mutating_attempt` |
| Unchanged configuration recovers a rejected schema without stale dispatch; failed refresh retains the old snapshot and repeated reconciliation is a no-op | `unchanged_reconcile_recovers_changed_mcp_schema_without_stale_dispatch` |
| Explicit recovery also handles terminal startup failure and headless invalidation | `warm_startup_failure_becomes_unavailable_through_native_adoption`, `headless_capability_adoption_refuses_busy_and_preserves_admitted_attempt` |
| Optional MCP failures preserve native tools while selected Managed Python failures remain fatal | `no_shared_mcp_revision_is_unavailable_not_fatal`, `an_oversized_mcp_diagnostic_is_bounded_before_authoritative_state`, `selected_invalid_python_refuses_composition_without_publication`, `python_shadowing_is_inert_and_a_malformed_higher_file_blocks_selected_admission` |
| Native projection preserves cached provenance | `connecting_projection_preserves_metadata_provenance_without_claiming_readiness` |
| WebUI/TUI distinguish Connecting, cached catalog and Prepared | `web-console/test/mcp-page.test.tsx`, `web-console/test/settings-workflows.test.tsx`, `tui/test/status.test.ts`, `web-console/test/e2e/mcp-startup.spec.ts` |

The typed startup presentation was checked in the pinned Chromium container at
[1440px](evidence/mcp-startup-1440.png) and [390px](evidence/mcp-startup-390.png).
The public MCP list shows Connecting and cached-catalog provenance; the narrow
layout wraps the complete status without claiming readiness.

The existing positive-TTL, failed-refresh and reconnect SDK-cache regression
suites remain part of the required checks; the optimization does not change their
fresh-network-discovery contract. The current configuration and Issue #380
adoption contracts are documented in [configuration](../configuration.md),
[invariants](../invariants.md), and [Issue #380](../issue-380-implementation.md).

## Reproducible timing

The worktree started from main commit
`741955eb18d0deb18e5dbafc950f07524f0a75fa`, including #498. Final integration
and timing use the newer main commit
`00978bbe3072b211efe835950f7f99666e1cc02d`, also including #502, so unrelated
Workflow Model changes are present on both sides of the comparison. Baseline
product sources are an exact `git archive` into a temporary directory.
[adapt-baseline.py](adapt-baseline.py) installs the same new HTTP
measurement fixture with only the absent API seams adapted: baseline eager
preparation, zero cache probes, no Connecting counters/watch or late-readiness
publication, and baseline cleanup. It does not change baseline product behavior.

The source timing test is
`capabilities::coordinator::startup_performance_tests::optional_mcp_startup_timing`.
It measures 0/1/4/8 admitted optional servers, fast/slow/failed discovery, and
cold/warm catalogs. Real loopback Streamable HTTP servers and the client share
one process; no server process is spawned by this fixture. Each successful source
has one controlled discover phase and one controlled tools/list phase (10 ms each
for fast, 750 ms each for slow/failed). Failed peers return a tools/list protocol
error. Warmup uses a separate fully discovered coordinator and closes its live
connections before the measured coordinator starts.

`composition_ready_ms` is the initial candidate/commit boundary and therefore a
**model-admission timing proxy**, not the timestamp of an actual provider request.
The companion
`capabilities::coordinator::startup_performance_tests::model::first_model_request_timing`
records **an actual `ModelRequest`** when `ModelAdapter::stream` is entered. It
constructs and activates a real `ConversationRuntime`, submits an inbound user
message, and lets ordinary admission, context assembly and `AgentExecution` issue
the request. The timer starts immediately before source preparation and includes
runtime construction; authored config, fixture servers and tool-runtime storage
setup are outside the timer on both revisions. The scripted provider returns one
text response, with no external provider latency. Request count and frozen tool
inventory are asserted. Startup work is allowed to finish after the measured
request, so teardown does not hide discovery cost. This companion uses the same
24 scenarios and baseline-only API adapters as the source timing fixture.
`first_tool_dispatch_ms` is observed by the real tools/call server handler;
`first_use_elapsed_ms` covers the native executor call, including any remaining
live-discovery wait. `capability_ready_ms` includes publication through the native late-readiness
candidate path. It captures terminal observations from owned connections without
starting another tools/list; an already complete initial snapshot needs no extra
preparation. Failure rows use null for unavailable success timestamps.

Cache hit counts report explicit eligible lookups immediately before measured
composition; they do not claim invisible internal-cache instrumentation. The
warm seed's network requests are recorded separately. This makes the retained
live-discovery cost visible even when cached composition is fast. Single runs on
a shared Linux host are evidence of the controlled mechanism, not a p50/p95 or
user-latency claim.

```sh
# In a fresh archive of the baseline commit:
python3 /path/to/head/docs/issue-499/adapt-baseline.py /path/to/baseline

# Build the matching supervisor helpers in each tree before measuring:
CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/path/to/shared/target \
cargo build --all-features --bin bash-supervisor --bin interactive-supervisor

# Run once in each tree with the same Rust environment and tmpfs fixture root:
CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/path/to/shared/target \
TMPDIR=/path/to/tmpfs \
RUSTX_STARTUP_TIMING_JSON=/path/to/startup.json \
cargo test --lib --all-features \
  capabilities::coordinator::startup_performance_tests::optional_mcp_startup_timing \
  -- --ignored --exact --nocapture

# Measure an actual first model request on the same scenario matrix:
CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/path/to/shared/target \
TMPDIR=/path/to/tmpfs \
RUSTX_MODEL_STARTUP_TIMING_JSON=/path/to/model-startup.json \
cargo test --lib --all-features \
  capabilities::coordinator::startup_performance_tests::model::first_model_request_timing \
  -- --ignored --exact --nocapture

# Retain the exact unchanged #498 workload on both trees:
CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/path/to/shared/target \
TMPDIR=/path/to/tmpfs \
RUSTX_SOURCE_TIMING_JSON=/path/to/source-preparation.json \
cargo test --lib --all-features \
  capabilities::coordinator::performance_tests::source_preparation_timing \
  -- --ignored --exact --nocapture
```

The recorded runs preserve a separate libtest/supervisor bundle for each
revision. Copied libtests keep both matching helpers beside the executable, so
helper discovery and expected process settlement still occur after another
revision is built into the shared Cargo target. Bundle paths, hashes and build
commands are recorded in each manifest.

The unchanged #498 Python rows are labeled `python_scripted_build`: a
`SupervisedProcessRunner` fixture supplies controlled `uv lock`/`uv sync` delays
and deliberately creates a non-executable interpreter. The phase timings measure
scripted build scheduling, not real package installation. Total `composition_ms`
also includes the expected failed MCP launch and settlement: the interactive
supervisor really starts before interpreter execution fails. These Python rows
do not measure process-spawn counts. Real process ownership, cancellation and
drain are covered by the native process/tool suites and the deterministic
initializer lifecycle tests above.

The unchanged #498 fixture's SHA-256 is
`545f007a604873966721bc6c5720a62b655dc7abd4b3108cf9014759ad7e6e0c`.
Machine-readable evidence and environment/command manifests live in
[evidence](evidence). Final measurements and check results are recorded alongside
these files.

## Measured results

Baseline is `00978bbe3072b211efe835950f7f99666e1cc02d`. All values below are milliseconds from one controlled run per scenario; the complete source, model-request, network-count and unchanged #498 matrices are in [evidence](evidence).

| Discovery | Sources | Cold first model, baseline → head | Warm first model, baseline → head |
| --- | ---: | ---: | ---: |
| fast | 0 | 69.1 → 58.1 | 38.0 → 35.3 |
| fast | 1 | 112.3 → 116.4 | 88.3 → 34.6 |
| fast | 4 | 91.0 → 149.5 | 95.3 → 39.2 |
| fast | 8 | 146.0 → 277.9 | 147.1 → 40.7 |
| slow | 0 | 31.5 → 32.8 | 31.0 → 31.2 |
| slow | 1 | 1,576.1 → 1,044.7 | 1,581.5 → 34.0 |
| slow | 4 | 1,565.2 → 1,041.5 | 1,588.3 → 40.7 |
| slow | 8 | 3,120.7 → 1,040.3 | 3,117.7 → 41.5 |
| failed | 0 | 31.3 → 45.5 | 31.1 → 31.4 |
| failed | 1 | — → 1,039.7 | — → 33.3 |
| failed | 4 | — → 1,036.1 | — → 40.7 |
| failed | 8 | — → 1,045.4 | — → 41.0 |

| Discovery | Sources | Cold first tool, baseline → head | Warm first tool, baseline → head | Cold all-ready, baseline → head | Warm all-ready, baseline → head |
| --- | ---: | ---: | ---: | ---: | ---: |
| fast | 0 | — → — | — → — | 1.0 → 1.2 | 0.3 → 0.3 |
| fast | 1 | 85.0 → 88.5 | 57.5 → 56.9 | 83.4 → 86.8 | 55.2 → 56.0 |
| fast | 4 | 54.7 → 117.2 | 51.8 → 104.8 | 52.5 → 115.5 | 49.7 → 118.1 |
| fast | 8 | 101.0 → 218.9 | 101.6 → 102.7 | 99.7 → 216.9 | 99.5 → 217.6 |
| slow | 0 | — → — | — → — | 0.2 → 0.3 | 0.3 → 0.3 |
| slow | 1 | 1,534.9 → 1,536.9 | 1,534.4 → 1,539.7 | 1,531.9 → 1,535.6 | 1,531.4 → 1,538.5 |
| slow | 4 | 1,539.0 → 1,554.1 | 1,534.3 → 1,569.4 | 1,536.4 → 1,594.8 | 1,531.9 → 1,613.1 |
| slow | 8 | 3,069.6 → 1,561.7 | 3,070.9 → 1,561.9 | 3,066.7 → 3,158.5 | 3,067.9 → 3,162.4 |
| failed | 0 | — → — | — → — | 0.3 → 0.2 | 0.1 → 0.3 |
| failed | 1 | — → — | — → — | — → — | — → — |
| failed | 4 | — → — | — → — | — → — | — → — |
| failed | 8 | — → — | — → — | — → — | — → — |

A dash means no successful request/dispatch/readiness was observed. Failed baseline composition emits no model request; optional failures on the changed implementation still permit the scripted request and never dispatch a failed tool. The 0-source rows have no tool to dispatch. Warm hints eliminate the optional source wait before the first request, while first use still waits for live validation. Cold slow sources consume one shared grace period; discovery continues afterward under the four-source bound.

In this run, the fast cold path also has a measurable cost: four-source first
model request time rises from 91.0 to 149.5 ms, and eight-source time from 146.0 to
277.9 ms. Eight slow sources reach complete readiness at about 3.16 seconds on
the head versus 3.07 seconds on the baseline. The benefit is earlier model
admission and earlier use of an individually ready source, not faster complete
discovery in every scenario. These fixtures do not isolate the cause of the
fast-path overhead.

Every nonempty HTTP row performs exactly one handshake and one `tools/list` per
source, including warm hints. Eligible hint hits equal the source count for warm
head rows and zero otherwise; peak controlled source phases never exceed four.
Each successful nonempty source fixture dispatches exactly one tool call; failed
and empty cases dispatch none. The real runtime emits one model request in all
24 head scenarios and in the 18 baseline scenarios whose composition succeeds.

| Unchanged #498 workload | Sources | Baseline composition | Head composition | Baseline/head peak phases |
| --- | ---: | ---: | ---: | ---: |
| mcp_http | 1 | 274.2 | 287.6 | 1 / 1 |
| mcp_http | 4 | 238.9 | 233.8 | 4 / 4 |
| mcp_http | 8 | 471.7 | 475.7 | 4 / 4 |
| python_scripted_build | 1 | 366.7 | 367.7 | 1 / 1 |
| python_scripted_build | 4 | 366.3 | 367.3 | 4 / 4 |
| python_scripted_build | 8 | 735.5 | 736.0 | 4 / 4 |

The unchanged #498 workload measures eager HTTP preparation and scripted Python build scheduling. Python phase timings use a controlled runner; total composition also includes the expected failed MCP launch/settlement after the interactive supervisor starts. Python process-spawn counts are not measured. These single shared-host samples are mechanism evidence, not a general latency guarantee. Network request counts, cache hit counts, first-use wait, composition timing, zero spawned HTTP server processes and exact binary/source identities are preserved in the JSON files.

## Validation

The final native suite passed **5,439 tests** across 20 target results, with zero
failures and 17 ignored tests; all three timing fixtures were then run explicitly
on both revisions. Clippy with all targets/features and denied warnings, format
checking, Linux test-lane coverage, and final protocol regeneration passed.

Final Web tests passed 3,234 cases (four skipped), and mandatory-emulator TUI
tests passed 1,192 (one macOS-specific case skipped on Linux). Protocol, Web, TUI
and development typechecks, development tests, Python CI suites, localization,
source/artifact provenance, production Web build and Office sandbox admission
also passed.

The complete browser run before the final native recovery/scope corrections
executed 405 cases: 404 passed and one existing console-save visibility deadline
failed. On the final native code, the exact unchanged console scenario and seven
related configuration/readiness cases passed 8/8 in the pinned Chromium
container. This is recorded as a complete run plus a successful focused rerun,
not a fully green 405-case run on the final source. No deadline was changed.
Earlier Web PTY and native dependency/network failures, their unchanged reruns,
local Node/CI version differences, Linux-only scope and log hashes are retained
in [validation.json](evidence/validation.json).

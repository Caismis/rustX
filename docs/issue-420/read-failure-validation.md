# Live durable read failure recovery

## Starting state and scope

Worktree `/home/caismis/Documents/codes/rustX-issue-420`, branch
`issue-420-incremental-projection-chat-stability`. Before editing, `git fetch origin`
confirmed local/remote/PR head `b7a2768b727fbd8a6c57430f4a6a6c28e14fa1eb`,
`origin/main` `ad863a24cf48fbb0d5182746e1046d4b64e8c167`, a clean implementation
worktree, and PR #423 open/non-draft against main with auto-merge disabled.
The primary development worktree was not modified.

One pre-edit CI inspection of run 36329407990 showed Linux Boundary, Linux
deterministic contracts, protocol, format/lint and TUI passed; Web and macOS
boundaries were still pending. No later CI result is inferred from that inspection.

Production changes are confined to Runtime Client host/projection. The successful
candidate remains request-owned at C. It additionally offers its successful result
to the existing live installer after folding queued semantics. The same exact
revision/prefix fence, no-unpublished condition and live/closed/shutdown checks
apply. Failure to install cannot fail or modify the completed candidate.

The installer accepts a current dirty **or failed** cut, and rejects exhausted,
obsolete or already-established cuts. Matching success clears failure/dirty;
only changed values publish ReadDomainsUpdated. A new invalidating revision
clears the previous failure and becomes dirty. A late failed read cannot poison
an already-established cut. See [read-domains.md](read-domains.md) for ownership.

## Deterministic regressions

- `authoritative_snapshot_repairs_idle_background_read_failure`: one-shot fault
  at the real blocking materializer's read boundary; real attachment subscription
  observes ResyncRequired. No further semantic event occurs before an authoritative
  snapshot succeeds and repairs the idle live cut. The same attachment subscribes
  after the captured cursor and resumes delivery without reconnect or another read.
- `historical_candidate_cannot_repair_newer_dirty_or_failed_cut`: capture C,
  deterministically advance to N inside the materialization hook; test both dirty
  and failed N. C returns unchanged, cannot clear N's state or publish a false
  update; a matching subsequent candidate repairs N.
- `read_failure_is_superseded_by_a_new_dependency_revision`: C fails, N invalidates,
  N has a clean repair opportunity, stale C cannot install, N succeeds. A late
  failure for the established N is rejected.

No sleeps, increased liveness deadlines, client retry workaround or mutation
replay were added. Existing process regression sources are unchanged. Background
cancellation and actual read-task join are unchanged; the test-only read hook now
can return a one-shot fault as well as block at the existing cancellation boundary.

## Validation

All commands run in the implementation worktree. Native commands use
`CARGO_BUILD_JOBS=2` to bound compilation only; test/Tokio concurrency and ordinary
Web concurrency are unchanged. Logs: `/tmp/issue420-read-recovery-*.log`.

| Command | Result |
| --- | --- |
| `cargo test --all-features --lib read_failure -- --nocapture` | 3 passed, including idle public-subscription recovery and new revision recovery |
| `cargo test --all-features --lib historical_candidate_cannot_repair_newer_dirty_or_failed_cut -- --nocapture` | 1 passed, both newer-cut states |
| `cargo test --all-features --lib runtime_client::` | 346 passed on final tests |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --lib boundary_suites::managed_selection::fastmcp4_availability_selection_request_and_invocation_share_one_authority -- --nocapture` | Isolated rerun passed (130.52s) |
| `cargo test --all-features --test process runtime_process::the_process_serves_a_real_conversation_runtime -- --nocapture` | Passed unchanged |
| `cargo test --all-features --test process app_server_reference_host_two_users_and_external_crash_recovery -- --nocapture` | Passed unchanged |
| `cargo test --all-features --test process app_server_process_exits_with_cancelled_blocked_presentation_read -- --nocapture` | Passed unchanged; no release before process exit |
| `cargo test --all-features --test process app_server_websocket_drain_supervises_active_root_and_cold_resume -- --nocapture` | Passed unchanged |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed on final source |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features` | Rerun: 3,965 passed, 1 failed, eight existing ignored; explicit PyPI network failure below |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 418 passed across seven targets |
| `cd protocol/app-server && pnpm generate && pnpm check && pnpm typecheck` | Passed; generated files unchanged |
| `cd tui && pnpm typecheck && pnpm test` | Passed; 852 tests |
| `cd web-console && pnpm typecheck && pnpm build && pnpm test && pnpm check:i18n && pnpm check:provenance` | Passed; 1,080 tests in 62 files at ordinary concurrency |
| `cd web-console && RUSTX_PERFORMANCE_OUTPUT=/tmp/issue420-read-recovery-performance.json pnpm test:issue-420-performance` | Passed; fixed jsdom counters exactly unchanged |
| `cargo build --bins` | Passed before E2E |
| `cd web-console && CONTAINER_ENGINE=podman RUSTX_E2E_PREVIEW_PORT=15473 RUSTX_E2E_FIXTURE_PORT=15474 pnpm test:e2e` | 125 passed (7.6m) |
| `git diff --check` | Passed |

The native focused suite retains candidate/replay, real JournalBatch Trace,
pending publication and shutdown/cancellation coverage. Browser validation uses
Podman and separate local ports; test concurrency and deadlines are unchanged.
Both jsdom and final Chromium measurement JSON values exactly match committed
`evidence/after` records; fixtures and recorded metrics are unchanged.
No local macOS execution or new hosted CI result is claimed.

The first full native run passed 3,353 library tests and failed only
`boundary_suites::managed_selection::fastmcp4_availability_selection_request_and_invocation_share_one_authority`
at `tests/boundary/managed_selection.rs:89` with
`ToolActivation("selected Agent capability cannot be admitted: Tool(SourceUnavailable { selector: \"source:python:healthy/ping\", source: ManagedPython(\"healthy\"), reason: Unavailable { reason: \"source preparation failed\" } })")`.
There is no exact network diagnostic, so this is not labeled a PyPI limitation.
No managed-source implementation, assertion or deadline was changed. The failed
run is preserved in `/tmp/issue420-read-recovery-full.log`; final rerun results are
listed separately above.

The full rerun passed all 3,354 library tests and the integration targets before
`tools`. That target passed 129 and failed only
`uv::production_uv_materializes_a_managed_package_environment` at
`tests/tools/uv.rs:43`. Exact cause from `/tmp/issue420-read-recovery-full2.log`:

```text
uv lock --no-config exited with code Some(2)
Request failed after 3 retries in 34.8s
Failed to fetch: https://pypi.org/simple/fastmcp/
client error (Connect)
tcp connect error
Network is unreachable (os error 101)
```

This is a verified local network limitation, distinct from the first run's generic
source-preparation diagnostic. The full command is recorded as failed, not passed;
no tests, deadlines, network requirements or assertions were weakened. The
subsequent external-boundary command passed all 418 tests, including this UV test;
that does not retroactively make the earlier full command pass. The full command
stopped at tools, so later targets from that invocation are not claimed as run.

During development, upgrading the first regression to the public attachment API
initially missed qualification of the protocol-version constant. That test compile
error was corrected; it was not a runtime or environment failure.

## Architectural self-review

1. An idle failure is repaired by a matching successful authoritative read, with
   no new semantic event required. Store failure itself remains explicit.
2. Failure belongs only to the current durable dependency fence. New invalidation
   clears it and marks a new repair opportunity; unrelated/Trace-only facts do not.
3. Both owners use `install_read_domains`; no second failure/install state exists.
4. A stale candidate cannot clear failure/dirty, overwrite data or publish an update.
5. Candidate success still returns historical C while live progress continues.
   Optional installation may append a later derived event; replay owns C+1...N.
6. Replay eviction still returns typed ResyncRequired at subscription. There is
   no string-based control flow or new protocol/error vocabulary.
7. Inspection does not repair live state. Exhaustion, shutdown, closed delivery
   and unpublished receipts retain their existing fencing boundaries.
8. Store I/O is outside projection synchronization; cancellation, SQLite policy
   restoration and actual worker/read join are preserved. No global Tokio change.
9. Trace frontier/invalidation, Web/TUI recovery and mutation non-replay, Chat's
   RAF writer/message seat/selectors and separate measurement lane are unchanged.
10. #419/#422 startup, FirstSubmissions, ACK and generation code is unchanged.

Only the existing PR/branch is updated. After push there is one bounded PR/head/state
verification, no CI watching or polling, and no merge.

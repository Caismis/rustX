# PR #416 integration with main #423

## Repository and topology

- Repository: `Caismis/rustX`.
- Worktree: `/home/caismis/Documents/codes/rustX-issue-411`.
- Branch: `issue-411-jobs-continuable-subagents`.
- Starting PR head: `a9bbbc5f0de892b127f7eef7b02a2f40dcd5666f`.
- Starting fetched main: `a64e8ae79b2fa03da87d9995038670f179434845`.
- Previous merge base: `ad863a24cf48fbb0d5182746e1046d4b64e8c167`.
- Normal merge commit: `32b35c185dc956f58480e2055c31e6b37ca254ce`, with the starting PR head and fetched main as its two parents. No reviewed history was rewritten.

The live starting PR was open, conflicting, 19 ahead / 1 behind main, with no auto-merge. Its prior run `36358149873` failed Linux boundary and macOS checks independently of the main advance.

## Conflict ownership

The complete 152-path unmerged-index inventory is [pr-416-integration-conflicts.txt](pr-416-integration-conflicts.txt). It includes the old and new paths of the two generated rename/rename conflicts. Most textual conflicts were current-generation imports, fixture versions, and WebSocket subprotocol assertions; resolving those did not choose an older implementation.

| Area | Final owner and resolution |
| --- | --- |
| Runtime Client | Main's incremental projection, finite read-domain candidates, contiguous cursor replay, exact read-failure fences, independent Trace invalidation and attachment fencing. Jobs and Agents fold through this same engine. Runtime Client remains v52. |
| App Server | One v27 source-defined protocol containing both incremental/read-domain fields and Job/Agent methods, events, snapshots and exact wait results. Rust-generated schema and generated TypeScript are canonical; v24/v25 generated artifacts are removed. |
| Durable store | SQLite's finite presentation reader, cancellation and close hooks coexist with Agent authority/admission methods. Main's rusqlite hooks feature is retained. Canonical history and journal ordering remain native store responsibilities. |
| TUI | Main's incremental/read-on-demand path, plus durable Agent and finite Job controls. The exact-request no-replay guards and bounded lanes remain; there is no older snapshot polling path. |
| Web | Main's shared incremental reducer, narrow selectors, separate Activity subscription, stable message seat and deterministic scroll ownership. Activity receives Jobs/Agents/Workflows without subscribing Chat to the full Session. |
| Agent execution | Existing Agent Loop/SubagentRegistry owns lifecycle, admission, exact activation capture, cancellation, recovery and physical settlement. Clients retain presentation/control responsibility only. Workflow children remain finite. |
| Processes | Existing Bash/interactive physical ownership primitives. No change to TERM grace, signal ordering, fixed membership, Darwin fallback or positive physical proof. |

## Deterministic repairs and investigation

### Exact pre-Reserved recovery

The failing fixture unlocked Y and then made one nonblocking reconciliation attempt. Recovered proof requires acquisition of both the activation evidence locks and the allocation-namespace lock; dropping/unlocking the test's lease does not prove that the next Try acquisition succeeds. Concurrent fork/exec may temporarily retain CLOEXEC descriptors. A failed Try leaves Y pending while X already owns the parked shared worker.

The existing reconciliation fixture seam now permits positive `prove_after_release` acquisition for explicitly named exact activations, only under `cfg(test)`. It retains the proof through the real reconciliation/settlement path. The test checks Y's removal from both pending and inflight, completion of B's captured wait/interrupt, and X's continued exclusion of Goal idle. X is independently released afterward. Production proof policy and `wait_recovery_settlement_for` are unchanged.

### Interactive pre-anchor conclusion

The historical macOS stall did not reproduce in the initial exact Linux run. The old fixture only waited for the final settlement, so its failure log cannot identify a stalled owner phase. No unsupported production root cause is claimed.

The trusted `InteractiveTestControl` reap-failure channel now carries a fixture socket. Fixed-label handshakes causally establish inner connection before injected setsid failure and entry into the outer's `conclude_pre_anchor`. The fixture releases each boundary explicitly. It observes the failure-write result and outer conclusion, then checks the driver's exact injected-failure receipt, control EOF, direct-child wait/reap, and settlement publication. A final wait failure reports the observed boundaries. The owner must publish `TerminalityUnproven`; neither `NoOwnership` nor group terminal proof is permitted. All existing deadlines are unchanged. The unresolved inner remains test-owned cleanup, never fabricated physical proof.

This is a deterministic fixture/observability repair, not evidence that the historical macOS stall's production cause has been established. Final hosted macOS evidence is still required.

### MCP routing headers

The exact integrated test passed once through Cargo and in 20 independent direct test-binary runs. The test now explicitly checks the `x-mcp-header` annotation on the schema returned by the same runtime generation used for the call. The rustX HTTP ownership wrapper forwards SDK custom headers verbatim and has no tool-schema cache. The pinned rmcp cache belongs to each transport worker, not to the repeated fixture `McpServerId` string.

Inspection found a candidate in rmcp 3.2.0: its normal SSE-message path caches tools/list metadata before delivery, whereas `drain_queued_stream_messages` forwards queued messages without that cache operation. This is a hypothesis, not a demonstrated explanation of run `36358149873`. No manual header injection, SDK logic duplication, dependency workaround or weakened assertion was added. The historical failure remains unexplained unless reproduced with causal evidence.

### Native capture synchronization

Regenerating main's native equivalence fixture exposed a separate test assumption: `AttemptSettled` can precede publication of the independently materialized decorated transcript. The capture now waits for the terminal response's `read_domains_updated` event before acquiring its final snapshot. The framed detach/reconnect contract likewise waits for that publication before asserting that the cursor is unchanged. It still verifies a contiguous wire suffix against an independently acquired native snapshot; no production read semantics or equivalence assertions were weakened.

### Workflow provider-gate ownership

The full conformance run exposed an aggregate deadline assumption in the shipped repair fixture: waiting for writer N's provider gate also covered writer N−1's physical settlement, checker, and workspace transitions. Temporary native Workflow-event diagnostics showed these independent stages progressing before the next writer started. The fixture now consumes `WorkflowsUpdated` to synchronize a monotonic writer-start watch before beginning the provider gate wait. The existing native event-loop and provider timeout values are unchanged; there are no sleeps, retries, or production Workflow changes.

## Integration coverage

- Native projection: Agent and Job observations preserve the read-domain fence, share contiguous cursors, retain coherent state in a captured snapshot candidate, and replay later activation/Job updates without mutating the earlier capture or publishing unrelated Trace/read-domain events.
- Web client: Agent/Job updates require no Session snapshot or Trace RPC; a cursor gap produces one resync, then exact replay resumes from the authoritative cut. Duplicate cursors cannot replace a later activation with an earlier one. A held exact Agent wait survives the same resync/replay, completes with its original activation, and is sent only once. `read_domains_updated` preserves Job/Agent state without Trace reads.
- TUI: captured Job/Agent state survives replacement/resync and subsequent incremental updates preserve earlier activation captures and transcript identity.
- Chat: Agent activity and later activations preserve the native message row, Assistant element, and expanded reasoning seat through canonical completion.
- Protocol: exhaustive v27 event vocabulary includes `read_domains_updated`, `job_updated` and `agent_updated`; conformance explicitly rejects v25 and v26 as well as an older version. Only the current generation is accepted.

## Preserved trust and physical boundaries

The [supervisor-environment ownership contract](pr-416-supervisor-environment-ownership.md) remains intact. Ordinary ToolEnvironment entries are encoded command data, applied only to Bash/server spawn after clearing the child's environment; private controls are configured by the runner/test owner. Same-named child variables cannot arm diagnostic gates, tracing, faults or physical authority. The interactive fixture socket uses an already-private test-control key, not command environment input. Physical continuation remains typed/private descriptor authority.

Exact activation recovery, pre-Reserved physical allocation transfer, `recovery_unreserved`/`recovery_pending`, consumed ordinals, workspace fencing, deletion proof and Goal-idle exclusion remain native ownership responsibilities. This integration does not restore Execution compatibility, duplicate projections, old protocol generations, retries, larger timeouts or broad suite serialization.

## Validation

All required local test targets have passing final results. The combined external command initially stopped at conformance; the same remaining targets and the corrected conformance target were then run explicitly, without changing test concurrency. Historical diagnostic failures are recorded below.

| Command | Final local result |
| --- | --- |
| `cargo test --lib --all-features pre_reserved_wait_settles_independently_of_another_agents_recovery -- --nocapture` | 1 passed |
| `cargo test --lib --all-features recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop -- --nocapture` | 1 passed |
| `cargo test --lib --all-features pre_reserved_allocation_blocks_destructive_deletion_until_exact_proof -- --nocapture` | 1 passed |
| `cargo test --lib --all-features unprovable_pre_anchor_reap_never_settles_physically -- --nocapture` | 1 passed |
| `cargo test --lib --all-features command_environment_cannot_configure_interactive_supervisor -- --nocapture` | 1 passed |
| `cargo test --all-features --test tools bash::ordinary_environment_cannot_arm_supervisor_term_gate -- --exact --nocapture` | 1 passed |
| `cargo test --all-features --test tools bash::ordinary_environment_cannot_enable_supervisor_controls -- --exact --nocapture` | 1 passed |
| `cargo test --all-features --test tools bash::background_cancel_records_term_before_trap_and_physical_terminal -- --exact --nocapture` | 1 passed |
| `cargo test --all-features --test tools bash::bash_background_cancellation_uses_the_same_process_group_path -- --exact --nocapture` | 1 passed |
| `cargo test --all-features --test tools bash::bash_kill_escalates_when_term_is_ignored -- --exact --nocapture` | 1 passed |
| `cargo test --all-features --test tools mcp_runtime::unix_tests::modern_streamable_http_is_stateless_and_forwards_sdk_routing_headers -- --exact --nocapture` | 1 passed |
| `cargo test --lib --all-features runtime_client::` | 354 passed |
| `cargo test --lib --all-features runtime::subagent::` | 222 passed |
| `cargo test --lib --all-features runtime::interactive_process::` | 20 passed |
| `cargo test --lib --all-features tools::native::bash::tests` | 37 passed |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3211 passed, 3 existing opt-in tests ignored |
| `cargo test --test contracts --test provider --all-features` | 28 passed; 166 passed, 5 existing opt-in tests ignored |
| `cargo test --lib --all-features -- boundary_suites::` | 194 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools` | 129 durable, 63 process, 45 subagent, 133 tools passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test conformance` | 22 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 26 cfg3_catalog and 5 cfg3_managed_output passed; initial conformance failure stopped this invocation (see diagnostics); all remaining targets passed separately |
| `cargo test --lib --all-features agents_and_jobs_share_incremental_cursor_and_captured_snapshot_cut -- --nocapture` | 1 passed |
| `RUSTX_PROJECTION_CAPTURE=web-console/test/fixtures/incremental-native.json cargo test --lib --all-features incremental_projection_independent_snapshot_capture -- --nocapture` | 1 passed; native fixture regenerated |
| `cargo run --example generate_app_server_protocol` | passed |
| `pnpm --dir protocol/app-server generate` | passed; sole v27 schema/types/fixtures generated from Rust |
| `pnpm --dir protocol/app-server check` | passed; no generated drift |
| `pnpm --dir protocol/app-server typecheck` | passed; schema contracts also covered by Rust/TUI suites |
| `pnpm --dir tui typecheck` | passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 892 passed |
| `pnpm --dir web-console typecheck` | passed |
| `pnpm --dir web-console test` | 1,127 passed; final extended exact-wait replay case also passed in its 24-test file |
| `pnpm --dir web-console exec vitest run test/incremental-equivalence.test.ts test/incremental-vocabulary.test.ts` | 2 passed against regenerated native capture |
| `pnpm --dir web-console test:issue-420-performance` | 1 passed |
| `pnpm --dir web-console check:i18n` | passed |
| `pnpm --dir web-console check:provenance` | 143 source records and 131 production-package notices passed |
| `pnpm --dir web-console build` | passed |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | 127 passed in the repository-pinned browser container; includes incremental/scroll and Agent lifecycle scenarios |
| `pnpm --dir dev typecheck` | passed |
| `pnpm --dir dev test` | 37 passed |
| `uv run --frozen pytest (in test-support/fake-provider)` | 51 passed |
| `cargo fmt --all -- --check` | passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | passed |
| `cargo build --bins` | passed |
| `git diff --check` | passed |
| `git diff --cached --check` | passed |


## Failed diagnostic runs (retained, not hidden)

- Initial all-target Rust check found two merge integration errors: the changed `SubagentLifecycle` struct variant pattern and an attachment binding still named `_attachment` despite use. Both were corrected before tests.
- Initial client typechecks found obsolete event names in main's exhaustive vocabulary, unused imports after narrow-selector integration, optional-array accesses in new tests, and one optional spread in the TUI fixture. Canonical v27 names and correctly typed fixtures resolved them.
- The first native Job/Agent regression used a noncanonical test ID and failed the UUIDv7 constructor invariant. It now uses canonical identities.
- The first new Web replay assertion compared object identity across a wire snapshot replacement; the first full TUI run similarly compared two independently normalized snapshots. Assertions now compare the installed snapshot's references across subsequent incremental events. No production assertion was removed.
- Two intermediate Rust test compilations failed for missing trait/type imports in the new fixture/helper (`AsyncReadExt`, qualified `SubagentId`). Corrected.
- Intermediate Clippy runs caught a documentation backtick, a long reconciliation body, test import placement/length and a generic default invocation. The existing test seam was extracted, test imports/defaults corrected, and the single causal scenario's length explicitly documented.
- Regenerating the native capture caused two Web test failures (equivalence and vocabulary): the capture omitted the independently published decorated read-domain suffix. The causal capture repair is described above; regenerated native equivalence passes without weakening the comparison.
- The first complete Runtime Client group had 353 passes and one framed reconnect failure (`14` versus `13`): independently published read-domain work advanced the cursor after the test's assumed completion point. The test now waits for the actual publication; the entire 354-test group passes.
- Browser E2E initially stopped because `docker` was not installed. The repository-supported `CONTAINER_ENGINE=podman` path ran the same pinned browser image and all 127 tests passed. No screenshot references were updated.
- Two typecheck invocations were accidentally launched relative to `test-support/fake-provider`; pnpm rejected the nonexistent relative directories. Both were rerun successfully from the worktree root.

No test timeout was increased, no assertion was weakened to accept a failed ownership outcome, and no suite retry/serialization workaround was introduced. Targeted reruns followed source or fixture corrections; the independent MCP repetitions were diagnostic sampling, not an eventual-pass criterion.

Additional broad-validation diagnostics:

- The first in-crate boundary run had 193 passes and one managed-Python source-preparation failure. A temporary fixture-only availability print exposed the underlying state on subsequent focused and full diagnostic runs; all observed healthy-source preparations were Ready, and all 194 boundary tests passed in the diagnostic run. The temporary production-source edit was removed. The original failure's cause remains unconfirmed; no source-preparation workaround was added.
- The combined external command passed both CFG targets, then stopped at conformance (21 passed, one provider writer-gate timeout), so it had not reached the remaining four targets. Those were explicitly run afterward: durable 129, process 63, subagent 45, tools 133 passed. Workflow diagnostics and the causal fixture correction are described above.

The final normal in-crate boundary run passed all 194 tests after temporary diagnostics were removed. The corrected full conformance run passed all 22 tests. No unresolved failure was silently omitted from the table above. The earlier managed-source preparation failure remains unexplained despite these subsequent diagnostic/final results.

Hosted CI is separate evidence. Local success does not establish merge readiness: the pushed final SHA must pass required hosted checks, including macOS. PR #416 remains open, with no merge or auto-merge action authorized or performed. The final pushed SHA and one current CI snapshot are reported with the completion message rather than embedding a self-referential commit hash in this file.

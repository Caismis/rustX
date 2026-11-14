# Workflow Run Canvas validation

The run view uses the existing Workflow canvas and native trace/value inspection.
It never reads the current Workflow file to reconstruct an older run. SQLite schema
88 intentionally refuses older databases without a migration or compatibility path.

## Contract and evidence

| Boundary | Evidence |
| --- | --- |
| Definition and presentation come from one admission | Resource-load and concurrent-admission tests capture the exact parsed source revision using canonical workspace/user-root authorities (matching `validate_project_resource_path` without fallback, including symlinked-alias boundary rejection tests). Resident layout-only republishing retains the executable digest while each run keeps its own presentation. |
| Publication is atomic | Ordinary `JournalStore::append_event` refuses `RuntimeEvent::WorkflowStarted` without leaving event or snapshot rows; `publish_workflow_start` SQLite transaction failure tests and external-provider `SIGKILL` tests cover precommit rollback, postcommit complete retention, and interruption with a held real child. There are no staging rows. |
| Conversation, activation and cut own access | App Server tests exercise Header/Chunk and cold `session/workflowTraceNode` (`ordinary()` spawned `History` ownership transfer retaining `ServerOperation` and `Association` across waiter cancellation and EOF), live and cold reads, cross-conversation and wrong-branch denial, absent/stale/future cuts, detached targets, child inspection and denial after warming the encoded-document cache. |
| Run lookup is bounded and readonly | `session/traceLocateWorkflow`, `agent/traceLocateWorkflow`, and `TraceInspectionContent::Workflow` in `trace.records.<seq>.detail` use existing indexes and finite record detail, expose exact typed `(run_id, workflow_id)` even for `WorkflowStarted`-only roots with empty `workflow.nodes` and absent display `native_id`, and do not scan the complete Trace. SQL VM work for `traceLocateWorkflow` and `workflowTraceNode` stays constant across 2,048+ unrelated runs; escaped long tool origins and scheduled origins remain locatable without a display `native_id`. |
| Start-only and late visits remain inspectable | Typed Workflow detail supplies run identity with zero node headers and omitted display IDs. Exact indexed point reads retain a selected late visit across new cuts without automatically paging all intervening history. |
| Concrete visits stay distinct | Unit tests distinguish run origins, qualified definition paths, arbitrary Start IDs, nested invocations (root-inclusive depth 8: up to 16 path indices and 9 invocation entries, rejecting malformed or one-over-bound instances) and visit numbers. Selected visits beyond the loaded two-page prefix or across advancing cuts resolve via bounded `conversation/workflowTraceNode` / `session/workflowTraceNode` without reusing old-cut headers, auto-paging, or fabricating `visit=0`, including exact-vs-paged equality for the admitted depth-8 `start_d8` visit at the same read cut. |
| History does not imply a live owner | Unsettled records require an exact current run and instance to display Running/Waiting. Ownerless history and unsettled instances of a settled run display Incomplete. Settled outcomes remain intact. |
| I/O and children keep native authority | The canvas uses the same original-body reader as Trajectory. Values load only for the selected Input/Output tab (zero eager body reads across graph load, paging, cut advancement, and exact visit header lookup). Held old-visit/old-cut/old-activation successes, values and expired-cut errors cannot replace or pollute the current selection. Historical child references come from exact same-cut native admission/ownership facts; opening a child uses the existing authorized route. |
| Immutable graph and mutable editor stay separate | Rendered-editor tests retain invalid JSON drafts, undo/redo, selection, field conflicts, source conflicts and uncertain saves across graph interaction and closure. No save, reconciliation or source reread is triggered by run inspection. |
| View lifetime and camera are stable | Paging and ordinary live updates preserve the mounted graph, pan/zoom and focus. Explicit owner-current closure restores the opener; conversation/attachment/activation retirement fences retained callbacks and releases the modal without focusing an obsolete opener. |
| Browser presentation | Pinned Linux Chromium exercises keyboard activation, focus containment, narrow and wide layouts, EN/ZH, light/dark, unavailable snapshots, paging, the real rendered authoring editor, and Chat/Trajectory over a real native App Server. |

## Resource bounds

- Native snapshots use the existing 16 MiB document ceiling and 65,536-byte chunks.
  The wire reader checks its 32 MiB ceiling, offsets, continuity and SHA-256 before
  parsing. Encoded native documents use a four-entry cache; authorization precedes
  every cached response. Browser derived layouts use a sixteen-entry cache scoped
  to the view and cleared on disposal.
- Exact `conversation/workflowTraceNode` / cold `session/workflowTraceNode` reads
  return one native node header at the authorized cut; they reuse the run/instance
  indexes and keep original bodies lazy.
- Initial node inspection reads at most two 32-header pages. Further pages require
  an explicit action. Original value/content reads are not part of graph loading.
  Per-node visit display retains at most 200 entries, including the exact selection.
- A shared fixture is compiled by the native compiler before being used as scale
  evidence: 256 nodes at depth eight, with nested Loops, Templates and Branches.
  A separate admitted 42-node repeated graph retains 3,903 concrete visits within
  the 4,096-step limit. These are separate bounds rather than an inadmissible
  combination of maximum static nodes and maximum repetition.
- Scale tests measure reassembly, deterministic ELK layout, scene projection and
  status updates, assert one layout per immutable identity, bounded caches/pages,
  zero eager value reads, and release on disposal. Timing is reported as a
  measurement; correctness does not depend on a machine-specific time threshold.

## Verification record

Commands run from the worktree root unless a directory is shown. The final PR
records completed command results and any remaining platform/CI limits. A test
count alone does not substitute for the ownership, crash and work-bound witnesses
above.

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features`
- `cargo test --lib --all-features issue552_`
- `cargo test --lib --all-features inspection_bounds_fixture_is_admissible`
- `cargo test --lib --all-features durable::sqlite::workflow_trace`
- `cd protocol/app-server && pnpm typecheck`
- `cd web-console && pnpm typecheck && pnpm check:i18n && pnpm check:provenance`
- `cd web-console && pnpm build && pnpm test`
- `cd web-console && bash scripts/browser-tests.sh test/e2e/workflow-run-canvas.spec.ts test/e2e/workflow-trajectory.spec.ts test/e2e/workflow.spec.ts`

Browser references use the repository's immutable Playwright 1.63.0 Linux image.
macOS and the remaining browser shards require their own current-head CI results;
local focused evidence must not be presented as a completed remote CI run.


Synthetic future-cut denial tests use the protocol's maximum safe counter bound,
rather than a small increment that can become valid during concurrent publication.
Both Header and Chunk continue to require denial; valid pre-run cuts still return
absence. The earlier failed fixture run and corrected final results are recorded
with the PR validation evidence.

# Auxiliary execution implementation plan

## Scope and development baseline

Baseline checked: 2026-10-18. Feature stages below remain pending beyond the synchronized Agent ownership foundation.

Implement the auxiliary series as four working increments: [AUX-01 #473](https://github.com/Caismis/rustX/issues/473), [AUX-02 #474](https://github.com/Caismis/rustX/issues/474), [AUX-03 #475](https://github.com/Caismis/rustX/issues/475), and [AUX-04 #476](https://github.com/Caismis/rustX/issues/476). The issues own their detailed acceptance contracts; this document defines implementation order, code ownership, integration boundaries and verification gates.

Use one development branch, `MiNisterPaX/rustX:aux-01-request-ownership`, for all stages. Fetch dependencies from `origin` (Caismis/rustX); push development commits to `fork` (MiNisterPaX/rustX). Preserve unrelated workspace changes. Do not create parallel long-lived AUX development branches or rewrite the existing published history. Each issue remains a separately reviewable, complete product increment; do not merge a half-converted owner/schema/reader transition. Reuse this branch for successive delivery gates, integrating actual upstream main between them.

The synchronization baseline is upstream commit `3c355f152d405a201482d53b6e45497df4f44a7f`, including immutable-history compaction, bounded observation, CI improvements and WAL/NORMAL. The fork's starting commit is `c26fc0d00fd1b60a7505888ef3b01893939ff88d`, containing:
- `8a4cbe8b29d861144d4d689e04b3903886843112`: auxiliary identities and closed request ownership.
- `c26fc0d00fd1b60a7505888ef3b01893939ff88d`: Agent ownership persistence, lookup and validation.

Those commits are foundations, not completed AUX-01. They do not implement Call lifecycle transactions, mixed-owner recovery/readers, or production auxiliary execution. Synchronization must retain upstream native selected-input manifests, bounded request probes, observation readers, WAL/archive cuts and optional-Source failure isolation while adapting the fork's Agent ownership checks. Do not restore the old whole-history response loop or snapshot-loading implementation to resolve conflicts.

[Storage issue #481](https://github.com/Caismis/rustX/issues/481) and [PR #484](https://github.com/Caismis/rustX/pull/484) remain separate dependencies to coordinate. #484 was unmerged at planning time. Its source-page codec and canonical append certificate are not included in this synchronization. Record its actual merged SHA when integrating it later. Do not reserve final schema numbers from its draft.

The synchronized ownership foundation uses SQLite development schema 70 and inspection archive v5 / Journal schema 3. Runtime Client 71 and App Server 50 retain their existing domains. These are the branch baseline, not reserved versions for subsequent AUX stages or #484 integration.

## Synchronization evidence

The synchronized code baseline is merge commit `141d31853de7413f51c88ec98c81c877aacaa505`, retaining fork head `c26fc0d0` and upstream main `3c355f15` as its parents.

Verification on that code passed in Ubuntu 24.04 WSL with the pinned toolchain:
- Rust formatting and all-target/all-feature Clippy with warnings denied.
- 373 SQLite tests, 12 event tests, nine identity tests, one primary Snapshot serialization test and one archive-v5 contract test: 396 passed, zero failed.
- Native protocol/configuration schema generation, TypeScript regeneration and protocol typecheck. A repeated generation produced no unstaged artifact drift.
- Diff whitespace checks against the integrated upstream baseline.

These checks validate synchronization of the existing foundation. They do not complete AUX-01 or replace its final acceptance matrix. Full CI, complete process/provider/browser suites and native macOS lanes were not run for this synchronization.

The premerge Git experiment compared the fork head with main 3c355f15 and draft #484 head 4596fbb2. Both reported the same four conflicted files: docs/architecture.md, src/durable/sqlite.rs, src/durable/sqlite/reading_tests.rs and src/runtime_client/response.rs. The first identity commit merged textually cleanly into both. This evidence identified integration work; it did not prove runtime correctness or establish #484 as a required implementation base. The synchronization commit and its checks provide the resulting baseline; subsequent implementation must use its current native contracts.

## Shared contracts

- RequestId denotes one physical intent and permits at most one adapter entry; AuxiliaryCallId denotes frozen semantic input and invocation across technical retries. CompactionOperationId remains business-owned. Neither identity nor purpose text grants authority.
- All model execution requires confirmed mandatory durable start facts. Auxiliary requests use the shared physical Started/Completed/Failed family without fabricated Agent Attempt, Turn, Snapshot or Assistant identities.
- Mandatory facts and exact confirmation remain authoritative when optional Source/index/display capture fails. Unknown commit state is neither rollback nor permission to repeat inference.
- Execution outcome, domain acceptance, business commit, output retention and delivery status remain distinct.
- Native Runtime owns admission, cancellation, supervision and physical cleanup. Business consumers own preparation, validation and commits. Observer/RPC loss does not cancel admitted work.
- Local audit facts, quota and submission association survive for the Conversation lifetime. Ephemeral output expiry does not erase audit or renew eligibility. Audit, input reconstruction and output reconstruction are separate capabilities.
- Preserve current process-crash durability under WAL/NORMAL; introduce no machine-power-loss guarantee, remote exactly-once promise, compatibility layer, migration or replay scheduler.
- Keep internal consumer extension points narrow. Do not add generic purpose dispatch, arbitrary prompt/plugin authority, a second provider catalog or executor branches for each business.

## Stage 1: finish AUX-01

### 1.1 Inventory and freeze the transaction contract

Audit production paths and their test oracles before enabling mixed facts. Maintain the implementation inventory with source locations, owner, input authority, historical cut and test coverage for:

| Concern | Primary implementation locations |
| --- | --- |
| Identity and serialized request/Call facts | src/runtime/identity.rs; src/model/snapshot.rs; src/events/types.rs |
| Semantic Store operations and all implementations | src/durable/inbox.rs; src/durable/sqlite.rs; implementation and fixture searches |
| Original fact validation and indexed lookup | SQLite start, terminal, reference, schema and receipt helpers |
| Agent recovery/publication | src/runtime/recovery.rs; src/agent/execution.rs; primary P/U/C validators |
| Native observation | src/durable/sqlite/observation*.rs; request/Trace layouts; src/runtime/observation.rs |
| Runtime Client readers | checkpoint, projection, response, Trace, fields and content authorization |
| Original-fact oracles | context occupancy; Trace/detail and response test oracles |
| Lineage and inspection export | src/durable/sqlite/lineage.rs; archive owners; src/session_archive* |

Define bounded Call facts, the closed owner encoding, exact-payload identity comparison and typed confirmation evidence before writing indexes. Mandatory indexes are derived transactionally from authoritative facts. Keep shared synchronous semantic Store transitions; do not introduce detached writes or an asynchronous Store framework in AUX-01.

### 1.2 Complete semantic persistence

Implement:
1. Unique Call start with bounded trusted source attribution.
2. Request start requiring an active Call, sequential ordinal, no open request, a unique RequestId and applicable native start permission.
3. Atomic final physical terminal plus Call settlement.
4. Call-only settlement for zero-request Calls; the later retry stage also uses it for backoff cancellation after a request has settled.
5. Exact lookup/confirmation returning confirmed commit, confirmed noncommit or unknown according to actual transaction evidence.

Preserve Agent start's atomic Snapshot/native selection/Context/Started transaction and exact retry checks. Auxiliary completion must not update the primary Snapshot P marker. Raw Journal insertion cannot bypass semantic transitions. Existing fork owner checks must extend the current native validators, not replace their root, digest, count, ordering or bounded-work checks.

Charge only confirmed starts at their actual transaction boundary; uncertainty does not manufacture a free start slot. An absent row cannot prove noncommit while its writer could still commit. AUX-02 owns physical writer supervision; AUX-01 provides the exact synchronous contract it consumes.

### 1.3 Isolate every primary consumer

Classify owners before recovery mutation, aggregation, LIMIT or pagination. Auxiliary facts must not clear an unanswered adopted Agent turn, become a primary predecessor, satisfy P/U/C, count as an Agent step/request, or trigger primary Snapshot occupancy reconstruction.

Update original and native readers together. Expose bounded auxiliary Call/request audit with keyed lookup, advancing pages and immutable cuts. Preserve native selected-input sharing, Source byte bounds, field authorization, unavailable/deferred semantics and canonical-only cut advancement. Missing optional headers cannot imply absence of mandatory facts.

### 1.4 Complete lineage, archive and formats

Implement the issue's inheritance matrix: an explicitly retained Summary dependency carries its eligible source audit; temporary Classification and BTW do not become target executions merely because their input history is copied. Saved Title display metadata is separate from local execution and quota.

Inspection archives expose explicit bounded projections and omissions; they do not serialize private native types wholesale or become executable recovery formats. No later terminal may leak into an earlier export cut.

Advance SQLite, event, archive, Runtime Client and App Server format domains only where actually changed; regenerate affected schemas/TypeScript/fixtures and update normative docs together. Reject obsolete and falsely current shapes without rebuilding user data. Any branch-local schema gate used during synchronization must be reassessed after #484 integration.

### 1.5 AUX-01 acceptance gate

Use real SQLite and deterministic fault points for atomicity, duplicate/conflicting identity, orphaned terminal, wrong owner, ordinal conflict, zero-request settlement, known rollback, committed-but-lost receipt and unavailable exact confirmation. Add owner-filter negative controls and bounded-work checks.

Compare equivalent primary histories with/without auxiliary audit interleavings under controlled scheduling; primary answers, recovery, occupancy, counts and compaction eligibility must agree after excluding intentional audit coordinates. Resource contention is a separate test dimension.

Before delivery, exercise the minimal supervised real-Store/scripted-adapter consumer required by #473: CallStarted -> RequestStarted -> one adapter entry -> atomic terminal/Call settlement, with cancellation before/after dispatch, optional observation failure and actual commit followed by receipt loss. Keep the seam private and carry it into AUX-02. This fixture is not a shipped production executor.

## Stage 2: AUX-02 supervised execution and Summary

Start after AUX-01's coherent Store/reader gate. Implement the shared executor in native Runtime and replace Summary's old polling implementation.

1. Map the admission, dispatch, child registration, shutdown and durability-failure transitions to existing native owners and locks. Document each linearization point and lock order.
2. Reserve process then Conversation capacity before expensive preparation. Initial total/general capacities are 32/24 per process and 4/3 per Conversation, preserving protected Compaction capacity. Return typed Busy immediately; no task queue or hold-and-wait acquisition.
3. Freeze input, invocation, model/resource revisions and media through existing Context/native model preparation. Automatic Summary inherits its Attempt's frozen policy; independent manual Compaction freezes at admission.
4. Supervise required blocking writes and exact confirmation through physical exit. Consume the one-shot dispatch grant only after confirmed start and current execution authorization. Panic, waiter drop or deadline cannot abandon required settlement.
5. Make the absolute Call and Compaction deadline five minutes, including admission/preparation/persistence/backoff. Required cleanup may outlive inference eligibility while retaining accountable ownership.
6. Put the Compaction budget outside input shrinking and carryover degradation: 12 confirmed starts, three total input shrinks, monotonic degradation and the original invocation/deadline.
7. Preserve stream-integrity acceptance through EOF, typed provider/local evidence and observed Usage. Domain-invalid Summary is distinct from model execution failure.
8. Transfer short manager operation leases to native ownership before waiting for inference. Native drain must reach cancellation and wait for actual execution/write exit or proven-exited failure containment.

Gate: working auto/manual Summary on the shared executor; old polling removed; no tools or primary-history fabrication; exact checkpoint receipt/commit authority preserved. Test admission-vs-drain, scope close, cancellation, panic, blocked SQL, receipt loss, terminal-to-EOF anomalies and unloading with native ownership still active.

## Stage 3: AUX-03 retries, budgets and Title

Add retries to the same executor only after local-stop and confirmed settlement.

- At most two retries / three requests per Call, two-second base delay, maximum acceptable single wait 60 seconds, original Call/parent deadline unchanged.
- Compute eligibility from failure observation plus max(local delay, valid Retry-After); persistence latency consumes that delay. Never clamp provider delay downward.
- Remote uncertainty may accompany an otherwise eligible transient retry and possible duplicate inference/billing. Permanent auth/configuration/quota failures, domain rejection, invalid stream order and cancellation do not become transient retry.
- Each retry gets a new RequestId and local stream/Usage state; frozen input/invocation/media and parent allowance remain unchanged. Re-planning creates a new Call under the same business authority.
- Add Title follow-Session/explicit policy through existing defaults, native overrides, bounded settings observations and Web/TUI whole-state updates.
- Select the exact first qualifying committed Human using native indexed facts, including missing timestamps and image-only cases. Do not reuse the ordinary opening-preview selector.
- Enforce source uniqueness and request limits transactionally. A zero-start settled operation can release its exact source claim; a confirmed first start consumes automatic eligibility across restart and configuration changes. Prefer this source/Call mechanism to a redundant quota service.
- Commit generated-title metadata through Catalog's conditional authority; preserve manual name precedence, provenance, rename races, uncertain outcomes and independent Clone/Fork display copying.

Gate: frozen retries, Compaction aggregate allowances and Title lifecycle pass their real Store/Catalog and adapter tests. Synthetic repair/multi-step consumers validate restricted composition privately without adding product APIs.

## Stage 4: AUX-04 external delivery, Classification and BTW

Add named authorized business submissions over the same execution path.

1. Define bounded DTOs and versioned canonical caller-semantic encoding. Correlation is Conversation/access-scoped, independent of transport RequestId and CallId. Known matching commands return existing work before dynamic preparation; changed caller semantics conflict.
2. Carry one close-fenced submission scope through decode, routing, authorization, manager handoff and native admission. Lookup returns Known, Pending, ConfirmedNotAdmitted or Unresolved from evidence; it never cancels admission or silently resubmits.
3. Keep execution and result capacity separate. Initial retention is 16 slots / 1 MiB per Conversation, 128 slots / 8 MiB per process, 64 KiB per complete encoded result record, five-minute completion TTL. Reserve before admission; count input/generation/transport/observer copies separately.
4. Preserve read/control authority separation and eight observation/six ordinary/two reserved control request slots. Exact native cancellation remains available during failed or blocked Source reads.
5. Classification accepts bounded text or authorized history plus unique labels, and validates exactly the supported JSON object. Reject duplicate properties, extra fields, trailing prose and out-of-set labels without corrective technical retry.
6. BTW captures one committed projection/Tail cut through Context, freezes selected media/resources and preflights the real adapter. It does not mutate canonical history, execute tools or interrupt unrelated Agent work.
7. Add bounded provisional segments, attempt reset/sequence and slow-observer retirement with independently readable terminal status. TTL/discard and observer loss never renew execution or erase durable submission identity.
8. Add first-party WebUI and actual model policies; guard late responses by target/incarnation/view/attempt and bound hidden panels, retained copies and pending reads.

Gate: real transport/browser submission, lost receipt/reconnect, revocation, cancellation, slow consumer, expiry and restart tests. Measure a conservative full allocation/copy envelope; the 8 MiB retained-result budget is not an RSS guarantee. No output replay journal or auxiliary ContentRef compatibility path.

## Work before and after #484

Proceed now on this branch with AUX-01 identity, semantic transactions, recovery/reader isolation, private consumer fixtures and focused tests. Shared files require integration attention, not an idle development freeze. Do not alter Source codecs or append algorithms merely to avoid merging them.

After #484 enters main, merge that actual main commit into this same branch and:
- retain its removed active_json layout and current authenticated page representation;
- preserve append-certificate invalidation, rollback and post-COMMIT seating;
- revalidate owner-aware observation at original cuts and physically bounded small/cold reads;
- recompute actual format gates and regenerate affected contracts;
- rerun mixed-history, primary P/U/C, native selected-input, archive/Fork and Source failure tests.

If #484 remains open when AUX-01 is otherwise complete, reassess the current shared diff explicitly; do not silently treat a draft head as merged. A temporary checkout may test one pinned dependency without creating another maintained development branch.

## Validation and delivery

Read current CONTRIBUTING.md, DEVELOPMENT.md, tests/README.md and CI workflow on every updated baseline. Use Linux/WSL with pinned Rust 1.98.1 and libclang, the external offline provider emulator for composed conformance, and private scripted adapters only for owner-level state machines. No live inference credentials are needed.

For each implementation commit, run relevant owner tests and inspect actual selected test counts. At each issue delivery gate run applicable:
- cargo fmt --all -- --check; cargo clippy --all-targets --all-features -- -D warnings;
- binary builds, all required Rust contract/boundary/process lanes and test-discovery coverage;
- provider emulator tests and composed tests with RUSTX_REQUIRE_PROVIDER_EMULATOR=1;
- schema/protocol generation and drift checks, dev/TUI/Web typechecks/tests, i18n/provenance and affected real browser/transport E2E;
- final-head FULL/Merge verification and applicable macOS lanes as required by repository policy;
- git diff --check.

Do not weaken assertions, enlarge test timeouts or hide unavailable lanes. Report exact commands, selected counts, environmental limitations and failing evidence. A branch synchronization check is not the complete AUX-01 acceptance matrix.

Keep one coherent PR-sized increment per issue, with actual base/head/dependency SHAs, changed owners and commit points, removed paths, limits, contract-to-test mappings and final-head CI evidence. Push to the fork branch; do not merge upstream automatically.

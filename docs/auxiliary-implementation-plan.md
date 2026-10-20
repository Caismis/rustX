# Auxiliary execution implementation plan

## Scope and development baseline

Initial baseline synchronized: 2026-10-18; current upstream integration: 2026-10-19. AUX-01 storage and inspection, review fixes and the CI Node pin are delivered through `4aedcda2c9415cec3cfd1b06d3539215009b2e09`, whose [FULL workflow](https://github.com/Caismis/rustX/actions/runs/39391027089) passed, including macOS full and Merge verification. Stages 2–4 remain planned. The proposed [AUX-02 implementation plan](aux-02-implementation-plan.md) defines the next increment and requires user review before implementation.

Implement the auxiliary series as four working increments: [AUX-01 #473](https://github.com/Caismis/rustX/issues/473), [AUX-02 #474](https://github.com/Caismis/rustX/issues/474), [AUX-03 #475](https://github.com/Caismis/rustX/issues/475), and [AUX-04 #476](https://github.com/Caismis/rustX/issues/476). The issues own their detailed acceptance contracts; this document defines implementation order, code ownership, integration boundaries and verification gates.

Keep AUX-01 on `MiNisterPaX/rustX:aux-01-request-ownership`. Develop AUX-02 on `MiNisterPaX/rustX:codex/aux-02-supervised-execution`, starting at the AUX-01 commit above. Fetch dependencies from `origin` (Caismis/rustX); push development commits to `fork` (MiNisterPaX/rustX). Preserve unrelated workspace changes and published history. Each issue remains a separately reviewable, complete product increment; do not merge a half-converted owner/schema/reader transition. Integrate actual upstream main containing the accepted prerequisite before the corresponding upstream delivery. AUX-02 work is performed by the primary agent without subagents or delegated workers.

The initial synchronization baseline was upstream commit `3c355f152d405a201482d53b6e45497df4f44a7f`, including immutable-history compaction, bounded observation, CI improvements and WAL/NORMAL. The fork's starting commit is `c26fc0d00fd1b60a7505888ef3b01893939ff88d`, containing:
- `8a4cbe8b29d861144d4d689e04b3903886843112`: auxiliary identities and closed request ownership.
- `c26fc0d00fd1b60a7505888ef3b01893939ff88d`: Agent ownership persistence, lookup and validation.

Those commits are foundations, not completed AUX-01. They do not implement Call lifecycle transactions, mixed-owner recovery/readers, or production auxiliary execution. Synchronization must retain upstream native selected-input manifests, bounded request probes, observation readers, WAL/archive cuts and optional-Source failure isolation while adapting the fork's Agent ownership checks. Do not restore the old whole-history response loop or snapshot-loading implementation to resolve conflicts.

[Storage issue #481](https://github.com/Caismis/rustX/issues/481) / [PR #484](https://github.com/Caismis/rustX/pull/484) is integrated at upstream merge `6771928953a9b2815a27fd784f6d39c5ccde8b56`. This integration preserves AUX review commit `38d64578e9918d94566b839edef1730fc25aac27` and upstream bounded Source codecs, small-source navigation, and transaction-local canonical append certificates. SQLite schema 74 gates the combined layout; archive v7 and transport versions are unchanged.

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

## Stage 1: AUX-01 durable ownership and inspection

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

The synchronous Store transitions provide:
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

The inheritance matrix carries eligible source audit only for an explicitly retained Summary-generation dependency. Temporary Classification and BTW do not become target executions merely because their input history is copied. Saved Title display metadata remains separate from local execution and quota.

Inspection archives expose explicit bounded projections and omissions; they do not serialize private native types wholesale or become executable recovery formats. No later terminal may leak into an earlier export cut.

The implemented formats are SQLite schema 74 and inspection archive v7, with Journal schema 5, projection/generation schemas 2, and local/inherited auxiliary-audit schemas 2. Runtime Client 71 and App Server 50 are unchanged; regenerated protocol and configuration artifacts have no content drift. Obsolete and falsely current stores are rejected without migration or reconstruction. Schema 74 rejects the pre-integration main and AUX layouts without rewriting either.

### 1.5 AUX-01 acceptance gate

Use real SQLite and deterministic fault points for atomicity, duplicate/conflicting identity, orphaned terminal, wrong owner, ordinal conflict, zero-request settlement, known rollback, committed-but-lost receipt and unavailable exact confirmation. Add owner-filter negative controls and bounded-work checks.

Compare equivalent primary histories with/without auxiliary audit interleavings under controlled scheduling; primary answers, recovery, occupancy, counts and compaction eligibility must agree after excluding intentional audit coordinates. Resource contention is a separate test dimension.

The private supervised real-Store/scripted-adapter fixture exercises CallStarted -> RequestStarted -> one adapter entry -> atomic terminal/Call settlement, including cancellation before/after dispatch, optional observation failure and actual commit followed by receipt loss. It waits for physical writer/task exits before confirmation. Carry this seam into AUX-02; no production auxiliary executor is shipped in AUX-01.

Implementation details, finite bounds, confirmation rules, lineage semantics and measured storage growth are recorded in [Auxiliary execution](auxiliary-execution.md). The native Store owns transaction/index integrity; compact observation headers preserve primary readers' existing bounded-work guarantees, and the inspection archive uses explicit projections.

Initial implementation verification at `8f54e195` used Ubuntu 24.04 WSL and pinned Rust 1.98.1:

- Formatting, all-target/all-feature Clippy with warnings denied, and whitespace checks passed.
- Across the complete library run and final focused fixture checks, all 4,076 runnable non-boundary library cases were exercised successfully; four cases remain ignored. The final four-case Session paging check removes host-clock ordering assumptions while retaining its ordering, pagination and zero-Store-open assertions.
- The 110-case auxiliary/ownership/lineage/Trace regression selection passed. Binary/example harnesses passed 27 tests; pure contracts passed 31; provider tests passed 179 with five ignored.
- All seven external Linux boundary targets passed 457 tests with one ignored; in-crate boundary suites passed 243. Both `rust-contracts` and `rust-boundaries` lane-discovery checks passed. Composed tests required the offline provider emulator; its own pytest suite also passed.
- Development launcher tests passed 41 cases. Web tests passed 2,881 with four skipped; TUI tests passed 1,180 with one skipped. All component typechecks, i18n, provenance and notice checks passed. Runs were separated to avoid WSL I/O contention without increasing test timeouts.
- Schema/protocol regeneration had no content drift. The changed browser Session-export test passed against the pinned Playwright container and compared browser download with remote TUI export.

These are local verification results, not a GitHub FULL/macOS CI verdict. The complete browser suite and native macOS lanes remain part of the repository's merge gate.

### Attribution review follow-up

The follow-up separates business-source admission from historical input visibility,
replaces the closed purpose/trigger vocabulary with bounded consumer-defined
attribution, and adds opaque consumer business references without execution
permissions. A shared terminal-envelope constructor removes the consumer's need
to duplicate private event-ID formatting. Native Summary lineage still requires
its explicit checkpoint dependency and owning Compaction operation.

On Linux x86_64 with Rust 1.98.1, all 502 selected library tests passed with no
failures or ignored tests. The selection covered all `durable::sqlite` tests,
auxiliary consumers/readers, archive and artifact-lineage tests, and request-owner
regressions. It includes the later-business-source negative controls, a custom
translation consumer's full lifecycle, archive attribution round-trip, and
schema-72 rejection without rewriting storage. All-feature binary builds,
all-target/all-feature Clippy with warnings denied, formatting and whitespace
checks passed. The `rust-contracts` test-lane discovery check passed. Native
configuration/protocol regeneration produced no drift.

The browser archive expectations now name v7. Browser E2E was not rerun because
this host has neither Docker nor Podman for the pinned browser image. These
focused results do not replace exact-head FULL or native macOS merge gates.

## Stage 2: AUX-02 supervised execution and Summary

The proposed [detailed implementation plan](aux-02-implementation-plan.md) owns this stage's source inventory, ownership/state/lock protocol, commit gates and contract-to-test matrix. Implementation waits for user approval. Start from verified AUX-01 Store/reader contracts and implement the shared executor in native Runtime, replacing Summary's old polling implementation.

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

## Integration with #484

The branch merges actual upstream main `67719289` after AUX review commit
`38d64578`, preserving both published histories. The integration retains the
Source binary codec, bounded small-source caches and spool spill limits, and
canonical append certificate invalidation on rollback or native mutation.
Auxiliary owner indexes and exact confirmation remain mandatory; Source capture
remains optional. Primary Trace selection retains the review's explicit marker
and header/owner validation, and new Summary checkpoints retain strict successful
generation admission at their declared cut.

SQLite schema 74 identifies the combined semantic layout. Main schema 71 and AUX
schema 73 are rejected unchanged. Archive v7 / Journal 5, local and inherited
auxiliary-audit schema 2, Runtime Client 71 and App Server 50 are unchanged by
this integration. AUX-02 continues on its dedicated branch defined above; no compatibility
readers, reconstruction or migrations are added.

### Integration validation

On Linux x86_64 / Rust 1.98.1, the merged code passed 584 selected library
regressions (all SQLite, auxiliary, Source read-layout/observation, archive and
artifact-lineage cases), all 243 in-crate boundary cases, all 147 external durable
cases and all 63 process cases: 1,037 passed, zero failed or ignored. Boundary
runs set `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. All-feature binaries, all-target /
all-feature Clippy with warnings denied, formatting and whitespace checks passed.
Native configuration and App Server protocol regeneration produced no drift.
Both `rust-contracts` and `rust-boundaries` test-lane coverage checks passed.

The reviewed AUX parent `38d64578` passed its complete FULL workflow, including
macOS and Merge verification, in [run 39221262290](https://github.com/Caismis/rustX/actions/runs/39221262290).
That result is parent evidence, not a FULL verdict on this new integration.
Browser E2E and native macOS were not rerun locally; the integrated head must
pass its own FULL workflow before merge.

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

# AUX-02: supervised execution and Summary implementation plan

## Scope

Ship one Runtime-owned auxiliary executor and route both automatic and manual Summary through it. Each Call executes at most one physical Request. Context retains Summary planning, text acceptance and checkpoint commits. Runtime owns admission, capacity, dispatch authorization, cancellation, execution tasks, required writes and drain.

Automatic and manual compaction must work end to end when the old production Summary polling loop is removed. Technical retries and Title remain in #475. Classification, BTW, external submission DTOs, result-retention quotas and TTLs remain in #476. A private second consumer proves executor reuse without shipping another feature or purpose dispatcher.

Build on [AUX-01's](https://github.com/Caismis/rustX/issues/473) accepted ownership, Store, reader and confirmation contracts. Use the upstream implementation containing those contracts and recheck affected behavior after integration. Existing formats are SQLite 74, archive v7 / Journal 5, Runtime Client 71 and App Server 50. Advance only domains whose contracts change; add no compatibility readers, migrations or database reconstruction.

## Source inventory and intended changes

| Source / current owner | Current behavior | Change |
| --- | --- | --- |
| `context/summarizer.rs`: `ModelBackedSummarizer::summarize`, `SummaryStreamState` | Constructs and directly polls the provider stream, with deadlines and generation/text checks | Retain deterministic transcript construction and domain acceptance; replace direct execution with the native executor |
| `context/compaction.rs`: `execute_compaction`, shrink loop | Complete-request planning, media preparation and checkpoint candidate construction; shrink counter local to this invocation | Carry one operation budget/deadline and settled Summary-generation evidence through every candidate |
| `agent/execution.rs`: `perform_compaction` | Creates CompactionOperationId outside carryover degradation but can re-enter inner preparation | Allocate the aggregate budget outside both loops and retain it across re-entry |
| `model/session.rs`, `invocation.rs`, `selected_input.rs`, `media.rs` | Existing frozen Summary policy, protected overlays, typed input and media witnesses | Reuse these owners and extract only genuinely shared preparation |
| `runtime/types.rs`: lifecycle/admission and DurabilityGate | Existing admission and absorbing durability failure; gate serializes ownership writes | Extend existing authorities for auxiliary registration/grant and safe blocking-work coordination |
| `runtime/conversation_runtime.rs`: manual compaction and drain | Manual compaction already has a native task, using the old Summary path | Register auxiliary children and the business tail under native drain, expose a transferable operation handle |
| `local_runtime/composition.rs`, manager and `subagent_child.rs` | Compose resident/child Conversations and control writer retirement | Inject one shared process budget into participating compositions; preserve writer-transfer authority |
| `durable/auxiliary.rs`, `inbox.rs`, `sqlite.rs`, `sqlite/auxiliary.rs` | Synchronous start/settlement, exact confirmation, typed rollback/Unknown, mandatory indexes | Consume existing transactions from supervised blocking work; add only required lease/cancellation seams |
| `durable/sqlite.rs`: ConnectionOwner; `sqlite/manifest.rs`, `append_validation.rs` | Cancellable connection waiting and native certificate/epoch ownership | Add active-write lease generations, preserve preupdate invalidation and prevent stale interruption of a successor |
| `local_runtime/session_runtime_manager.rs`: OperationLease/start_operation | Reply loss does not abandon work, but lease lasts through the entire wait | Add compaction-specific short admission/transfer; preserve other operations' lease semantics |
| `runtime_client/host.rs`, `app_server/connection.rs` | Native compact receipts and delivery fences | Await transferred work outside the manager lease and preserve exact invocation receipts/control authority |
| `durable/sqlite/auxiliary_supervision_tests.rs` | Private real-Store fixture joins writers/tasks and confirms lost receipts | Carry its actual boundary cases into production executor tests |

All paths in this table are relative to `src/`. Store/model facts must not import the concrete executor. No universal Operation framework, second Agent runner, provider catalog, task queue or duplicate physical-request event family is introduced.

## Module and caller interface

Add `src/runtime/auxiliary/` with one owned-call registry and a small native facade. Internal admission, writer and stream files separate implementation concerns without creating parallel Call lifecycles.

The interface provides native admission/preparation, submission of frozen input/invocation, completion waiting and explicit cancellation. A registered preparation reservation is obtained before expensive selected-history/media work and consumed or unwound once. Submission moves frozen data into that already-owned record. Callers do not write facts, poll providers, settle requests or reuse RequestIds themselves.

Use a Runtime-provided Context interface so Context does not depend on concrete executor internals. The Context Summary adapter owns transcript/instruction construction and structured-text acceptance. Native execution returns confirmed execution evidence. Remove the old summarizer's direct adapter ownership after production callers migrate; keep test implementations of the same narrow Context contract.

The owned record retains Call/Request identity, native authority/parent relation, frozen invocation/resources/input, absolute deadline, cancellation/grant state, capacity reservations, execution join, current exact write intent/join and sealed outcome. Register before scheduling. Avoid Runtime/task strong-reference cycles and discarded JoinHandles.

Typed outcomes distinguish NotStarted, capacity Busy, cancellation/deadline/revocation, provider/stream failure, confirmed settlement, supervision failure and durability uncertainty. Confirmed model execution is distinct from Context accepting the text and committing a checkpoint. Domain rejection and optional observation failure do not cause a second model execution.

## Ownership, state and synchronization

### One Call progression

These phases describe one record. A pending write is evidence attached to it, not a separate shutdown state machine.

| Transition | Required evidence and effects |
| --- | --- |
| Unregistered -> preparing | Bounded authorization, lifecycle/parent registration and all initial reservations; unwind completely on refusal; no durable Call yet |
| Preparing -> Call started | Frozen preparation accepted; exact CallStarted committed or confirmed |
| Call started -> Request started | Parent start allowance reserved; RequestStarted confirmed; charge once |
| Request started -> dispatch consumed | One synchronized grant wins against cancellation, parent closure, drain, deadline and durability failure; never restore it |
| Executing -> outcome sealed | Valid terminal and EOF for success, or prompt valid failure/control outcome; retain observed provider outcome separately |
| Outcome sealed -> settlement pending | Submit retained exact envelopes once to the serialized writer; SQL latency cannot rewrite local outcome |
| Settlement pending -> settled | Physical write exit and authoritative receipt/confirmation; only then expose normal success or allow dependent Context commit |
| Admitted -> native failure handoff | All owned execution, write and confirmation work has physically exited and its leases have returned; unresolved exact intent moves to existing durability/manager authority; not successful settlement |

Before confirmed CallStarted, failure is NotStarted and creates no invented terminal. A Call without an open Request uses Call-only settlement. An open Request settles its final Request and Call atomically. A confirmed start cancelled before dispatch remains charged. Grant consumption authorizes local entry but does not prove remote execution; restart never cold-dispatches incomplete Calls.

Outcome precedence is also owned by this record: cancellation/revocation accepted before sealing prevents local success; EOF observed at or after the absolute deadline cannot seal success. A successful seal requires accepted terminal, EOF and current authorization. Once sealed, later cancellation or settlement latency cannot rewrite that execution outcome or provider observation, although the parent checkpoint grant must still reject expired/revoked business authority. Test both winners explicitly with a manual clock and barriers.

Independent Calls retain their own LifecycleAdmission. Automatic Summary shares its parent's admission/maintenance authority, registers in a close-fenced child barrier and consumes auxiliary capacity without reacquiring the Compaction maintenance lock. Scope closure prevents new children and cancels inference while retaining cleanup. Parent return waits for children or an accountable native handoff.

### Capacity and required-write design

| Reservation | Process | Conversation | Lifetime |
| --- | ---: | ---: | --- |
| Total active Calls | 32 | 4 | Preparation until all owned execution, write and confirmation work exits, all leases return, and required obligations are confirmed or handed off after proven exit |
| General-purpose Calls | 24 | 3 | Same; only native Compaction authority can use total capacity without a general permit |
| Required-write responsibility | One reserved serialized lane per admitted Call; at most 32 | Bounded by the four local Calls | Reserved before durable start; retained until the same physical-exit and obligation conditions as Call permits are met |

Each admitted Call retains one bounded native writer lane with one outstanding exact intent. Prefer a lazy dedicated blocking worker so provider tasks cannot occupy the workers needed for settlement. Real task/worker joins and connection return prove exit; notifications, counters, wrapper abortion and stream drop do not. Join offloading and confirmation work remain bounded by existing Call/lane ownership and cannot acquire a fresh execution permit. Establish actual worker/thread/task bounds and cleanup progress before dispatch; 32 lanes alone do not prove a 32-thread total, and an unconstrained spawn_blocking backlog is insufficient.

Retirement uses a fixed process-owned pool of 32 join workers, installed before the first physical Call owner starts. Combined with at most 32 owner threads and 32 write threads, the auxiliary bound is 96 threads. The retirement channel holds at most 32 records, and existing Call permits bound queued and in-progress retirement. A worker releases receiver synchronization before the actual join; one unexited owner cannot hold unrelated Conversations behind it. Cleanup reserves no fresh execution capacity, and handoff/completion flags never replace physical joins.

Release total/general Call permits and required-write responsibility together only after both conditions hold: all owned execution, write and confirmation work has physically exited with its leases returned, and required obligations are confirmed or transferred through an explicit proven-exited durability-failure handoff. A completed write does not release capacity while confirmation is still active. A transfer of live cleanup carries its permits; historical uncertainty after proven-exited handoff does not retain physical capacity indefinitely.

Initial reservation is nonblocking: process total/general permits, Conversation total/general permits, then required-write responsibility. Unwind partial acquisition once in reverse order. These precede expensive history/media preparation. AUX-02 has no retained-result reservation; #476 adds it before Call permits. Start allowance precedes the authoritative start transaction. SQL/connection waits occur only in supervised work. Observer delivery uses bounded try-send after native effects.

Purpose/trigger text cannot claim protected capacity. Private saturation tests must show general work leaves the prescribed Call and write capacity for native Compaction. Simultaneous Compactions or genuinely stuck writes may still return precise Busy. No queue, preemption or automatic Agent-turn resubmission is added. Initial refusal creates no auxiliary admission facts.

### Lock and wait protocol

The existing DurabilityGate holds a synchronous mutex across ownership writes; failure commitment currently occurs under the coordinator. Auxiliary supervision must avoid blocking Tokio workers or inverting coordinator/storage waits.

Keep one DurabilityGate and one absorbing failure fact. Make contended new-ownership/failure acquisition awaitable outside the coordinator, let synchronous ownership writers hold their gate without calling Runtime, and keep failure evidence readable without waiting behind long SQL. Reuse the existing committed/already-failed result for one native failure publication after releasing the write gate. Preserve the total order of new ownership, failure and dispatch; do not replace it with independent health flags.

Already-owned terminal settlement and authoritative exact confirmation do not reacquire healthy-state admission and cannot be refused because DurabilityFailed has been installed. Their existing native owner retains supervision, physical writer exclusion and Store integrity checks. A failure closes new ownership and dispatch without revoking the authority needed to settle existing obligations.

Audit both existing ownership-write clients (`tools/background.rs`, `runtime/subagent/registry.rs`) and every failure/read client before changing the gate. Record the final concrete acquisition graph in Step 2.

| Boundary | Required rule |
| --- | --- |
| Capacity/registry | Short try-acquisition and state changes; no SQL, capacity or provider wait while held |
| Coordinator and lifecycle/parent | Existing native commit boundary owns short registration/grant decisions; obtain asynchronously contended permission outside coordinator, then revalidate at the grant |
| Required ownership write | Ownership gate -> ConnectionOwner lease -> SQLite transaction; worker never calls coordinator failure handling while holding these |
| Existing settlement/confirmation | Retained native owner -> ConnectionOwner lease -> Store transaction/read; no healthy-state admission gate; preserve writer exclusion and integrity validation |
| Durability failure | Wait for ownership boundary outside coordinator; install absorbing winner, release gate, then bounded native publication; no duplicate failure authority |
| Cancellation | Close the owned grant immediately and wake a connection waiter without taking the SQL lease |
| Active interruption | ConnectionOwner checks exact active lease/generation before interrupting; stale signal is a no-op after lease transfer |
| Observation/transport | No read/publication lock held across required write, confirmation, execution grant or native cancellation |

Gate-held SQL, both ownership/failure winners, already-owned settlement after failure, cancellation and successor-write races require deterministic physical-boundary tests. Preserve background/subagent settlement-survival contracts and test constrained/current-thread runtime scheduling so progress does not depend on spare asynchronous workers. Audit actual call chains and held locks; replacing a mutex with an asynchronous mutex alone does not establish the protocol. Keep the implementation within existing native owners, without a replacement scheduler or general asynchronous Store framework.

## Implementation steps and commit gates

Each step is a focused commit or coherent small commit group. Intermediate commits must compile and preserve the working product. They are not separate upstream deliveries of a half-migrated executor.

Establish every safety guarantee before enabling the production path that depends on it. Steps may form a coherent commit group when needed, but later integrated verification cannot substitute for migration prerequisites.

### 1. Freeze contracts and typed outcomes

**Files:** `context/summarizer.rs`, `context/error.rs`, `model/session.rs`, `model/invocation.rs`, `context/resources.rs`, owning tests.

- Capture automatic/manual Summary's exact input, safety cap, refusal/empty semantics, selected-input ordering, media scopes and current compaction receipts as regression controls.
- Introduce only internal contracts needed for native preparation authority and typed outcomes. Separate Context text acceptance from stream execution checks.
- Inventory every Call/Request/operation identity and real commit, rollback and Unknown evidence. Keep tools empty and both continuation fields absent.

**Exit:** controls pass against existing behavior; the interface expresses Busy, NotStarted, confirmed settlement and durability failure without flattening them into SummaryFailed text.

### 2. Establish ownership, capacity and physical write supervision

**Files:** new `runtime/auxiliary/`, `runtime/types.rs`, `conversation_runtime.rs`, native composition/manager/child composition, `durable/sqlite.rs`, `sqlite/manifest.rs`, `append_validation.rs`, existing gate clients where necessary.

- Inject one process budget into participating Conversations, including native children; no per-Session budget masquerading as a process limit.
- Register preparation/Call records and parent child barriers before spawning; define winners for grant, cancellation, close, drain and durability failure.
- Implement reserved serialized write lanes, retained exact intents/joins and bounded confirmation. Retain Call permits and write responsibility through active confirmation, releasing them once only after physical exit and confirmed obligations or explicit handoff. Extend lease-generation cancellation without breaking manifest epochs, append certificates or existing read cancellation.
- Wire proven-exited uncertainty handoff to existing Conversation durability and manager writer certainty. Live/unproven work keeps its old allocation isolated; exited historical uncertainty does not retain global capacity forever.
- Complete the owner/state/lock inventory, all existing gate users, every permitted wait and actual worker/thread/task bounds, including join offloading and confirmation. Lane count alone is not a total thread bound.

**Exit:** no adapter entry yet. Real SQLite/task tests cover blocked waiting/running writes, delayed confirmation after writer exit, lost acknowledgement after real commit, rollback, panic, waiter loss, stale interrupts, shutdown timeout and permanent confirmation failure. Prove both ownership/failure winners, auxiliary settlement after DurabilityFailed and existing background/subagent settlement survival, including current-thread scheduling. Saturation with delayed confirmation must preserve 4/3 and 32/24 admission limits, protected write capacity and exactly-once release. Every admitted Call can finish required cleanup without a fresh execution permit or unbounded queue.

### 3. Implement single-request execution

**Files:** `runtime/auxiliary/`, existing model deadline/generation/event/selected-input/media seams where needed; Store transactions remain synchronous.

- Reserve before expensive preparation; freeze exact invocation/input/resources and finish deterministic adapter preflight before confirmed Call and Request starts.
- Reuse MediaSnapshot::capture_selected and Context materialize_selected_media, with upload projection before image projection and unchanged manifest/media witnesses shared. Preflight validates modalities, image count/resolution, protected parameters and actual allocation limits; no primary Context admission or publication authority is acquired.
- Consume one RequestId-specific grant, then enter the existing adapter at most once. Unknown start or a losing grant never dispatches.
- Use one purpose-independent stream fold with per-request Timing, ModelRequestDeadline, GenerationGuard and monotonic time. Validate continuation envelopes then discard valid metadata; reject tools, invalid order and duplicate/illegal terminal tails.
- Success requires EOF, with the absolute Call deadline still active after Completed; Completed-then-Pending must time out. Valid Failed is prompt. Cumulative Usage replaces prior trusted observations; missing final usage retains the last trusted observation, and illegal tails cannot overwrite evidence.
- Seal observed provider terminal, local outcome, usage completeness and remote uncertainty before persistence. Confirm atomic settlement before success. Optional Source/delivery failures cannot regenerate or rewrite native results.
- Missing remote Usage stays unknown rather than zero. Known RequestId-scoped subtotals are not complete billing evidence; a later successful operation cannot erase earlier missing reports or remote uncertainty.

**Exit:** production-facade tests inherit the AUX-01 real-Store fixture's cancellation and receipt-loss cases. A private second consumer proves reuse. Cover both grant winners, adapter-entry panic, EOF timeout, generation failure and Usage; no retry or second Request per Call exists.

### 4. Migrate automatic Summary and aggregate operation budgets

**Files:** `context/mod.rs`, `summarizer.rs`, `compaction.rs`, `engine.rs`, `resources.rs`, `agent/execution.rs`, selected-input/media seams.

- Create the operation context outside both perform_compaction carryover degradation and the inner Summary shrink loop. Retain CompactionOperationId, frozen model/resources, parent scope, deadline and budget through all re-entry.
- Operation deadline is five minutes; each Call ends inference eligibility at the earlier of its own five-minute admission deadline and the parent deadline. Registration, preparation and durable waits consume it. Cleanup may outlive inference eligibility while owned.
- Allow 12 confirmed physical starts and three total input shrinks across the operation, plus existing finite monotonic carryover degradation. Reserve before Started, charge confirmed commit, release proven noncommit and preserve uncertainty. Confirmed Started cancelled before dispatch still counts. AUX-03 reuses this accounting for technical retries.
- Each semantic input change creates a new Call under the same operation; each has at most one Request. Shrink/carryover cannot refresh invocation, budgets or deadlines.
- Preserve the one-pass retired-transcript renderer, complete-request planning and conservative SearchIncomplete. Share immutable roots/ranges and frozen media; no whole-history flattening, repeated reconstruction or candidate-time resource resampling.
- Context accepts settled Summary text and commits its existing checkpoint transaction with a real successful Summary-generation dependency. Domain rejection is separate from execution failure and causes no corrective technical retry.
- Before switching automatic Summary to the executor, pass parent cancellation/deadline versus checkpoint-grant tests at the actual commit boundary. These guarantees must already hold when the production path is enabled.

**Exit:** automatic Summary uses the executor. Exact adapter input and unchanged-item/media reuse hold at increasing history sizes; budgets survive both loops; primary P/U/C, recovery, occupancy and continuation remain owner-scoped.

### 5. Migrate manual Summary and manager lease transfer

**Files:** `runtime/conversation_runtime.rs`, `local_runtime/session_runtime_manager.rs`, `runtime_client/host.rs`, `app_server/connection.rs`, native model/configuration composition.

- Split manual admission/transfer from waiting. Register its auxiliary child and Context business tail before releasing the short OperationLease. Preserve full-lease semantics for other start_operation callers.
- Resolve outside long-held coordinator locks and revalidate model/resource revisions at final admission. A configuration update winning first affects admission; admission winning first retains frozen authority. Invalid explicit policy fails without fallback.
- Preserve automatic Attempt-frozen policy, existing follow-Session/explicit profiles, shallow overlays, protected fields and Summary output safety cap. Expose no settings for unimplemented consumers.
- Return this invocation's exact ManualCompactionOutcome, including generation/checkpoint/activation/token facts, even when observation is unavailable or successor B commits before A's response. Preserve newly_committed handling for reconciled receipt loss versus ordinary duplicate lookup.
- Reply loss, detach and controller handoff do not cancel native work. Explicit cancellation/revocation/drain do. Read/delivery revocation remains independent; a successor authorized controller can cancel.
- Before enabling transferred manual Summary, prove business-tail drain ownership, actual OperationLease release, shutdown reaching cancellation and writer exclusion while blocking work is still live.

**Exit:** real manager/Runtime Client/App Server tests prove business-tail drain ownership, early lease release, shutdown reaching cancellation, writer exclusion with live work, exact A/B receipts, lost responses, optional observation failure and no duplicate generation/publication. The transfer is not production-enabled before these tests pass.

### 6. Complete drain, checkpoint authority and failure containment

**Files:** registry, native drain/manager owners, Context commit seam and existing control/compaction/observation tests.

This step completes integrated verification of the guarantees established before automatic and manual migration.

- Close admission/children, cancel inference and join actual tasks/writes. A timed-out shutdown cannot transfer Store/writer authority while work may still run.
- Keep the existing native shutdown wait budget distinct from the five-minute inference deadlines. Returning a bounded failed shutdown does not prove a still-running task exited; its actual isolation and capacity accounting remain visible.
- After proven physical exit, bounded exact confirmation ends in settlement or explicit failed-unload handoff. Permanent unreadability does not loop forever or rebuild storage. Reopen requires exclusive writer access and normal integrity/recovery checks; incomplete Calls remain inactive history.
- Check parent cancellation/deadline at the checkpoint grant. A winning commit completes required installation; Unknown confirms the same operation/candidate and restores the actual current head without another Summary.
- Preserve expected projection head, Attempt fence and scoped dependencies. Auxiliary Journal appends and permitted canonical Tail growth do not invalidate a frozen cut wholesale. Add neither event-name exclusions nor whole-Journal-head equality.
- Keep native cancellation/control reachable during optional Source read/index/publication failure. Such failure cannot roll back confirmed settlement or become provider retry.

**Exit:** blocked-write shutdown, exclusive reopen, permanent confirmation failure, source-unavailable cancellation and checkpoint grant races pass. No unproven lease is called quiescent; no exited historical Unknown permanently holds process slots.

### 7. Remove obsolete paths and validate the complete increment

**Files:** old Summary polling/state implementation; `docs/architecture.md`, `invariants.md`, `context-engine.md`, `auxiliary-execution.md`, manual-compaction protocol documentation and actually affected generated contracts.

- Retain one production stream loop, registry and settlement path. Keep pure transcript/domain helpers; remove replaced adapters/tests instead of compatibility modes.
- Record changed schema, archive, Runtime Client, App Server and handshake domains independently. Regenerate affected artifacts and prove a second generation clean.
- Run the complete contract matrix after removing the replaced paths and regenerating affected artifacts.

**Exit:** the contract matrix and FULL/Merge verification pass, including applicable macOS lanes.

## Contract-to-test matrix

Use private consumed hooks, manual clocks and channels/barriers, never sleeps as race evidence. A lost-ack fixture commits a real SQLite transaction before losing the receipt.

| Contract | Required tests / existing suites |
| --- | --- |
| Lifecycle and grant | Runtime auxiliary/types tests: registration/drain, child admission/close, durability/grant, both cancellation/close winners at the production CallRecord grant, capacity/lifecycle Busy after cancellation with active controls, immediate completion/waiter loss; adapter entry <= 1 |
| Capacity and progress | 4/3 local, 32/24 across Conversations; protected-class spoof; partial rollback; preparing/late cleanup/active confirmation counts; saturate after writes exit but confirmation is delayed; exactly-once release and live-cleanup transfer; optional saturation with/without reserves; separately saturate required writes without starving cleanup |
| Durability frontier and settlement | Both new-ownership/failure winners; already-owned auxiliary, background and subagent settlement after DurabilityFailed; actual lock ownership and progress on constrained/current-thread runtimes |
| Actual write ownership | AUX-01 supervision and ConnectionOwner fixtures: waiting/running SQL, panic, actual commit/lost receipt, rollback, cancellation wake, delayed interrupt after successor lease and native certificate invalidation |
| Physical exit vs certainty | Manager/drain and `tests/boundary/durable/process_death/`: timeout with live work, exited Unknown, permanent read failure, unrelated Conversation progress; SIGKILL after confirmed RequestStarted, granted dispatch and adapter entry; quiescent cold reopen preserves unresolved facts without replay |
| Stream evidence | Scripted model support and provider emulator: invalid order/tools, safeguards, duplicate/illegal tail, no completion/EOF, prompt Failed, envelope-validated/discarded continuation, cumulative/absent/illegal-tail Usage and timing |
| Summary semantics/input | `tests/scripted/context/`, `tests/support/context.rs`, selected-input/media and `agent/execution_selected_input_tests.rs`: one-pass input at multiple sizes, SearchIncomplete, media scopes, protected overlays, domain-invalid Summary |
| Budget/frozen authority | Shrink plus carryover re-entry, starts cancelled before dispatch, Unknown/noncommit accounting, configuration before/after admission, absolute parent/Call deadlines and checkpoint grant; grant races pass before automatic migration |
| Native/control/observation | `runtime_client/host/compaction_head_tests.rs`, `native_cancellation_tests.rs`, SQLite projection tests: blocked Source, detach/handoff/revocation, independent cancellation and exact manual A/B receipts; business-tail drain, manager deletion while a completed receipt is parked, exact receipt delivery after deletion, actual lease release and live-writer exclusion pass before manual transfer |
| Primary/lineage isolation | Existing request-owner, auxiliary, Trace, occupancy, artifact-lineage and archive-cut suites; equivalent primary histories with auxiliary interleavings under controlled scheduling; contention measured separately |

## Verification

Use focused owning tests at each commit. At delivery, follow current CONTRIBUTING.md, DEVELOPMENT.md, tests/README.md and CI selection; one name-filtered Cargo run is not complete coverage.

```text
git diff --check
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --bins --all-features
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features
cargo run --all-features --example check_test_lanes
```

Prepare the documented provider emulator with uv sync --frozen and run pytest. Run affected protocol/configuration generation/drift checks, dev/TUI/Web typechecks/tests, i18n/provenance checks, real transport/browser E2E and Linux/macOS boundaries. No live inference credentials are required. Respect libclang, repository Rust and shared-CI Node prerequisites; do not mix binaries, profiles or source revisions when claiming results.

Distinguish pre-existing failures from regressions through controlled comparison. Do not weaken assertions, enlarge timeouts or omit required lanes. Task abortion is not physical-exit evidence.

# Process-death conformance (FND-06 / Issue #111)

FND-01 … FND-05 established the durable runtime contracts. This document is
their proof against **real process death**: a real child process running the
real runtime stack over a real durable file, frozen at a named durable
boundary, ended with an uncatchable `SIGKILL`, then reopened and recovered.

The suite is `tests/boundary/durable/process_death/` (65 conformance tests
plus the child entry point): in-crate **boundary conformance**, selected in CI
by its `boundary_suites::durable::process_death` prefix. Every row of the
tables below names the test that proves it.

## Why the child is the crate's own test binary

A conformance child must run the real `ConversationRuntime`, the real durable
`SQLite` authority, the real Tool Plane, the real publication plane, and the
real filesystem resource loader, while also using two seams that must never
exist in the published API:

- the scripted provider adapter (`ScriptedProviderAdapterFactory`), so a model
  turn is a fixed sequence of canonical events rather than a network call;
- the process-death boundaries in `crate::runtime::process_death`;
- `SubagentRegistry::push_staged_override`, for the subagent rows only (see
  §8), which replaces the spawn and startup handshake and nothing else.

`tests/boundary/mod.rs` explains why such boundary suites compile into the
crate's own test build. FND-06 follows the same rule and the same
re-execute-the-test-binary pattern the M7 MCP stdio fixture uses: the parent
spawns `current_exe()` selecting one child entry point. Everything below the
seam — composition, admission, the Agent Loop, durability, recovery — is the
code the `rustx` binary runs.

## What a boundary is

`process_death::reach` is compiled only under `cfg(test)`; in every other build
its two entry points are empty `const fn`s. A child parks at exactly one
boundary, announces it over its control socket, and blocks forever:

```text
before:<transition>   the durable transaction is open and committed nothing
after:<transition>    the durable transaction committed
```

Both variants park while the store's connection mutex is held, so a parked
child cannot commit anything durable from any other thread. "Killed before P"
therefore means the durable authority provably contains no P.

Two durable planes outside the conversation store use the same seam with their
own exclusion, because the fact they linearize is not a SQLite transaction:
`reload:prepared` / `reload:published` sit under the runtime's own
one-reload-at-a-time gate, and `before/after:publish_session` /
`before/after:publish_node` bracket the Session catalog's visibility rename
under the supervisor state mutex the whole publish operation holds. In each
case the parked thread owns the only path that can advance that plane, so the
durable world is frozen for the same reason.

The second rendezvous kind is a **control rendezvous**: the child announces a
fact and blocks reading its next command, so it is executing nothing at all.
That is how the parent edits resources underneath a live runtime, and how it
kills a process while a compaction summary side request is in flight.

### Determinism

No conformance assertion depends on a sleep, a poll, or a log ordering.
Ordering claims are read from the durable Event Journal by sequence. The only
wall-clock values in the suite are outer liveness guards whose expiry is a
harness failure, never a verdict. The harness runs each child in its own
process group and ends it with `killpg`, so no OS child and no runtime-private
state survives a test — including the real detached `sleep 300` the background
cases start.

## 1. Canonical history atomicity

| Boundary | Durable before kill | Allowed after reopen | Forbidden | Recovery action | Test |
| --- | --- | --- | --- | --- | --- |
| `before:accept_inbound` | nothing | empty conversation | any pending or canonical trace of the message | none; Class A | `inbound_acceptance_is_atomic` |
| `after:accept_inbound` | pending inbound ×1 | pending ×1, canonical ∅ | canonical adoption | none; Class A, `PendingInboundOnly` | `accepted_inbound_survives_as_pending_only` |
| composed but never activated | pending inbound ×1 | pending ×1 | adoption by a process that never activated | none | `accepted_inbound_is_never_adopted_by_a_dead_process` |
| `before:adopt_pending_batch` | pending ×1 | pending ×1, canonical ∅ | half-adopted Surface | none | `pending_adoption_is_atomic` |
| `after:adopt_pending_batch` | canonical User + its answer obligation | canonical User, pending ∅ | both pending and canonical; an adopted turn without its obligation | `ContinueAdoptedTurn` (see “Defect found”) | `adopted_inbound_is_canonical_and_no_longer_pending` |
| the same, one turn later in a multi-turn conversation | a settled attempt, then canonical User + obligation | the second turn is answerable | the second turn being read as already answered | `ContinueAdoptedTurn` under Class D | `a_turn_adopted_after_a_settled_attempt_continues` |
| the same, drained into a **live** attempt at its safe boundary | a known request outcome, then canonical User + obligation | the drained turn is answerable | the drained turn being stranded by the previous request's outcome | `ContinueAdoptedTurn` under Class E | `a_turn_drained_into_a_live_attempt_continues` |
| a fully answered conversation | request start consumed every obligation | nothing outstanding | re-answering an answered turn | `PendingInboundOnly` | `an_answered_turn_is_not_continued_after_reopen` |
| `before/after:commit_canonical_publication` | ∅ / canonical Assistant ×1 | exactly zero or exactly one Assistant | a partial Assistant | none | `assistant_canonical_commit_is_atomic` |
| `before/after:append_canonical_batch` | ∅ / `ToolResult` batch | whole batch or none | a partial sibling batch | repair only when absent | `tool_result_batch_commit_is_atomic` |
| `before:event:background_terminal_published` | ownership only | no terminal message | a half-published lineage terminal | publish the terminal exactly once | `background_terminal_publication_is_atomic` |
| `before/after:commit_compaction` | see §6 | see §6 | see §6 | see §6 | `compaction_surface_replace_is_atomic` |

## 1.1 Agent Status opportunity and suppression recovery

`PostToolBatch` is attempt-local eligibility, not a durable obligation. The
Agent Loop sets one marker only after the complete canonical ToolResult batch
commits, and `prepare_model_turn` consumes it together with any pending
FreshInbound member. If the process dies before that next primary step, the
reopened attempt has no marker and recovery does not synthesize one. In the
settled-tool case covered by
`post_tool_batch_marker_is_not_recovered_after_external_side_effect_blocks_continuation`,
the earlier `ToolExecutionStarted` fact proves that external work happened, so
the existing recovery architecture intentionally terminalizes the dead
attempt and reports `PendingInboundOnly` rather than replaying a post-tool
continuation. It does not create a replacement model step merely to consume
the dead marker. (`BlockedIndeterminate` remains the recovery disposition for
the separate case where a started external outcome is still unknown.) A
status candidate prepared but cancelled before model-turn-start likewise
leaves no canonical status message, `AgentStatusEmitted` fact, or
latest-emission head.

The converse boundary is the combined model-turn-start commit: the canonical
Agent Status User message, its canonical-message-bound emission fact(s), and
the bounded latest-emission head(s) commit with the Request Snapshot and
`ModelRequestStarted`. Reopen therefore preserves suppression through one
bounded `(module, key)` lookup even when compaction has retired the status
message from the active Surface. Todo suppresses an identical bounded
fingerprint while fewer than four later newly committed first requests of
logical primary model steps have followed its store-assigned durable origin,
and is eligible again at exactly four; changed fingerprints bypass that
duplicate window. The origin is assigned after same-start context and status
messages have staged, so those messages contribute zero elapsed progress.
Time, Background, RuntimeToolObservation, compaction, and overflow retries do
not advance the Todo-specific `todo_progress_sequence`. Overflow retry reuses
the accepted generation and cannot add a second emission fact.

## 2. Provider / publication / conversation separation

```text
P — ModelRequestCompleted      U — publication terminal      C — canonical Assistant
```

| Boundary | Durable before kill | Settlement after reopen | Forbidden | Test |
| --- | --- | --- | --- | --- |
| `before:event:model_request_completed` (frames staged and released) | staged frames, no P, no U | `Incomplete` | any P; a fabricated Assistant | `kill_before_p_with_released_frames_is_incomplete` |
| structural `assembler.finish()` rejection after released frames | staged frames, no P | `Incomplete` | any P; any tool execution | `structural_finish_failure_is_incomplete_without_p` |
| `after:event:model_request_completed` | P, no U | `Incomplete` | `Unaccepted`; canonical Assistant | `p_committed_and_u_missing_is_incomplete` |
| `after:commit_publication_terminal` | P, U, no C | `Unaccepted` | canonical Assistant from the audit | `u_committed_and_c_missing_is_unaccepted` |
| terminal-only frame path | P, U in one transaction | `Unaccepted` | U preceding P | `terminal_only_publication_reaches_u_atomically` |
| `after:commit_canonical_publication` | P, U, C | canonical only | any audit item | `c_committed_settles_canonically_without_an_audit` |
| every boundary above | — | `C ⇒ U ⇒ P` holds before **and** after reconciliation | durable `U` without `P`, durable `C` without `U` | `no_boundary_creates_u_without_p_or_c_without_u` |

## 3. Model-proposed tool calls never become execution through audit

| Case | Durable before kill | Forbidden after reopen | Test |
| --- | --- | --- | --- |
| partial proposal, no C | staged proposal frames, no P | `ToolExecutionStarted`, `ToolResult` | `structural_finish_failure_is_incomplete_without_p` |
| complete proposal, U, canonical acceptance never committed | P, U, no C | any execution fact; any repaired result slot | `an_unaccepted_proposal_never_becomes_an_execution` |
| audited output vs. later model context | `Unaccepted` audit | no canonical Assistant/User/MessageId or lineage identity; a bounded request-only projection may be frozen once in a later eligible `RequestSnapshot` | `a_publication_audit_carryover_stays_request_only` |

Issue #137 makes that last boundary deliberately narrow. Publication Audit is
still the only body authority. A terminally unresolved audit can leave only
its `PublicationStreamId` in the conversation root. Live settlement and
recovery use the same keyed selector over the last durably started request's
descending retry ordinals, and the selected source is installed/replaced or
cleared in the same durable transaction as the attempt terminal. Recovery
terminalizes publication evidence before that selection, so a crash before the
terminal/pointer commit leaves a valid prefix.

The first successfully started eligible primary model step is the only
consumer. Its start transaction freezes the exact bounded request-only
representation and insertion anchor in the Request Snapshot while clearing
the pointer. Cancellation before the commit preserves the pointer. FreshInbound
places carryover immediately before its first canonical inbound message;
Continuation places it after canonical history and before newly staged current
context. Transient retries reuse the frozen value, while overflow can only
degrade it from full to reduced, metadata-only, or omitted. No carryover event,
Runtime Client state, TUI row, canonical message, tool authority, summary
input, or lineage seed is created.

The second-crash regression
`unresolved_output_carryover_recovery_reuses_the_same_audit_across_second_crash`
parks recovery before the combined attempt-terminal/pointer commit twice. The
third recovery commits it once, and the later eligible start consumes the same
source exactly once. The retry-success regression
`recovery_of_internal_retry_success_does_not_install_old_carryover` proves
that an accepted later retry suppresses earlier failed-generation audits.

## 4. Tool external-outcome recovery

| Boundary | Durable before kill | Repaired canonical result | Resume | Test |
| --- | --- | --- | --- | --- |
| `before:event:tool_execution_started` | canonical Assistant with the call | `Cancelled { ParentCancelled }` | `PendingInboundOnly` | `kill_before_tool_execution_start_authorizes_nothing` |
| `after:event:tool_execution_started` | start committed, outcome unknown | `OutcomeUnknown` — never inferred from workspace state | `BlockedIndeterminate` | `started_tool_with_unknown_outcome_stays_unknown` |
| `after:event:tool_execution_completed` | outcome durably known, no canonical settlement | the exact durable result, by value | `PendingInboundOnly` | `known_tool_outcome_is_preserved_into_the_canonical_slot` |
| `after:append_canonical_batch` | canonical `ToolResult` | none — nothing to repair | `PendingInboundOnly` | `tool_result_batch_commit_is_atomic` |

No case re-executes the tool: `ToolExecutionStarted` stays at exactly one
occurrence across the crash and the recovery.

## 5. Interaction ordering

| Case | Durable before kill | Allowed after reopen | Forbidden | Test |
| --- | --- | --- | --- | --- |
| waiter pending at death | `InteractionRequested` only | the requested audit as transcript history | any settlement; any execution; a recreated waiter | `a_pending_interaction_leaves_a_requested_audit_and_nothing_else` |
| approval settled, killed before the execution start | `InteractionRequested < InteractionSettled`, no `ToolExecutionStarted` | the settled audit as history | a tool auto-executing on the historical approval | `a_settled_approval_never_authorizes_a_later_execution` |
| approval settled, killed after the execution start | `settled < started` | unknown external outcome | the approval being replayed as authorization | `approval_settlement_and_the_tool_start_boundary_compose` |

These rows describe live interaction authority, not only audit replay. A
Runtime Client detach while the originating runtime is still live leaves its
pending coordinator waiter untouched; a later attachment reconstructs the
root projection from that live state. Conversely, process death destroys the
waiter even though InteractionRequested and any earlier settled fact may
remain as historical audit evidence. A child process death removes only that
child's root-facing actionable projections, and a delayed response receives
interaction_not_pending. No recovery path synthesizes Allow, Deny,
questionnaire decline, cancellation, or execution from historical facts.

Reliable route loss is a third case at the same boundary, not another spelling
of provider absence. A root admission refusal before the permit produces
`Unavailable` without `InteractionRequested`. Once the permit and requested
fact exist, loss of the reliable Requested route interrupts supervised child
execution and leaves the requested fact as evidence; it does not create an
ordinary unavailable ToolResult. If the coordinator has already committed a
settlement but the reliable Settled route fails, that selected outcome remains
audit evidence while the child stops under the existing control-loss
semantics. Reopen never recreates either waiter, treats the settled fact as a
live response, or authorizes a tool from historical `Approved`.

## 6. Compaction

| Boundary | Durable before kill | Surface after reopen | Ledger | Test |
| --- | --- | --- | --- | --- |
| summary side request in flight | nothing of the compaction | the pre-compaction Surface, authoritative | untouched | `kill_during_the_compaction_summary_keeps_the_old_surface` |
| `before:commit_compaction` | nothing of the compaction | the pre-compaction Surface | untouched | `compaction_surface_replace_is_atomic` |
| `after:commit_compaction` | one `CompactionCompleted` | exactly the planned span replaced by its summary | historical prefix intact, summary appended | `compaction_surface_replace_is_atomic` |
| project instructions and Skill catalog edited across the compaction | — | the post-compaction request still carries the loaded generation | no canonical project-instruction or Skill-guidance fact | `compaction_never_refreshes_resource_derived_authority` |

Compaction is not a resource reload boundary: it does not discover, refresh,
suppress, or remove resource-derived System authority, and the resource
revision of the request after a committed compaction equals the one before it.

## 7. Context / System / resource / lineage authority

The lab writes an `AGENTS.md`, one discovered Skill package, one ordinary
workspace file, and a runtime configuration; the parent edits any of them while
a child is live.

| Case | Loaded generation | Filesystem mutation point | Reload / reopen boundary | Old vs. new model API context | Test |
| --- | --- | --- | --- | --- | --- |
| live external edits | R1 | between two attempts of one live runtime | none | requests 1–3 all send R1 project instructions, R1 Skill catalog, R1 Tool definitions and share one resource revision | `live_external_edits_never_expose_a_new_generation` |
| progressive disclosure | R1 catalog | same | none | a later native `Read` returns the **current** `SKILL.md` body while the catalog stays R1 | `live_external_edits_never_expose_a_new_generation` |
| successful explicit reload | R1 → R2 | at a quiescent boundary | `reload_resources()` | request 1 = R1; request 2 = R2 instructions + R2 catalog + R2 Tool definitions together; no canonical message, no synthetic diff; the R1 request still reconstructs exactly | `explicit_reload_publishes_one_complete_generation` |
| failed reload | R1 | config corrupted before reload | `reload_resources()` returns a failure | the next request still sends complete R1; no revision published | `a_failed_reload_keeps_the_previous_generation` |
| reload while an attempt owns the session | R1 | — | `reload_resources()` returns `Busy { Attempt }` | no mixed generation; one R1 request only | `reload_while_an_attempt_owns_the_session_is_busy` |
| reload while a **compaction** owns the session | R1 | — | `reload_resources()` returns `Busy { Compaction }` | no mixed generation; the answered turn *and* the compaction summary side request are both R1 | `reload_while_a_compaction_owns_the_session_is_busy` |
| reload while a pending **interaction** owns the session | R1 | — | `reload_resources()` returns `Busy { Interaction }` | no request admitted under a new generation; the pending approval authorizes nothing | `reload_while_an_interaction_owns_the_session_is_busy` |
| reload while a **running foreground Tool execution** owns the session | R1 | — | `reload_resources()` returns `Busy { Attempt }`, strictly after the durable `ToolExecutionStarted` | no continuation admitted under a new generation; the running execution's outcome stays unknown and Class C blocks continuation | `reload_while_a_running_tool_execution_owns_the_session_is_busy` |
| `SIGKILL` at `reload:prepared` / `reload:published` | R1 (+ an unpublished/just-published R2) | before the reload | the reload build/publish boundary | no request was admitted under a half-published generation; the reopened process performs a normal fresh load and sends pure R2 | `death_around_the_reload_publish_boundary_reloads_from_scratch` |
| cold resume after external edits | R1 → R2 | while the process is dead | reopen | the first new request is R2; the historical Ledger, transcript order, and old `RequestSnapshot` (prompt + Tool definitions) are unchanged; no replacement message is appended | `cold_resume_uses_current_resources_and_preserves_history` |
| deleted already-discovered Skill | R1 → R2 | between two processes | reopen | the historical `ToolResult` keeps the old body by value; a new `Read` returns the normal read error | `a_deleted_skill_leaves_history_by_value_and_reads_as_an_error` |
| invalid current resources | — | config corrupted before reopen | runtime creation | creation fails explicitly; the historical generation is never used as live authority and no request is admitted | `invalid_current_resources_fail_runtime_creation` |
| death before the first `ModelRequest` | R1 loaded, never recorded | after the death | reopen | no `RequestSnapshot` and no canonical resource/Skill-guidance fact exists; reopen simply loads current resources | `death_before_the_first_request_leaves_no_resource_record` |

The current `RuntimeResourceSnapshot` is process-local, not durable recovery
authority. That is what makes the reload-boundary row provable: there is no
durable half-generation to recover, only a fresh load.

### Lineage authority: the cut, and what history may not resurrect

These rows compose the **real** native `SessionCatalog` and
`LocalSessionSupervisor` over the lab's runtime-private root, so `/fork` and
`/branch` are the production supervisor operations rather than a harness
re-implementation. The child scripts its provider and answers no protocol
input; everything else — the catalog transaction, the seed `initialize`, the
quiescence, the new lineage's own runtime and recovery — is the real path.

A cut is not a copy. The new lineage is seeded through `initialize` with the
exact canonical prefix *before* the selected human message, and `initialize` is
not an adoption: it commits no answer obligation, no attempt, and none of the
source's durable ownership.

What makes that non-vacuous is **where the cut is taken**. The source lineage
first starts a detached background execution and delegates to a real subagent
child, and only then is the boundary chosen — so the copied prefix genuinely
carries the source's ownership identities as canonical text:

```text
tool result      execution_id "exec_1", plus the source lineage's own private
                 output path .../conversations/conversation-1/...
tool result      subagent_id "conversation-1-subagent-1",
                 child_agent_id "agent-conversation-1-subagent-1"
user message     source = Agent { agent-conversation-1-subagent-1 }
Agent Status ×2  "Background executions:\n- exec_1 | bash | running"
```

The supervisor quiesces the old runtime *before* the publication, so the
source's own terminals are published in the source lineage; what the
destination inherits is the words, and the rows below are what it must not do
with them.

| Case | Durable before kill | Allowed after reopen | Forbidden | Test |
| --- | --- | --- | --- | --- |
| `/fork` at a boundary after both ownership turns | four answered turns; one background ownership; one subagent ownership | the new lineage holds the exact prefix — both tool results, the child's own `UserSource::Agent` message, and both Agent Status footers — by value; its Journal is **empty**; recovery reports `NotStarted` / `PendingInboundOnly` with zero background and subagent ordinals | inheriting any ownership, ordinal watermark, attempt, or obligation; changing the source lineage | `a_forked_lineage_cuts_the_history_and_inherits_no_durable_ownership` |
| `/branch` at the same boundary | the same | the same — a new node inside the active Session is a different catalog transaction with a different parent linkage, so it is proven separately | the same | `a_branched_lineage_cuts_the_history_and_inherits_no_durable_ownership` |
| the cut lineage then answers a turn that starts **nothing** | a destination context naming `exec_1`, `conversation-1-subagent-1`, `agent-conversation-1-subagent-1`, the source's private output path, and two live-execution status footers | every copied `MessageId` is reissued; the attempt domain starts at that lineage's own ordinal zero; the newly composed Agent Status carries **no** background section while the copied ones still name `exec_1` | resolving, adopting, reattaching, or relaunching any copied identity; publishing a terminal — that is, spuriously cancelling — for work this lineage never owned | `a_cut_lineage_never_resolves_the_copied_source_identities` |
| historical Agent Status naming a live execution, in the **same** lineage | a status message literally containing `Background executions: exec_1 \| bash \| …` | the historical status is retained **by value**; the status composed after the reopen contains no background section at all | reconstructing ownership from what history *says*; a second ownership commit; a second terminal | `historical_status_and_history_never_revive_background_ownership` |

The last two rows are the sharpest. Agent Status is a canonical message, so the
text naming a live execution stays in the Ledger forever, survives a cut into a
different lineage, and is read back by a reopened runtime as ordinary history.
Ownership authority is the durable ownership facts plus the process-local
registry, never the message text — so the same rendered section is the evidence
on both sides: history keeps it, and the runtime's own newly composed status
does not.

### Lineage publication atomicity

The rows above describe what a *completed* publication leaves behind. That the
publication is atomic at all is a separate claim, and it needs its own
boundary: `prepare_*` seeds the destination database on disk **before** the
catalog transaction opens, so there is a real window in which a fully formed
conversation exists that nothing names.

The `before:`/`after:publish_session` and `before:`/`after:publish_node`
boundaries bracket the catalog **visibility commit** — the single atomic rename
inside `SessionCatalog::commit` — and nothing else. The two sides are therefore
the only two durable worlds a crash can leave:

```text
before  a seeded destination the catalog does not name
        -> the source lineage is still active; the seed is an inert orphan:
           complete on disk, named by no Session and no node, so it is neither
           selectable nor resumable
after   a catalog naming the complete new lineage
        -> the destination is active and holds the whole cut prefix, owes
           nothing, and is immediately resumable
```

There is no third state — in particular no catalog entry pointing at a missing
or partial database, which would be unrecoverable. `/fork` and `/branch` are
proven separately because they are different catalog transactions that allocate
from different identity domains (`session-2` / `conversation-2` versus a new
node under `session-1` with `conversation-node-2`).

| Boundary | Durable before kill | Allowed after reopen | Forbidden | Test |
| --- | --- | --- | --- | --- |
| `before:publish_session` | destination database seeded, catalog unchanged | the source lineage is still active and fully recoverable; the orphan seed is named by nothing | a catalog naming the destination; a changed active selection | `death_before_the_fork_visibility_commit_keeps_the_source_active` |
| `after:publish_session` | the visibility rename committed | the new Session is active and holds the complete cut prefix with an empty Journal | a partial prefix; a missing destination database | `death_after_the_fork_visibility_commit_publishes_the_whole_lineage` |
| `before:publish_node` | the same, for a branch node | the same | the same | `death_before_the_branch_visibility_commit_keeps_the_source_active` |
| `after:publish_node` | the same | the same | the same | `death_after_the_branch_visibility_commit_publishes_the_whole_lineage` |

In every one of the four, the source lineage keeps both owning turns, still
owns exactly its own background execution, and terminalizes it exactly once in
its own lineage: a lineage publication is not a conversation-store transition
and never leaves the source half-written.

## 8. Background / subagent recovery

| Boundary | Durable before kill | Recovery action | Forbidden | Test |
| --- | --- | --- | --- | --- |
| `after:event:background_execution_committed`, real `sleep 300` child alive | one ownership fact | terminalize and publish exactly once; a repeated recovery adds nothing | a second terminal; a reattached or relaunched owner | `committed_background_ownership_terminalizes_exactly_once` |
| the same, followed by a full reopen | one ownership fact | one terminal | a second ownership commit; resurrecting the dead process's execution | `a_reopened_runtime_never_relaunches_a_dead_background_execution` |
| `before:event:background_terminal_published` | ownership, terminal candidate known | publish the terminal exactly once | a half-published lineage terminal | `background_terminal_publication_is_atomic` |

The killed child's whole process group dies with it, so the `sleep 300` is a
genuine orphan candidate: the proof that nothing reattaches it is that the
reopened runtime commits no second ownership and starts no second process.

Subagent ownership is proven **directly**, not by analogy with the background
rows: the subagent plane has its own durable ownership and terminal facts, its
own recovery evidence and reconciliation, its own ordinal domain, and its own
child-process lifecycle, so none of those is covered by proving the background
plane.

| Boundary | Durable before kill | Recovery action | Forbidden | Test |
| --- | --- | --- | --- | --- |
| `after:event:subagent_ownership_committed`, real child process alive | one ownership fact, no terminal | terminalize and publish exactly once; the ordinal domain is recovered for reseeding | a second terminal; a reattached or relaunched child | `committed_subagent_ownership_terminalizes_exactly_once` |
| `before:event:subagent_terminal_published`, terminal candidate known | ownership, no terminal | publish the terminal exactly once | a half-published lineage terminal; a second publication | `subagent_terminal_publication_is_atomic` |
| the same, followed by a full reopen and a live turn | one ownership fact | one terminal | a second ownership commit; re-adopting the historical `SubagentId` ordinal | `a_reopened_runtime_never_readopts_a_dead_subagent` |

The child process is real, runs in the conformance child's process group, and
is reaped by the same `killpg`. What the harness replaces is only the spawn and
the startup handshake, through `SubagentRegistry::push_staged_override` — the
same `cfg(test)` seam the in-crate registry tests use — because a conformance
child's `current_exe()` is the test binary rather than `rustx`. Everything the
rows above assert is the real path: the real `subagent` intrinsic, the real
registry commit, the real durable ownership transaction, the real driver and
control channel, the real recovery evidence, and the real terminal
publication.

### Nested supervised process units inside a subagent child (Issue #145)

A subagent child that runs Bash, MCP stdio (a synthesized `python:<folder>`
server included), or Skill environment
work owns supervised units whose inner `setsid()` group is deliberately
outside the child's own process group, so killing the child's group cannot
reach them. The generic anchor protocol makes every death boundary decidable
without scanning:

| Boundary | State at the kill | Required outcome | Forbidden | Test |
| --- | --- | --- | --- | --- |
| child dies **before** the anchor ACK | offer outstanding | the inner never received `START`, so the semantic command was never spawned; the unit settles as a process-control failure | starting the command anyway; reporting success | `a_refused_anchor_never_starts_the_nested_command` |
| child dies **after** the anchor ACK | the parent holds the exact `pgid` | on Linux the parent contains the adopted group (`WNOWAIT` retention, one anchored `SIGKILL`, group-scoped `ECHILD`) before it publishes settlement | publishing settlement on the direct child's reap alone; signalling a `pgid` whose identity is not retained | `a_committed_child_settles_its_nested_anchors_before_publishing` |
| pre-commit rollback with a retained anchor | `StagedChild` owns child + anchors | the rollback kills and reaps the direct child, then contains every retained anchor; an unprovable one returns `RollbackError::NestedContainment` | reporting a rolled-back ownership decision while owned work may be alive | `staged_rollback_contains_every_retained_nested_anchor` |
| several units, one proves terminal | multiple retained anchors | exactly that unit's anchor is released; every other retention survives | correlating releases by ordering or by approximate `pgid` | `releasing_one_unit_removes_only_that_anchor`, `an_offered_anchor_is_retained_before_it_is_acknowledged` |
| the platform cannot adopt the orphaned anchor | anchor retained, `waitid` answers `ECHILD` | the settlement is published as explicitly unproven and appended to the terminal's bounded diagnostic | claiming Linux-level terminality; signalling the cached `pgid` anyway | `an_unadoptable_anchor_is_reported_unproven` |

The Linux prerequisite (`PR_SET_CHILD_SUBREAPER`) is established **before**
the child is spawned — a subreaper installed afterwards does not retroactively
adopt — and a spawn that cannot establish it fails with
`SpawnError::ContainmentPrerequisite` rather than claiming containment
authority it does not have
(`the_containment_prerequisite_precedes_child_staging`).

## 9. Transcript recovery

After a real process death and reopen, the derived transcript still pages
committed user messages, canonical Assistant/`ToolResult` history, the
interaction audit, both publication-audit kinds, and history retired from the
current Surface by compaction — `the_transcript_pages_every_durable_owner_after_process_death`.

Client presence at crash time is not part of the durable contract: a child
composed with **no** observation bridge at all yields the same canonical shape
and the same audit settlement as one with a client-facing consumer attached —
`client_presence_at_crash_time_changes_no_durable_result`.

## Combination scenarios

Bounded, chosen to prove that independently owned planes compose, not to
enumerate the state product.

| Scenario | Composition proven | Test |
| --- | --- | --- |
| streaming model + inbound accepted mid-stream + kill before P | the open stream settles `Incomplete` while the new inbound stays ordinary pending work; `BlockedIndeterminate` blocks continuation, not acceptance | `streaming_output_and_pending_inbound_compose` |
| background-capable turn + streaming publication + kill before C | one audit settlement, no execution, no background ownership to reattach | `background_capable_turn_and_streaming_publication_compose` |
| settled tool result + continuation request in flight + new inbound | the settled result stays canonical and unrepaired, the continuation is indeterminate, the new inbound stays pending | `settled_tool_result_and_new_inbound_compose_with_an_indeterminate_request` |
| approval + tool-start boundary + process death | `settled < started`, unknown outcome preserved, exactly one settled audit | `approval_settlement_and_the_tool_start_boundary_compose` |
| R1 request history + external edits + reload / cold reopen to R2 | §7 rows above | `death_around_the_reload_publish_boundary_reloads_from_scratch`, `cold_resume_uses_current_resources_and_preserves_history` |

## Defect found and fixed

`adopted_inbound_is_canonical_and_no_longer_pending` exposed one real contract
gap that only a real process-death boundary can reach.

Canonical adoption commits **before** the admitted attempt publishes its
`AttemptStarted` fact. A process that dies in that window leaves an
adopted-but-unanswered canonical human turn with *zero* attempt evidence, and
the reopened runtime — finding no pending inbound — would never answer a
message it had already accepted into canonical history.

The first fix was local: let that one attempt class continue, and exclude a
lineage whose immutable bootstrap prefix ends in a human message. It was
wrong in the same way the original bug was wrong, because it still *inferred*
an answer obligation from canonical shape. Two counterexamples survive it, and
both are ordinary:

```text
turn 2 of any conversation      attempt 0 settles, message 2 is adopted, the
                                process dies before AttemptStarted
                                -> attempt facts exist, so the class is D
                                -> the trailing message looks exactly like the
                                   answered one

drain into a live attempt       the attempt answers turn 1, drains message 2 at
                                its safe boundary, and dies before the next
                                request start
                                -> the attempt's request plane reports the
                                   previous request's *known* outcome, so the
                                   class is E
```

The real fix is a durable **answer obligation** owned by the transition that
creates it. `RuntimeEvent::InboundTurnAdopted` is committed inside the
canonical adoption transaction, naming exactly the messages it adopts, and the
store rejects any adoption whose obligation names anything else. It is consumed
by the first `ModelRequestStarted` that carries the turn to the provider, or by
the first attempt terminal that **decides** the turn — whichever commits first.
That is the explicit persistent ownership chain across adoption, request start,
and attempt terminal, and it makes the rule uniform:

```text
an external outcome is indeterminate  -> BlockedIndeterminate
otherwise, an obligation is open      -> ContinueAdoptedTurn
otherwise                             -> PendingInboundOnly
```

### Which terminals decide the turn

Every terminal a *live* runtime commits decides the turn: completion,
cancellation, timeout, limit exhaustion, and an ordinary runtime or model
failure all mean the runtime finished with it, answered or not.

`RuntimeError::RestartInterrupted` is the one terminal no live runtime ever
commits — recovery writes it, about an attempt whose process-local execution
state is gone. Counting it as a decision would make recovery destroy its own
permission, and the destruction is durable:

```text
adopt turn 2, AttemptStarted, SIGKILL
  recovery #1 -> ContinueAdoptedTurn, and commits AttemptFailed(RestartInterrupted)
  the continuation starts a new attempt and dies before ModelRequestStarted
  recovery #2 -> the journal holds an adopted canonical turn, a terminal
                 attempt, and no obligation; nothing is pending, so nothing
                 ever re-admits the message
```

The obligation therefore **survives** a `RestartInterrupted` terminal and
transfers to whichever attempt continues the turn next. Recovery stays
absorbing in every other respect — no second terminal, no re-adoption, nothing
resent — and an arbitrarily long chain of deaths in the
adoption/request-start window never strands an accepted message.

Recovery therefore continues exactly the turns a live runtime would still owe
an answer for. A turn the user cancelled before any request stays unanswered
across the restart, because that terminal *did* decide it — exactly as it does
live. Supplied bootstrap history enters through
`initialize`, which is not an adoption and commits no obligation, so the
bootstrap-prefix probe (and its `load_bootstrap_history()` read, which
deserialized a whole forked prefix to compute one boolean) is deleted rather
than optimized: the evidence is one `Option<MessageId>`.

The rows that prove it, all under a real `SIGKILL`:

| Claim | Test |
| --- | --- |
| adoption/attempt-start window | `adopted_inbound_is_canonical_and_no_longer_pending` |
| a later turn of a multi-turn conversation (Class D) | `a_turn_adopted_after_a_settled_attempt_continues` |
| a turn drained into a live attempt (Class E) | `a_turn_drained_into_a_live_attempt_continues` |
| an answered conversation is never re-answered | `an_answered_turn_is_not_continued_after_reopen` |
| the obligation survives recovery's own terminal, across a second real death | `an_adopted_turn_survives_the_recovery_terminal_of_a_second_death` |

with the consumption rules, the cancelled-turn case, and indeterminacy
dominance proven beside the abstraction in `src/runtime/recovery.rs`, and the
seeded-lineage case (a fork/clone/tree seed owes nothing, but the same lineage's
own adopted turn does) in
`tests/durable/recovery.rs::a_seeded_lineage_owes_no_answer_for_its_bootstrap_prefix`
— and its real-process-death counterpart in §7 below.

The obligation's survival rule is proven twice more in
`tests/durable/recovery.rs`, at the durable-fact level:
`the_answer_obligation_survives_a_chain_of_recovery_terminals`,
`a_decided_terminal_still_consumes_the_answer_obligation`, and
`a_known_outcome_terminal_transfers_the_obligation_of_a_later_adopted_turn`.

Because the obligation is now a durable fact recovery depends on, a database
written before it existed can no longer be read: its adoptions committed no
obligation, and reading that silence as "no answer is owed" would strand
exactly the crash states above. `SQLITE_SCHEMA_VERSION` therefore moved 9 → 10
for the structured Questionnaire audit vocabulary, 10 → 11 for typed Agent
Status generation metadata, and 11 → 12 for the complete
canonical-message-coupled Agent Status emission facts, bounded Todo
latest-emission heads, and the Todo-specific progress sequence. Version 11 and
older files are refused at open —
`pre_answer_obligation_schema_is_rejected_explicitly`,
`pre_structured_questionnaire_schema_is_rejected_explicitly`, and the status
schema gate coverage.
The envelope's own `EVENT_SCHEMA_VERSION` is unchanged: adding a
`RuntimeEvent` variant changes what a journal may contain, not how an envelope
is framed.

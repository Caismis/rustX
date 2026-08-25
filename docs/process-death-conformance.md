# Process-death conformance (FND-06 / Issue #111)

FND-01 … FND-05 established the durable runtime contracts. This document is
their proof against **real process death**: a real child process running the
real runtime stack over a real durable file, frozen at a named durable
boundary, ended with an uncatchable `SIGKILL`, then reopened and recovered.

The suite is `tests/scripted/issue111_process_death/` (42 conformance tests
plus the child entry point). Every row of the tables below names the test that
proves it.

## Why the child is the crate's own test binary

A conformance child must run the real `ConversationRuntime`, the real durable
`SQLite` authority, the real Tool Plane, the real publication plane, and the
real filesystem resource loader, while also using two seams that must never
exist in the published API:

- the scripted provider adapter (`ScriptedProviderAdapterFactory`), so a model
  turn is a fixed sequence of canonical events rather than a network call;
- the process-death boundaries in `crate::runtime::process_death`.

`tests/scripted/mod.rs` already explains why such suites compile into the
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
| `after:adopt_pending_batch` | canonical User | canonical User, pending ∅ | both pending and canonical | Class A + `ContinueAdoptedTurn` (see “Defect found”) | `adopted_inbound_is_canonical_and_no_longer_pending` |
| `before/after:commit_canonical_publication` | ∅ / canonical Assistant ×1 | exactly zero or exactly one Assistant | a partial Assistant | none | `assistant_canonical_commit_is_atomic` |
| `before/after:append_canonical_batch` | ∅ / `ToolResult` batch | whole batch or none | a partial sibling batch | repair only when absent | `tool_result_batch_commit_is_atomic` |
| `before:event:background_terminal_published` | ownership only | no terminal message | a half-published lineage terminal | publish the terminal exactly once | `background_terminal_publication_is_atomic` |
| `before/after:commit_compaction` | see §6 | see §6 | see §6 | see §6 | `compaction_surface_replace_is_atomic` |

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
| audited output vs. later model context | `Unaccepted` audit | the audit entering the Ledger or a later `RequestSnapshot`'s frozen context | `a_publication_audit_never_reenters_model_context` |

## 4. Tool external-outcome recovery

| Boundary | Durable before kill | Repaired canonical result | Resume | Test |
| --- | --- | --- | --- | --- |
| `before:event:tool_execution_started` | canonical Assistant with the call | `Cancelled { ParentCancelled }` | `PendingInboundOnly` | `kill_before_tool_execution_start_authorizes_nothing` |
| `after:event:tool_execution_started` | start committed, outcome unknown | `Interrupted` — never inferred from workspace state | `BlockedIndeterminate` | `started_tool_with_unknown_outcome_stays_unknown` |
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
| `SIGKILL` at `reload:prepared` / `reload:published` | R1 (+ an unpublished/just-published R2) | before the reload | the reload build/publish boundary | no request was admitted under a half-published generation; the reopened process performs a normal fresh load and sends pure R2 | `death_around_the_reload_publish_boundary_reloads_from_scratch` |
| cold resume after external edits | R1 → R2 | while the process is dead | reopen | the first new request is R2; the historical Ledger, transcript order, and old `RequestSnapshot` (prompt + Tool definitions) are unchanged; no replacement message is appended | `cold_resume_uses_current_resources_and_preserves_history` |
| deleted already-discovered Skill | R1 → R2 | between two processes | reopen | the historical `ToolResult` keeps the old body by value; a new `Read` returns the normal read error | `a_deleted_skill_leaves_history_by_value_and_reads_as_an_error` |
| invalid current resources | — | config corrupted before reopen | runtime creation | creation fails explicitly; the historical generation is never used as live authority and no request is admitted | `invalid_current_resources_fail_runtime_creation` |
| death before the first `ModelRequest` | R1 loaded, never recorded | after the death | reopen | no `RequestSnapshot` and no canonical resource/Skill-guidance fact exists; reopen simply loads current resources | `death_before_the_first_request_leaves_no_resource_record` |

The current `RuntimeResourceSnapshot` is process-local, not durable recovery
authority. That is what makes the reload-boundary row provable: there is no
durable half-generation to recover, only a fresh load.

## 8. Background / subagent recovery

| Boundary | Durable before kill | Recovery action | Forbidden | Test |
| --- | --- | --- | --- | --- |
| `after:event:background_execution_committed`, real `sleep 300` child alive | one ownership fact | terminalize and publish exactly once; a repeated recovery adds nothing | a second terminal; a reattached or relaunched owner | `committed_background_ownership_terminalizes_exactly_once` |
| the same, followed by a full reopen | one ownership fact | one terminal | a second ownership commit; resurrecting the dead process's execution | `a_reopened_runtime_never_relaunches_a_dead_background_execution` |
| `before:event:background_terminal_published` | ownership, terminal candidate known | publish the terminal exactly once | a half-published lineage terminal | `background_terminal_publication_is_atomic` |

The killed child's whole process group dies with it, so the `sleep 300` is a
genuine orphan candidate: the proof that nothing reattaches it is that the
reopened runtime commits no second ownership and starts no second process.

Subagent ownership is deliberately **not** exercised as a separate row.
`SubagentRegistry` commits ownership only after the child startup handshake
succeeds, and a conformance child's `current_exe()` is the test binary rather
than `rustx`, so no honest ownership-committed boundary exists in this harness.
The lifecycle points the issue names for subagents — ownership committed,
terminal candidate known, terminal published — are the same durable transitions
the background rows above prove, through the same `append_event` and
`accept_inbound_with_event` transitions and the same recovery reconciliation.

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
adopted-but-unanswered canonical human turn with *zero* attempt evidence.
`RecoveryPlan::classify` applied the `awaits_model_turn` continuation guard to
Class B only, so this state classified as Class A with
`ResumeDisposition::PendingInboundOnly` — and the reopened runtime, finding no
pending inbound, would never answer a message it had already accepted into
canonical history.

The fix is in the owning abstraction, not in the test: Class A now also reaches
`ContinueAdoptedTurn`, under a stricter guard than Class B's. That is sound
because Class A here carries *strictly weaker* external history than Class B —
no attempt ever existed, so no `ModelRequestStarted` and no
`ToolExecutionStarted` can exist either — and the runtime already consumes
`ContinueAdoptedTurn` without caring which class produced it.

The stricter guard is the important half. Class A's permission requires the
trailing human message to be one this conversation durably **adopted**, so a
lineage whose *immutable bootstrap prefix* ends in a human message — a
fork/clone seed, a persona lineage, a fixture-seeded runtime — is excluded.
Supplied history is context, not work rustX accepted, and answering it was never
a promise this conversation made. `RecoveryEvidence` therefore reads the
bootstrap prefix identity of the trailing message and
`awaits_adopted_model_turn` excludes it. Both branches also have pure
classification tests beside the abstraction, in `src/runtime/recovery.rs`:
`an_adopted_unanswered_turn_without_attempt_evidence_continues` and
`a_bootstrap_trailing_human_message_is_not_an_adopted_turn`.

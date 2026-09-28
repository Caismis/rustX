# Native Turn process ownership — PR #409 follow-up

The terminal marker previously preserved only outcome, identity and clock. It did
not attach committed process rows to that Turn. A failed Turn could therefore
render as unrelated reasoning/tools followed by a zero-count Failed control.

## Native authority and control invariant

`TurnProcessView` replaces both `CompletedProcessView` and `TerminalTurnView`.
Its identity is the original `(ConversationId, AttemptId)`, independent of the
closing message, terminal event position, loaded page or active Attempt. The
shared finite Event Journal fold owns running, completed, cancelled, failed,
timed-out and limit-exceeded projections. `RuntimeClientAttempt` remains the
live execution/interaction projection; it is not another historical Turn owner.

Every exact committed Assistant member receives the same native process summary.
Tool results receive it through their native `(assistant_message_id, block_index)`
occurrence, never their display name or provider call ID. Membership comes from
`AssistantMessageCommitted` events scoped by Attempt; the accepted canonical
blocks supply counts. Message count includes committed Assistant process messages
and excludes a successful closing answer; Tool count includes canonical Tool-call
occurrences, not Tool results. rustX does not currently split this count into a
separate subagent presentation count.

The native control boundary is immediately before the minimum immutable transcript
cursor among the Attempt's committed Assistant members. A content-free terminal
Turn uses its own terminal event's transcript cursor. The first committed process
member establishes the durable seat; settlement does not move it to the end.
Both member and terminal-item projections carry the same boundary, whole-process
counts, outcome, native start/end clock and original identity. A terminal event is
still a durable ordering fact, but is no longer a separate presentation process.
No extra persistence, second history store or browser inference is introduced.

Web creates one keyed control from the supplied native owner and sorts it at that
native boundary. If the anchor is outside the bounded page, its control still
sorts before the loaded suffix. Loading earlier reveals more members without
changing the owner or boundary. Each page is independently resolvable, including
pages with only a Tool result or only a terminal event. Native pending-response
refresh policy still rebases stale live rows rather than retaining old outcomes.

Failed/cancelled/timed-out/limit-exceeded controls are always open and disabled;
they own their actual process content and native counts. Settled live execution
without a native terminal projection still cannot create historical failure.
Independent inbound and Agent Status anchors retain their current positions.

## Successful response and lineage policy

Successful Turns use the same process summary, while `CompletedResponseView`
continues to own the exact finalized answer, response boundary, immutable origin,
usage/timing, retry input and TurnTail. Existing successful disclosure/status
placement, inline reasoning and direct Copy/Fork/Branch/Retry remain intact.
Native counts are available even when earlier members are outside the page.

Lineage deliberately remains selective. Canonical and Surface history and
immutable **completed-response** provenance cross fork/branch boundaries.
Unsuccessful source execution facts do not. Inherited unfinished process messages
remain canonical content, but no child Failed/Stopped process is fabricated.
This distinguishes a portable finalized result from the source execution that
stopped without finalizing one; children still start a new execution epoch. The
terminal transcript reference is not included in canonical lineage bootstrap.
The shared lineage/remapping regression exercises actual content, child SQLite
reopen, absence of source outcome in the child, and unchanged source ownership.
Existing multi-generation successful provenance regressions remain in force.

## Harness comparison

Read-only checkout: `/home/caismis/Documents/codes/deepseek-harness`.
Reconfirmed commit: `477b4f420553e8a52c2fbccc464d7561b239c443`.
Re-read:

- `packages/client/ui-chat/src/client/chat/TurnProcessNodeView.tsx`
- `packages/client/ui-chat/src/client/contract/turn-process.ts`
- `packages/client/ui-chat/src/client/conversation-nodes/turn-process.ts`
- `packages/client/ui-chat/src/client/conversation-nodes/turn-process-presentation.ts`

Harness retains Turn process specifications, process ranges and counts after
aborted/error endings; `turnProcessAlwaysOpen` disables collapse without removing
ownership. rustX maps that contract to native Attempt/Journal and transcript
identities. `control_cursor` is the native control boundary; exact per-entry
ownership replaces Harness sequence-range membership. Optional `final_message_id`
and unchanged completed-response provenance supply the successful answer boundary.
No Harness runtime or source code was imported in this correction.

## Regression evidence

- Native `terminal_turns_survive_reconstruction_later_attempts_and_paging`:
  all four outcomes with Attempt/Turn start, reasoning, Tool call/result and
  intermediate Assistant output; two messages and one Tool; exact clocks and
  identities; later success and running Attempts; reopen; 1/2/5/64-entry pages;
  complete JSON wire round trip.
- Native `empty_terminal_uses_its_own_native_cursor`: no-content seat and counts.
- Native `process_membership_uses_exact_attempts_across_steering_and_pages`:
  interleaved Attempt identity and successful answer ownership.
- Native `unfinished_process_content_crosses_lineage_without_source_execution_outcome`:
  explicit selective lineage policy, including reopened child and stable source.
- Web `turn-process.test.tsx`: all four outcomes with actual reasoning, Tool
  occurrence/result and intermediate text; native counts, exact owner attributes,
  one disabled/open control, same DOM control as earlier pages load, isolated
  member and terminal pages, later Attempt, JSON reconstruction and remount.
- Existing `conversation-residency.test.tsx` retains no-client-terminal-inference,
  real shell function isolation and local-error regressions.
- App Server v24 generated schema/TypeScript and Runtime Client v49 replace their
  predecessors without compatibility shims. All Web/TUI consumers move together.

## Main integration

Fetched and merged `8b59e770225cf8ae39bfdfe2a6e50b0137bfa146` before final
validation. The merge had no textual conflicts. Semantic audit confirmed all
#398 cap-std changes in Cargo.toml, Cargo.lock, uploads.rs,
uploads/cap_std_validation.rs and its validation document exactly match main.
The correction does not touch MCP runtime behavior or test policy.

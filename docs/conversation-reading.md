# Native conversation reading

The durable ConversationStore owns the conversation turn outline and exact
transcript locations. A user-facing turn is one native Attempt, matching the
existing TurnProcess and conversation-statistics vocabulary. Its identity is
`(origin ConversationId, AttemptId)`. Ordinals are one-based presentation indexes
at a read cut, never identity. Model retries, logical steps and Tool calls do not
create turns. Automatic continuation creates a distinct native Attempt and turn.
The existing statistics fold counts `AttemptStarted` as a turn and `TurnStarted`
as a logical step; TurnProcess groups members by Attempt. Lineage reading
provenance preserves origin turns without inventing destination execution facts.

Local order is indexed `AttemptStarted` Journal order. Failed, cancelled,
interrupted, timed-out and limited Attempts retain that identity. A location is
the first committed assistant process member, or the exact Attempt terminal when
there is no assistant member. A just-started Attempt with neither has no location;
the live Runtime Client still supplies its current identity; historical locate
remains disabled. Preview is the first text/refusal block at that location,
limited to 240 Unicode characters; no preview is fabricated for a terminal.

`TurnReadingProvenance` is a separate immutable lineage/bootstrap domain from
`CompletedResponseProvenance` (finalized response, Retry, timing and usage).
The durable owner captures native Attempt origins in start order; Session
remaps only retained Assistant members and the canonical predecessor of a
terminal-only location. It preserves historical outcome and native clock information,
including failed/interrupted/cancelled/timed-out/limited work. No Journal events,
requests or executable state are copied. An interrupted retained process is a
turn even when it has no finalized response; genuinely unowned content remains
unowned and creates no turn.

## Lineage copy authority: (R, C)

R is the selected `SurfaceRevision`, the structural Surface cut. C is the
invocation-time native `ConversationReadCut`, the temporal read cut.
`ConversationStore::read_lineage_cut(R)` returns one `LineageReadCut`; its first
SELECT in one SQLite read transaction is the linearization point that captures C.
R fixes Surface operation history, the canonical Ledger identity closure reachable
from that history, and retained canonical messages. C fixes Attempt start
visibility, historical outcomes, completed-response provenance and terminal-only
settlement known at that point. Both are read in that same snapshot. Session never
assembles this authority from independent current-state reads.

A SurfaceRevision is not a complete execution-history timestamp. Copying the same
R while retained Attempt A is running at C1 inherits A as `IncompleteAtCut`;
copying R after A terminalizes at C2 inherits the genuine terminal outcome.
Similarly, a terminal-only Attempt absent at C1 may be present at C2 even if its
predecessor already belonged to R. Same R, different C, different inherited
temporal meaning is intentional. Once `(R, C)` is captured, later source activity
cannot change that copy, including while copy preparation is parked.

Message-backed turns require a native member through C AND at least one retained
member in the structural closure. Terminal-only turns require terminal sequence
`<= C.journal` AND a predecessor in the retained structural prefix (or the native
before-all-content position); their predecessor then decides placement.
A predecessor existing before C never admits a terminal committed after C.
Terminal publication winning the first SELECT is visible; publication losing it
is excluded even if copy preparation is parked and resumes after settlement.
No later Journal or Ledger append can enter the frozen seed.

`InheritedTurnOutcome` has no `Running` variant. Durable output from a source
Attempt still running at C retains its origin and mapped location with
`IncompleteAtCut`, without an invented terminal or end timestamp. That immutable
state remains incomplete after source settlement, reopen and repeated lineage.
It projects as `TurnProcessOutcome::IncompleteAtCut`, with no live timer or
failure/cancellation claim. Destination execution owns no inherited Attempt,
request, Tool or settlement. Genuine terminal outcomes before C remain unchanged.

Clone copies all selected structure through R and temporal evidence through C.
A later clone of the same R may differ because it captures a newer C. Fork and
same-Session tree branch additionally narrow that frozen structure at their
message boundary; a turn must satisfy both temporal and structural membership.
They cannot widen C. All three call `lineage_cut` / `remap_seed` and initialize
one `LineageSeed`. Retained turns keep immutable origin identity in bootstrap
array order; local destination Attempts follow every inherited turn in local
`AttemptStarted` order. Retained members map to destination MessageIds and exact
transcript positions. For a terminal-only turn, a native `inherited_turn` spine
reference is inserted after its retained canonical predecessor (before all
content if none). Clone retains all such native terminals; a prefix fork/branch
retains one only when its predecessor belongs to that prefix. A fork before the
first input retains no turns. References resolve the minimal bootstrap summary,
never fabricated execution events or copied message bodies. Repeated lineage
and reopen retain these same origins, ordering and destination mapping.
Compaction changes the canonical Surface, not these durable origins/order.
`session/boundaries` stays specific to user-message lineage/fork/retry cuts.

## Typed native reads

`session/turns {target, cut?, offset?, limit}` returns `ConversationTurnPage`:
exact cut, total, offset and at most 64 ordered native turns. Omitted offset selects
the final page; an explicit offset directly indexes history. Each mark carries its
origin, ordinal, bounded preview and optional exact transcript cursor.

`session/transcript {target, at, limit}` replaces the old root `before` request
with one selector vocabulary and returns `transcript_window`:

- `latest`: newest finite page at the current cut;
- `older {before, cut?}`: ordinary older read, or cut-bound historical adjacency;
- `newer {after, cut}`: first finite page after a historical window;
- `turn {id, cut}`: first finite page starting at the exact native turn location.

All root windows are 1–64 entries. The result carries the cut, page, actual
older/newer cursors and optional exact target identity/cursor. A direct locate
reads no intervening transcript pages. Agent/internal transcript APIs retain their
separate registry and read-model ownership.

A read cut C is `(ConversationId, journal, transcript, mutation_revision)`.
`journal` and `transcript` are inclusive historical upper bounds: only Attempt
starts/locations and projection facts at or below C may be read. They are not
requirements that today's frontiers equal C. `mutation_revision` is the native
monotonic epoch of successful pending-body edits/removals. Pending adoption
preserves its accepted body and position; new admissions and ordinary execution
append facts. Surface compaction preserves immutable Ledger/Journal meaning.
These append-only transitions do not invalidate C, even during a running Attempt.

SQLite accepts C when its Conversation matches, its upper bounds are no later
than current frontiers and its mutation epoch equals the current epoch. The
store cannot reconstruct an edited/removed pending body after a mutation, so
that semantic change rejects the old cut rather than silently substituting a
new body. Future/foreign cuts, a target absent at C, a target whose first native
location did not yet exist at C, and unavailable/corrupt native references fail
explicitly without nearest-target fallback. Attachment/runtime replacement is
separately rejected by App Server's `AttachmentTarget` and browser authority
fences; a read cut never grants control authority.

Target and outline reads can be reconstructed after later appends. Selection
happens in one SQLite transaction, bounded to `transcript`; Tool results are
included only if their own canonical transcript position belongs to C. Runtime
Client folds only the selected native Attempts through `journal` for response,
process, terminal, timing and usage decorations. A final compatibility check
rejects a concurrent semantic mutation while accepting concurrent appends.
The result returns C exactly. Neither current response settlement nor later
Tool results can leak backward into that window.

`older` and `newer` with C page exclusively inside that same frozen prefix;
older/newer availability also excludes appended positions above C. Reaching
C's tail ends newer paging even if current execution has advanced. `latest`
and `older` without a cut capture fresh bounds. A fresh outline is required to
see new identities/locations or recover from an invalid mutation epoch or
replacement authority, not to navigate a known historical address after each
streamed event. Reads execute no model/Tool work and create no request snapshots
or durable writes.

## Browser reading and scrolling

The browser retains one outline page of at most 64 marks/previews, plus at most
one pinned current identity from live Runtime Client state. `turn-rail-items.ts`
merges these native sources; a current identity can have no historical cursor.
Loaded transcript/process anchors remain a separate presentation capability.
The first native process `control_cursor` absent from the outline cut triggers
one refresh to make the current turn locatable before settlement. Subsequent
same-Attempt appends/text deltas do not refresh the outline. A current mark is
active in follow mode even before a durable anchor exists. Detached reading
publishes the native semantic owner of the rendered region at the reading position.
`data-chat-turn-owner` carries Conversation/Attempt identity independently of the
exact `data-chat-anchor-key`: a finite window beginning inside A retains A ownership
without fabricating its missing `control_cursor` location. Owned regions remain
active through separate body, Tool output and tail until another rendered boundary
reaches the reading position. The final owned region stays active beyond its start
marker. Genuinely unowned rows publish explicitly unknown detached ownership;
the rail cannot substitute a stale navigation target or unrelated live Attempt.
Only an exact native anchor satisfies a navigation scroll. Return to latest
restores the current live identity and follow mode through ChatViewport. Paging and an
ordinal input reach distant pages without accumulating outline history. Transcript
cache limits remain 512 entries / 8 MiB; ordinary prepend reads are 64 entries.
Historical jumps and adjacent pages replace a finite window. Two outstanding
window reads per Session are permitted; further intents retire older work without
queuing another read. A same-cut loaded exact anchor avoids another locate read.

Outline paging intent is explicitly `latest` or `page(offset)`, independently of
the native response's offset. Initial load and automatic start/location/settlement
refreshes in latest mode omit the offset, so 64→65 and 128→129 select the new final
page. Older, intermediate newer and ordinal selections establish an explicit
page; automatic refresh preserves it as live turns arrive. Selecting the newest
page explicitly restores latest intent. One refresh demand arriving during an
in-flight outline read is retained and serviced after its reply; streaming text
deltas create no demand. The cache still holds only one outline page.

Every window read captures the attachment target, connection generation,
attachment epoch, resync authority, cache epoch and monotonic user reading intent.
Only the newest still-valid reply installs a window/active mark or requests a
scroll commit. Session/node/runtime changes, resync and user scrolling retire old
replacement work. Ordinary prepend can finish after user detachment: it merges
native rows while ChatViewport preserves the newer reading position. A new
navigation/action still retires that prepend. Outline reads have independent paging intent and survive window changes;
resync retires both domains.

ChatViewport is the sole automatic Chat scroll writer. The rail and Return to
latest share its existing overlay surface, without a second flex wrapper or
scroll coordinator. It coalesces React and
ResizeObserver changes into one animation-frame commit. Explicit navigation
receives an intent ticket and resolves an exact native anchor in that frame.
Later native scrolling synchronously retires the ticket. Detached reading retains
stable rendered anchors across prepend, width/sidebar/panel reflow, Tool disclosure
and image/content growth. Missing anchors use retained adjacent reading anchors
then a clamped absolute position; they never restore follow mode.

Ordinary scrolling away from the live bottom exposes Return to latest even for
short history. Historical navigation also exposes it and suppresses live output
within that historical window. The explicit action restores the latest snapshot
window and frame-owned bottom/follow mode; subsequent streaming follows until
another user scroll or navigation detaches. The HISTORY_LIMIT recovery remains
an independent bounded-cache case.

## Width preference

ConversationWidthControls measures the actual conversation column with
ResizeObserver and publishes the shared transcript/composer/card CSS width axis.
Pointer capture and one drag frame provide live symmetric adjustment. Pointer-up
with at least two pixels of deliberate movement commits the displayed width once;
Escape, cancellation and lost capture restore the saved preference. Keyboard
arrows (16 px, Shift 64 px), Home and End are explicit commits.

Temporary layout clamps never persist. Restoring column space recovers the saved
preference. Missing, malformed or throwing storage is ignored safely. Desktop
handles are exposed only when the column has usable margin, with slider labels
and values; narrow columns clamp to available space and hide handles. Reduced
motion disables rail animation. Width changes create no Session/runtime state or
request; ChatViewport preserves the reading anchor through the resulting reflow.

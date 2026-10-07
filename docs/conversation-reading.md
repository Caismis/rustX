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
the live Runtime Client still supplies its current identity; navigation to it
remains disabled.

Each turn carries two native previews, matching DeepSeek Harness's turn outline.
`prompt` is the turn's first human prompt: the earliest User message with Human
source and ordinary Message kind adopted with this Attempt, or adopted while idle
after the previous Attempt started and before this one did. Steering of the
previous turn carries that turn's identity and never opens this one; automatic
continuation has no prompt. Queued inbound reserves its transcript position at
acceptance, so transcript adjacency is never used to find a prompt. `response`
is the newest text/refusal-bearing Assistant member, present only once the turn
has a terminal at the cut. Both join text blocks with single spaces, collapse
whitespace and end with an ellipsis when clipped: at most 50 and 120 Unicode
characters (one and three rail-card lines). Inherited prompt previews use the
earliest text-bearing Human input explicitly named by the turn's
`TurnReadingProvenance.prompt_message_id` or its completed response's native
`retry_message_id`. Both relationships are remapped to destination identity on
copy and dropped when the input is outside the retained cut. An opening prompt
precedes later steering input; when only response replay input is retained, that
recorded input provides the preview. Neither nearby User messages nor live reads
of the source Session establish ownership. The inherited response is
the newest text-bearing retained member of a settled inherited turn.

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
request, Tool or settlement; each inherited turn only carries its origin Attempt's
recorded execution totals for whole-conversation statistics and occupancy. Genuine terminal outcomes before C remain unchanged.

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

`session/turns {target, offset?, limit}` returns `ConversationTurnPage`: the
cut it was captured at, total, offset and at most 64 ordered native turns.
Omitted offset selects the final page; an explicit offset directly indexes
history. Each mark carries its origin, ordinal, bounded prompt and response
previews and optional exact transcript cursor.

`session/transcript {target, at, limit}` reads one contiguous transcript
vocabulary and returns `transcript_window { page }`:

- `latest`: the newest finite page;
- `older {before}`: the finite page strictly before an already-read cursor.

All root pages are 1–64 entries; `next_cursor` is present while older history
exists. A turn's native cursor from the outline is a position in this same
order, so a reader reaches any turn by paging `older` until its page covers that
cursor. Agent/internal transcript APIs retain their separate registry and
read-model ownership.

A read cut C is `(ConversationId, journal, transcript, mutation_revision)`, the
inclusive upper bounds one read was captured at. `mutation_revision` is the
native monotonic epoch of successful pending-body edits/removals. Pending
adoption preserves its accepted body and position; new admissions and ordinary
execution append facts. Surface compaction preserves immutable Ledger/Journal
meaning. Every read captures a fresh C; no request names a past cut.

Selection happens in one SQLite transaction, bounded to C's `transcript`; Tool
results are included only if their own canonical transcript position belongs to
C. Runtime Client folds only the selected native Attempts through C's `journal`
for response, process, terminal, timing and usage decorations. A final
compatibility check rejects a semantic mutation that landed during the read,
while accepting concurrent appends. Reads execute no model/Tool work and create
no request snapshots or durable writes.

## Browser reading and scrolling

The browser retains one outline page of at most 64 previews, plus at most one
pinned current identity from live Runtime Client state. The rail draws a
fixed-pitch virtual mark for every turn the outline counts: marks inside the
retained page are loaded, the rest are known by ordinal only and read their
native page before navigating. The pinned identity is drawn only while the
retained page is the newest one. `turn-rail-items.ts`
merges these native sources; a current identity can have no cursor yet.
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
restores the current live identity and follow mode through ChatViewport. An
unloaded mark reaches a distant page without accumulating outline history.

As in DeepSeek Harness's Chat window (`loadOlder`/`loadThrough`), the browser
transcript is one contiguous window from its oldest loaded page through the live
tail. Older pages only ever prepend; nothing replaces or freezes the window, and
there is no later-content paging. A loaded exact anchor lands without a read. Any
other jump pages `older` reads of 64 entries until the window's head reaches the
turn's native cursor, then lands; the pages publish together once, including those
read before a failure, whose error stays visible with Load earlier still usable.
One older read runs per Session: a repeated Load earlier is a no-op, and a jump
arriving during a read joins it and lowers its shared target. A page that does not
move the head ends the jump. A turn the history cannot reach fails visibly rather
than landing elsewhere.

Outline paging intent is explicitly `latest` or `page(offset)`, independently of
the native response's offset. Initial load and automatic start/location/settlement
refreshes in latest mode omit the offset, so 64→65 and 128→129 select the new final
page. Selecting an unloaded mark on any other page establishes an explicit
page; automatic refresh preserves it as live turns arrive. Selecting a mark on
the newest page explicitly restores latest intent. One refresh demand arriving during an
in-flight outline read is retained and serviced after its reply; streaming text
deltas create no demand. The cache still holds only one outline page.

Every older read captures the attachment target, connection generation,
attachment epoch, resync authority and cache epoch. Node/runtime changes,
reattachment, resync and a window rebase orphan it. Only the newest navigation
intent lands, and only while ChatViewport's intent ticket is current: user
scrolling retires the landing, never the read, whose rows still merge while
ChatViewport preserves the newer reading position. Outline reads have
independent paging intent and survive window changes; resync retires both domains.

ChatViewport is the sole automatic Chat scroll writer. The rail and Return to
latest share its existing overlay surface, without a second flex wrapper or
scroll coordinator. It coalesces React and
ResizeObserver changes into one animation-frame commit. Explicit navigation
receives an intent ticket and resolves an exact native anchor in that frame.
Later native scrolling synchronously retires the ticket. Detached reading retains
stable rendered anchors across prepend, width/sidebar/panel reflow, Tool disclosure
and image/content growth. Missing anchors use retained adjacent reading anchors
then a clamped absolute position; they never restore follow mode.

Ordinary scrolling away from the live bottom or a turn jump exposes Return to
latest even for short history; live output keeps rendering in the window. The
explicit action restores frame-owned bottom/follow mode without discarding loaded
history; subsequent streaming follows until another user scroll or navigation
detaches.

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

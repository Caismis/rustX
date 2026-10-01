# Native conversation reading

The durable ConversationStore owns the conversation turn outline and exact
transcript locations. A user-facing turn is one native Attempt, matching the
existing TurnProcess and conversation-statistics vocabulary. Its identity is
`(origin ConversationId, AttemptId)`. Ordinals are one-based presentation indexes
at a read cut, never identity. Model retries, logical steps and Tool calls do not
create turns. Automatic continuation creates a distinct native Attempt and turn.

Local order is indexed `AttemptStarted` Journal order. Failed, cancelled,
interrupted, timed-out and limited Attempts retain that identity. A location is
the first committed assistant process member, or the exact Attempt terminal when
there is no assistant member. A just-started Attempt with neither has no location;
clients show a disabled mark. Preview is the first text/refusal block at that
location, limited to 240 Unicode characters; no preview is fabricated for a terminal.

Completed-response provenance copied by lineage precedes local Attempts in its
immutable bootstrap order. It retains the source origin identity while mapping
process MessageIds to destination transcript positions. A source unsuccessful
message copied without Attempt provenance remains unowned content, not an invented
turn. Compaction changes the canonical Surface, not these durable origins/order.
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

The read cut is `(ConversationId, Journal sequence, allocated transcript position,
Surface revision, pending population and aggregate native CAS revision)`.
The two pending coordinates retire edits/removals that do not append Journal
facts; new visible admissions advance the transcript frontier. Selection occurs in one SQLite transaction. Response/Tool
annotation uses only the window's named native Attempts through the cut; final
cut validation rejects a concurrent mutation. A later Journal/Surface/transcript
change retires a cut. Stale or foreign targets fail explicitly; clients reload the
outline rather than navigating to a nearby turn. Reads execute no model or Tool
work and do not create request snapshots or durable writes.

## Browser reading and scrolling

The browser retains one outline page of at most 64 marks/previews. Paging and an
ordinal input reach distant pages without accumulating outline history. Transcript
cache limits remain 512 entries / 8 MiB; ordinary prepend reads are 64 entries.
Historical jumps and adjacent pages replace a finite window. Two outstanding
window reads per Session are permitted; further intents retire older work without
queuing another read. A same-cut loaded exact anchor avoids another locate read.

Every window read captures the attachment target, connection generation,
attachment epoch, resync authority, cache epoch and monotonic user reading intent.
Only the newest still-valid reply installs a window/active mark or requests a
scroll commit. Session/node/runtime changes, resync and user scrolling retire old
replacement work. Ordinary prepend can finish after user detachment: it merges
native rows while ChatViewport preserves the newer reading position. A new
navigation/action still retires that prepend. Outline reads have independent paging intent and survive window changes;
resync retires both domains.

ChatViewport is the sole automatic Chat scroll writer. It coalesces React and
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

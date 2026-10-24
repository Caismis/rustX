# Native conversation reading

## Bounded observation and deferred bytes

Root attachment returns an identity receipt. Clients load a bounded checkpoint,
then follow the registration installed by its physically accepted subscription
receipt. Recovery preserves the locally Applied Cursor until an authoritative
checkpoint and contiguous continuation are installed. Child reads use the same
checkpoint/event/content contracts and retain the distinct `Live` and
`DurableOnly` source modes; a read failure does not declare an execution outcome.

Content reads retain original byte identity and use at most 65,536 decoded bytes.
The authorized readable end permits an empty EOF read; a read beyond that end
returns `invalid_range`, separately from an unavailable source or expired cut.
The shared client byte cache retains at most 64 ranges and 4 MiB, including empty
ranges in the count. Cache eviction never changes Applied. Deferred raw JSON and
structural metadata display in a separate original-byte view, so reading metadata
cannot replace a loaded payload or round an opaque Tool integer. Typed native u64
quantities use the protocol's decimal-string codec. A structural value containing
an unsafe opaque integer remains readable as bytes and is refused as rounded
editor input.

`model_status.available` is native Session admission availability;
`model_status.body_deferred` separately describes its display body. A missing
inline model is therefore not an unavailable model or a missing frozen Attempt.
Catalog defaults can be selected through exact native binding revision and native model
and profile ordinals without loading a giant identity into a mutation request.

Display owner identities preserve the original Conversation and Attempt. A
bounded inline Attempt ID is exact; an oversized ID is deferred through its
original Journal field or inherited Turn/Response ordinal. These are read
selectors, not replacement execution IDs. Canonical Turn and lineage identities
remain required and unchanged. A response-only inherited owner does not become
an invented Turn.

Each native observation transition carries its bounded Trace window at the same
read cut. Default checkpoint reads retain that published window; explicit Trace
inspection reads are independent caches. Preparing a later transition cannot
replace the checkpoint's source evidence or expose its unfinished bytes.

A standalone Trace page carries its complete `read_cut`. An embedded checkpoint
or event page inherits the enclosing cut. Clients retain that cut with each loaded
record and lifecycle update; selecting a field never substitutes the latest
checkpoint. Canonical position can advance without a Journal event, so a Journal
sequence alone cannot identify the cut for a Tool result or request context.

Trace lifecycle queries read finite typed headers captured from the original
Journal records. Numeric original owner sequences join starts and terminal facts;
optional request, context and proposal summaries do not own that lifecycle.
A Compaction preview comes from that record's exact Completed-J bootstrap,
including its original truncation flag. A later projection activation cannot
replace historical summary content. Truncated Summary and Raw views remain
explicitly deferred; no canonical summary Message is synthesized.
Selected fields advertise either the existing bounded Value reader or an existing
Content binding. Canonical metadata uses the original Content source, rather than
an invented message-identity selector. A deferred request or Tool identifier is
never replaced by a synthetic execution ID. Navigation to a committed Assistant
requires an explicit native identity relation at the captured canonical frontier.

Request field metadata is selected through the Store's immutable selected-input
root and its authenticated Source augmentation. Requests retain their own original
capture witness, sequence and occurrence order; shared nodes do not substitute a
later request's identity. Original ranges and bounded field previews use the same
composed source at the captured cut. Their work limits differ from ordinary
segmented canonical, Journal and state sources; see [Request field metadata](#request-field-metadata)
for the current page, traversal and byte limits.

Loaded Trace pages accumulate with one protected selection; the browser does not
discard earlier records or refuse further paging at a total retained-row limit.
Lifecycle repair admits at most eight explicit interests in one read; clients batch their cache independently of
the native event cursor. A page bounds both its records and repair bytes, so a
large loaded cache cannot turn a bounded row into an oversized response.

Publication Audit settlement is positive Publication Plane evidence. Its bounded
`publication_settlements` head retains exact native stream/request/Attempt/Message
identities, transcript position, source cut, settlement kind, time, and block count
when the audit body is deferred. Audit events carry the native-selected complete
head, including its retention decision. A late settlement never clears a later
publication, changes canonical history, or settles execution. Deferred content
remains readable through its original source evidence.

Canonical commit callbacks carry a complete finite canonical head at their SQL
cut, including the indexed opening preview, human clock and Todo summary. A later
callback can therefore cover an earlier committed canonical effect without a
second history or callback registry. Journal and transcript readiness still fence
unreleased owner effects, including hidden Context commits. Loaded transcript rows
remain owned by their captured read cut until a native read-domain update replaces
the window; a canonical commit alone does not insert rows into an older window.

Canonical Tool commitment and execution settlement remain separate observations.
`ToolResultCommitted` fills `committed_result` and its source references; it does
not advance the Tool execution state. Journal terminal evidence advances that
state. Clients can display a later canonical body without replacing the Journal's
terminal status, duration or exit evidence. A pre-start policy result can be saved
without claiming that an executor started or physically settled.

## Native history authority

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

A display directory selects at most 64 owners from finite, source-cut headers.
Local headers belong to their original Start, member and terminal Journal facts;
inherited headers belong to the original immutable lineage ordinal. Indexed
numeric owner and ordinal ranges select identity, location and previews without
decoding an oversized Attempt ID or historical message body. Missing required
header evidence is an explicit observation failure. Later member or terminal
facts cannot enter a page captured at an earlier cut.

Native inherited loading, lineage reconstruction/reinitialization and archive
validation retain full payload-digest and canonical-membership checks. The
ordinal-scoped reverse membership index requires each inherited Turn to declare
exactly its assigned canonical Assistant members. Those native checks remain
separate from bounded display headers and do not turn display read failures into
execution or canonical-history failures.

Each turn carries two native previews, matching DeepSeek Harness's turn outline.
`prompt` is the turn's first eligible opening input: the earliest User message
with Human or delegated-Agent source and ordinary Message kind adopted with this Attempt, or adopted while idle
after the previous Attempt started and before this one did. Steering of the
previous turn carries that turn's identity and never opens this one; automatic
continuation has no prompt. Queued inbound reserves its transcript position at
acceptance, so transcript adjacency is never used to find a prompt. `response`
is the newest text/refusal-bearing Assistant member, present only once the turn
has a terminal at the cut. Both join text blocks with single spaces, collapse
whitespace and end with an ellipsis when clipped: at most 50 and 120 Unicode
characters (one and three rail-card lines). Inherited prompt previews use the
earliest text-bearing Human or delegated-Agent input explicitly named by the turn's
`TurnReadingProvenance.prompt_message_id` or its completed response's native
`retry_message_id`. Both relationships are remapped to destination identity on
copy and dropped when the input is outside the retained cut. An opening prompt
precedes later steering input; when only response replay input is retained, that
recorded input provides the preview. Neither nearby User messages nor live reads
of the source Session establish ownership. The inherited response is
the newest text-bearing retained member of a settled inherited turn.

Retry identity uses this same original adoption order. RequestStart captures the
opening input in its native transaction, using canonical ordinary-user, nonblank
text and Human/delegated-Agent source eligibility. Later steering and frozen
Original/Derived model inputs do not replace that identity. The resolver does not
depend on optional display headers or preview Source availability; the optional
deferred retry-ID field receives the already selected native identity.

Local prompt lookup uses the existing Attempt/type/sequence index for the
selected Attempt and the exact idle range between the previous and current
starts. It does not inspect unrelated future Attempt adoptions. Deterministic
SQLite VM instrumentation compares 0, 128 and 512 later Turns: Human lookup stays
at 107 instructions, delegated-Agent at 110, idle at 104, and absent inputs at
66. The former kind-only search grows to more than 3600 instructions.

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

`session/turns {target, offset?, limit}` returns `ObservedTurnPage`: the
cut it was captured at, total, offset and at most 64 ordered native turns.
Omitted offset selects the final page; an explicit offset directly indexes
history. Each mark carries its origin, ordinal, bounded prompt and response
previews and optional exact transcript cursor. The display identity carries an
exact inline Attempt ID or its original bounded field selector; it never changes
the canonical Turn ID. Total is an exact decimal-string quantity, while offsets
and navigation ordinals must satisfy the protocol's safe-integer constraints.

`session/transcript {target, at, limit}` reads one contiguous transcript
vocabulary and returns `transcript_window { cut, page, newer_cursor, target, target_cursor }`:

- `latest`: the newest finite page;
- `older {before, cut}`: the finite page strictly before an already-read cursor; a missing cut captures current authority;
- `newer {after, cut}`: the finite page strictly after the cursor at the same cut;
- `turn {id, cut}`: native resolves the canonical Turn anchor and reads forward from it.
- `turn_ordinal {ordinal, cut}`: the source resolves the original outline position
  at that exact cut without reconstructing an oversized Attempt ID.

Root and Child navigation capture the selected row's page cut at the gesture.
A later outline refresh cannot substitute a newer cut. The server validates the
source Conversation and cut before resolving the ordinal; a response-only
identity cannot be used to fabricate a Turn anchor.

All root pages are 1–64 entries; `next_cursor` is present while older history
exists. A distant Turn takes exactly one native window read, independently of its
distance from the live tail. Native identity and the captured cut select its
position; the browser never walks the intervening conversation. Agent/internal transcript APIs retain their separate registry and
read-model ownership.

A read cut C is `(ConversationId, journal, transcript, mutation_revision)`, the
inclusive upper bounds one read was captured at. `mutation_revision` is the
native monotonic epoch of successful pending-body edits/removals. Pending
adoption preserves its accepted body and position; new admissions and ordinary
execution append facts. Projection activation preserves immutable Ledger/Journal
meaning. Latest reads capture a fresh C. Anchor and continuation reads name C and reject
foreign, future or mutated cuts; append-only growth remains reconstructible.

Selection happens in one SQLite transaction, bounded to C's `transcript`; Tool
results are included only if their own canonical transcript position belongs to
C. Runtime Client folds only the selected native Attempts through C's `journal`
for response, process, terminal, timing and usage decorations. A final
compatibility check rejects a semantic mutation that landed during the read,
while accepting concurrent appends. Reads execute no model/Tool work and create
no request snapshots or durable writes.

## Browser reading and scrolling

Hover and focus use the same single-flight outline pipeline as committed reads,
but only return disposable preview data. They cannot publish an outline window,
paging selection, transcript position, follow mode or navigation error. The rail
retains one preview Turn; pointer leave, explicit navigation, native mutation,
attachment replacement and generation replacement revoke its authority. An
explicit navigation can acquire a matching in-flight read; only that new owner
may commit its native page. Growth owes one refresh of the committed paging
selection, independently of a preview's offset. A focused mark retains its
tooltip while explicit navigation resolves, without promoting the preview's
identity into the committed directory.

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

Each native Chat page has at most 32 entries and an 8 MiB serialized UTF-16
admission limit. These are per-response limits, not a cap on retained Session
history. Initial reading and older-history actions assemble a Harness-style
window: at least 50 conversational messages and two Turns, with a 500-message
ceiling or history exhaustion. Tool results and model-only Context do not consume
that conversational message budget. Loaded Chat and Trace pages accumulate;
normal live updates do not evict previously read rows or create artificial
"load earlier" recovery work.
Returning to the bottom of ordinary Chat preserves all loaded pages; it does not
reload a smaller latest window. Only an explicit historical jump needs its frozen
window released on return to latest. Root and child readers share this behavior.

An unresolved response or Tool that leaves the live head is reread automatically
by its exact native message identity. The old row remains mounted until its fresh
projection is available. These passive reads coalesce newer observations, retain
source bindings, and obey the same attachment, Conversation, epoch and navigation
fences as other native reads. They neither expose an internal cache-reset notice
nor detach the reader's scrolling position.

A jump to an already loaded exact anchor needs no read; any other jump requests
one native 32-entry window. Explicit historical navigation leaves intervening
history unloaded until its continuation is requested. Live streaming continues
from the independent bounded runtime projection; overlapping cursors render
exactly once using the current native entry. Return to latest releases the
historical window.

A new navigation intent retires the previous read's commit authority immediately.
An older page never joins or broadens a newer jump. Failed reads preserve the last
valid presentation and expose recovery; pending-body invalidation retires the cut
and returns to current native authority with an explanation. No retries or timeout
heuristics determine ordering.

Committed outline paging is explicitly `latest` or `page(offset)`, independently
of the native response's offset. Only an authoritative demand's successful
response commits both page and paging in one `setSession`. Enqueueing, deferring,
canceling or failing a demand never commits its requested mode.

A selected ordinal N always resolves through the fixed offset
`floor((N - 1) / 64) * 64`, even if N belonged to the latest page at the click.
Append-only growth cannot move that identity lookup. The returned native identity
and cut then own the single direct transcript window read. Rail mode is separate:
selecting the newest page restores `latest`; selecting another page commits
`page(offset)` when its response is accepted. Automatic refresh uses the surviving
committed mode, so latest refreshes omit the offset and follow 64→65, 128→129, etc.
Missing ordinals report unavailability rather than synthesizing an identity.

Per Session, one RPC and one replaceable explicit pending demand are retained.
Equivalent explicit demands share a read but retain only the newest gesture.
Automatic reads keep independent ownership instead of lending their validity to
a cancelable gesture. An active response is fenced by native authority, its own
intent and any *still-valid* pending demand. Canceling the pending gesture before
the active reply restores no state: committed paging never changed, and the
independently valid active reply may publish directly without a replacement RPC.

A boolean coalesces automatic refresh obligations arriving during a read. Pending
replacement, cancellation and explicit read failure do not clear that obligation.
A suppressed automatic response also retains its obligation. After explicit work
settles, one automatic read uses the final committed mode; this is demanded refresh,
not a failure retry. Authoritative failures preserve page/paging and publish an
error; obsolete successes and failures publish neither. Native invalidation,
resync and attachment/connection replacement retire the entire read owner,
including active/pending promises and its queued refresh obligation.

The viewport gesture starts at the click, before outline resolution; only its
still-current continuation may request the single direct transcript window.
Streaming text deltas create no outline demand. The cache still holds one page.

The rail uses native ordinal minus one as its fixed-pitch virtual index. Its model
retains that native page and at most one pinned live identity; unloaded marks are
computed on demand. It has no total-sized mark array, index Map or measurement
cache. Only visible indexes plus three-mark overscan and focused neighbours are
materialized. Hover/focus keys map to ordinals arithmetically; native identity
lookup inspects only the retained page.

Every window read captures the attachment target, connection generation,
attachment epoch, resync authority and navigation intent. Node/runtime changes,
reattachment, resync and a window rebase orphan it. Only the newest navigation
intent lands, and only while ChatViewport's intent ticket is current: user
scrolling synchronously retires both the client reading intent and the landing.
Both native authority and the initiating viewport ticket must remain valid at
`setSession` window installation. A stale success or failure leaves the last valid
window unchanged. The layout frame independently checks the same ticket again,
so a scroll after installation still wins over the deferred viewport movement. Outline reads have
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
explicit action restores frame-owned bottom/follow mode and releases the historical window; subsequent streaming follows until another user scroll or navigation
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

## Exact fork source message windows

App Server v48 adds the Message window selector over canonical MessageId. The
store resolves its existing indexed transcript position inside the same read
transaction, then returns at most 64 entries forward from that exact position.
It never scans intervening transcript pages or substitutes a nearby Turn. A
provided cut obeys the same foreign, future and semantic-mutation fences as other
windows; an absent cut captures current native facts. A message outside the cut
or no longer readable fails explicitly. The result carries target_cursor while
target remains absent: a message identity does not invent Turn ownership.

The Web source action validates the exact message at target_cursor, attachment
authority and navigation intent before publishing or anchoring. A superseding
read, reattachment, node switch or user cancellation retires the result. The
retained presentation remains bounded and later source activity cannot rewrite
the fork seed boundary.

The immutable inherited transcript boundary was introduced in SQLite schema 58 and remains part of the current schema 80 lineage bootstrap. Initialization records the final native transcript watermark in the same transaction as the seed, including terminal-only inherited Turns and excluding invisible Context messages. Repeated bounded transcript and checkpoint reads resolve this boundary through a single bootstrap primary-key lookup. Older development stores are rejected without migration.

## Request field metadata

SQLite schema 80 stores frozen Request metadata separately from the native
immutable selected-input tree. The Store owns that balanced tree and its canonical
Original/Derived provenance. Request rows bind its root identity, digest and count;
explicit historical reconstruction loads the selected input, while publication and
settlement read only authenticated request metadata. Older development stores are
rejected without migration.

Request Source augments those same native nodes with immutable encoded fragments
and field geometry. It does not own a second input selection. Unchanged native
subtrees share Source augmentation; a Request retains a finite root and original
capture witness. Journal sequence and occurrence rank are substituted at read
time with checked, independent decimal-width accounting: 20 Journal widths and
four occurrence-rank widths cover native sequences and the 8,192-item limit.
Equal sums of decimal widths do not imply equal header admission. The resulting
bytes preserve the original JSON representation.
Reuse validates the earlier capture's witness rather than treating a newly hashed
optional row as authoritative. Native root identity, readable cut, representation
and original publication authorization remain mandatory.

Fragment pages are authenticated and limited to 65,536 bytes before allocation.
Schema 71 uses the `RXSP` version-1 binary page encoding: a typed leaf or branch,
bounded little-endian lengths, raw value bytes and raw 32-byte child hashes.
Decoding rejects unknown versions/types, excess count/length, invalid UTF-8 keys,
truncation, trailing bytes and inconsistent ordering/depth. There is no JSON-page
fallback or migration. Both leaves and branches have at most 64 entries and obey
the same byte limit; byte limits can force earlier splits. Producers sort up to
256 facts with at most 65,536 bytes of owned key/value buffer capacity, then spill
to the existing SQLite temporary spool. Vec metadata is additionally bounded by
that count; large inputs never accumulate an unbounded in-memory fact list.
Their BLOBs occupy an ordinary rowid table with a separate unique, non-null
32-byte hash index. Lookup searches that index and then the table; rowids never
enter Source identity or authorization. Physical measurements include both
B-trees, while the bounded BLOB handoff remains one page value per lookup.
The fragment index has depth below 16 and native selection paths have height at
most 32. A range visits at most 64 emitted pieces and may return a shorter positive
chunk with an exact continuation. Bounded previews visit at most 512 pieces and
retain the 32-node display budget. Pages retain both item and encoded-byte limits.
The former two-content-row bound described schema 64's copied segments; it does
not describe these composed reads. Tests account for actual index pages, native
and Source nodes, metadata bytes, and emitted content separately.

Each field Fact is stored once. A parent/ordinal child index entry contains only
the canonical child path, which the existing field lookup resolves. Reading that
reference checks its 4,096-byte encoded bound before parsing, exact parent and
one-level depth, component bounds and canonical encoding. Page hashes and the
independent original capture witness still fence replacement; a rehashed child
path cannot authorize a different native source. This format removes duplicated
Fact payloads without adding a lookup or changing original content bytes.

Fixed identity columns precede growing payload columns in SQLite records. All
optional Source creation remains inside its existing savepoint; failure leaves
native Request, canonical and Journal commitment intact. Source metadata remains
under the Store's retention and read-cut ownership, and never grants client
access by itself.


### Frozen binary artifact ranges

Canonical binary content remains in the settled managed artifact. Its original
canonical indexing transaction streams the artifact once, holding its native
reader reservation, verifies the native settlement seal, and freezes SHA-256
evidence for each 65,536-byte segment.
Schema 69 retains those finite digests under the canonical source and block; it
does not duplicate the artifact bytes. Optional-index failure rolls back that
observation index without undoing canonical commitment.

Each `BinaryBytes` read validates source authority and frozen length, then reads
and verifies only the one or two intersecting segments. Artifact payload reads
and SHA input total at most 131,072 bytes for a response of at most 65,536 decoded
bytes, plus the fixed 64-byte native seal header and bounded digest lookups.
Returned bytes are sliced from the verified buffers, never reread after checking.
Missing integrity evidence or changed bytes makes the source unavailable, including
a same-length external overwrite. Native one-shot writers and their reservation
locks still own settlement; those advisory locks do not prevent external filesystem
writers. No whole-file rehash occurs during a range read, and no modified content
is silently recaptured under an existing reference.

Native Artifact settlement is independent of this optional display index.
`ArtifactWriter::finish()` retains exclusive reservation ownership while it syncs
the bytes and publishes the fixed-format integrity seal in the existing reserved
file. An abandoned writer has no successful settlement seal. Fork, Clone and tree
copy validate source segment evidence and write those same verified buffers to
the destination; they never authenticate an unverified replacement by hashing it
only after copying. A failed copy is discarded before catalog publication.
Healthy copies remain possible after optional display-index rollback, and their
owned bytes remain available after the original allocation is deleted. The copy
keeps its existing 256-Artifact/64-MiB aggregate copied-payload budget. Destination
finalization additionally rereads that payload to bind its seal to the source
aggregate before publication. A canonical
ContentRead still reads at most two segments; lineage verification does not add a
whole-file scan to range reads. Session archive streaming also verifies each
existing 64 KiB source buffer before compression and checks the seal aggregate
before successful completion; it does not prescan bodies under the capture barrier.

## Canonical append frontier

Upstream schema 70 removed `surface_head.active_json`; the integrated schema 80 retains that layout.
The immutable Ledger and selected checkpoint remain the active-identity authorities.
Cold/invalidated append validates the same bounded suffix and Original dependencies.
Warm append may reuse a disposable connection-local validation count, checked
inside its write transaction against mutation/data/schema versions and the live
frontier/publication. Native duplicate/order checks remain; only successful COMMIT
seats advanced evidence. Tracked identity/checkpoint mutations, external writes,
schema changes and rollback invalidate it. Arbitrary out-of-band historical disk
corruption need not be rediscovered immediately on every append; actual reads and
explicit full validation retain their checks. Context construction and checkpoint
publication validation remain separate work, so whole requests are not O(1).
See [append validation boundaries](issue-481/append-validation.md).
Canonical acceptance, events and staging remain atomic, and publication still
follows its committed native transition. Older layouts are rejected; there is
no migration, compatibility reader or dual write.

## Ephemeral auxiliary output

`auxiliary/status`, `auxiliary/result` and `auxiliary/submissionLookup` use current attachment read authority and DeliveryAccess. Results are repeatable five-minute retained reads without ACK, ContentRef or persistent reconstruction. `auxiliary/segment` is registration-fenced provisional output, separate from the one durable cursor owner. Source unavailability preserves loaded output and stops automatic reading; exact native Call cancellation uses the current controller independently. See [auxiliary consumer bounds and display repair](auxiliary-consumers.md).

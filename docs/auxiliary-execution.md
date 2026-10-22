# Auxiliary request facts and inspection

Auxiliary execution shares the physical model-request Journal with Agent execution. Native Runtime owns supervised execution and physical cleanup; Store supplies synchronous transitions and bounded inspection. Durable audit does not retain generated auxiliary output or make historical Calls runnable after restart.

## Identity and authority

ModelRequestStarted owns the request-to-owner relation. The closed owner is either an Agent RequestIdentity or an AuxiliaryCallId plus physical request ordinal. Completed and Failed repeat that owner as checked evidence: the Store rejects a terminal that differs from its authoritative start. A RequestId can have at most one start and one physical terminal. Identifiers, purpose, trigger and fingerprints grant no authority.

Agent requests retain their native Snapshot, selected-input root/digest/count, Context transaction and P/U/C rules. Auxiliary requests require an admitted Call and never create an Agent Attempt, Turn, Assistant identity or primary Snapshot. Only an Agent completion can establish the primary P marker.

A Call records bounded purpose, trigger, business source, input-selection evidence and fingerprints. Purpose and trigger are consumer-defined attribution strings, each limited to 512 UTF-8 bytes, nonempty and without control characters. Trigger describes the activation cause, not a transport category. The Store has no business-purpose dispatch or consumer allowlist. Business sources may be native Compaction/Human references or opaque consumer-owned namespace/reference pairs with the same text bounds. Opaque references grant no storage, inheritance or execution authority.

Suggested-input generation uses the same contract with consumer-defined attribution and frozen historical context or temporary draft input. Candidate presentation and expiry belong to the consumer. An explicit user submission follows the normal Human input path; generating or displaying candidates leaves canonical history and primary Agent obligations unchanged.

Business source and input selection have independent coordinates: native source references must exist when the Call starts, but may be newer than its frozen input cut. Only selected historical input references must be visible at that input cut. A historical selection records the complete native ResourceReadCut, including canonical and projection coordinates; a Journal sequence alone does not identify canonical history. Temporary input has no implied historical cut or retained copy. Explicit selected message references are independently bounded. These facts describe what was selected, not how to reconstruct absent temporary bytes.

## Semantic writes and confirmation

Call start, physical request start, final request/Call settlement and Call-only settlement are specialized synchronous Store operations. The final physical outcome and Call outcome commit atomically. Zero-request cancellation creates only a Call terminal. Raw event insertion cannot bypass these transitions. Consumers construct terminal envelopes through `durable::auxiliary::auxiliary_settlement_envelope`, which shares canonical event identity construction with Store validation. The returned timestamp and payload are retained unchanged for retry and confirmation; the constructor itself neither authorizes work nor commits facts.

Mandatory request-owner, Call, request and audit indexes are derived in the same transaction as the original Journal facts. Reads and confirmations validate their complete identities and relationships. Exact primary-start replay validates the mandatory owner row against its original Started fact before returning the original receipt; missing or mismatched ownership is rejected without repairing indexes. Missing mandatory index evidence is corruption; missing optional Source, Trace or display material is not proof that execution did not commit.

`ConversationStore::read_auxiliary_call_requests` returns up to 32 local request audit projections for one Call at a captured Journal cut. Requests are ordered by their original start sequence; the last returned `started_sequence` advances the next query, and an empty result ends iteration. Each supplied cursor must name a visible request start in that same local Call. Selection uses the Call/sequence index within one SQLite read transaction and validates the original owner and mandatory audit evidence. The query excludes unrelated Calls and inherited source audit without materializing their history. Global audit pages bound both ends of the dense-offset interval at the captured total, so old final and exhausted pages do not scan later execution history.

An exact replay returns the original receipt and does not republish events or allocate another Journal sequence. Unsupported envelope metadata and conflicting caller semantics fail explicitly. Local write failure distinguishes a confirmed rollback/no-write path from an unknown commit or rollback outcome. Read-only confirmation returns a known matching commit, a conflict/error, or Unknown when absence cannot prove noncommit. It never creates work. Only actual completed-write evidence can establish noncommit; there is no caller-supplied boolean or fabricated writer-exit token.

Commit certainty and required-write failure cause are independent. Known
noncommit refunds a start allowance, while a genuine required Store failure
still closes native durability admission. A confirmed Call can settle Failed
after its RequestStarted transaction rolls back; successful cleanup does not
erase the storage failure. Typed Store cancellation and benign start validation
refusals remain distinct. Historical resource-cut and projection-activation
validation, shared request-start probes and Journal frontier lookups preserve
the raw SQLite interrupt cause before formatting diagnostics. An independent
required Storage failure remains authoritative under coincident cancellation.
Optional observation writes remain inside that same transaction. Their failure
may degrade only after successful savepoint cleanup with the outer transaction
intact. Cancellation, automatic rollback and failed cleanup stop the append
through every nested optional helper before later SQL can independently commit
a Journal head, index or transcript coordinate. Serialization adapters preserve
the original SQLite interruption cause. SQLite-proven automatic rollback before
any COMMIT, with no subsequent SQL escaping the transaction,
is direct noncommit evidence ([SQLite transaction state](https://www.sqlite.org/c3ref/get_autocommit.html));
absence observed later is not. Failed COMMIT or unproven rollback starts as
Unknown; exact confirmation may establish the matching committed intent.

Local outcome is separate from observed provider terminal. A locally failed stream may have observed provider completion before rejecting an illegal tail. A locally completed request must agree with a known observed completion kind. Request evidence retains only the closed `stop`, `tool_calls`, `length`, `content_filter`, `refusal` or `other` completion kind; unknown provider prose remains in the physical diagnostic and is not duplicated into audit. Observed usage, completeness and remote-execution uncertainty remain independent. Missing usage is not zero, and a known subtotal is not a complete bill. Results and domain acceptance are separate consumer responsibilities.

## Bounded reads and ownership isolation

Auxiliary audit uses a Journal-only cut because it reads execution facts without resolving canonical content. Historical input and primary request detail retain their complete native cuts. Later settlement cannot fill an earlier audit view.

Audit pages contain at most 32 records. Call-start metadata is limited to 8 KiB, and each typed audit record is limited to 12 KiB before structural JSON allocation. The Runtime Client checks each complete encoded page item against 16 KiB after adding identity and envelope fields. The 4 KiB headroom covers complete page identity/envelope and exact-u64 decimal expansion: JSON escaping expands a 512-byte request identity by at most 6x, leaving enough room for the repeated request identity, model name, fixed fields and finite integer fields. Physical terminal envelopes retain their 16 KiB limit. A transactionally derived dense audit ordinal supplies real collection counts and advancing offsets. The Journal sequence remains a separate source coordinate. A long unrelated Journal does not manufacture audit items or force a history-wide count for a small page.

The Runtime Client exposes auxiliary.audit through the existing checkpoint page path and current read authority. Cold checkpoints discover retained audit without requiring a new live event. Live auxiliary facts advance their audit domain without changing primary Attempt, Context or Trace state. Optional Source unavailability does not erase mandatory Journal evidence.

Primary occupancy, statistics, recovery answer obligations, request predecessors, retry context, subagent activity and Trace remain owner-scoped. SQL ownership predicates are applied before LIMIT and aggregation, with indexes supporting that scope. Auxiliary Usage cannot hide the latest primary occupancy reading. Pure folds use the explicit owner rather than guessing from absent Attempt fields or an ID prefix.

## Lifetime, lineage and export

Local Call/request facts and their confirmation basis live for the Conversation lifetime. Future Title consumption and submission correlation remain tied to those facts; output expiry will not erase them. Active-capacity limits and bounded pages do not bound lifetime disk growth.

A completed one-request Call appends four Journal facts and four audit-offset rows,
plus one row each in `auxiliary_calls`, `auxiliary_requests` and `request_owners`.
Its three write receipts are reconstructed from these facts and indexes; there is
no additional serialized receipt table. The thirteen explicit ownership/audit indexes
retain 27 entries per completed Call, in addition to uniqueness indexes and the
shared Journal/Source/observation indexes. Call-start metadata is capped at 8 KiB,
request identity and model at 512 UTF-8 bytes each, terminal envelopes at 16 KiB
including reserved sequence digits, and derived audit records at 12 KiB. Generation
offsets retain the common observation limit of 9,007,199,254,740,991 milliseconds;
Usage retains exact `u64` values.

In the pre-#484 layout, the real SQLite fixture
`aux_01_representative_storage_growth_of_completed_one_request_calls` measured
100 completed Calls with fixed UUID-sized identities, a short model name and small
Usage values on Ubuntu 24.04 with 4,096-byte pages. After WAL checkpoints, Journal
JSON grew by 216,292 bytes (2,162.92 bytes per Call) and database pages by 3,698,688
bytes (36,986.88 bytes per Call), from a 901,120-byte empty-store baseline. The
physical delta includes the existing Source and observation layouts, all indexes
and page allocation; it is not solely auxiliary metadata overhead. This is a
reproducible representative workload, not a maximum or a retention budget. Larger
bounded metadata, different page packing and inherited dependencies change the
cost. These measurements are not a schema-74 cost baseline. No history is pruned to meet them.

Clone/Fork canonical overlap never creates target-local auxiliary execution or reserves a local correlation. Only an explicitly retained Summary-generation dependency may carry source audit, with original source identity and the exact eligible prerequisite closure. Source audit remains distinct from target-local request ownership, quota and admission. Independent retained dependencies must survive source deletion.

A new local checkpoint naming a Summary-generation dependency requires the successful physical request and completed local Call to be visible at the dependency's declared Journal cut. A later settlement cannot authorize a dependency captured before that settlement. Historical inspection and inherited source audit preserve open or failed execution evidence at their captured cuts; that evidence grants no new checkpoint commit authority.

Inspection archives keep requests.jsonl for actual primary Snapshots. Auxiliary facts have a separate auxiliary_audit.jsonl projection at the captured Journal cut; generation rows identify their owner. Complete audit does not imply reconstructed prompts or generated output. Archive dependency validation walks the captured native Journal/activation prefix one checkpoint at a time. Each local Summary audit closure has its own bounded materialization budget; released history does not consume a lifetime archive allowance. Later checkpoints cannot enter an older captured archive. Archive projections explicitly exclude private diagnostics and execution bindings instead of serializing newly added native fields indiscriminately.

## Verification boundary

Real SQLite tests cover transaction atomicity, owner/index consistency, rollback and receipt-loss evidence, immutable cuts, pagination and corruption refusal. Mixed-history regressions compare primary behavior with auxiliary facts interleaved. Native execution tests exercise the production executor with a private second consumer, actual starts before adapter entry, panic after trusted usage, waiter loss and parent close. Writer boundary tests retain capacity through actual confirmation and joins after a real COMMIT. Consumed production-executor barriers cover cancellation/parent-close on both sides of the single-use dispatch grant. Real child-process kills cover confirmed RequestStarted, granted dispatch and adapter entry; quiescent cold reopening preserves unresolved Call/Request facts and never redispatches their RequestId. A completed, parked manual-compaction reply permits manager deletion and then delivers the exact original receipt. These are separate from Context deciding whether generated text is a usable Summary.

## Native execution

Automatic and manual Summary use the Runtime-owned single-request executor.
Native preparation reserves process total/general, Conversation total/general
and write responsibility without waiting. Limits are 32/24 and 4/3; only
native Compaction authority may omit the general reservations. A purpose or
trigger string grants no execution or protected-capacity authority. Both
capacity and lifecycle-lock Busy refusals arbitrate under cancellation; an
earlier cancellation remains cancellation, and active callers never wait for
either resource.

Each Call confirms CallStarted and RequestStarted before consuming its only
dispatch grant. Unknown start never enters the adapter. The owner retains
execution state outside the polled future, so panic cannot discard trusted
usage or cause reentry. Required terminal Request and Call facts settle
atomically. Already-owned settlement and exact confirmation remain available
after admission closes.

Success requires a valid completion and EOF under current native authority;
the absolute deadline continues through EOF. Valid provider observations and
usage survive a later local refusal. Context separately rejects empty/refused
Summary content and commits its checkpoint only with an accepted parent grant.
The checkpoint names the confirmed local Call at its exact settlement Journal
cut. No input body, generated output, credentials or logical budget is copied
into auxiliary audit metadata.

Each live Call owns at most one owner thread and one serialized write thread.
Before the first physical owner starts, 32 process-owned retirement workers
are installed, independently joining up to the existing 32 admitted Calls.
The global bound is 96 auxiliary threads; queued retirement records remain
bounded at 32 and cleanup reserves no new execution permit. A record handed
off before its owner physically exits retains its real join and permits;
that join cannot prevent another Conversation from delivering confirmed work
and releasing its own allocation. Actual write/confirmation and owner joins
precede resource release. The reaper then clears the Call's resource slot and
drops its own resources before releasing admission, removing the Call or
sending its result. A completed record retained by a descheduled reaper owns
no Store allocation. Unresolved exact intent moves to the native failed-durability
owner only after physical exit; historical uncertainty does not pin global
capacity indefinitely. Live/unproven cleanup retains its old allocation.

One Compaction shares twelve confirmed/unresolved starts, three total input
shrinks and a five-minute absolute deadline across carryover and input replans.
The model and resources remain frozen. Manual compaction transfers its native
business tail before the manager lease is released, and a lost observer or
later checkpoint cannot replace the exact result of the original invocation.
The transferred reply owns only its native completion handle and receipt
conversion inputs, never the Runtime Client, Runtime or Store allocation.
A completed reply awaiting delivery cannot prevent manager deletion.
Technical retries, Title, Classification/BTW and result retention remain
outside this executor's shipped consumer scope.

Changes to stored facts or read layouts advance the affected development-format gates. Unsupported or falsely current layouts are rejected without migrations, fallback readers, data deletion or database reconstruction.

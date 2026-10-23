# Classification and temporary questions

Classification and BTW are named consumers of the native auxiliary executor.
The public boundary is `auxiliary/classify`, `auxiliary/btw`, `auxiliary/status`,
`auxiliary/result`, `auxiliary/submissionLookup`, `auxiliary/cancel` and
`auxiliary/discard`. There is no public generic dispatch. These methods require
an exact current attachment and DeliveryAccess. Starts, cancellation and discard
require the current controller; status, result and lookup require read authority.
The managed Web launcher supplies a distinct owner-only Delivery Access token
through its authenticated no-store bootstrap. Remote Settings accepts it
optionally; native capability confirmation gates the BTW action. Credentials stay
in page memory and never enter browser storage or a question fingerprint.

A start response contains only CallId, correlation and original submission scope.
Native ownership transfers first; the successful response waits for confirmed
`CallStarted` (not inference). Preparation failure returns an error with no
successful receipt and no provider dispatch. The manager OperationLease covers
admission and transfer; confirmation waits outside that lease. Closing a panel,
transport or attachment cannot cancel admitted work. Explicit cancellation uses
the exact native Call scope without consulting an observation Source or waiting
for a Source read. Discard releases output reservation; it does not cancel work.

## Ownership and commit points

1. Structural validation and the original SubmissionPermit precede asynchronous
   App Server routing. Association retirement closes this same permit.
2. Exact Conversation-local durable correlation lookup precedes changing model,
   context or resource preparation. A matching version-1 semantic fingerprint
   returns the original receipt, including after output expiry. A mismatch is a
   conflict. Inherited audit is never a target-local submission authority.
3. Result count/bytes are reserved before the existing general auxiliary Call
   permit. All acquisitions are nonblocking and unwind on refusal. Existing
   protected Compaction capacity is unchanged.
4. Native registration locks the original permit, attachment authority, then
   the active-submission map. Lookup releases the map before acquiring that gate;
   a generation check refuses admission if concurrent ownership changed. Cancel
   and detach take attachment authority before the map. A losing original scope
   cannot register later. Preparation runs under the native execution owner.
   Store reads run on a blocking worker, whose real join remains owned by the
   manager operation even when the RPC waiter disappears.
5. The existing required-write owner commits Call/Request facts. The physical
   join callback makes the compact result available, closes submission evidence,
   then removes live correlation ownership. Inference stopped, durable execution
   outcome, settlement confirmation and output availability remain separate facts.
6. App Server replies keep counted result copies through final disposition.
   Registration and DeliveryAccess are revalidated at physical Writer acceptance;
   the same mechanism covers direct, Stdio and WebSocket carriers. Auxiliary
   notifications neither advance nor manufacture a Runtime Client cursor.

Lookup returns Known from original local durable facts, Pending from a matching
live original scope, ConfirmedNotAdmitted only after original closure/physical
exit and a successful authoritative absence read, and Unresolved otherwise.
The process registry has 128 active and 128 retired scope records. Eviction or
process restart loses negative proof, yielding Unresolved, never guessed absence.
A Pending CallId is provisional identity for exact cancellation, never a
successful receipt; readers keep querying the original scope until Known or a
proven negative result. There is no automatic resubmission, result ACK or
consumption lease.

## Inputs, models and domain acceptance

Correlation is 1–128 ASCII bytes (`A-Z a-z 0-9 - _ . :`). Input text is 1–16384
UTF-8 bytes. Classification has 1–32 distinct labels of 1–128 bytes. Its input is
explicit text or one authorized historical MessageId. The fingerprint encodes
version, consumer, Conversation, exact input and ordered labels; transport state,
resolved models and sampled history are excluded. Whitespace is semantic input.

Classification accepts exactly one JSON object with the sole `label` property,
whose string exactly matches a supplied label. Arrays, extra/duplicate members,
prose, fences and out-of-set labels are domain failures even when the provider
completed successfully. Neither consumer commits a canonical message, grants
tools, routes a workflow, or sets primary continuation.

BTW captures the current committed Context projection and tail at one store read
transaction. The temporary question is request-only input with no canonical
MessageId. Its independent instruction treats historical content as data. Context
media preparation, required sealed-artifact reads and provider translation finish
before RequestStarted. Input that cannot fit is rejected; this path never invokes
compaction or silently truncates historical text. Existing image projection rules
still determine the supported image subset.

`classification_model` and `btw_model` source policies independently choose
`mode = "session"` or an explicit catalog model/profile/request parameters/output
limit, using the same native policy resolver as Title. They are exposed in native
settings, effective readers and Web Settings. Session whole-state policies are
`classificationModel` and `btwModel`; primary model changes preserve both. Policy
resolution is frozen at admission. A duplicate returns before resolution.

## Retention, delivery and browser behavior

Completed compact results expire five minutes after completion. Repeated reads
neither consume nor renew them. Native retention reserves 16 slots/1 MiB per
Conversation and 128 slots/8 MiB per process. A separate copy budget has the same
bounds and survives discard/expiry until response disposition. Each complete
auxiliary response/notification is limited to 64 KiB; result payload counting
reserves 4 KiB envelope headroom. Visible generated text is at most 8 KiB; the
normalized generation budget, including reasoning, is 256 KiB. Oversize output
fails rather than becoming a truncated successful result.

One weakly owned cleanup task per result owner handles TTL; rapid completion and
discard cannot accumulate timer tasks or keep an idle Runtime resident. A bounded
16-entry disposition ring distinguishes recent expiry/discard; older unavailable
results remain associated with their original durable Call.

BTW streams Reset/Text records with CallId, RequestId, attempt and UTF-8 byte
frontier. Reset precedes a new attempt. Each observer has 16 queued records;
text chunks are at most 1024 bytes. There are at most 16 observers per Conversation
and 128 per process. Full/closed observers retire without blocking execution.
A missing reset/chunk means incomplete display. Only an authorized retained final
result repairs it; late fragments cannot overwrite that completed answer.

The Web client retains at most 16 question entries across visible and hidden
views. Each retains at most 16 KiB input, 8 KiB output and bounded identity/error
metadata; capacity refusal occurs before submission. There is one observation
owner and one sequential read in flight. Panel close aborts reads only. Source
unavailability or a failed read stops automatic polling; Check status explicitly
retries reading. Exact target, connection generation, CallId, RequestId and attempt
fence updates. Correlation survives transport loss and is queried read-only after
reattachment. Ctrl/Command+Enter submits, Escape closes, and focus returns to the
opener. English/Chinese states distinguish provisional, incomplete, uncertain,
settling and complete output. Model text renders as text.

## Capacity envelope and maintenance

Logical freezing does not imply zero-copy media. The following counts are
simultaneously owned payloads; allocator capacity, container/node overhead,
thread stacks, SQLite pages and TLS/kernel buffers additionally contribute to RSS.
Those implementation-dependent costs must be measured when dependencies change.

| Component | Per admitted Call / owner | Multiplicity and release |
| --- | --- | --- |
| Execution | Existing 3 general / 4 total per Conversation; 24 general / 32 total process | Physical adapter and writer join, including cancellation |
| Selected history | 8192 items, 16 MiB total; 64-item/4 MiB read pages, 1 MiB item | Existing bounded Context reader; immutable roots may share, materialization allocates |
| Invocation parameters | 128 KiB serialized before cloning | One admitted invocation plus adapter request copies |
| Frozen request | 64 MiB counted neutral serialization; separate 64 MiB final provider-JSON count | Full real translation, including media and escaped tool parameters, is counted before CallStarted; frozen retries preserve that shape |
| Required media snapshot | At most 32 × 256 KiB = 8 MiB; selected request at most 16 images = 4 MiB | Snapshot, materialized request, retry request and base64/provider value may coexist |
| Image decode | Existing 48 MiB decoder work budget | Serial capture per Call; concurrent Calls multiply this bound |
| Upload materialization | Existing 16 MiB capture/render bounds | Frozen projection and materialized request coexist |
| Raw provider response | 2 MiB cumulative HTTP body, including error bodies | Checked before SSE/JSON parsing in all three adapters; an unterminated frame cannot grow indefinitely |
| Generation | 256 KiB normalized output; 8 KiB visible; existing 4096 content-block frontier | One sequential request per Call; at most 3 attempts |
| Required persistence | Call start 8 KiB; Request start/terminal 16 KiB; audit record 12 KiB | Existing 32 process write scopes and bounded reaper pool |
| Retained result | 16/128 slots, 1/8 MiB Conversation/process | Pending reserves maximum; completion shrinks bytes; TTL/discard releases |
| Result response copies | Separate 16/128 slots, 1/8 MiB | DTO, encoding and transport acceptance buffers coexist until disposition |
| Observer queue | 16 records × 16/128 observers; 1024-byte text chunks | Retired on first Full; existing physical outbox remains separately bounded |
| Physical transport | 32 outbound records plus documented writer/encoding copies, 16 in-flight requests | Existing transport budgets, with a 64 KiB auxiliary record ceiling |
| Browser | 16 entries × (16+8+2) KiB logical UTF-8 payload | JS UTF-16 and transient concatenation/JSON copies are additional; one read owner |

For each active Call, account for all of the following together, even where
particular phases make their maxima mutually exclusive:

- Selected history `H <= 16 MiB`, a read page `h <= 4 MiB`, captured uploads
  `U <= 16 MiB`, the media snapshot `S <= 8 MiB`, and decoder workspace
  `D <= 48 MiB`. This consumer uses the same historical/complete view, so its
  actual image snapshot selects at most 16 images (4 MiB); 8 MiB is the shared
  Context component's conservative allowance.
- Frozen request `F <= 64 MiB` (counted serialization, excluding image bytes)
  plus `I <= 4 MiB` image bytes. A dispatched retry clone adds another `F + I`
  allowance; immutable input roots share where possible, but image Vecs copy.
- Provider translation has separate typed values, detached JSON values and one
  serialized HTTP body. Count up to three copies of the translated payload `P`;
  image encoding within each copy contributes `4 * ceil(I / 3)` bytes. Count
  escaping and protocol metadata in `P`, rather than adding only raw media.
  `validate_bounded_prepared_request` counts the actual final JSON through a
  bounded sink and enforces `P <= 64 MiB` before CallStarted on every protocol.
  This includes strings produced by re-encoding historical Tool JSON arguments,
  actual protocol wrappers and base64 data URLs. The boundary test uses NUL,
  newline, quote, backslash, Unicode and nested arguments, accepting the exact
  encoded length and rejecting one byte less for all three constructors.
  Preflight and actual dispatch both translate, sequentially. The pinned
  async-openai 0.41.3 request factory serializes once into shared `Bytes`; its
  factory/request clones share that buffer. Anthropic uses one reqwest JSON
  body. Reevaluate this multiplicity when either implementation changes.
- Raw parser input `B <= 2 MiB`, its SSE frame, parsed event and normalizer
  state can overlap. Charge four `B` payload allowances plus two normalized
  generation buffers `G <= 256 KiB`; JSON object/array and allocator overhead
  is additional. Even an oversized individual network chunk is rejected before
  entering these accumulators. The already-received chunk and HTTP/TLS buffering
  remain transport-library allocations, not retained model output.

Thus a conservative *payload ledger*, in MiB, is
`H + h + U + S + D + 2(F + I) + 3P + 4B + 2G`, at most
`236.5 + 3P <= 428.5 MiB` per admitted Call. This conservatively charges
all phase-local payload copies together; it is not a reserved RSS quota.
Multiplying by 3 gives `1285.5 MiB` per Conversation and by 24 gives
`10284 MiB` per process for external execution payloads. Preflight rejection
still performs bounded local translation from the already bounded neutral input;
its temporary tree/allocator overhead is part of the measured preparation work,
not an admitted oversized wire buffer. An RSS claim must include those overheads
and the other library costs above. The total/general caps of 4/3 and 32/24 still reserve Compaction
capacity. Retries multiply cumulative allocation and network traffic by at most
three; sequential physical join prevents multiplying simultaneously live adapter
copies by three again. Cancelled Calls retain this accounting until local exit.

Delivery adds, independently of execution, at most 8 MiB retained payload and
8 MiB counted response copies per process. Observer text payload is at most
`128 * 16 * 1024 = 2 MiB`, plus fixed identities/frontiers and container overhead.
With `C` admitted physical connections, auxiliary-only outbound encoding adds
`C * ((32 + 5) * 64 KiB + 32 * 2 KiB + 1024 bytes)` using the existing queue,
candidate, acceptance, frame and writer-buffer copy accounting. Mixed traffic
keeps the existing 1 MiB general-record transport envelope. In-flight input
records and manager/read owners retain their existing finite host budgets.
Browser retention adds at most `16 * 26 KiB = 416 KiB` logical UTF-8 payload,
up to twice that for UTF-16 strings, plus one response parse and transient
concatenation; one observation owner prevents parallel unbounded read copies.

The adapter maximum-media measurement uses 16 distinct PNG payloads
(256 KiB each; each decodes to 16 MiB), three concurrent real HTTP requests held
at response gates, three successive physical phases, and cancellation with real
join before reuse. It prints image Vec capacities, encoded request bytes and
Linux VmRSS/VmHWM at each held phase. The fixture deliberately retains its HTTP
request captures; its RSS therefore includes server-side copies and must not be
reported as native-client-only memory. This is an allocation experiment, not a
wall-clock assertion.

The separate native measurement enters the real BTW endpoint over the ordinary
catalog/HTTP binding and SQLite store. Three Calls capture the same 16 sealed
Artifacts through Context; two use all three real executor attempts, while the
third is cancelled at its first response gate and joined. After capture, the
fixture overwrites those Artifact files with different same-length bytes: all
retry HTTP bodies must remain byte-identical, while a new capture must fail
before another provider request. Phase snapshots report RSS/HWM including
concurrent PNG validation, native materialization, preflight and real translation.
The final snapshot is after host shutdown and all execution/writer joins.
Linux measured results (Rust 1.98.1, debug build; shared test process and fixture):

| Experiment | Encoded HTTP bytes/request | RSS at held phases (KiB) | HWM after joins (KiB) |
| --- | --- | --- | --- |
| Three real adapter requests, three phases | 5,593,720 | 188,996 / 212,136 / 254,580 | 254,708 |
| Three native BTW Calls, two retries each for two Calls, one cancelled | 5,593,928 | initial 223,632; first 408,124; retry phases 410,332 / 427,876 / 450,264 | 457,256 |

The native experiment made exactly seven HTTP requests, compared each successful
Call's three bodies byte-for-byte and rejected the post-tamper capture with no
eighth request. All assertions passed. These figures include allocator retention
and fixture captures; they establish the observed workload, not an upper RSS
bound or a promise that freed allocator pages immediately return to the OS.

Run these experiments explicitly with:

```sh
cargo test --test provider --all-features measure_auxiliary_maximum_media_concurrent_adapter_copies -- --ignored --nocapture
cargo test --lib --all-features measure_btw_maximum_shared_media_native_retries_and_cancel_join -- --ignored --nocapture
```

The exact/+1 byte unit tests and valid-response negative controls additionally
verify raw-budget enforcement for OpenAI Chat, OpenAI Responses and Anthropic,
including a valid over-budget SSE comment with no delimiter and valid whitespace
before an HTTP error object. All three controls succeed/parse when the cap is
removed, so an unrelated parser failure cannot make the enforcement test pass.

Increasing a limit requires reevaluating concurrent copies, physical owners,
encoded envelopes and the measured dependency costs, not just a test threshold.

Lifetime disk growth is intentionally distinct: one local Call plus at most three
Requests and their required settlement/audit facts remain for the Conversation
lifetime. TTL does not bound audit disk usage or erase submission association.

## Consumer integration template

A new internal consumer defines validated semantic input/fingerprint, reserves its
bounded delivery state, freezes its own model/resources, prepares a FrozenCall and
installs an AuxiliaryConsumer. It implements output/generation limits, optional
attempt/text observation and domain acceptance in the physical-settlement callback.
It uses existing admission, required-write, cancellation and physical join owners.
Do not add a purpose switch, scheduler, detached inference task, second durable
cursor, result ACK state machine or primary-runtime continuation to the executor.
Expose a named authorized boundary only when the product needs that consumer.

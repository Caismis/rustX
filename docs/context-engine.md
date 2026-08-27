# Context Engine and Context Assembly (M7.5b)

This document defines the implemented Issue #54 conversation model and the
Issue #55 context/request boundary. The important separation is:

~~~text
Message Ledger
    immutable committed conversational facts
        ↓
Conversation Surface @ SurfaceRevision
    active identity/order/visibility
        ↓
Context Engine
    finite projection, token accounting, retention, compaction
        ↓
Request-time Effective System Prompt
        ↓
frozen RequestSnapshot
        ↓
provider-neutral ModelRequest
        ↓
Model Adapter
    protocol translation only
~~~

Context Assembly is a sibling of the Context Engine and is coordinated by
the Agent Loop. It is not a plugin host and it does not mutate conversation
state.

The assembly boundary is genuinely awaited. `ContextContributor::contribute`
returns a boxed future over typed proposals and `ContextAssembly::assemble`
is async, so bounded native or future certified-extension reads settle at one
semantic boundary before the Agent Loop's final admission check. Contributors
still receive only the finite immutable snapshot; they do not receive an
execution, host, registry, history, ID allocator, or adapter handle.

## 1. Conversation authority

ConversationState is the single move-owned mutable conversation authority.
It contains an append-only MessageLedger and a first-class
ConversationSurface. The ledger owns immutable message bodies and their
canonical MessageIds. The surface owns the active ordered identities and
the accepted SurfaceOp history:

~~~rust
enum SurfaceOp {
    Append { message_id: MessageId },
    Replace { start: MessageId, end: MessageId, replacement: MessageId },
}
~~~

Every accepted operation advances a stable SurfaceRevision. A historical
revision is reconstructed by replaying the retained surface operations and
then hydrating those identities from keyed ledger reads. Later appends and
replacements do not change an earlier revision. No transcript, checkpoint,
summary cache, or provider request is a second history authority.

The ordinary commit path appends one canonical message and one surface
operation together. Compaction commits one canonical
UserSource::Runtime/InboundKind::CompactionSummary message and one
complete-message SurfaceOp::Replace. It never edits or deletes ledger
facts, splits an Assistant/tool structural unit, or stores summary truth
outside the ledger.

The Context Engine reads only the finite current surface, hydrates its active
messages by identity, and owns no contributor registry, provenance decision,
admission decision, request snapshot store, or provider adapter.

## 2. Unified Context Assembly

There is one rustX-owned assembly contract in src/context/assembly.rs.
Native observations and certified-extension observations enter the same
validation and ordering pipeline:

~~~text
claimed inbound (already canonical; not extension-controlled)
    ↓
ContributorInputSnapshot
    ↓
native observations + certified extension contributors
    ↓
typed transient ContextProposal values
    ↓
await all bounded contributor work
    ↓
rustX validation, provenance, semantic lanes, deterministic ordering
    ↓
AcceptedContext
    ↓
Agent Loop admission
    ↓
canonical User context facts + RequestSnapshot
    (including the frozen Effective System Prompt)
~~~

### Finite contributor input

ContributorInputSnapshot is an immutable, finite value containing exactly
the current invocation facts exposed by the implementation:

- attempt_id, conversation_id, and the model turn;
- the exact surface_revision and finite active surface_ids;
- the finite claimed_inbound message batch;
- canonical workspace_root;
- immutable attempt capability_revision.

Contributors receive no ConversationState, ledger/surface mutator,
registry, capability lease, current model configuration, clock, provider
adapter, or arbitrary runtime handle. The runtime samples native facts before
assembly. If a future contributor needs a bounded cancellable read, it must
be added as an explicitly owned immutable seam; accepted output is still
frozen before admission.

### Proposals and trust boundaries

The request-time contributor proposal kind is:

- UserMessageProposal { content };

Certified-extension System Sections are registered as immutable resource
values in the owning Runtime Resource Snapshot. They are not dynamically
proposed by an extension on every request.

Proposals contain no MessageId, UserSource, ContextKind, priority, surface
operation, admission command, or provider data. A proposal is transient and
cannot mutate committed history. RustX assigns the semantic kind/lane,
trusted provenance, canonical identity, and commit operation.

Native inputs currently include project/workspace instructions, Skill
capability guidance, Agent Status, core runtime identity, and agent profile.
Project instructions and Skill capability guidance are request-time System
Sections, while Agent Status is a canonical User context fact. Native
identities use
ContextContributorIdentity::Native; extensions use
CertifiedExtensionIdentity, which is canonicalized and validated by rustX.
Native logical keys cannot be claimed by an extension.

The logical extension identity is stable and serializable. An optional
attestation/package/content generation is recorded separately in
ContributorGeneration and does not affect ordering. Thus a package upgrade
that preserves its logical key cannot reorder unrelated extensions.
Contributor invocation order is canonical logical identity order, never
registration order, discovery timing, an address, a handle, or an index.

### Semantic lanes

The finite user lanes are ordered as:

1. ClaimedInbound — already claimed canonical input, never extension-owned;
2. RuntimeToolObservation — the lane of the **native runtime observation
   owner** (`NativeContextContributor::RuntimeToolObservation`);
   native-reserved, never extension-owned;
3. ExtensionEnvironment — multiple certified extensions;
4. AgentStatus — one native-reserved owner.

This is a semantic lane order, not a physical adjacency promise: the
request-scoped AgentStatus message is the final User-context lane even when
other canonical messages or provider-specific wire encoding occur between
the inbound fact and its presentation.

The native observation lane sits directly after claimed inbound because that
owner's facts describe what the environment just did for the *preceding* tool
batch, while every later lane describes the *current* request.

### Timing is not ownership

A lane names *who owns a fact*, never *when the fact became eligible*. The
Issue #56 `ToolResultObserver` seam establishes eligibility — a proposal it
returns is *deferred*, admitted at the next primary step rather than this one —
and eligibility is a lifecycle-timing property owned by the Agent Loop.

Semantic ownership stays here. Every deferred proposal reaches assembly
carrying the producer reference the Agent Loop stamped from its observer's
*binding*. Assembly resolves that reference to an authoritative registration
(next section) and only then derives the lane, the `UserSource`, and the
`ContextKind` — through the same table it applies to that owner's request-time
proposals:

| producer identity | lane | `UserSource` | `ContextKind` |
| --- | --- | --- | --- |
| `Native(RuntimeToolObservation)` | `RuntimeToolObservation` | `Runtime` | `RuntimeToolObservation` |
| `Native(AgentStatus)` | `AgentStatus` | `Runtime` | `AgentStatus` |
| `CertifiedExtension(key)` | `ExtensionEnvironment` | `Extension { key }` | `ExtensionEnvironment` |

So a certified extension that produces deferred post-tool context keeps its
extension identity, its extension provenance, and its own lane. There is no
rule converting post-tool proposals into native runtime context.

`Native(WorkspaceInstructions)` and `Native(SkillGuidance)` are intentionally
absent from this User-context table. They publish only the
`SystemSectionLane::WorkspaceInstructions` and
`SystemSectionLane::NativeCapabilityGuidance` sections respectively. Neither
has a `UserSource`, `ContextKind`, User message, Ledger entry, or Surface
identity. Their bytes come from the attempt's immutable Runtime Resource
Snapshot; Skill catalog text contains routing metadata only.

### Registration is the only semantic admission authority

A deferred proposal does not arrive with a trusted identity. It arrives with a
`DeferredContextProducer`, which is a **reference**:

- `NativeRuntimeObservation` — rustX owns this semantic owner, so it needs no
  registration and carries no attestation;
- `CertifiedExtension { identity }` — a logical key and nothing more. Any
  caller can construct a `CertifiedExtensionIdentity`; the string proves
  nothing.

`ContextAssembly::assemble` resolves the reference against the extensions
registered through `ContextAssembly::register_extension` — the one place an
extension becomes trusted — and uses that registration's own
`ContributorGeneration`, attestation included. An unknown key is rejected with
`ContextAssemblyError::UnregisteredContributor`: no lane, no
`UserSource::Extension`, no synthesized generation, and no partially admitted
batch. A lifecycle observer therefore cannot become a second extension
registry by naming an extension, and a certified extension that only produces
deferred context still gets its authoritative generation without contributing
anything at request time.

### Deferred proposals are User context

A deferred proposal is a `UserMessageProposal`, never the full
`ContextProposal` vocabulary. The post-tool seam publishes conversational
context about a settled tool batch; the Effective System Prompt stays owned by
the request-time contributor path. The restriction is carried by the
`ToolResultObserver` return type, so a deferred system section is
unrepresentable rather than rejected at runtime.

Inside one `(lane, contributor)` bucket, a deferred fact precedes the same
owner's request-time fact, because it describes the batch that precedes the
step. Beyond that the order is the Agent Loop's staging order, which is
`(canonical ToolCall batch position, producer identity, proposal FIFO)`;
physical tool completion timing and observer registration order never reach
it. Owners with no lane for a proposed kind (`CoreSystemIdentity` and
`AgentProfile` publish no User context) are rejected rather than relaned.

The deferred batch is bounded by `MAX_DEFERRED_CONTEXT_PROPOSALS` over all
producers together; the Agent Loop enforces the same bound earlier, at its
observer transaction boundary, together with the established
`MAX_PROPOSALS_PER_CONTRIBUTOR` bound on each single observation. There is no
separate deferred-only proposal constant.

The finite system-section lanes are ordered as:

1. CoreRuntimeIdentity — native-reserved, single owner;
2. AgentProfile — native-reserved, single owner;
3. WorkspaceInstructions — native-reserved, single owner;
4. CertifiedExtension — multiple extensions, sorted by logical identity;
5. NativeCapabilityGuidance — native-reserved.

There is no arbitrary numeric priority. A second single-owner semantic owner
is rejected; last-wins, first-wins, priority-wins, registration order, and
package discovery order are not conflict policies. The manifest is derived
from the same lane, native-owner, provenance, and proposal-kind constants
used by assembly validation.

## 3. Canonical model-visible context

When accepted, a UserMessageProposal becomes a normal canonical
MessageBlock::User with InboundKind::Context(ContextKind):

| semantic fact | trusted source | context kind |
| --- | --- | --- |
| certified extension context | Extension { contributor } | ExtensionEnvironment |
| Agent Status | Runtime | AgentStatus(metadata) |

The Agent Loop admission function is the only path that allocates context
message identities, appends ledger facts, and advances the surface. It
allocates IDs once, commits each accepted message through
ConversationState::commit, and records the accepted ContextGeneration.
Contributors cannot select provenance or identity.

Agent Status remains structured runtime-owned data before rendering. At the
single primary-model-step preparation boundary, the Agent Loop freezes one
finite Pre-Status Surface view by copying the active Surface identities and
hydrating only those identities from the Message Ledger. It samples the clock
once and captures one immutable authoritative Background registry snapshot and
one committed Todo snapshot; the closed, rustX-owned Time, Background, and Todo
modules then evaluate those shared inputs once against one finite
`AgentStatusOpportunitySet`. FreshInbound and PostToolBatch are independent
members of that set and may coexist; neither opportunity makes a module
contribute automatically.
The engine validates `Time <-> Temporal`, `Background <-> BackgroundExecution`,
and `Todo <-> Todo`, applies module-local bounds, then admits whole sections
under a global UTF-8-byte cap in `Time -> Background -> Todo` source order. It
never scans conversation prose or infers current state from regular
expressions: visible status membership comes from typed canonical generation
metadata, whose private validated representation rejects invalid durable
membership, current Background activity comes only from the registry, and Todo
state comes only from `ConversationTodoList::committed()`.
Two status generations with identical bytes at different admitted steps are
different facts and receive different MessageIds; Todo's separate semantic
fingerprint is about suppression, not canonical message identity. A failed
module is quarantined for the current attempt while surviving modules
continue. Overflow compaction retries reuse the accepted generation and do not
rescan, recapture, or reevaluate it.

The complete canonical ToolResult batch is committed before the Agent Loop
marks the attempt-local PostToolBatch opportunity. The marker is consumed only
by the next already-existing primary step, is never persisted or recovered,
and cannot create a model request. RuntimeToolObservation and AgentStatus
remain separate producers; the former is admitted before the latter.

The production Todo policy is deliberately bounded and semantic: a committed
snapshot with at least one non-terminal Pending or InProgress task is
actionable; blocked active tasks remain relevant and are marked/count as
blocked. Empty, completed-only, and deleted-only snapshots emit nothing. The
first InProgress task is `current`, remaining active tasks stay in creation
order, at most six active tasks are shown, and subjects/active forms are capped
at 256 UTF-8 bytes. Complete-snapshot active, blocked, completed, deleted, and
omitted counts are included. The stable key is `active_actionable`; the
fingerprint is the SHA-256 of the bounded structured presentation. A bounded
latest-emission head suppresses an identical fingerprint while fewer than four
later newly committed first requests of logical primary model steps have
followed the reminder's store-assigned origin; the identical state is eligible
again at exactly four, while a changed fingerprint is eligible at the next
opportunity. The progress coordinate is the durable
`todo_progress_sequence`, which advances once per successful
`retry_number == 0` model-turn start. Same-start context/status,
RuntimeToolObservation, Time, Background, compaction, overflow retries,
cancellation, and failed transactions do not advance it. It is not a
wall-clock or generic cooldown framework and never schedules a model turn.

The Skill catalog is sampled from the immutable per-attempt capability
snapshot and assembled as a request-time native capability section. Normal
rustX agent composition always contains canonical native Read, so Skill
visibility is filtered at the Skill level (`disable-model-invocation`) rather
than by a downstream optional-Read predicate. It is distinct from
`ModelRequest.tools`: tool definitions remain capability/request state, and the
Skill catalog is never copied into canonical User messages. Skills are trusted
instruction packages in the current rustX threat model; structural catalog
escaping remains, without a semantic trust tier.
When visible Skills exist, each entry contains deterministic name and
description metadata plus the host path of the package's `SKILL.md`. The
guidance tells the model to read that path, and to resolve a Skill's own
relative references against the directory it names. Full Skill bodies are loaded only
after an explicit native Read call and enter the ordinary tool-result
conversation path.

The former model-request-only semantic attachment paths are removed. There
is no hidden Agent Status or Skill insertion during adapter translation and
no projection-only second transcript.

## 4. Effective System Prompt

`ModelRequest.effective_system_prompt` is the one provider-neutral System
authority. RustX renders it with `render_effective_system_prompt` from:

1. native CoreRuntimeIdentity sections;
2. native AgentProfile sections;
3. frozen project/workspace instructions;
4. certified-extension sections in canonical logical identity order;
5. native capability guidance owned by rustX, including the
   request-time Skill catalog when the immutable snapshot has visible Skills.

The section family is the SystemSectionLane contract. Extensions can
contribute only the certified-extension lane; they cannot claim or shadow a
native section and cannot replace the entire prompt. RustX owns the final
section ordering, separators, and rendered string. The exact rendered string
is frozen by value in every RequestSnapshot; reconstruction never reruns
section contributors, rediscover Skills, or reads current configuration.
Compaction rewrites only canonical conversation facts, so it cannot remove
or suppress project instructions or the Skill catalog section. Provider
adapters translate this value into their protocol's System/instructions field
when non-empty and emit no System authority when it is empty; they never scan
historical messages.

## 5. Request preparation and RequestSnapshot

RequestSnapshot is the provider-independent frozen non-history boundary for
one actual primary request. Its implemented fields are:

~~~rust
struct RequestSnapshot {
    request_id: RequestId,
    identity: RequestIdentity,
    surface_revision: SurfaceRevision,
    effective_system_prompt: String,
    system_sections: Vec<AcceptedSystemSection>,
    runtime_resource_revision: RuntimeResourceRevision,
    invocation: ModelInvocationConfig,
    context_window_tokens: u64,
    reasoning_profile: Option<ReasoningProfileId>,
    reasoning_enabled: bool,
    tool_definitions: Vec<ModelToolDefinition>,
    capability_revision: CapabilityRevision,
    context_generation: ContextGeneration,
    continuation: Option<ProviderContinuationState>,
    request_context_ids: Vec<MessageId>,
}
~~~

RequestIdentity contains attempt_id, turn, and retry_number.

The value/reference decisions are deliberate:

- surface_revision is a reference to the immutable historical
  ConversationSurface operation history. It is exact and reconstructable;
  the request does not copy a second transcript.
- effective_system_prompt is stored by value because it is request-time
  rendered content. `system_sections` stores its exact ordered inputs by value;
  reconstruction does not invoke contributors again.
- runtime_resource_revision records which process-local immutable resource
  generation the attempt observed. It is an audit identity, not a historical
  lookup key or a durable resource registry.
- invocation stores the effective ModelInvocationConfig by value. It
  includes the effective model identity/configuration and opaque request
  parameters, so current session settings and the model catalog are not
  consulted later.
- context_window_tokens, reasoning_profile, and reasoning_enabled are
  frozen effective model/reasoning values. The attempt's immutable model
  snapshot owns their authority.
- tool_definitions are stored by value. capability_revision is retained
  for audit/explanation; the frozen definitions, rather than a mutable
  capability registry lookup, make historical reconstruction exact.
- context_generation records the accepted contributor logical identities
  and separate attestations. It explains assembly without requiring a
  contributor to be invoked again.
- continuation is the exact opaque provider continuation state used by that
  request, if any.
- request_context_ids records the exact request-scoped canonical context facts
  committed atomically with request start.

RequestSnapshot::reconstruct(&ConversationState) resolves only the
referenced historical Surface revision, hydrates its canonical messages,
and combines them with the frozen non-history fields. The Agent Loop calls
this before adapter translation and structurally compares the result with
the actual ModelRequest; a mismatch is a core reconstruction failure.

Historical reconstruction never reads current model/session settings,
current model catalog, Skill discovery, contributor registration or
execution, package contents, filesystem state, runtime status, or latest
Surface head.

### Primary request lineage and the summary side request

The primary model lineage is:

~~~text
pinned RuntimeResourceSnapshot + pinned CapabilitySnapshot
    + current Effective System Prompt
    + canonical active Surface
    + current Tool definitions
    + the primary provider continuation, when compatible
~~~

Compaction deliberately leaves that lineage. Its summarizer is one isolated,
runtime-owned side request:

~~~text
runtime-owned summary instruction
    + the planned retired Surface messages rendered as a bounded plain-text
      transcript, in order
    + no Tools
    + no primary System guidance, project instructions, Skill catalog,
      extension Tool definitions, or primary continuation
~~~

The rendering is deliberately not the canonical JSON encoding. Canonical JSON
is the durable interchange format; pushing it through a model request spends
more input on structure, field names, and escaping than on the conversation
itself, which is how a compaction triggered by a context overflow overflows
again. The transcript keeps who said what, which tools ran with which
arguments, and how they ended, and truncates the bulk contributors — tool
results, replayed reasoning, and tool-call arguments — always with an explicit
truncation notice, so a summary model can never mistake a cut-off result for a
complete one. The rendering remains a pure deterministic function of the
retired span and is shared by the planner's estimate and the production
request.

Two budgets bound this request, and both carry the session reserve:

~~~text
soft input limit    = window - reserve - primary output budget
summary input limit = summary window - reserve - summary output budget
~~~

The reserve is on both sides because the estimate that sizes them is
approximate. When a provider measures that approximation — by rejecting a
request as oversized — the measurement is used rather than discarded. A
summary-model rejection replans the same compaction against a halved summary
input budget (bounded, strictly decreasing, floored); a primary-model overflow
produces an `EstimateCorrection`, the exact integer ratio between this
runtime's estimate for the rejected request and the provider's reported count
for it, and the soft input limit above is scaled by that ratio for the
recovery compaction. With no reported count the correction is a fixed
three-quarters shrink. Either way the recovery never aims at the budget that
just failed.

A correction never crosses to the summary input limit, even when the summary
invocation names the same model. The ratio is a measurement of one request,
not a calibration of a tokenizer: the deviation it records can come from the
provider continuation, the tool schemas, the effective system prompt, or
request-specific fixed overhead, and the summary request carries none of
those. A stored continuation alone can put six figures of provider-counted
input behind a primary request that the summary request will never send, so
reusing the ratio could compress a workable summary budget to `CannotFit` on
evidence about something else. Each request is measured by its own rejection:
the summary model's own rejection is what shrinks the summary budget.

The summary request does not inherit the primary Effective System Prompt,
depend on the primary request prefix, share provider KV/cache continuity, or
re-enter the Agent Loop. The returned value remains an opaque free-form
`String`. Its Pi-inspired organization is prompt guidance only: rustX does
not parse headings, validate a schema, extract fields, or maintain a separate
previous-summary accumulator. A useful non-empty summary is accepted even
when it does not follow suggested organization; empty or whitespace-only text
fails the compaction.

Runtime and Agent Status observations in retired history are historical
evidence. The summarizer may describe a task as having run earlier and later
completed, but the resulting text never becomes current runtime authority.
Absence of a later Agent Status section is not lifecycle completion unless
the relevant status contract explicitly gives absence that meaning.

The Ledger, Surface, and RequestSnapshot have separate ownership:

- the Ledger is the immutable, auditable set of historical conversation facts;
- the Surface is the active historical representation currently sent to a
  primary model, and compaction replaces exactly one valid span in it;
- a RequestSnapshot freezes the exact request-time Effective System Prompt,
  System sections, Tool definitions, model values, Surface revision, and
  continuation needed to reconstruct one historical primary request;
- the Runtime Resource Snapshot is process-local current executable
  authority, not durable compaction state and not a source for rewriting old
  snapshots or summaries.

Compaction never discovers or reloads resources. Explicit reload remains a
quiescent runtime lifecycle operation, and a cold reopen may publish a new
current resource generation for future attempts while preserving old Ledger
facts, summaries, and RequestSnapshots byte-for-byte.

## 6. Context Engine responsibilities

The Context Engine receives canonical messages, immutable model/tool inputs,
and the exact Effective System Prompt. It owns only:

- finite current-Surface projection;
- deterministic token estimation and provider-measurement provenance;
- soft-limit pressure detection;
- complete-message retention and compaction planning;
- compaction summary generation and the prepared command consumed by the
  durable `ConversationStore::commit_compaction` transaction.

The default estimate is the deterministic UTF-8 serialized request content
with ceil(bytes / 4). Tool definitions and the Effective System Prompt
contribute to the full request estimate; only conversation content counts
toward keep_recent_tokens. A projection contains complete canonical
messages only and carries surface_revision, messages, the exact prompt, and
its measurement.

That estimate is only the fallback. A provider-reported `input_tokens` is a
measurement of a real request, and it is reused for as long as it remains
true:

~~~text
exact  same fingerprint                     -> ProviderReported (the number)
anchor measured messages are an ordered
       prefix, non-conversation input
       unchanged                            -> ProviderAnchored
                                               (the number + estimate of the
                                                messages appended since)
none   otherwise                            -> Estimated (whole projection)
~~~

The anchored case is the one that matters. A provider-neutral `bytes / 4`
approximation drifts further from the truth the longer a conversation runs,
and a whole-conversation estimate compounds that drift over every message
ever sent — precisely when the soft-limit decision matters most. Anchoring
confines the error to the messages appended since the last completed request.
The anchor covers the Effective System Prompt and the tool definitions of the
measured request, because their cost is inside the reported number; a change
to either, or a compaction Surface rewrite that destroys the prefix, refuses
the anchor rather than patching it with a guessed delta.

The Context Engine is deliberately narrow. It is not a lifecycle or hook
host: the Issue #56 `PreStepPolicy` and `ToolResultObserver` seams belong to
the Agent Loop (`src/agent/lifecycle.rs`), and the engine never observes,
evaluates, or stages them.

Compaction is structural. It never splits an Assistant message or a
tool-call/result pair. It appends one canonical runtime summary and applies
one complete-message Surface replacement. Historical ledger facts remain
addressable, while normal reads hydrate only finite active surface IDs.

## 7. Model-turn start and cancellation

The Agent Loop owns the model-turn start boundary in
`AgentExecution::start_model_turn` (Issue #12, M9b):

~~~text
Agent-Loop-owned deferred proposals (staged after the previous tool batch
reached structural settlement, each stamped with its producer identity)
    ↓ drained into the assemble() deferred argument
ContextAssembly::assemble (transient proposals; lanes and provenance
                           derived from producer identity)
    ↓
PreStepPolicy → Enter | Reject(reason)          (Issue #56)
    ↓ test synchronization seam, when enabled
stage_context → scratch validation + prepared canonical commits
    ↓ (no durable effect)
prepare_model_turn → frozen RequestSnapshot + provider ModelRequest
    ↓
┌─ cancellation-vs-start arbitration (start gate held) ──────────┐
│ cancellation check                                             │
│     ↓ not cancelled                                            │
│ ConversationStore::commit_model_turn_start                     │
│     → ONE transaction: canonical request-scoped User context  │
│       + ledger/Surface + RequestSnapshot (frozen prompt)      │
│       + ModelRequestStarted + sequence binding                 │
└────────────────────────────────────────────────────────────────┘
    ↓
invoke adapter
~~~

Cancellation that linearizes before the arbitration produces no accepted
dynamic-context messages, no associated Surface advancement, no
RequestSnapshot, no start fact, and no provider request. The contributors
may have performed one bounded read, but their transient proposals are
discarded. Once the start commit linearizes first, the request is durably
started; a later cancellation settles that started request and can never be
reclassified as never-started. The race regressions park the execution
immediately before the arbitration (and inside it, before the commit)
through the `StartBoundaryPause` test seam — ordinal-counted parks and
explicit releases, never a sleep.

A pre-step rejection or policy failure settles the attempt with the same
guarantee: no accepted dynamic context, no Surface advancement, no
RequestSnapshot, and no provider request. Because deferred proposals enter
the *same* final batch, an observer cannot commit context around the policy
either — whatever identity it produces context for.

A start-commit durability failure rolls the whole transaction back: no
half-committed canonical User context, no snapshot, no start fact, and no
provider request. Accepted system sections are transient assembly values;
their rendered value is durable through RequestSnapshot. After the start
commit succeeds, the accepted context is
historical: provider failure, disconnect, timeout, or cancellation does not
roll back the ledger, surface, context generation, or snapshot.

## 8. Overflow compact-and-retry

The bounded ContextWindowExceeded path is:

~~~text
one staged primary step
    → start arbitration → RequestSnapshot #1 / provider request #1
    → overflow
    → complete-message Surface compaction (independent durable commit)
    → start arbitration → RequestSnapshot #2 / provider request #2
~~~

Assembly, Agent Status capture/evaluation, Skill snapshot rendering,
extension invocation, logical ordering, contributor generation, and staging
happen once. The retry reuses the staged ContextGeneration and the canonical
context facts committed at the first start, including the one
canonical-message-bound Agent Status emission settlement. It never reinvokes
contributors, rereads Todo authority, reevaluates the opportunity set, or
stages a duplicate context batch, but it passes through the same
cancellation-vs-start arbitration as every model turn: cancellation that
linearizes before the retry's start commit stops the retry while the
already-committed compaction remains an independent durable fact.

Between the overflow and the retry's start arbitration, the compaction
planning filter evaluates every candidate through the same `TokenEstimator`
over the exact hypothetical post-compaction request — the retained Surface
plus the staged (not yet committed) request-scoped context overlay
(`CompactionConstraints::staged_request_context`) plus the Effective System
Prompt plus tools. The overlay is neither canonical nor retirable, and it is
never folded into a scalar token delta: the estimator has no additive or
monotonic contract, so a staged-context token reservation is not a reusable
quantity across different candidate projections. The staging is planning-only
state with no durable effect.

`ContextWindowExceeded` is not evidence that a model invocation observed the
fresh inbound turn: the provider rejected that request. Therefore overflow
compaction receives the still-pending `FreshInboundTurn` constraint and may
not retire the fresh messages. This constraint is independent of the
accepted dynamic-context generation, so preserving fresh inbound never causes
assembly to run again.

Compaction may change the exact SurfaceRevision, projected messages,
continuation compatibility, and therefore the retry request identity. The
retry gets retry_number = 1; the original request remains independently
reconstructable from its own snapshot and surface revision. If the provider
overflows again, the bounded retry budget is exhausted.

The overflow regression uses a deterministic fake adapter and the closed
Agent Status capture/evaluate counter. Its compaction candidate would cross
pending fresh inbound without the constraint; the protected retry still
contains the inbound identity. It proves one Agent Status generation per
primary step, one accepted context generation, changed revisions when
compaction occurs, and structural equality for both actual requests and their
reconstructions.

### Manual compaction

`ConversationRuntime::compact_context` exposes the same compaction pipeline
as an explicit idle maintenance operation. It freezes the session model,
summary invocation, context policy, and one immutable capability snapshot at
admission. Active tool definitions and the snapshot's rendered Skill catalog
are sampled together. The catalog remains non-conversational and cannot be
summarized or retired, but it is part of the Effective System Prompt used by
planning and exact post-compaction fit validation. The operation then checks
out the sole `ConversationState` while the provider-backed summary runs. It
allocates no attempt or turn identity, invokes no tools, and commits through
the same atomic summary + Surface replacement transaction as automatic
compaction.

The operation is rejected as `Busy` when an attempt or another manual
compaction owns the conversation. Inbound messages accepted during the
operation remain in the durable pending inbox and are admitted after the state
is restored. A pre-commit planning, summary, fit, or cancellation failure
restores the prior state; a durable-commit failure enters the runtime's
absorbing durability-failed gate. Runtime drain cancels and awaits the summary
task instead of abandoning checked-out state.

Runtime Client projects `ContextCompactionStarted`,
`ContextCompactionFailed`, and `ContextCompacted` for both entry points. The
automatic path carries its `AttemptId`; manual maintenance carries no attempt
identity. A manual completion is published only after `ConversationState` has
returned to the coordinator and the maintenance slot is clear, so
`compaction_in_progress = false` is live ownership state rather than an early
durable-commit signal. The TUI `/compact` command invokes exactly one
`compact_context` request and renders the published in-progress state as
`Compacting context…`.

## 9. Durable RequestSnapshot ownership (M8 / Issue #11)

`AgentExecution` is the bounded active execution owner: it carries the
current ConversationState, current request/turn assembly, continuation and
tool work needed to proceed, but no complete Request Snapshot or Event
Journal history. `AgentExecutionResult` returns the settlement candidate and
current conversation state, not a historical archive.

During execution, the current request snapshot is prepared from the exact
provider-neutral request. `ConversationStore::commit_model_turn_start`
commits the canonical request-scoped User context, that immutable snapshot
(including the frozen Effective System Prompt), and its
`ModelRequestStarted` Event Journal fact in one transaction before the
adapter is invoked, under the attempt's start gate (M9b). `RequestHistory` is a durable
read handle, not an append-only `Vec<RequestSnapshot>` and not a second
transcript. Keyed lookup reconstructs one request on demand; historical
listing uses a bounded, fallible page with an exclusive durable sequence
cursor, so runtime bootstrap and normal admission never enumerate the full
history.

For a status-bearing primary request, the successful durable start commit is
also the publication boundary: the canonical status User message, its
canonical-message-bound `AgentStatusEmitted` fact(s), and the latest-emission
head(s) are committed atomically with the Request Snapshot and
`ModelRequestStarted`. The canonical status message is observed first, then
the structured Agent Status observation is published, and only then is the
provider invoked. The typed start receipt exposes the newly committed
`ModelRequestStarted` followed by every `AgentStatusEmitted` fact in durable
sequence order; the live `AgentExecutionObserver` receives that same order
after COMMIT, while the Runtime Client projection intentionally folds the
internal emission fact into the one structured status observation. If
cancellation wins before that commit, neither status view nor any
emission/head settlement is visible. Exact start retries verify the complete
status context and emission set; contradictory metadata is rejected, and an
idempotent retry does not republish historical events to the live observer.

The Event Journal follows the same boundary: append durably, publish the live
observer, then release the committed event body from the attempt. Historical
execution facts are read with bounded `ConversationStore::read_events` pages.

`ConversationStore::reconstruct_model_request` loads one snapshot, replays its
immutable Surface revision, and resolves only the referenced Ledger bodies.
It never invokes contributors, reads current model/tool/capability state,
reruns Skills or extensions, samples status, or inspects the workspace. The
Agent Loop compares this independent result with its live request before
dispatch. Historical requests remain reconstructable after later messages,
compactions, restart, and configuration changes.

## 10. Provider boundary

Adapters receive the final canonical projection and frozen request values.
They may translate provider protocol details, serialize the effective system
prompt, encode tools, handle continuation fields, and merge consecutive
user-role messages when a provider requires it. Such wire merging does not
change canonical message identity, order, provenance, or semantic kind.

Adapters must not sample Agent Status, discover Skills, read current
configuration, invoke contributors, allocate canonical IDs, mutate the
ledger/surface, admit context, or repair a missing prompt/context field.

OpenAI Chat Completions, OpenAI Responses, and Anthropic Messages therefore
translate the same provider-neutral ModelRequest. The external fake provider
regression inspects the actual wire body and verifies that Agent Status and
Skill context arrived through the canonical assembled semantics, not through
hidden adapter injection.

## 11. Compatibility manifest

`ContextAssembly::compatibility_manifest()` returns
ContextCompatibilityManifest with:

- `abi_version` (currently `3`; the v3 contract freezes certified-extension
  System Sections in runtime resources and limits dynamic proposals to
  conversational User facts);
- canonical user_context_lanes;
- canonical system_section_lanes;
- native-reserved slots;
- multi-extension slots;
- trusted provenance namespaces;
- allowed proposal kinds.

The values are mechanically derived from the same finite enum arrays,
native identity list, UserSource namespace list, and proposal-kind list used
by validation. This is a machine-readable contract projection, not a plugin
loader, middleware framework, package marketplace, or DSH/Cordis runtime.

## 12. Invariants and test seams

The implementation tests deterministic ordering under registration
permutation, logical identity stability across attestation changes,
reserved-slot and provenance rejection, immutable contributor input,
history-mutation isolation, distinct IDs for equal bytes, historical
reconstruction after live-state changes, prompt freezing, pre-start
cancellation, post-start failure, overflow reuse, and adapter wire
translation. cfg(test) barriers/channels make the start races reproducible.

These boundaries preserve the core equation:

~~~text
Conversation Surface @ revision X
    + RequestSnapshot X
    = exact provider-neutral ModelRequest X
~~~

There is no second canonical transcript and no compatibility path for the
superseded model-request semantic attachment architecture.

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
canonical User context facts and request-time system sections
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

The only proposal kinds are:

- UserMessageProposal { content };
- SystemPromptSectionProposal { content }.

Proposals contain no MessageId, UserSource, ContextKind, priority, surface
operation, admission command, or provider data. A proposal is transient and
cannot mutate committed history. RustX assigns the semantic kind/lane,
trusted provenance, canonical identity, and commit operation.

Native observations currently include workspace instructions, Skill
capability guidance, Agent Status, core runtime identity, and agent profile.
Skill capability guidance is a request-time system section, while Agent Status
is a canonical User context fact. Native identities use
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
3. WorkspaceInstructions — one native semantic owner;
4. ExtensionEnvironment — multiple certified extensions;
5. AgentStatus — one native-reserved owner.

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
| `Native(WorkspaceInstructions)` | `WorkspaceInstructions` | `Runtime` | `WorkspaceInstructions` |
| `Native(AgentStatus)` | `AgentStatus` | `Runtime` | `AgentStatus` |
| `CertifiedExtension(key)` | `ExtensionEnvironment` | `Extension { key }` | `ExtensionEnvironment` |

So a certified extension that produces deferred post-tool context keeps its
extension identity, its extension provenance, and its own lane. There is no
rule converting post-tool proposals into native runtime context.

`Native(SkillGuidance)` is intentionally absent from this User-context table.
It publishes only the `SystemSectionLane::NativeCapabilityGuidance` section;
it has no `UserSource`, `ContextKind`, User message, Ledger entry, or Surface
identity. The catalog text is rendered from the attempt's immutable
`SkillSnapshot` and contains routing metadata only.

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
3. CertifiedExtension — multiple extensions, sorted by logical identity;
4. NativeCapabilityGuidance — native-reserved.

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
| workspace/project instructions | Runtime | WorkspaceInstructions |
| certified extension context | Extension { contributor } | ExtensionEnvironment |
| Agent Status | Runtime | AgentStatus |

The Agent Loop admission function is the only path that allocates context
message identities, appends ledger facts, and advances the surface. It
allocates IDs once, commits each accepted message through
ConversationState::commit, and records the accepted ContextGeneration.
Contributors cannot select provenance or identity.

Agent Status remains structured runtime-owned data before rendering. It is
composed from authoritative runtime facts: the runtime clock/timezone,
persisted inbound timestamp, and the background registry snapshot. It never
scans conversation prose or infers current state from regular expressions.
Two status snapshots with identical bytes at different admitted steps are
different facts and receive different MessageIds; content deduplication is
not a semantic operation.

The Skill catalog is sampled from the immutable per-attempt capability
snapshot and assembled as a request-time native capability section. It is
distinct from `ModelRequest.tools`: tool definitions remain capability/request
state, and the Skill catalog is never copied into canonical User messages.
When visible Skills exist, the section contains only deterministic name and
description entries plus the virtual `.rustx/skills/<skill-name>/SKILL.md`
Read instruction. Full Skill bodies are loaded only after an explicit native
Read call and enter the ordinary tool-result conversation path.

The former model-request-only semantic attachment paths are removed. There
is no hidden Agent Status or Skill insertion during adapter translation and
no projection-only second transcript.

## 4. Effective System Prompt

SystemMessageBlock remains a durable canonical system-authority fact.
Effective System Prompt is a separate request-time provider-neutral value.
RustX renders it with render_effective_system_prompt from:

1. active canonical System message content;
2. native CoreRuntimeIdentity sections;
3. native AgentProfile sections;
4. certified-extension sections in canonical logical identity order;
5. any native capability guidance section owned by rustX, including the
   request-time Skill catalog when the immutable snapshot has visible Skills.

The section family is the SystemSectionLane contract. Extensions can
contribute only the certified-extension lane; they cannot claim or shadow a
native section and cannot replace the entire prompt. RustX owns the final
section ordering, separators, and rendered string. The exact rendered string
is frozen by value in every RequestSnapshot; reconstruction never reruns
section contributors, rediscover Skills, or reads current configuration.
Compaction rewrites only canonical conversation facts, so it cannot remove
or suppress the Skill catalog section.

The bounded Issue #54 Surface rule still applies: a SurfaceOp::Replace
cannot contain a canonical System message. This is a history/compaction
constraint, not a rule that pins all messages before the last System
message.

## 5. Request preparation and RequestSnapshot

RequestSnapshot is the provider-independent frozen non-history boundary for
one actual primary request. Its implemented fields are:

~~~rust
struct RequestSnapshot {
    identity: RequestIdentity,
    surface_revision: SurfaceRevision,
    effective_system_prompt: String,
    invocation: ModelInvocationConfig,
    context_window_tokens: u64,
    reasoning_profile: Option<ReasoningProfileId>,
    reasoning_enabled: bool,
    tool_definitions: Vec<ModelToolDefinition>,
    capability_revision: CapabilityRevision,
    context_generation: ContextGeneration,
    continuation: Option<ProviderContinuationState>,
}
~~~

RequestIdentity contains attempt_id, turn, and retry_number.

The value/reference decisions are deliberate:

- surface_revision is a reference to the immutable historical
  ConversationSurface operation history. It is exact and reconstructable;
  the request does not copy a second transcript.
- effective_system_prompt is stored by value because it is request-time
  rendered content.
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

RequestSnapshot::reconstruct(&ConversationState) resolves only the
referenced historical Surface revision, hydrates its canonical messages,
and combines them with the frozen non-history fields. The Agent Loop calls
this before adapter translation and structurally compares the result with
the actual ModelRequest; a mismatch is a core reconstruction failure.

Historical reconstruction never reads current model/session settings,
current model catalog, Skill discovery, contributor registration or
execution, package contents, filesystem state, runtime status, or latest
Surface head.

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
│     → ONE transaction: request-scoped context appends          │
│       + ledger/Surface + RequestSnapshot                       │
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
half-committed request-scoped context, no snapshot, no start fact, and no
provider request. After the start commit succeeds, the accepted context is
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

Assembly, Agent Status sampling, Skill snapshot rendering, extension
invocation, logical ordering, contributor generation, and staging happen
once. The retry reuses the staged ContextGeneration and the canonical
context facts committed at the first start. It never reinvokes contributors
or stages a duplicate context batch, but it passes through the same
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

The overflow regression uses a deterministic fake adapter and a contributor
counter. Its compaction candidate would cross pending fresh inbound without
the constraint; the protected retry still contains the inbound identity. It
proves one contributor invocation, one Agent Status sample, one extension
fact in the ledger, one accepted context generation, changed revisions when
compaction occurs, and structural equality for both actual requests and their
reconstructions.

## 9. Durable RequestSnapshot ownership (M8 / Issue #11)

`AgentExecution` is the bounded active execution owner: it carries the
current ConversationState, current request/turn assembly, continuation and
tool work needed to proceed, but no complete Request Snapshot or Event
Journal history. `AgentExecutionResult` returns the settlement candidate and
current conversation state, not a historical archive.

During execution, the current request snapshot is prepared from the exact
provider-neutral request. `ConversationStore::commit_model_turn_start`
commits the request-scoped context, that immutable snapshot, and its
`ModelRequestStarted` Event Journal fact in one transaction before the
adapter is invoked, under the attempt's start gate (M9b). `RequestHistory` is a durable
read handle, not an append-only `Vec<RequestSnapshot>` and not a second
transcript. Keyed lookup reconstructs one request on demand; historical
listing uses a bounded, fallible page with an exclusive durable sequence
cursor, so runtime bootstrap and normal admission never enumerate the full
history.

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

ContextAssembly::compatibility_manifest() returns
ContextCompatibilityManifest with:

- abi_version;
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

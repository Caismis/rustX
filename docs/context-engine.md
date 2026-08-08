# Context Engine (M4)

This document describes the M4 context plane implemented in `src/context`
and its integration into the agent loop (`src/agent/execution.rs`),
mirroring the M2/M3 boundary documents.

## 1. Core invariant

```text
Canonical history is durable truth.
Context is a deterministic projection of that truth.
Compaction changes the projection, never canonical history.
Agent Status is an ephemeral projection of runtime facts, never history.
```

Canonical history is the `Vec<MessageBlock>` committed by the agent loop
(`AgentExecutionResult.messages`). The context engine never pushes, drains,
or rewrites it: `AgentExecutionResult.messages` remains initial canonical
messages plus committed agent and tool messages and drained inbound user
messages (Issue #22). No checkpoint summary, no projection-only agent
slice, and no Agent Status artifact ever appears there. Drained inbound
messages enter canonical history at a safe turn boundary before the next
projection/compaction, so the model request corresponding to a selected
inbound batch always contains that batch.

## 2. Data flow

```text
canonical history
    ↓
safe boundary: drained inbound batch appended as distinct UserMessageBlocks
    ↓
ContextEngine
    ↓
ContextProjection (+ ephemeral AgentStatusAttachment for a pending
                    FreshInboundTurn)
    ↓
projection compiler (compile_projection → CompiledContext)
    ↓
canonical ModelRequest context + agent_status attachment
    ↓
ModelAdapter (adapter-owned wire placement)
    ↓
provider
```

A drained batch is never special-cased inside the engine: it may push the
projection over the M4 soft input threshold, which is the normal proactive
compaction trigger, and canonical inbound messages remain in
`AgentExecutionResult.messages` even when older model-facing history is
summarized. The whole drained batch becomes one `FreshInboundTurn`, so the
next request carries exactly one Agent Status snapshot targeting the final
drained message.

## 3. ContextProjection

`ContextProjection` is the runtime-owned model-visible projection:

```rust
pub struct ContextProjection {
    pub items: Vec<ProjectionItem>,
    pub agent_status: Option<AgentStatusAttachment>, // projection-only
    pub estimated_input: TokenMeasurement,
    pub checkpoint_generation: Option<u64>,
}
```

`ProjectionItem` is either a whole canonical `MessageBlock` or a
projection-only `AgentSlice` (`source_message_id` + content blocks) that
only split-turn compaction creates. An `AgentSlice` is never persisted, never
emitted as `AgentMessageCommitted`, and never placed into
`AgentExecutionResult.messages`. When the projection is compiled into the
current `ModelRequest.messages` boundary, an `AgentSlice` is materialized
transiently under its original source `MessageId` as a model-context view
only; it is never authoritative ledger content. The normal whole-message
path stays zero-surprise.

`AgentStatusAttachment` is the Layer 0 cross-layer request attachment owned
by `src/model/types.rs`: the context plane composes and renders it, and the
projection carries it, but the type itself never lives in the context
layer.

The projection fingerprint covers the projection items, the checkpoint
generation, **and the exact Agent Status attachment**: a provider-reported
input measurement applies only to a byte-for-byte identical projection, so
a new status snapshot (for example a new `current_time`) invalidates the
old observation.

Item order is deterministic: pinned system prefix, checkpoint summary (when
a checkpoint exists and is not absorbed by the pinned prefix), then the
retained literal suffix. A checkpoint whose coverage lies fully inside the
current pinned system prefix is *absorbed*: its covered history is literal
again, so its summary is not injected (that would duplicate covered history
next to its summary), and a later compaction establishes a fresh
checkpoint without mutating canonical history.

## 4. System pinning

`SystemMessageBlock` is authority, not summarizable history. The M4 rule is
conservative and deterministic: the pinned prefix extends through the last
`SystemMessageBlock`. Everything in that prefix remains literal and is
outside summary coverage; the compactable region begins after it. A summary
is never used to replace a system message, and a system message is never
role-demoted to `User` or fed to the summarizer.

Limitation: when system policy is interleaved deep inside a conversation,
everything up to the last system message stays uncompressed; the engine does
not invent multi-summary semantics for interleaved system policy.

If pinned context plus tool definitions and the summary reservation alone
cannot fit the window, compaction fails explicitly
(`ContextErrorKind::CannotFit`); the engine never pretends compaction can
fix an impossible pinned budget.

## 5. Context configuration and threshold

`ContextConfig` (mirrored additively into the M1 `ContextManifest` as
`context_window_tokens`) is runtime-owned; the engine keeps no
hard-coded model catalog.

```text
soft_input_limit = context_window_tokens - reserve_tokens - max_output_tokens
```

Derived with checked arithmetic; impossible configurations are rejected
(`context_window_tokens <= reserve_tokens`, or
`<= reserve_tokens + max_output_tokens`). No fallback constant is hidden.
`max_output_tokens` is the runtime-resolved generation budget;
`reserve_tokens` is an additional safety reserve.

Automatic compaction triggers when
`estimated_input_tokens >= soft_input_limit`. Equality compacts
deterministically (tested).

## 6. Token measurement provenance

Every projected input measurement carries explicit provenance:

```rust
pub struct TokenMeasurement {
    pub input_tokens: u64,
    pub source: TokenMeasurementSource, // ProviderReported | Estimated
}
```

- **Provider-reported**: when a completed provider request reports
  `ModelUsage`, `ModelUsage.input_tokens` is the authoritative observed
  input measurement for that exact request. The observation is tied to a
  deterministic projection fingerprint: it applies only when the next
  projection is byte-for-byte identical to the measured one. Missing
  provider usage is never fabricated, estimates are never converted into
  `ModelUsage`, and cumulative `UsageUpdate` snapshots are never summed
  (the loop folds usage exactly as M3 defined: terminal `Completed.usage`,
  else latest `UsageUpdate`, else `None`).
- **Estimated**: before a request, context size uses the deterministic
  `TokenEstimator` abstraction unless an exact observed measurement for
  exactly that projection exists. The default provider-neutral fallback is
  frozen and tested:

```text
estimate = ceil(deterministic UTF-8 serialized bytes / 4)
          = (bytes + 3) / 4
```

  applied over the runtime-owned canonical serialization of the projection
  items, the tool definitions, and the exact Agent Status attachment. Tool
  definitions and Agent Status always contribute to the planned request
  estimate — the status snapshot is real model input and can itself change
  the compaction decision. Scripted estimators in tests supply exact
  weights.

The recent-conversation estimate (`estimate_conversation_input`) measures
the projection's conversation content only: tool definitions and the Agent
Status attachment never count toward satisfying `keep_recent_tokens`.

## 7. Cut-point rules

A deterministic structural index is built from canonical history: every
`AgentMessageBlock`'s `ToolCall` ids are recorded, and every
`ToolMessageBlock` resolves its `tool_call_id` to the requesting agent
message. Malformed history — a tool message with no requesting agent
message, a call issued twice, a duplicated result — is rejected
explicitly (`ContextErrorKind::MalformedHistory`), never guessed around.

A whole-message cut is valid only if no tool-call/result edge crosses it:
a retired call's results must be retired too, and a retained result's call
must be retained. Because results always follow their calls, this reduces
to: every retired agent message's turn end (its last result position, or
its own position when the call is pending) lies before the cut. Cuts
therefore fall between complete M3 turns. Candidate selection is
deterministic (tested).

## 8. Recent-token retention

`keep_recent_tokens` is a token target, never a message count target. The
target is measured over conversation content only: tool definitions and the
Agent Status attachment affect the full request estimate, the soft-limit
threshold, and the hard fit, but they never count toward satisfying
`keep_recent_tokens`.

The frozen selection priority:

1. a whole-turn boundary that satisfies the recent-token target and the
   hard fit;
2. if none exists, a hard-fitting whole-turn boundary that retains as much
   useful recent complete-turn context as possible (the most-retaining
   whole cut under the hard fit);
3. split a turn only when a single oversized turn prevents a viable
   complete-turn projection (no whole cut retains any recent context within
   the hard fit).

The latest turn is never split merely because the configured target cannot
be fully achieved. Structural correctness wins over the exact target; a
token target may retain fewer messages than a count target would.

Planning reserves room for the compaction summary using the summarizer's
configured maximum output budget (`max_output_tokens`) as a conservative
bound.

### Fresh-inbound retention constraint

A fresh inbound turn that has not yet been observed by a successfully
completed model invocation must remain literal in the projection. When the
earliest fresh inbound message is at canonical position `p`, a whole cut
must satisfy `cut <= p`: the boundary may never retire the fresh inbound
material. The split-turn planner applies the same rule (the split agent
message must lie strictly before the earliest fresh message).

The constraint is kept separate from the continuation constraint:

```text
continuation owner → successful compaction must retire through this
fresh inbound      → successful compaction must not retire this or
                     anything after it
```

If no valid projection can fit while preserving pinned context, the fresh
inbound material, the Agent Status attachment, the tool definitions, and
the required output/reserve budget, planning fails explicitly with
`ContextErrorKind::CannotFit`. The current unobserved user instruction is
never summarized merely to make the request fit.

## 9. Split-turn compaction

`AgentMessageBlock` values contain multiple `AgentContentBlock` values
(including multiple tool calls), with `ToolMessageBlock` values following
separately. When one turn dominates the budget, the engine splits it at a
complete content-block boundary:

- retired agent prefix: `content[..k]` — never inside text bytes, reasoning
  content, tool-call arguments, or tool-result content;
- retained agent slice: `content[k..]` (a projection-only `AgentSlice`);
- `ToolMessageBlock`s of retired calls go to the summary input;
- `ToolMessageBlock`s of retained calls stay literal.

The split boundary is `ContextBoundary::InsideAgent { message_id,
first_retained_block }`. A retired tool call never leaves a literal tool
message behind, and a retained tool message always keeps its call in the
retained slice. If no structurally safe split exists, a safe whole-turn cut
wins even if it violates the soft recent-token preference; it is legal to
summarize an entire latest turn and retain no literal portion of it. A
large tool-call/result pair that is retired is supplied intact to the
summarizer; reasoning blocks are never silently dropped.

## 10. Checkpoints

```rust
pub struct ContextCheckpoint {
    pub conversation_id: ConversationId,
    pub generation: u64,                    // monotonic, starts at 1
    pub summary: UserMessageBlock,          // Runtime / CompactionSummary
    pub boundary: ContextBoundary,          // AfterMessage | InsideAgent
    pub tokens_before: TokenMeasurement,    // provenance preserved
    pub estimated_tokens_after: u64,
}
```

`ContextBoundary::AfterMessage { message_id }` covers compacted non-pinned
history through that canonical message. `InsideAgent { message_id,
first_retained_block }` covers earlier history plus the retired prefix of
the split message and the tool results of its retired calls; the retained
projection is the remaining content slice, only the tool messages of
retained calls, and later canonical history. Identities are stable
`MessageId`/`ToolCallId` values, never raw vector positions.

Summary messages use deterministic namespaced identities
(`{conversation_id}-summary-{generation}`); no random ids appear in
assertions. Saving replaces/advances the latest checkpoint for the
conversation and generation must increase monotonically (the store rejects
stale generations).

M4 owns the checkpoint contract through a synchronous abstraction:

```rust
pub trait ContextCheckpointStore: Send + Sync {
    fn load(&self, conversation_id: &ConversationId) -> Result<Option<ContextCheckpoint>, ContextError>;
    fn save(&self, checkpoint: &ContextCheckpoint) -> Result<(), ContextError>;
}
```

`InMemoryCheckpointStore` is the deterministic development/test
implementation; M8 owns the durable backend.

## 11. Incremental compaction

Repeated compaction is first-class. The first compaction feeds the raw
retired prefix to the summarizer; later compactions feed the previous
checkpoint summary plus only the canonical material newly retired since the
previous checkpoint. History already covered by a prior checkpoint is never
re-fed. This holds for `AfterMessage` boundaries and for `InsideAgent`
boundaries: retiring a previously split message later feeds only the
residual slice (`SummaryInputItem::AgentSlice`), never the original prefix.

The stored previous checkpoint and the active incremental summary source
are distinct: an absorbed checkpoint (coverage fully inside the current
pinned system prefix) keeps its generation lineage, but its old summary is
never fed into the next summarization (`SummaryRequest.previous_summary` is
`None`), and the newly retired material begins strictly from the currently
compactable region after the pinned prefix.

## 12. Summary provenance

The summary service is provider-neutral and fakeable:

```rust
pub trait ContextSummarizer: Send + Sync {
    fn summarize(&self, request: SummaryRequest, cancellation: ModelCancellation)
        -> BoxFuture<'_, Result<String, ContextError>>;
}
```

`SummaryRequest` distinguishes the previous checkpoint summary, the newly
retired complete history, and any split-turn retired prefix; these
distinctions are never flattened before the API boundary.

The production `ModelBackedSummarizer` uses the canonical `ModelAdapter`
boundary with a one-off `ModelRequest`: no tools, `continuation = None`,
the execution's model/protocol/reasoning and runtime `max_output_tokens`,
and a deterministic instruction plus the serialized input. It never
recurses into `AgentExecution`, never calls provider SDKs directly, and
discards any provider continuation the one-off request emits. A refusal, a
tool request, an invalid stream, or a model failure is a compaction
failure; cancellation aborts the summary. Summary activity is represented
at the event layer only by `CompactionStarted`/`CompactionCompleted`/
`CompactionFailed` — no fake `AgentMessageCommitted` events and no
canonical-history append.

After successful compaction the generated text is wrapped as:

```rust
MessageBlock::User(UserMessageBlock {
    id: deterministic_checkpoint_summary_message_id,
    source: UserSource::Runtime,
    kind: InboundKind::CompactionSummary,
    content: vec![UserContentBlock::Text(...)],
})
```

This message belongs to the checkpoint/projection, never to canonical
history: no `history.push(summary)`, no `drain`, no in-place replacement.

## 13. Mandatory progress rule

A summary string alone never justifies a retry or a checkpoint update. A
successful compaction must satisfy both:

1. **coverage advances** — the new checkpoint retires at least one
   additional compactable canonical unit;
2. **projected estimate strictly decreases** — the deterministic estimate
   of the post-compaction projection is strictly below the deterministic
   estimate of the pre-compaction projection
   (`estimated_after < estimated_before_tokens`).

Both sides of the comparison come from the same estimator over the actual
projection content, so the decision never depends on incomparable token
provenance: a `ProviderReported` measurement is preserved separately as
checkpoint/plan metadata and never compared against an `Estimated`
after-count. If either condition fails: no checkpoint is saved,
`CompactionFailed` is emitted, and no model retry follows that compaction.
This is the central anti-loop invariant. After the real summary is produced the full projected estimate
is recomputed; if it still exceeds the soft limit, compaction fails
explicitly.

## 14. Continuation-state policy

- **No compaction**: the pending `ProviderContinuationState` is preserved
  exactly as M3 defines; nothing is inspected or transformed.
- **Successful compaction**: the pending continuation is invalidated
  (`pending_continuation = None`), and the context engine receives the
  structural constraint that the new boundary must retire the
  continuation-owning turn completely — its agent message and the complete
  tool-result portion of that turn. The continuation-owning agent message
  is never split. Opaque provider state is never placed into a checkpoint.

This prevents pairing a new summary/projection with an old opaque provider
continuation and avoids depending on adapter-specific replay behavior.

If a new `SystemMessage` pins the continuation-owning turn into the literal
prefix, no compaction can retire the owner: the constraint is
unsatisfiable, and `plan_compaction` fails explicitly (no checkpoint, no
cleared continuation) instead of clearing the continuation while leaving
its boundary literal.

## 15. Context overflow compact-and-retry

`ContextWindowExceeded` is the only retry the loop implements:

```text
ModelRequestStarted
[zero or more provisional model-derived RuntimeEvents]
ModelRequestFailed(ContextWindowExceeded)
CompactionStarted
CompactionCompleted
ModelRetryScheduled { attempt_number: 1, retry_delay_ms: None }
ModelRequestStarted
...
```

A recoverable overflow does not settle the attempt: the execution state
remains an active model-running state, `ExecutionStateMachine::fail()` is
not called, and no attempt terminal event is emitted between the overflow
and the retry. Only the model invocation that successfully completes
advances the normal model/turn state. The retry uses the smaller projection
and the cleared continuation.

The retry limit is frozen:

```rust
pub const MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN: u32 = 1;
```

No exponential backoff, rate-limit retry, timeout retry, transport retry,
or provider fallback exists. The retry budget is genuinely per model turn: every turn is entitled to
its own single `ContextWindowExceeded` retry, and the budget never persists
across turns. If the retry also overflows, the attempt settles with the
second overflow error as its final model failure; no second compaction and
no second retry occur inside any individual turn. If the first overflow is followed by a
failed/no-progress compaction, the attempt fails with the original
normalized overflow as the final model failure while
`CompactionFailed.error` carries the compaction diagnostic. A proactive
compaction failure settles as
`AttemptFailed(Runtime(ContextCompactionFailed { message }))` — a local
context service failure is never fabricated into a `ModelError`.

### Preparation failures are distinct from compaction failures

Failures that occur while preparing model context **before any compaction
starts** classify as `RuntimeError::ContextPreparationFailed`:

- invalid pending fresh-inbound state discovered during projection/status
  preparation (including a `FreshInboundTurn` that violates canonical
  ordering);
- a failing Agent Status section provider;
- a projection preparation failure that is not itself a compaction
  operation (checkpoint load, projection build, threshold derivation).

`RuntimeError::ContextCompactionFailed` is reserved for an actual proactive
compaction pipeline failure (planning, summary generation, application,
progress rule, checkpoint save). For overflow recovery the existing terminal
behavior is preserved: a failed recovery compaction keeps the normalized
`ContextWindowExceeded` as the final model failure with the compaction
diagnostic in `CompactionFailed.error`, and overflow is never turned into a
generic runtime preparation failure.

## 16. Provisional identity across retry

The failed invocation emits no committed `AgentMessageBlock`. The first
invocation keeps the M3 identity `{attempt}-agent-{turn}`; the context
retry uses the deterministic retry-specific identity
`{attempt}-agent-{turn}-retry-1`. The successfully completed invocation's
identity is the one committed; no committed event or message exists for the
failed provisional request.

## 17. Cancellation behavior

Cancellation is observed at deterministic check points with biased races:

- before compaction begins — no `CompactionStarted`, no summary, no
  checkpoint, no retry, `AttemptCancelled`;
- while summary generation is pending — allowed trace
  `CompactionStarted, AttemptCancelled`; the pending summary future is
  dropped; no `CompactionCompleted`/`CompactionFailed`/`ModelRetryScheduled`
  follows;
- after the summary returned but before the checkpoint commit — checked
  again before saving; no checkpoint, no `CompactionCompleted`, no retry;
- after a completed compaction but before the retry — the committed
  checkpoint may remain (compaction already committed successfully), but no
  model retry is issued.

Every cancellation scenario produces exactly one attempt terminal event.

**Checkpoint commit point**: the checkpoint is saved before
`CompactionCompleted` is emitted, so `CompactionCompleted` means the new
checkpoint is committed to the M4 checkpoint store. A save failure is
`CompactionFailed`.

## 18. Agent Status (mandatory ephemeral projection)

Agent Status is the mandatory, provider-neutral, ephemeral context
projection that gives every rustX agent current runtime awareness on a
fresh inbound turn. It is owned by the context plane (`src/context/status.rs`),
exists only while a `FreshInboundTurn` is pending, and is projection-only:
never canonical history, never checkpoint history, never returned in
`AgentExecutionResult.messages`, never emitted as a committed-message event.

### Explicit fresh inbound identity

Fresh inbound identity is explicit execution state (`FreshInboundTurn {
message_ids }`), never inferred from message role, history shape, or
timestamps:

- non-empty, ordered in inbound order, duplicate-free;
- every referenced message exists in canonical history, is
  `MessageBlock::User` with `InboundKind::Message`, and carries a persisted
  timestamp;
- a compaction summary (user-role history) can never be marked fresh;
- the final message is the Agent Status target;
- the referenced messages must occur in canonical history in strictly
  increasing canonical position in `message_ids` order
  (`FreshInboundError::OutOfCanonicalOrder` otherwise). The runtime never
  sorts or reinterprets a caller-supplied turn order; invalid execution
  state fails explicitly, and canonical inbound order — never a timestamp
  maximum — is authoritative for the final message.

The attempt's first-turn execution mode is an explicit trigger, never an
`Option` used as a status switch:

```rust
pub enum InitialTurnTrigger {
    FreshInbound(FreshInboundTurn),
    Continuation,
}
```

- `FreshInbound`: the model has not yet observed the referenced turn;
  validation is mandatory, Agent Status is mandatory, fresh-inbound
  compaction protection applies, and the trigger stays pending until one
  successful model invocation observes it — a provider overflow failure does
  not consume it, a successful `ToolCalls` response does.
- `Continuation`: there is intentionally no new inbound user turn for the
  first model invocation, so no Agent Status is attached; this is never a
  configuration switch for disabling status on inbound messages.

There is no `disable_status`, no optional status mode, and no legacy
no-context execution path: Agent Status can never be silently suppressed by
omitting an optional field.

Lifecycle:

```text
attempt starts
→ pending fresh inbound = request.initial_turn_trigger (FreshInbound)

model invocation successfully completes
→ pending fresh inbound is consumed (including a ToolCalls response)

safe-boundary mailbox drain returns a batch
→ append the whole batch to canonical history
→ one new FreshInboundTurn from the drained ids in sequence order

next model turn
→ one Agent Status for that FreshInboundTurn
```

A failed `ContextWindowExceeded` attempt does not consume the trigger: the
retry still represents the same fresh inbound turn. Foreground-tool-only
continuation (no new drain) carries no Agent Status.

`inbound_message_time` is the persisted timestamp of the final message in
the ordered `FreshInboundTurn` — the mailbox sequence is the delivery-order
authority, never `min(timestamp)`, `max(timestamp)`, the drain time, or
current time. Producer wall-clock timestamps may be non-monotonic; the
final message in inbound order always wins (regression-tested).

### Structured composition and rendering

```text
runtime facts
→ structured AgentStatus sections
→ canonical deterministic renderer
→ rendered AgentStatusAttachment (Layer 0 contract)
→ provider wire compiler
```

- Section ids are stable; `temporal` and `background_execution` are
  reserved built-ins. Extensions cannot register, replace, or shadow them,
  and duplicate extension ids fail explicitly.
- A provider's section identity is captured **exactly once at
  registration** and frozen as runtime-owned registration metadata:
  `section_id()` is validated against reserved and duplicate ids and never
  queried again. Composition, provider ordering, diagnostics, and
  provider listing all use the stored identity, so a stateful provider can
  never shadow a reserved id or mutate into a duplicate identity after
  registration (regression-tested with mutating fake providers).
- Deterministic order: mandatory temporal section, future built-in
  sections, then extensions in explicit registration order. `HashMap`
  iteration is never used for rendering order.
- Extension providers return **structured runtime facts**, never
  pre-rendered footer lines: a section carries ordered `AgentStatusFact`
  (`label` + `value`) pairs, and the canonical renderer is the only place
  status text is produced — it owns labels, separators, and layout. The
  structured seam is what a future M5 background runtime populates; no
  schema framework, templating language, or plugin ecosystem exists.
- An optional provider returning `None` is intentional absence; a provider
  failure is a context-preparation failure (`StatusFailed` →
  `RuntimeError::ContextPreparationFailed`), never a silent absence and
  never mislabeled as a compaction failure.
- The provider seam is narrow and read-only; it exists so a future M5
  background runtime can project its registry. No plugin ecosystem or DI
  framework exists. The `background_execution` section is reserved for M5
  and has no M4 implementation.

### Temporal section

The temporal section is mandatory whenever Agent Status is present and
contains `current_time`, the conversation `timezone` (when known), and
`inbound_message_time`. The clock goes through the narrow
`AgentStatusClock` trait (production: system UTC; tests: fixed/scripted);
no renderer or assertion calls `Utc::now()` directly. When the timezone is
known, instants render in that timezone with the RFC3339 numeric offset
plus the IANA identifier line; when unknown, instants render in UTC and the
timezone line is omitted. The process/system local timezone is never
consulted.

### Snapshot lifecycle and accounting

One request preparation composes exactly one status snapshot and reuses the
exact rendered attachment throughout that preparation's proactive
compaction planning and application. A `ContextWindowExceeded`
compact-and-retry is a new preparation and composes a fresh snapshot.

Agent Status is actual model input:

- it participates in the full request estimate (`estimate_input`), so the
  status snapshot itself can change the compaction decision;
- it is excluded from the recent-conversation estimate
  (`estimate_conversation_input`) and can never satisfy
  `keep_recent_tokens`;
- it participates in the projection fingerprint, so a different snapshot
  invalidates old `ProviderObservedInput` observations;
- compaction candidate hard-fit estimates include the exact same snapshot.

## 19. AgentExecution integration

The M4 integration point is immediately before construction of every agent
`ModelRequest`:

```text
canonical history + latest ContextCheckpoint + pending FreshInboundTurn
    ↓
compose Agent Status (one snapshot per request preparation)
    ↓
ContextEngine
    ↓
ContextProjection (+ agent_status attachment)
    ↓
projection compiler (CompiledContext)
    ↓
ModelRequest.messages + ModelRequest.agent_status
```

The M4 context path is **mandatory**: there is exactly one normal execution
model, and every `AgentExecution` is constructed with a `ContextRuntime`:

```rust
AgentExecution::new(request, adapter, tools, cancellation, context_runtime)
```

The obsolete no-context compatibility path and `with_context_runtime` are
gone; there is no Agent Status disable flag and no legacy execution mode.
`AgentExecutionRequest` carries the explicit `initial_turn_trigger`
(`InitialTurnTrigger::FreshInbound(fresh)` or
`InitialTurnTrigger::Continuation`) and the per-execution/conversation IANA
`timezone` metadata.

`ContextRuntime` owns the engine, the summary service, the Agent Status
composer, and the checkpoint store; one store can be shared across attempts
of one conversation. No hidden model-specific defaults are added to
`AgentExecution::new`.

Before each request: check cancellation, compose the status snapshot (when
a pending fresh inbound turn exists), load the latest checkpoint, build the
projection, estimate the full model input (including the status), compare
against the soft input limit; at/above the threshold, compact first, then
rebuild the projection from the persisted checkpoint, and only then issue
the request.

## 20. Provider isolation

`src/context/` contains only runtime-owned canonical types. Provider SDKs
and wire structures (async-openai, reqwest, adapter-private OpenAI/Anthropic
modules) are forbidden there — a source-level test enforces this. The
context engine decides what canonical context is visible; the adapter
decides how that canonical context is encoded on the wire. The projection
compiler emits canonical messages plus the ephemeral `agent_status`
attachment; the adapter is the only place the status footer is placed on
provider wire structures, and a checkpoint summary flows through the
existing User-message translation like any other runtime-provided inbound
message. Adapters append the rendered status as one final content unit of
the target fresh user message (Chat Completions text part, Responses
`input_text` unit, Anthropic text block) and fail explicitly when a stored
continuation would slice the target out of the transmitted tail.

The `AgentStatusAttachment` itself is a Layer 0 contract in
`src/model/types.rs`, mirroring the existing `model → message, tools,
runtime` direction: `ModelRequest` and every adapter depend only on that
runtime-owned attachment type, and `src/model` contains no `context::`
reference (source-level guard). The context plane produces the attachment
through the composer and canonical renderer; it never re-exports it as a
context-owned type.

## 21. RuntimeEvent policy

M4 reuses the existing events `CompactionStarted`, `CompactionCompleted`,
`CompactionFailed`, `ModelRequestStarted`, `ModelRequestCompleted`,
`ModelRequestFailed`, and `ModelRetryScheduled`. No debug events
(`ContextAlmostFull`, `CutPointChosen`, `SummaryGenerated`,
`ProjectionCreated`) were added: the compaction events are the canonical
execution facts, and attempt terminal events are unchanged. Agent Status
emits no event of its own: it is projection-only.

## 22. Known limitations

- The pinned prefix extends through the last `SystemMessageBlock`; deeply
  interleaved system policy is not partially compacted.
- The estimator is a provider-neutral byte-based fallback; it is an
  estimate, never provider usage.
- Checkpoint storage is an abstraction plus an in-memory development/test
  implementation; durable storage and crash recovery are M8.
- Summary generation is a single one-off model request with no retry: a
  summary failure is a compaction failure.
- Only `ContextWindowExceeded` is retried, exactly once, after a compaction
  that made measurable progress.
- The `background_execution` Agent Status section is reserved for the M5
  tool plane and has no M4 implementation; Agent Status is otherwise
  complete (temporal section, provider seam, deterministic rendering).

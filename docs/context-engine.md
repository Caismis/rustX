# Context Engine (M7.5)

This document describes the conversation domain (`src/conversation`), the
context plane implemented in `src/context`, and their integration into the
agent loop (`src/agent/execution.rs`).

> **M7.5 (Issue #54) supersedes the M4 projection-only compaction model.**
> The M4 design — a `Vec<MessageBlock>` canonical history, a
> checkpoint-owned summary, a projection-only `AgentSlice`, split-turn
> compaction, checkpoint absorption, and a last-`System` pinned prefix — is
> gone. A compaction summary is now a genuine canonical conversational
> fact, and compaction rewrites the Conversation Surface, never the Message
> Ledger.

## 1. Core invariant

```text
Message Ledger        immutable committed conversational facts
        ↓
Conversation Surface  the sole authority for current active model-visible
                      identity/order/visibility, at a stable SurfaceRevision
        ↓
Context Engine        token pressure, retention, compaction planning, and a
                      finite provider-neutral projection
        ↓
compaction = commit one canonical User(Runtime / CompactionSummary)
           + one complete-message Surface Replace
```

Concretely:

- **Ledger facts are immutable.** Once committed, a message body is never
  edited, replaced, or deleted, and its `MessageId` stays addressable
  forever. Compaction *appends*; it never rewrites an earlier record.
- **The Surface alone owns visibility.** The Ledger carries no `active`,
  `visible`, or `shadowed` flag; the Surface holds identities only, never
  bodies.
- **The compaction summary is canonical.** It is an ordinary
  `UserMessageBlock` with `UserSource::Runtime` and
  `InboundKind::CompactionSummary`, committed to the Ledger like any other
  fact and externally visible in the Runtime Client read model. It is never
  elevated to `System`, and no second copy of it lives anywhere else.
- **Agent Status and the Skill catalog remain ephemeral projections** of
  runtime/capability facts, never conversation facts.

## 2. Data flow

```text
ConversationState (MessageLedger + ConversationSurface)
    ↓
safe boundary: drained inbound batch committed as distinct UserMessageBlocks
    ↓
Surface @ revision → finite active MessageIds → keyed Ledger hydration
    ↓
ContextEngine
    ↓
ContextProjection (complete canonical messages
                   + ephemeral AgentStatusAttachment for a pending
                     FreshInboundTurn
                   + ephemeral SkillCatalogAttachment)
    ↓
canonical ModelRequest context + agent_status + skill_catalog
    ↓
ModelAdapter (adapter-owned wire placement)
    ↓
provider
```

A drained batch is never special-cased inside the engine: it may push the
projection over the soft input threshold, which is the normal proactive
compaction trigger, and the original inbound messages remain committed
Ledger facts even after the Surface retires them. The whole drained batch
becomes one `FreshInboundTurn`, so the next request carries exactly one
Agent Status snapshot targeting the final drained message.

### Finite read boundary

The read direction is fixed and finite:

```text
Surface @ current revision
  → finite active MessageIds
  → select a structurally valid compactable span
  → fetch only the required message bodies (keyed Ledger lookups)
  → summarize
```

Normal projection and compaction never iterate the Ledger and never hydrate
retired history, so their cost is a function of the active Surface alone.
`MessageLedger` instruments this deterministically: `LedgerAccess`
counts keyed reads and full enumerations separately, only the explicit
`audit_records()` path increments the enumeration counter, and a regression
proves that a projection + plan + prepare cycle performs **zero**
enumerations and the same number of keyed reads over 20 and over 2,000
retired messages. Retired Ledger records stay auditable and addressable;
they simply do not participate in later compaction.

## 3. ConversationState

```rust
pub struct ConversationState {
    ledger: MessageLedger,
    surface: ConversationSurface,
}
```

`ConversationState` is the single mutable conversation authority. Its
ordinary commit path is one function:

```rust
state.commit(block)?     // one Ledger append + one Surface Append
```

Independent `ledger.push()` / `surface.push()` call sites do not exist
anywhere in the runtime.

### MessageLedger

Append-only, commit-ordered records plus a `MessageId` → position index.
Duplicate `MessageId` commits are rejected. There is no database, no paging
framework, no repository abstraction, and no storage strategy trait — M8
owns durability.

### ConversationSurface

The Surface holds the current active ordered `MessageId`s plus the ordered
log of accepted operations. Its mutation vocabulary is deliberately
minimal — there is no insert, move, delete, reorder, or patch:

```rust
pub enum SurfaceOp {
    Append  { message_id: MessageId },
    Replace { start: MessageId, end: MessageId, replacement: MessageId },
}
```

`Replace` spans are **inclusive on both ends** (`[start ..= end]`;
`start == end` replaces exactly one message), and the replacement takes the
position `start` occupied. The convention is frozen and tested.

### SurfaceRevision

```rust
pub struct SurfaceRevision(u64);   // INITIAL = 0
```

`SurfaceRevision` is its own identity domain, distinct from `MessageId`,
`AttemptId`, `RuntimeClientCursor`, `InboundSequence`, the Event Journal
sequence, and `CapabilityRevision`. The empty Surface of a new conversation
is `SurfaceRevision::INITIAL`, and every accepted operation advances it by
exactly one, so revision `n` is precisely "the Surface after the first `n`
accepted operations".

Given a historical revision, `ConversationState::reconstruct(revision)`
deterministically returns the exact active ordered `MessageId`s of that
revision by replaying the retained operation log. Reconstruction never
touches the Ledger, and later mutations never change an earlier
reconstruction. Only after identities are known may a caller resolve bodies
with `hydrate`.

`SurfaceRevision` is also the seam Issue #55's `RequestSnapshot` consumes: a
request's visible-conversation identity is `ConversationSurface @ revision`,
never "whatever messages happened to exist around that time".

### The compaction generation

The compaction generation is *derived*: it is the number of accepted
`Replace` operations in Surface history. There is no separate store that
could drift from it. Summary messages use deterministic namespaced
identities (`{conversation_id}-summary-{generation}`); no random ids appear
in assertions.

### Rejected mutations

Every Surface mutation is validated against the **current** revision before
anything changes, so a rejected mutation leaves the state exactly as it was.
Rejected: an unknown `start` or `end`, a reversed span, an endpoint that is
no longer active (a retired span can never be replaced again), a
replacement identity that is already active or already committed, a span
containing a trusted `System` message, a span that would separate a tool
call from its result, and a command prepared against a stale revision
(`SurfaceError::StaleRevision`).

## 3.5 ContextProjection

`ContextProjection` is the finite request-preparation value derived from one
exact Surface revision:

```rust
pub struct ContextProjection {
    pub surface_revision: SurfaceRevision,
    pub messages: Vec<MessageBlock>,                 // complete canonical only
    pub agent_status: Option<AgentStatusAttachment>, // projection-only
    pub skill_catalog: Option<SkillCatalogAttachment>, // projection-only
    pub estimated_input: TokenMeasurement,
}
```

Every projected item is a **complete canonical message**. There is no
projection-only partial agent message: `ProjectionItem`, its `AgentSlice`
variant, `CompiledContext`, and `compile_projection` are all gone, because
once every item is a whole canonical message the projection's messages *are*
the request messages.

`AgentStatusAttachment` and `SkillCatalogAttachment` are Layer 0 cross-layer
request attachments owned by `src/model/types.rs`: the context plane
composes and renders them and the projection carries them, but the types
themselves never live in the context layer.

The projection fingerprint covers the Surface revision, the hydrated active
messages, the exact Agent Status attachment, and the exact Skill catalog
attachment. A provider-reported input measurement therefore applies only to
a byte-for-byte identical request context: a Surface rewrite, an append, a
new status snapshot, or a changed catalog all invalidate a stale
observation.

## 4. System semantics (interim rule)

`SystemMessageBlock` is authority, not summarizable history. The narrow
interim rule of Issue #54 is:

> A Surface `Replace` span must never contain a `System` message.

Compaction planning therefore selects its span from the **earliest
contiguous run of non-`System` active messages**. Trusted system content is
never demoted into a runtime `User` summary and is never fed to the
summarizer.

The removed M4 coupling is important: a later `System` message no longer
pins every older conversational message, and it never resurrects a
previously retired Surface span. Checkpoint "absorption" — where covered
history became literal again once the pinned prefix advanced — is gone
entirely, because Surface visibility is now authoritative. A later `System`
message only bounds the *current* compactable run at its own position; the
run before it stays compactable, and retired history stays retired. A
deterministic regression asserts exactly this.

The complete Effective System Prompt architecture belongs to Issue #55; this
rule is the narrowest coherent interim contract.

Limitation: when system policy is interleaved deep inside a conversation,
the compactable run stops at the next `System` message; the engine does not
invent multi-summary semantics for interleaved system policy.

If retained context plus tool definitions and the summary reservation alone
cannot fit the window, compaction fails explicitly
(`ContextErrorKind::CannotFit`); the engine never pretends compaction can
fix an impossible budget.

## 5. Context configuration and threshold

Configuration is split by ownership (Issue #42):

```text
SessionContextPolicy   session-owned, static
    reserve_tokens, keep_recent_tokens, summary_output_cap
                +
attempt model snapshot  the selected catalog model's context window and
                        output budget
                =
ContextConfig           derived per attempt
```

The context *window* belongs to the model, not to the process, so a session
model change between attempts changes the next attempt's compaction
arithmetic and never the running one's. An attempt on a 32k model never plans
compaction with a previously selected 128k window, and no window captured at
process start survives a model change.

`ContextConfig` is runtime-owned; the engine keeps no hard-coded model
catalog.

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

`TokenMeasurement` and `TokenMeasurementSource` are Layer 0 runtime-owned
value contracts in `src/runtime/types.rs`. The Context Engine still owns the
behavior around them: `ProviderObservedInput`, deterministic estimators,
projection fingerprint matching, provenance application, and compaction
threshold accounting remain in `src/context/tokens.rs` and the context plane.

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

## 7. Structural rules

A deterministic structural index is built over the **active Surface
messages** (never over the complete Ledger): every `AgentMessageBlock`'s
`ToolCall` ids are recorded, and every `ToolMessageBlock` resolves its
`tool_call_id` to the requesting agent message. Malformed structure — a tool
result with no active owning agent message, a call issued twice, a
duplicated result — is rejected explicitly
(`ContextErrorKind::MalformedHistory`), never guessed around.

The ownership contracts are frozen:

```text
Agent/Assistant owns ToolCall identity and arguments
Tool            owns the execution result and references ToolCallId
```

A span `[start ..= end]` is replaceable only when no tool-call/result
relationship crosses either boundary: for every committed call/result pair,
either both are inside the span or both are outside. A retained tool result
can therefore never lose its active owning tool call, and a retained call
never loses its result. A pending call with no committed result imposes no
edge and remains representable, as the Agent Loop contract requires. Raw
message counts are never used as a structural boundary heuristic, and
candidate selection is deterministic (tested).

## 8. Recent-token retention

`keep_recent_tokens` is a token target, never a message count target. The
target is measured over conversation content only: tool definitions and the
attachments affect the full request estimate, the soft-limit threshold, and
the hard fit, but they never count toward satisfying `keep_recent_tokens`.

Candidate spans are the inclusive prefixes of the compactable run
(§4). Every candidate must contain complete canonical messages only, must
not split a tool pair, must satisfy the continuation and fresh-inbound
constraints, and must fit the summary model's input limit. The frozen
selection priority is:

1. the largest candidate whose retained recent conversation content still
   meets `keep_recent_tokens`, when the resulting request fits the hard
   limit;
2. otherwise the most-retaining candidate that fits the hard limit.

A message is never split merely because the configured target cannot be
fully achieved: there is no third "split a turn" priority any more.
Structural correctness wins over the exact target, and a token target may
retain fewer messages than a count target would.

Planning reserves room for the compaction summary using the summary
invocation's effective `max_output_tokens` as a conservative bound.

### Summary-model input bound

The selected span is additionally bounded by the summary model's own
request budget, so an arbitrarily large span can never be serialized into an
impossible summary-model request:

```text
summary_input_limit = summary context window - summary max_output_tokens
```

The summary request is a bounded one-off — no tools, no Agent Status, no
Skill catalog, no continuation — so its bound is the summary model's own
window minus its output budget; the session's conversational safety reserve
belongs to the primary loop, not to this single request. A candidate whose
span estimate exceeds the limit is not planned. If no candidate remains,
planning fails explicitly with `CannotFit`.

### Fresh-inbound retention constraint

A fresh inbound turn that has not yet been observed by a successfully
completed model invocation must remain active. When the earliest fresh
inbound message is at active position `p`, a span must end strictly before
it. The constraint is kept separate from the continuation constraint:

```text
continuation owner → successful compaction must retire through this
fresh inbound      → successful compaction must not retire this or
                     anything after it
```

If no valid span can fit while preserving retained context, the fresh
inbound material, the attachments, the tool definitions, and the required
output/reserve budget, planning fails explicitly with
`ContextErrorKind::CannotFit`. The current unobserved user instruction is
never summarized merely to make the request fit.

## 9. Complete-message compaction

Compaction operates on complete canonical messages only. Split-turn
compaction is gone: `ContextBoundary::InsideAgent`,
`ProjectionItem::AgentSlice`, `SummaryInputItem::AgentSlice`,
`SplitTurnSummaryInput`, `SummaryRequest.split_turn_prefix`, split candidate
selection, and partial agent compilation no longer exist.

A single oversized message therefore never produces a half-message Surface
node. If no valid complete-message span leaves a fitting request, planning
returns the explicit `CannotFit` (or, when the constraints leave nothing
retirable at all, `NoProgress`). There is no compatibility mode for
oversized messages.

A large tool-call/result pair that is retired is supplied intact to the
summarizer; reasoning blocks are never silently dropped.

## 10. The compaction semantic commit

Compaction has exactly one semantic commit / linearization point:

```rust
state.commit_compaction(commit)?   // ConversationState
```

The command it applies is prepared, not executed, by the engine:

```rust
pub struct CompactionCommit {
    pub summary: UserMessageBlock,        // Runtime / CompactionSummary
    pub span: SurfaceSpan,                // inclusive [start ..= end]
    pub expected_revision: SurfaceRevision,
}
```

`ContextEngine::prepare_compaction` builds it after planning and
summarization, enforces the progress rule, and returns the projection the
commit will establish — while mutating nothing. `commit_compaction` then
re-validates everything against the current revision (rejecting a stale
command outright) and performs the Ledger append and the Surface `Replace`
together.

```text
before the commit point          after the commit point
-----------------------          ----------------------
old Ledger                       the summary is a committed Ledger fact
old Surface                      a new Surface revision exists in which the
old continuation semantics       summary replaces the selected active span
                                 every covered Ledger fact remains intact
                                 continuation incompatibility is known
```

Because validation completes before any mutation, a rejected or cancelled
compaction leaves neither a half-committed summary nor a half-applied
Surface rewrite. This is an in-process domain commit; there is no
distributed transaction machinery.

The applied commit yields derived metadata only:

```rust
pub struct CompactionRecord {
    pub summary_message_id: MessageId,     // identity, never a second copy
    pub replaced: SurfaceSpan,
    pub surface_revision: SurfaceRevision,
    pub generation: u64,                   // derived from Surface history
}
```

`ContextCheckpoint`, `ContextBoundary`, `ContextCheckpointStore`, and
`InMemoryCheckpointStore` are **removed**. Everything the checkpoint carried
is now either a canonical Ledger fact (the summary) or derivable from
committed conversation state (the span, the revision, the generation), so
there is no second store that could become a competing active-projection
authority and no place for a duplicate summary truth to live.

## 11. Repeated compaction

Repeated compaction is first-class and always operates from the **current
Surface**:

```text
Ledger:   A B C D                 Surface: A B C D
compact:  A B C D S1              Surface: S1 D
grow:     A B C D S1 E F          Surface: S1 D E F
compact:  A B C D S1 E F S2       Surface: S2 F
```

The second compaction selects its span from the active Surface, so it never
rediscovers `A B C` merely because they remain committed in the Ledger. A
still-active previous summary such as `S1` is simply one canonical
`User(Runtime / CompactionSummary)` message inside the selected span; there
is no hidden "previous checkpoint summary" channel beside it.

## 12. Summary provenance

The summary service is provider-neutral. Production composition always goes
through `ContextRuntime::for_attempt`, which derives the concrete
`ModelBackedSummarizer` and both compaction budgets from the immutable attempt
model snapshot. Deterministic tests may use a hidden fixture-only summary
seam; it is not a production configuration mode.

```rust
pub trait ContextSummarizer: Send + Sync {
    fn summarize(&self, request: SummaryRequest, cancellation: CancellationSignal)
        -> BoxFuture<'_, Result<String, ContextError>>;
}
```

```rust
pub struct SummaryRequest {
    /// The selected span of complete canonical active messages, in Surface
    /// order.
    pub retired: Vec<MessageBlock>,
}
```

The request carries exactly the complete canonical messages of the selected
Surface span. Nothing is flattened, nothing is partial, and there is no
hidden previous-summary channel: a still-active previous compaction summary
appears here as an ordinary `User(Runtime / CompactionSummary)` message.

The production `ModelBackedSummarizer` uses the canonical `ModelAdapter`
boundary with a one-off `ModelRequest`: no tools, no Agent Status, no Skill
catalog, `continuation = None`, and a deterministic instruction plus the
serialized input. It is constructed from the attempt's **frozen summary
policy**, never from an independently injected summarizer — production has
exactly two modes:

Compaction planning carries a named pair of budgets: the primary effective
output budget controls the primary soft input limit, while the effective
output budget of the frozen (and possibly capped) summary invocation is the
summary reservation recorded in `CompactionPlan.summary_reservation`.
`plan_compaction` takes them as an explicit `CompactionBudgets` value —
`primary_output_budget`, `summary_output_budget`, and `summary_input_limit`
— and there is no conversion from a single number, so every call site names
each budget explicitly. They remain distinct concepts even when their
current numeric values coincide.

- `session` — the summary uses the attempt's own primary invocation: same
  provider binding, model, protocol, selected reasoning profile, and
  effective request parameters;
- `explicit` — a separately resolved catalog model, resolved through the same
  catalog, credential binding, compat handling, reasoning-profile validation,
  protected-key validation, and shallow overlay as a primary model, and
  frozen at admission so a later mutation of live session state cannot change
  an already-admitted attempt's summary model.

The context plane's `summary_output_cap` is applied through the runtime-owned
protected max-output field of that invocation; it never mutates a reasoning
profile or a request-parameter object.

It never recurses into `AgentExecution`, never calls provider SDKs directly,
and discards any provider continuation the one-off request emits. A refusal, a
tool request, an invalid stream, or a model failure is a compaction
failure; cancellation aborts the summary. Summary activity is represented
at the event layer only by `CompactionStarted`/`CompactionCompleted`/
`CompactionFailed` — no fake `AgentMessageCommitted` events and no
canonical-history append.

After successful compaction the generated text is wrapped as:

```rust
MessageBlock::User(UserMessageBlock {
    id: summary_message_id(conversation_id, generation),
    source: UserSource::Runtime,
    kind: InboundKind::CompactionSummary,
    content: vec![UserContentBlock::Text(...)],
})
```

This message is a **canonical conversational fact**: it is committed to the
Message Ledger, observed through the ordinary commit seam, and visible in
the Runtime Client read model like any other committed message. It is never
elevated to `System`, and no second copy of it exists anywhere. Compaction
appends it and rewrites the Surface; it never mutates or replaces an earlier
Ledger record.

## 13. Mandatory progress rule

A summary string alone never justifies a retry or a Surface rewrite. A
successful compaction must satisfy all of:

1. **coverage advances** — the plan retires at least one complete canonical
   message;
2. **the summary has content** — an empty or whitespace-only summary is
   rejected at this boundary, so no summarizer (including a custom or
   scripted one) can erase conversation through an empty summary;
3. **the projected estimate strictly decreases** — the deterministic
   estimate of the post-compaction request context is strictly below the
   deterministic estimate of the pre-compaction one
   (`estimated_after < estimated_before_tokens`).

Both sides of the comparison come from the same estimator over the actual
request context — including the plan's exact Agent Status and Skill catalog
attachments — so the decision never depends on incomparable token
provenance: a `ProviderReported` measurement is preserved separately as plan
metadata and never compared against an `Estimated` after-count. If any
condition fails: nothing is committed, `CompactionFailed` is emitted, and no
model retry follows that compaction. This is the central anti-loop
invariant. After the real summary is produced the full projected estimate is
recomputed; if it still exceeds the soft limit, compaction fails explicitly
before anything is committed.

## 14. Continuation-state policy

- **No compaction**: the pending `ProviderContinuationState` is preserved
  exactly as M3 defines; nothing is inspected or transformed.
- **Successful compaction**: a successful incompatible Surface rewrite
  establishes a new visible-conversation boundary, so the pending
  continuation is invalidated **exactly once**, immediately after the
  semantic commit, from a single ownership path in `perform_compaction`.
  No caller clears it a second time (a source-level regression enforces the
  single assignment site). The context engine receives the structural
  constraint that the span must retire the continuation-owning turn
  completely — its agent message and the complete tool-result portion of
  that turn — and the continuation-owning agent message is never split.
  Opaque provider state is never inspected, transformed, or stored in
  compaction metadata.
- **Failed or cancelled compaction**: the continuation is *not* cleared.
  Beginning summary generation is never enough; only a committed Surface
  rewrite invalidates it.

This prevents pairing a new Surface with an old opaque provider
continuation and avoids depending on adapter-specific replay behavior.

If the continuation-owning turn lies outside the current compactable run —
for example behind an intervening `System` message — no compaction can
retire it: the constraint is unsatisfiable, and `plan_compaction` fails
explicitly (nothing committed, continuation untouched) instead of clearing
the continuation while leaving its message active.

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
  operation (Surface hydration, projection build, threshold derivation).

`RuntimeError::ContextCompactionFailed` is reserved for an actual proactive
compaction pipeline failure (planning, summary generation, the progress
rule, the fit check, the semantic commit). For overflow recovery the existing terminal
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
  canonical summary commit, no Surface rewrite, no retry,
  `AttemptCancelled`;
- while summary generation is pending — allowed trace
  `CompactionStarted, AttemptCancelled`; the pending summary future is
  dropped; no `CompactionCompleted`/`CompactionFailed`/`ModelRetryScheduled`
  follows;
- after the summary returned but before the semantic commit — checked again
  immediately before `commit_compaction`; nothing is appended, nothing is
  rewritten, the old state stays authoritative, no `CompactionCompleted`,
  no retry;
- after a completed compaction but before the retry — the committed summary
  and the new Surface revision remain (compaction already committed
  successfully), but no model retry is issued.

Every cancellation scenario produces exactly one attempt terminal event, and
none can leave a half-committed summary or a half-applied Surface rewrite.

**Semantic commit point**: `ConversationState::commit_compaction` runs
before `CompactionCompleted` is emitted, so the event means the canonical
summary is a Ledger fact and the new Surface revision exists. A rejected
commit is `CompactionFailed`.

Committed compaction has exactly one execution-fact path:

```text
state.commit_compaction(commit)
    -> AgentExecutionObserver::observe_committed(summary)   // canonical fact
    -> RuntimeEvent::CompactionCompleted { generation, summary_message_id,
       surface_revision, tokens_before, estimated_tokens_after }
    -> AgentExecutionObserver::observe_event
    -> RuntimeClientProjection::fold_event
    -> RuntimeClientEvent::ContextCompacted
```

No client-visible `ContextCompacted` can imply success before the summary
Ledger record and the new Surface revision are committed: the commit
strictly precedes both the canonical event and its downstream projection.
The Runtime Client event is a downstream projection of the canonical runtime
event. There is no separate compaction-completion observer callback, and the
attempt terminal event remains later and last.

## 18. Agent Status (mandatory ephemeral projection)

Agent Status is the mandatory, provider-neutral, ephemeral context
projection that gives every rustX agent current runtime awareness on a
fresh inbound turn. It is owned by the context plane (`src/context/status.rs`),
exists only while a `FreshInboundTurn` is pending, and is projection-only:
never a Message Ledger fact, never on the Conversation Surface, never
emitted as a committed-message event.

### Explicit fresh inbound identity

Fresh inbound identity is explicit execution state (`FreshInboundTurn {
message_ids }`), never inferred from message role, history shape, or
timestamps:

- non-empty, ordered in inbound order, duplicate-free;
- every referenced message is active on the Conversation Surface, is
  `MessageBlock::User` with `InboundKind::Message`, and carries a persisted
  timestamp;
- a compaction summary (user-role history) can never be marked fresh;
- the final message is the Agent Status target;
- the referenced messages must occur on the active Surface in strictly
  increasing position in `message_ids` order
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
→ commit the whole batch through the one canonical commit path
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
→ extension providers: structured AgentStatusFact values only
→ composer: converts extension facts into composed sections,
  and is the only constructor of built-in section variants (Temporal)
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
- Deterministic order: mandatory temporal section, the runtime-owned
  `background_execution` built-in section when active executions exist,
  then extensions in explicit registration order. `HashMap` iteration is
  never used for rendering order.
- Extension providers return **structured runtime facts only**, never
  pre-rendered footer lines and never the internal composed section
  representation: the provider contract's result type is an ordered list of
  `AgentStatusFact` (`label` + `value`) pairs, so a provider is
  **structurally incapable** of constructing the runtime-owned `Temporal`
  variant or the `BackgroundExecution` built-in variant. Built-in section
  variants are runtime-owned and can only be constructed by the
  composer/built-in composition code, which converts extension facts into
  the internal `Facts` section form. The canonical renderer is the only
  place status text is produced — it owns labels, separators, and layout.
  No schema framework, templating language, or plugin ecosystem exists.
- An optional provider returning `None` is intentional absence; a provider
  failure is a context-preparation failure (`StatusFailed` →
  `RuntimeError::ContextPreparationFailed`), never a silent absence and
  never mislabeled as a compaction failure.
- The provider seam is narrow and read-only; the M5 background runtime is
  **not** an ordinary provider. The executing attempt samples the
  authoritative `ConversationBackgroundRegistry` into a read-only active
  snapshot carried by the render context, and the composer builds the
  runtime-owned `BackgroundExecution` section itself (active entries only,
  in execution-allocation order). `ContextRuntime`/the composer never own
  or mutate the background registry. No plugin ecosystem or DI framework
  exists.

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
- it participates in the request-context fingerprint, so a different
  snapshot invalidates old `ProviderObservedInput` observations;
- compaction candidate hard-fit estimates include the exact same snapshot.

## 18.5 Skill catalog (M6 mandatory capability projection)

The Skill catalog follows the Agent Status attachment architecture exactly:

```text
attempt capability snapshot (immutable for the attempt)
    ↓
SkillCatalogAttachment { rendered }   (Layer 0, src/model/types.rs)
    ↓
ContextEngine::build_projection(..., skill_catalog)
    ↓
ContextProjection.skill_catalog
    ↓
ModelRequest.skill_catalog
    ↓
provider adapter → trusted system context
```

Semantics:

- the attachment is produced once per attempt from the pinned immutable
  Skill snapshot and is identical on every turn of the attempt; the same
  exact attachment is used on both sides of every compaction progress
  comparison (`CompactionPlan.skill_catalog`);
- it participates in the deterministic request-context fingerprint, the full
  request estimate (`DefaultTokenEstimator::serialized_bytes`), the soft
  compaction threshold, the hard-fit calculation, and `CannotFit` — a
  large catalog can make an otherwise compactable projection fail;
- it is excluded from the recent-conversation estimate
  (`estimate_conversation_input`) and can never satisfy
  `keep_recent_tokens`;
- it is projection-only capability context: never a Message Ledger fact,
  never on the Conversation Surface, and never emitted as a
  committed-message event; `SystemAuthority::Skill` is not used for it;
- when no Skill is active the attachment is absent entirely.

## 19. AgentExecution integration and conversation ownership

The integration point is immediately before construction of every agent
`ModelRequest`:

```text
ConversationState (owned by the attempt) + pending FreshInboundTurn
    ↓
compose Agent Status (one snapshot per request preparation)
    ↓
ContextEngine (Surface @ revision → keyed hydration → finite projection)
    ↓
ContextProjection (+ agent_status attachment, + skill_catalog attachment)
    ↓
ModelRequest.messages + ModelRequest.agent_status + ModelRequest.skill_catalog
```

The context path is **mandatory**: there is exactly one normal execution
model, and every `AgentExecution` is constructed with a `ContextRuntime` and
an attempt capability lease. There is no Agent Status disable flag and no
legacy no-context execution mode. `AgentExecutionRequest` carries the
explicit `initial_turn_trigger` (`InitialTurnTrigger::FreshInbound(fresh)`
or `InitialTurnTrigger::Continuation`) and the per-execution/conversation
IANA `timezone` metadata.

### One mutable conversation authority at a time

Ownership of the one `ConversationState` transfers structurally, not by
convention:

```text
idle           RuntimeClientHost owns the ConversationState
admission      the state is MOVED into the AgentExecution
running        the host holds None — it physically cannot mutate a copy
settlement     the state is MOVED back through AgentExecutionResult
```

`AgentExecutionRequest.conversation` and
`AgentExecutionResult.conversation` are moves, not clones: the M4
`initial_messages` / `messages` clone-based APIs are gone, so two
independently mutable conversation copies are not representable. While an
attempt runs, client submissions stay mailbox-owned; the loop commits them
at its own safe boundary. The Runtime Client remains a pure read model over
this one authority.

(This is the bounded #54 ownership model. The complete `ConversationRuntime`
extraction belongs to Issue #61.)

### The one canonical commit path

Every canonical commit of the loop — drained inbound user messages,
committed agent messages, committed tool messages, and the runtime
compaction summary — goes through `commit_canonical`, which performs one
`ConversationState::commit` (or, for compaction, one
`commit_compaction`) and then fires the commit observation at that same
linearization point.

`ContextRuntime` owns the engine, the summary service, and the Agent Status
composer. There is deliberately no checkpoint store: compaction lineage is
derived from Conversation Surface history, so no second store can drift from
the authoritative state. No hidden model-specific defaults are added to
`AgentExecution::new`.

Before each request: check cancellation, compose the status snapshot (when a
pending fresh inbound turn exists), build the projection from the current
Surface, estimate the full model input (including the attachments), compare
against the soft input limit; at/above the threshold, compact first — the
canonical summary commit and the Surface rewrite are already applied before
`CompactionCompleted` is emitted — then rebuild the projection from the new
Surface revision, and only then issue the request.

## 20. Provider isolation

`src/context/` contains only runtime-owned canonical types. Provider SDKs
and wire structures (async-openai, reqwest, adapter-private OpenAI/Anthropic
modules) are forbidden there — a source-level test enforces this. The
context engine decides what canonical context is visible; the adapter
decides how that canonical context is encoded on the wire. The projection
compiler emits canonical messages plus the ephemeral `agent_status`
attachment; the adapter is the only place the status footer is placed on
provider wire structures, and a runtime compaction summary flows through
the existing User-message translation like any other runtime-provided
inbound message. Adapters append the rendered status as one final content unit of
the target fresh user message (Chat Completions text part, Responses
`input_text` unit, Anthropic text block) and fail explicitly when a stored
continuation would slice the target out of the transmitted tail.

The Skill catalog is placed by adapters in **trusted system context**:
OpenAI Responses combines it deterministically with the canonical system
instructions in the top-level `instructions` channel on every request
(including stored continuations, whose sliced canonical history never
carries the catalog), Anthropic Messages pushes it into the top-level
`system` content after the canonical system blocks, and OpenAI Chat
Completions emits one deterministic system message in the trusted system
prefix after the canonical system messages and before the conversational
transcript. It is never attached to a user message. The catalog is
carried by the Layer 0 `SkillCatalogAttachment` (`src/model/types.rs`);
`src/model` never references context or capability modules.

The `AgentStatusAttachment` itself is a Layer 0 contract in
`src/model/types.rs`, mirroring the existing `model → message, tools,
runtime` direction: `ModelRequest` and every adapter depend only on that
runtime-owned attachment type, and `src/model` contains no `context::`
reference (source-level guard). The context plane produces the attachment
through the composer and canonical renderer; it never re-exports it as a
context-owned type.

## 21. RuntimeEvent policy

The context plane reuses the existing events `CompactionStarted`,
metadata-bearing `CompactionCompleted`, `CompactionFailed`,
`ModelRequestStarted`, `ModelRequestCompleted`, `ModelRequestFailed`, and
`ModelRetryScheduled`. No debug events (`ContextAlmostFull`,
`CutPointChosen`, `SummaryGenerated`, `ProjectionCreated`) were added: the
compaction events are the canonical execution facts, and attempt terminal
events are unchanged.

`CompactionCompleted` carries derived metadata only — `generation`,
`summary_message_id`, `surface_revision`, `tokens_before`, and
`estimated_tokens_after` — and references the summary by identity. It never
carries summary text: the summary's *content* travels the ordinary
committed-message path, because it is an ordinary canonical Ledger fact. The
Runtime Client projection folds the event into the snapshot and publishes
`context_compacted` with the same derived metadata. Agent Status emits no
event of its own: it is projection-only.

## 22. Known limitations

- The interim System rule bounds the compactable run at the next `System`
  message, so deeply interleaved system policy is compacted one run at a
  time; the full Effective System Prompt architecture is Issue #55.
- The estimator is a provider-neutral byte-based fallback; it is an
  estimate, never provider usage.
- The Message Ledger and the Conversation Surface are in-memory pre-M8
  structures; durable storage and crash recovery are M8.
- A selected span must fit the summary model's own request budget. When a
  single conversation is so large that no complete-message span both
  reduces the request and fits the summary model, compaction fails
  explicitly with `CannotFit` rather than splitting a message.
- Summary generation is a single one-off model request with no retry: a
  summary failure is a compaction failure.
- Only `ContextWindowExceeded` is retried, exactly once, after a compaction
  that made measurable progress.
- The `background_execution` Agent Status section is implemented by the M5
  tool plane as a runtime-owned built-in (read-only active registry
  snapshot, active entries only, deterministic allocation order); Agent
  Status is otherwise complete (temporal section, provider seam,
  deterministic rendering).

## 23. Issue #27 live validation

The live validation uses the same production path as an operator:

```text
rustx-tui
  -> Runtime Client Protocol v1 over stdio/JSONL
  -> local rustx conversation runtime
  -> ContextRuntime / Agent Loop / provider adapter
  -> local vLLM
```

Use `cargo build --bin rustx`, then start the TUI with explicit paths to the
model catalog, session configuration, workspace, and private runtime root.
For a reasonably sized development run, copy the local catalog/session to an
untracked temporary directory and adjust only context/output budgets; never
commit credentials or throwaway configuration. The ordinary request path must
cross the threshold twice in one coherent conversation. After each committed
compaction, `/status` and `/debug` report the compaction count, the latest
generation and Surface revision, the pre-compaction measurement source, and
the deterministic post-compaction estimate.

The repeated-compaction invariant is:

```text
Surface @ revision R (generation N)
    -> select a structurally valid span of the current Surface
    -> commit one canonical summary + one Surface Replace
    -> Surface @ revision R+1 (generation N+1)
```

Tool call/result ownership is unchanged, and every retired original remains
a committed, addressable Message Ledger fact; only the active Surface
changes. Provider-reported input usage is shown as provider-reported
only when its projection fingerprint still matches. A changed Agent Status
attachment or other projection change correctly falls back to the
deterministic estimate.

The vLLM live case covered session-summary mode, two automatic compactions,
coherent fact retention, a real `glob` tool call/result, fresh Agent Status,
and provider usage. Explicit-summary mode requires a genuinely different
usable catalog model; the available local vLLM endpoint exposed only
`Qwen/Qwen3`, so the explicit-mode contract is validated by the deterministic
session-model tests instead. A provider context-overflow compact-and-retry is
likewise validated deterministically unless the local service can produce a safe,
reliable overflow without distorting the live scenario.

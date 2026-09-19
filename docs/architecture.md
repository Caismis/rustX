# Architecture

## App Server product topology

In externally managed deployments, one authenticated user maps to one rustX
App Server process, one configuration/source environment and one durable runtime
root. A process hosts many durable Sessions and independently loaded runtimes;
there is no process-global active Session. The manager admits at most one writable
live runtime per Conversation, with one resident node per Session in v1.

The higher-level host owns authentication, user/process routing, workspace
allocation/isolation, environment/credential injection and external restart policy.
The internal `AppServerHost` owns rustX admission/drain and diagnostics; it is not
that higher-level product host. Configuration, catalog, runtime, interaction and
projection owners retain their native authority after commit.

```text
local TUI   -> stdio JSONL -> TUI-owned App Server child
Web Console -> WebSocket  -> externally managed App Server
remote TUI  -> WebSocket  -> the same externally managed App Server
                            one semantic protocol, many Sessions
```

Connection lifetime, runtime residency, durable Session lifetime and process
lifetime are independent. Network disconnect is detach. Normal local TUI exit
explicitly requests owned-child drain; persistent work across client exit uses an
external server. Session cwd is execution/config-resolution context, not a
filesystem sandbox. See [protocol and ownership](app-server-protocol.md) and the
[composition evidence and developer flows](app-server-acceptance.md).

Goal joins the existing Runtime Client projection contract: the inactive bootstrap
cut seeds its bounded view, and native authoritative observations update that copy
and publish `goal_changed` through the single cursor/replay owner. Journal Written
and RoundAdmitted facts commit atomically with Goal state and ordinary round
acceptance; those two are the complete bounded fact vocabulary. Journal facts
never reconstruct Goal state. Durable `GoalPhase` is the one Goal lifecycle
authority, so no projected view can describe an Active Goal that is not actually
authorized to continue. See the [integration and drain
contracts](goal-extension.md#execution-facts-and-drain-ownership).


Goal is the third closed Native Agent Extension. `GoalDomain` is the sole durable
revisioned state authority; its commands and context are adapters. The synchronous
`GoalRoundDriver` participates in the existing coordinator worker and submits
ordinary typed Pending Inbound. It never owns model execution, canonical history,
Tool execution, request assembly, or a separate queue. See the exact transaction
and lock contract in [Goal extension](goal-extension.md).

Canonical `ProductRoot` is the sole authority for rustX-owned product storage
paths. Session, Conversation and child allocations are derived from that identity
before any private path is authored. Equivalent root aliases converge; symlinks
below the product root remain invalid private identities. Subagent IPC v22 carries
canonical product identity plus child Conversation identity and an incarnation
name, never a second absolute private runtime root. Inspection uses the same
identity-derived allocation. Embedded workspace managers may remain independent;
native workspace managers derive storage from their composed Conversation access.

## Native Session archive packaging (Issue #365)

`SessionArchiveProducer` in the native library reads catalog/lineage, immutable
SQLite history and ArtifactStore bytes. It owns one finite cut and one versioned
inspection archive, without loading runtimes or depending on transport/Trace.
App Server v12 prepares a scoped streaming-download capability; Web consumes it
through the browser download manager and TUI writes bytes to a client-local file.
Neither client composes the archive. Execution coordination ends before history
serialization, compression or transport backpressure. See the exact authority,
cut, cancellation and safety contracts in [Session archive export](session-archive.md).

## Session ownership and local lifecycle exclusion (Issue #254)

Session deletion cascades along durable ownership, never provenance. `/tree`
nodes belong to the same Session; `/fork` and `/clone` materialize independent
Sessions. Catalog membership and native typed child ownership commits establish
the finite target. Retained worktrees and branches are blockers requiring
explicit disposal, not implicit cleanup targets. A Workflow Agent borrowing a
Workflow-owned candidate/worktree acquires no independent physical disposal
authority. Preflight validates the typed borrow against the durable Workflow
owner in the same Conversation and emits one Workflow blocker, regardless of
the number of borrowers. Workflow disposal clears that blocker without child
terminal events; each child still owns its Conversation/private runtime state. Shared environments, capability
resources, caches, config, credentials and project files remain outside it.

Canonical `ProductRoot` identity, `ProductController` admission and target
Conversation lifecycle access are separate. Preflight freezes ownership
transitions, derives native ownership, then locks only target Conversation
allocations exclusively in sorted identity order. A live unrelated Session and
its Runtime Client remain usable; actual target runtime/child/inspection/private
writer access blocks exclusivity. Ordinary activity does not hold the ownership
freeze. Guards release through drop or OS process death; aliases share identity.
The local WorkspaceManager retains ownership-mutation authority from before
reading disposal facts through durable Started, physical removal and settlement,
including retries. `WorkflowWorkspaceDisposalStarted` is the destructive
admission boundary: a retained ownership snapshot excludes it. Conversely, an
admitted disposal excludes new ownership snapshots until it finishes. Neither
operation can cross the other's conflicting boundary. Started alone does not
change the semantic blocker revision.
Local composition binds ConversationAccess to WorkspaceManager independently of
its durable store, including existing-only access for historical management.
ConversationStore exposes durable semantics only, with no local OS-lock capability.
Concrete SQLite internally guards ownership-sensitive event transactions; the
WorkspaceManager independently guards the full physical disposal interval.
The semantic revision hashes only target membership, owned allocations and
final workspace-blocker state, never raw catalog bytes or execution history.
Management reads never create missing stores or directories. See
[the ownership and storage contract](session-deletion-ownership.md) for the exact
lock order, acquisition/release points, participant lifetimes and regression map.


## Native invocation ownership

| Concern | Owner |
| --- | --- |
| Canonical Agent ToolCall/ToolResult framing | Agent Loop |
| Fixed Workflow graph and local values | WorkflowRuntime |
| Source-qualified Tool selection | capabilities::selection |
| Trusted Leaf/Composite policy | Tool registration/admission |
| Foreground arbitration and typed cancellation provenance | shared ForegroundInvocation lifecycle |
| Physical execution, cancellation and settlement evidence | ToolExecutor |
| Human Tool approval rendezvous | InteractionCoordinator |
| Durable exact preparation/approval subject | durable interaction authority |
| Ordinary native execution facts | best-effort Event Journal observation |
| Canonical conversation truth | canonical history/store |

An Ask decision commits its immutable interaction subject before prompt publication;
ordinary Prepared/Started/progress facts have no permission authority. Native
cancellation winners publish an absorbing typed cause before signalling descendants.
Composite deadlines never borrow the attempt's default cancellation reason.
See [Tool lifecycle](tool-lifecycle.md) for frontiers and settlement semantics.

## 1. Architectural objective

rustX is an execution kernel, not an agent application framework and not a control plane. Its responsibility is to execute an immutable runtime manifest, produce durable execution facts, and expose stable runtime-owned contracts to higher-level systems.

The architecture is layered so that external SDKs, storage backends, process managers, and UI protocols can change without rewriting the agent kernel.

## 1.1 Native durable conversation authority (M8 / Issue #11)

The conversation has one backend-independent durability owner,
`ConversationStore`, implemented locally by `SqliteConversationStore`. One
SQLite database is physical colocation only; semantic ownership remains
separate:

| Domain | Durable authority |
| --- | --- |
| Pending Inbound Inbox | Accepted, not-yet-adopted deliveries, one shared `InboundSequence`, and correlation/idempotency state. |
| Message Ledger | Append-only canonical `MessageBlock` bodies, stable `MessageId`, and commit order. |
| Conversation Surface | Immutable `SurfaceOp` history plus current `SurfaceRevision`, active identity order, and compaction generation. |
| Request Snapshot | Immutable non-history inputs for one actual request, bound to one historical Surface revision and one `RequestId`. |
| Event Journal | Append-only typed execution facts with `EventId`, per-conversation `EventSequence`, schema version, reference checks, and terminal constraints. |
| Checkpoint/index metadata | Current Surface head and structural/context checkpoint copies used for bounded bootstrap validation; never a transcript. |
| `ConversationInboundMailbox` | Process-local acceptance coordination and wakeup only; it is not durable authority. |
| Runtime Client | Projection, cursor, and control adapter only; reconnect never supplies recovery facts. |

There is no `ConversationRecord`, full transcript table, request-message
copy, generic repository, or second durable history. Canonical message bodies
are stored once in the Ledger. A Surface revision stores identity/order
transitions, and a historical request combines that revision with its frozen
snapshot on demand.

The current SQLite development schema is defined by `SQLITE_SCHEMA_VERSION`.
Version 34 adds native revisioned Goal state and atomic Goal/inbound accounting.
Version 32 freezes Issue #258’s durable
`profile_digest` for the effective admitted child execution profile. Version 33 establishes
non-creating rollback-journal management reads and separated workspace storage.
Version 31 froze Issue
#242's provider-independent typed Questionnaire interaction audit: a requested
subject carries canonical requester identity and a typed `AnswerSpecification`
per question, and a settled submission carries typed scalar answers addressing
choices by option index rather than by authored label. A version-30 journal can
hold the obsolete choice-only payload, so it is refused at store open rather
than decoded under the new vocabulary. Version 30 adds concrete Loop
iteration admission/settlement and satisfied/exhausted exit facts. Existing
Runtime Client 21 and child IPC 19 identity vectors represent nested iterations.
Version 29 adds native Review
audit facts and required Questionnaire invocation correlation. Version 28 adds a distinct
Workflow resource recovery guard for uncertain final candidate settlement;
child IPC 18 and Runtime Client 20 are unchanged. Version 27 adds native
Workflow candidate ownership, settlement, invocation correlation and disposal,
with borrowed-run association in child IPC 18 and Runtime Client 20.
Version 26 adds typed native
deadline interruption to interaction outcomes/audit settlements; Runtime Client 19
and child IPC 17 mirror it. Version 25 added caller-neutral
approval correlation and native Workflow invocation facts with typed Workflow
failure status. Runtime Client 18 mirrors approval identity; child IPC 16 mirrors
the new invocation-bearing interaction data. No migration or compatibility
decoding is provided. The Event Journal envelope version remains 1: framing is
unchanged; the development durable schema gates its changed payload vocabulary.
Version 24 adds scoped
Workflow lifecycle facts and concrete instance correlations (Issue #217).
Version 23 preserved `Denied` in background terminal facts (Issue #206). An incompatible database
fails explicitly; there is no migration chain, legacy reader, compatibility
fallback, dual write, or old storage mode. Version 10 froze the structured
Questionnaire interaction audit vocabulary introduced by Issue #126. Version
11 added the typed Agent Status generation descriptor: its UTC generation
instant and admitted module membership are durable with the canonical status
message. Version 12 added the complete canonical-message-coupled Agent
Status emission facts, bounded latest-emission heads, and the Todo-specific
durable progress sequence. Version 14 freezes the typed
`ToolCancellationPhase` carried by canonical cancelled tool results. Version
15 adds the one-shot unresolved-output pending source and the frozen
request-only carryover representation/anchor in Request Snapshots. Version 16
freezes typed compaction-summary metadata, version 17 freezes named-subagent
ownership identity, and version 18 freezes subagent workspace snapshots and
preserved-worktree handoffs. Version 19 freezes the native Workflow execution
fact vocabulary. Version 20 replaces the conflated subagent workspace path
with explicit logical-child scope and physical-worktree ownership facts.
Version 21 freezes the first retained-workspace disposal protocol: durable
disposal intent, the typed `WorktreeRemoved` partial phase, and final
`Disposed` settlement. Version 22 replaces the terminal event's optional
handoff with a typed resource disposition that also durably preserves
`PreservedUnresolved` physical ownership when terminal inspection cannot
prove a complete handoff. Version 21 and every older development schema are
rejected rather than decoded with missing or invented workspace authority; the
review-only intermediate schema history is not a supported format.
File-backed stores
use rollback journaling (`DELETE`), `synchronous=FULL`, foreign-key enforcement, and a busy timeout. A
successful SQLite commit is the local durability linearization point
documented here.

Version 9 froze the durable **answer obligation** (Issue #111): adoption
commits an `InboundTurnAdopted` fact naming the exact adopted batch, and
startup recovery decides continuation from that fact alone. No table changed —
which is exactly why the version gate matters. A v8 journal predates the
vocabulary, so a current reader would read its silence as "no answer is owed"
and strand precisely the crash states the obligation rescues.

Version 10 freezes the structured Questionnaire interaction audit vocabulary
(Issue #126): one requested fact stores the complete bounded questionnaire by
value, and one terminal settlement stores either the canonical submitted
answers, an explicit decline, or owning-attempt cancellation. A v9 journal may
contain the obsolete Question/Answered payloads and is rejected rather than
decoded or migrated.

The physical tables are deliberately semantic rather than generic:

| Table | Purpose and constraints |
| --- | --- |
| `rustx_store` | One-row conversation binding, schema version, durable next `InboundSequence` / Event Journal / transcript position counters, the Todo-specific logical-primary-start progress sequence, and the pending unresolved-output `PublicationStreamId` pointer. |
| `pending_inbound` | Pending deliveries keyed by `InboundSequence`, with unique `MessageId`, serialized User body, and optional correlation. |
| `inbound_correlation` | Exactly-once correlation mapping to the accepted sequence and unique `MessageId`. |
| `message_ledger` | Append-only canonical bodies keyed by commit `position` and unique `MessageId`. |
| `bootstrap_identity` | Immutable initial-history count and digest, including an explicit empty bootstrap. |
| `surface_ops` | One immutable `Append` or `Replace` operation per `SurfaceRevision`, with compaction generation. |
| `surface_head` | Current Surface revision, active identity order, and compaction generation. |
| `context_checkpoints` | Current structural/index checkpoint matching `surface_head`; it is not message history. |
| `request_snapshots` | One immutable non-history snapshot per `RequestId`, its frozen provisional Assistant identity, Surface revision, committed start sequence, and optional request-only carryover source/representation/anchor. |
| `events` | Append-only typed envelopes keyed by per-conversation Event Journal sequence and unique `EventId`. |
| `agent_status_emission_heads` | One materialized latest-emission record per `(AgentStatusModuleId, semantic key)`, including the store-assigned Todo cooldown origin, maintained only by the combined model-turn-start transaction. |
| `lifecycle_state` | Durable terminal markers enforcing zero-or-one terminal event and terminal absorption for attempt, turn, and background-execution lifecycles. |
| `publication_streams` | One frozen publication generation per provider request, with terminal marker and one of the three settlements. |
| `publication_frames` | Contiguous transient release staging for one publication stream. |
| `publication_proposals` | Proposal ownership keyed by `(stream_id, ToolCallId)`, with frozen block/tool/name identity, explicit `started`/`completed` state, execution, and settlement state. Provider call IDs are publication-scoped. |
| `publication_audits` | One bounded immutable audit for each non-canonical publication settlement. |
| `transcript_order` | A narrow durable ordering spine of references to canonical Ledger messages, publication audits, and interaction audit events; it stores no message or audit body. |

`MessageId`, `InboundSequence`, `SurfaceRevision`, `RequestId`, `EventId`,
Event Journal sequence, `AttemptId`, `TurnId`, `ToolCallId`,
`ToolExecutionId`, `CapabilityRevision`, and `ContextGeneration` remain
distinct identity domains even where SQLite stores their serialized values as
integers or text. The unique constraints prevent identity reuse; semantic
reference checks prevent an Event Journal fact or Surface operation from
pointing at an unavailable authority.

The semantic write pattern is always:

```text
prepare and validate all fallible state
        ↓
one ConversationStore SQLite transaction
        ↓ COMMIT = durable authority linearization
install the already-validated hot result, or reload it from the authority
```

Inbound acceptance commits sequence allocation, pending delivery, and
correlation state together. Adoption commits the selected finite pending
watermark, canonical User Ledger rows, Surface Append revisions, checkpoint
metadata, and pending deletion together. Ordinary canonical appends and
ToolResult sibling batches append Ledger bodies and Surface revisions in one
transaction; committed-message events share that transaction. Compaction
commits the summary body, Surface Replace revision, generation/checkpoint
metadata, and `CompactionCompleted` reference together. Request start commits
the immutable Request Snapshot and `ModelRequestStarted` fact together before
the provider adapter is called. When that snapshot carries unresolved-output
carryover, the same start transaction clears its pending source pointer.
Background terminal publication commits its terminal inbound row and reference
fact together.

Publication opening is a second durable admission boundary: the store decodes
the named Request Snapshot and its exact start event before it can insert or
idempotently reopen a stream. The stream must match the snapshot's
`RequestId`, `AttemptId`, `TurnId`, provisional Assistant `MessageId`, and
derived `PublicationStreamId`. Provider outcomes must identify that same
started snapshot and envelope generation; only one successful
`ModelRequestCompleted` can establish P.

The durable request-start flow is:

```text
assemble provider-neutral ModelRequest
→ freeze RequestSnapshot / RequestId
→ commit snapshot + ModelRequestStarted
→ independently reconstruct and compare exact ModelRequest
→ invoke provider adapter
```

Historical reconstruction reads only the immutable snapshot, the referenced
Surface operation history, and keyed Ledger bodies. It never reruns
contributors, Skill discovery, extension/DSH logic, current status sampling,
workspace inspection, or current model/tool/capability configuration.

The Event Journal lifecycle table rejects a second terminal attempt, turn, or
background-execution fact and rejects every later fact in that lifecycle. An
attempt terminal is an attempt-level fact; it is not treated as a late event
inside a turn that already emitted `TurnCompleted`.

The generic `ConversationStore::append_event` transition is limited to facts
whose durable owner does not need a purpose-specific receipt. It rejects
`InteractionRequested` and `InteractionSettled` before opening a transaction;
the dedicated `append_interaction_audit` transition commits the Event Journal
fact and its `transcript_order` reference together and returns the exact
`TranscriptCursor` to the live publication path.

Runtime bootstrap loads only the current Surface head, active IDs, active
message bodies, structural checkpoint metadata, pending items, and bounded
projection state. The Runtime Client also receives only the newest bounded
page of the derived transcript; old transcript entries are read lazily from
the ordering spine. Event pages, old requests, retired Ledger rows, and
historical Surface revisions are read lazily. M8 stores the evidence; M9a
(Issue #12) adds the startup recovery that classifies and reconciles it — see
[Recovery model](#7-recovery-model). Model-turn cancellation redesign (M9b)
and runtime supervision/quiescence (M9c) are delivered below; replay/resend
policy and retry orchestration remain intentionally outside this architecture.

## 1.2 Runtime supervision and quiescence (M9c / Issue #12)

`ConversationRuntime` owns one lifecycle authority:

```text
Inactive --activate--> Running --shutdown linearization--> Draining
                                                               |
                         all owned work durably/native settled  v
                                                           Quiescent
```

The `Running -> Draining` transition is performed under the coordinator
state lock. It is the total-order point against inbound acceptance, model
updates, and attempt admission. Background ownership transfer and capability
revision commit use the same lifecycle at their native registry/coordinator
boundaries; they cannot commit new semantic work after drain wins.

An authoritative MCP `PhysicalSettlement` failure uses that same coordinator
critical section: failure publication and the `ConversationLifecycle`
transition to `Draining` are one runtime coordinator linearization point. The
persistent MCP failure latch is retained as diagnostic/admission evidence;
`ConversationLifecycle` remains the generic gate that closes inbound,
attempt, compaction, reload, interaction, background, and subagent semantic
ownership. A background-late settlement failure follows this identical
failure-drain transition, so there is no post-publication interval in which a
runtime is still healthily `Running`.

`ConversationRuntime::shutdown()` is the one public semantic shutdown
operation. It is asynchronous and idempotent: concurrent and repeated calls
join one drain completion, and success means `Quiescent`, not merely that a
cancellation signal was set. `Inactive` shutdown is refused. `Draining` still
permits only required settlement mutations; `Quiescent` refuses even stale
settlement callbacks.

The ownership graph is concrete:

```text
ConversationRuntime
├─ admission worker (explicit exit boundary)
├─ current AgentExecution
│  ├─ M9b model-start arbitration and provider settlement
│  └─ Agent Loop foreground tool-batch structural settlement
├─ ConversationBackgroundRegistry
│  └─ conversation-owned runners and terminal Pending Inbound publication
├─ CapabilityCoordinator
│  ├─ counted capability/environment preparation
│  ├─ runtime-owned capability revision commit boundary
│  └─ retained MCP runtimes and notification subscriptions, closed/joined
│     through their existing physical settlement contract during drain
└─ native process composition
   └─ existing TERM → grace → KILL → group terminality → reap/containment proof
```

Cancellation requested, operation settled, and runtime quiescent are
distinct. A started model request is awaited after cancellation; started
foreground tools are awaited by the Agent Loop; committed background work is
cancelled and awaited through `wait_until_settled`; and a background record
does not become terminal until its exactly-once terminal Pending Inbound fact
has durably committed. An accepted Pending Inbound item is never adopted into
a new attempt after drain and remains durable at quiescence.

### Supervision does not stop at the first failure

Drain is a supervisor, not a short-circuiting pipeline:

```text
close admission
  -> request cancellation/closure of every concrete owner
  -> supervise EACH owner to its own native terminal boundary
  -> collect settlement/durability failures
  -> decide: Quiescent, or one aggregated settlement failure
```

A failure in one participant is an error **fact**; it is never permission to
abandon a sibling that can still produce an external effect. Every wait has a
native boundary that cannot be starved: a background record either settles
terminally or explicitly abandons its bounded durable terminal publication
(neither fact leaves any callback authority behind), an MCP runtime's close
proves or disproves physical settlement, and a counted lifecycle admission is
released only by its owner. No drain wait is conditioned on a global health flag, so
one owner's failure can never be read as another owner's settlement. The
collected failures are rendered as one bounded deterministic diagnostic —
a diagnostic aggregation, not an error framework. `Ok(())` therefore still
means exactly `Quiescent`; `Err(RuntimeOwnedSettlement)` means admission is
closed and every settleable owner was supervised to its strongest available
boundary while some ownership/physical/durable terminal condition stayed
unproven. An unresolved `PublishingTerminal` record remains explicitly
non-terminal and is never reinterpreted as success.

### A settlement fact never precedes the owner's last callback

`publication_abandoned` is the fact drain consumes as one background
execution's settlement, so publishing it is a linearization point, not a
bookkeeping detail:

```text
durable terminal publication attempts #1 and #2 exhausted
  -> failure sink callback begins (coordinator lock, durability health,
     possible `DurabilityFailed` observation)
  -> failure sink callback completes
  -> `publication_abandoned` commit
  -> waiters notified
  -> zero remaining conversation callback authority
  -> drain may treat this owner as settled-with-failure
```

Reporting the failure is real semantic runtime work, so the abandoned fact
must not become observable while that callback can still run; otherwise drain
could aggregate the abandoned evidence and cache a failed shutdown before the
runner finished calling back into the conversation. The continuation is held
inside one counted settlement admission across both steps, because a *failed*
drain leaves the lifecycle `Draining` — where settlement callbacks remain
intentionally legal — and so cannot rely on the admission refusal that
protects a successful `Quiescent`. The contract is logical callback
settlement, not the runner task's syntactic return: once
`publication_abandoned` is observable, that execution owns no failure-sink
callback, observer callback, Pending Inbound attempt, durability-health
mutation, progress callback, terminal retry, or semantic registry mutation.
`shutdown().await` — `Ok` or `Err` — therefore always returns after every
runtime-owned operation reached its strongest honest settlement boundary.

### Waiter lifetime is not ownership lifetime

A caller's future is never the owner of a physical resource. MCP connection
establishment makes this explicit, because a stdio process exists before the
handshake completes:

```text
no physical owner
  -> conversation-counted preparation owner (own task, own counted lifecycle
     admission, own ownership cancellation signal)
  -> physical MCP process ownership established
  -> either  A. transferred into the coordinator's retained `mcp_runtimes`
     or      B. cancelled/failed and driven to physical settlement
```

The counted admission is released only after A or B, so aborting or dropping
the `prepare_candidate` caller cannot remove the physical owner from the
quiescence proof. Runtime drain **cancels** those owners; it never drops
their futures, because `Drop` merely requesting a shutdown is a cancellation
signal, not proof of settlement.

### Attempt slot settlement and attempt task lifetime

Clearing the current-attempt slot hands the conversation state back to the
coordinator. It does not end the attempt task, which still owes the
coordinator its final admission callback. The attempt task therefore holds
its own counted lifecycle admission from the publication that fills the slot
until the task body has fully returned, so quiescence covers the task's
callback authority and not only the slot. `AgentExecution` remains the
execution and terminal semantic authority; `ConversationRuntime` remains the
task/runtime lifetime composition authority.

Foreground tool results retain one slot per model call and canonical model
call order. A committed background execution survives attempt cancellation
but not conversation drain. The process runner's existing physical proof is
composed transitively; no global process registry is introduced. Capability
preparation is counted until its owner returns, so shared `EnvironmentStore`
materialization is not cancelled globally by one conversation but cannot
create a late runtime callback or revision. Retained conversation-owned MCP
stdio runtimes are explicitly closed and physically settled before
quiescence; an unproven process or notification-task settlement is reported
as a runtime failure, not as successful shutdown. All runtime capability
commit points refuse after drain.

Runtime Client remains projection, control, and attachment state only. Client
detach, stdio EOF, TUI exit, and attachment drop do not cancel or drain a
conversation. The async client shutdown request awaits the runtime operation;
the `RuntimeShutdown` projection event marks admission closure, while the
`shutdown_completed` response marks successful quiescence.

## 1.3 Native interaction and approval coordination (M9.2 / Issue #100)

rustX has one provider-independent human-interaction plane. It is a
conversation-owned rendezvous, not a second execution engine:

```text
ToolRegistry preflight
  -> canonical Assistant ToolCall commit
  -> effective ToolApprovalPolicy / ApprovalMode
       Never ------------------------------┐
       Always -> AttemptLifecycle::pre_tool│
       Allow ------------------------------┐
       Deny -> one typed denied result      │
       Ask -> InteractionCoordinator       │
                   -> Runtime Client       │
                   <- typed response       │
       -> existing cancellation/start frontier
       -> exact original PreparedInvocation, or one result slot
```

`InteractionCoordinator` is the sole owner of interaction identity,
pending state, terminal response/cancellation coordination, and the waiter
rendezvous. `AgentExecution` remains the owner of tool scheduling and
execution. The Runtime Client and TUI only project and transport the
request; neither can mutate canonical history, execute a tool, or rewrite
arguments.

The ownership table is:

| Concern | Owner |
| --- | --- |
| Interaction identity and pending registry | `InteractionCoordinator` |
| Pre-tool decision | the attempt's required `PreToolPolicy` |
| Tool scheduling and start | Agent Loop |
| Tool identity and validated arguments | original `ToolCall` + `PreparedInvocation` |
| Tool result settlement | Tool Plane / Agent Loop |
| Response transport | Runtime Client |
| Rendering and input | TUI projection |
| Attempt cancellation | `AgentCancellation` |
| Tool cancellation observation | owner-observing `ExecutionCancellation` with one-way child derivation |
| Native Questionnaire capability | crate-private `QuestionnaireRequester` bound by the Agent Loop attempt |
| Runtime drain and quiescence | `ConversationRuntime` / `ConversationLifecycle` |
| Crash recovery | existing M9 recovery owner |

The pre-tool seam is total and typed: every `AttemptLifecycle` carries one
`PreToolPolicy`, while a runtime-created attempt receives one concrete native
binding to its owning `InteractionCoordinator`. The binding is not a
replaceable production rendezvous strategy, and no public generic interaction
trait exists. The only Tool Plane consumer is native `ask_user`, which gets a
crate-private `QuestionnaireRequester` containing the attempt identity, the
owner-observing `ExecutionCancellation` capability, and that coordinator. A standalone
inert execution has no interaction provider and therefore fails an `Ask`
closed. The configured
`ToolApprovalPolicy` is resolved only after exact registry preflight. The
runtime-wide `ApprovalMode` then computes effective approval: `Policy` keeps
the Tool's `Never`/`Always` value, while `FullAccess` maps eligible calls to
`Never` without changing any Tool definition. This issue does not add a
permission language, risk-classification engine, allowlist, routing layer, or
form framework.
`PreToolPolicy` runs only after registry identity resolution, reserved metadata
stripping, tool-owned semantic normalization, and business-argument
validation, and after the Assistant `ToolCall` is canonical. For `ask_user`,
preflight validates one ordinary typed questionnaire object and rejects
malformed, unknown, duplicate, reserved, or out-of-bounds values before a
`PreparedInvocation` can be returned. It never parses JSON stored in strings or
coerces string booleans. The executor consumes that canonical invocation; it
cannot rediscover model-argument validity. The policy cannot resolve a tool,
dispatch it, or alter the prepared invocation.

Questionnaire is a separate bounded interaction kind, not an approval
variant. The native `ask_user` Tool uses the ordinary Tool Plane path and
fixed foreground/sequential/approval-never policy. One invocation is one
coordinator interaction, even when it contains several related questions:

```text
Assistant ToolCall(ask_user)
  -> registry preflight and one immutable QuestionnaireSpecification
  -> ordinary executor
  -> one InteractionCoordinator Questionnaire publication
  -> one pending Runtime Client questionnaire
  -> one submitted or declined response
  -> one durable settlement
  -> ordinary ToolResult
  -> model continuation
```

The model-facing contract is one ordinary root object with only the required
`questions` array. It accepts 1–4 questions, each with a required non-empty
`question` (at most 4096 Unicode scalar values), short `header` (at most 16),
2–4 authored `{label, description, preview?}` options, and an optional
`multi_select` boolean defaulting to false. Labels are bounded to 60 scalar
values, descriptions to 1024, previews to 8192, and custom answers to 4096:

```json
{
  "questions": [
    {
      "question": "Which visual direction should I use?",
      "header": "Visual style",
      "options": [
        {
          "label": "Swiss / Klein blue (Recommended)",
          "description": "Information-first typography with strong hierarchy and blue highlights.",
          "preview": "Optional Markdown preview"
        },
        {
          "label": "Electronic magazine",
          "description": "Serif typography, warmer colors, and a more editorial composition."
        }
      ],
      "multi_select": false
    }
  ]
}
```

Custom text is always available as a client-owned row **for `ask_user`**,
because every question it authors declares `allow_custom` (see the typed
vocabulary below); the model never sends `allow_free_text` and never authors an
`Other` sentinel. Related blocking
questions belong in one call. A client response carries only question indices
and typed decisions. Submitted answers may be partial; accepted answers are
normalized into question order and canonical option order. Previews are
rendered for single-select questions. With no interaction-capable client,
`ask_user` returns an explicit failed ToolResult. A user decline is a successful
result `{ "cancelled": true, "answers": [] }`, while attempt cancellation
remains `ToolExecutionStatus::Cancelled`; neither response can replace the
original Tool arguments.

##### The typed question vocabulary (Issue #242)

Those model-facing arguments are **`ask_user`'s** authoring surface, not the
interaction contract. The runtime-owned contract is a small, finite,
provider-independent vocabulary in which every question declares the exact
shape of a legal answer, and `ask_user` is one point in it:

```text
QuestionSpecification { question, header, answer }
                                           |
       +----------+----------+-------------+-----------+--------------+
       |          |          |             |           |              |
     Text      Number     Integer       Boolean   SingleChoice   MultiChoice
   bounded    bounded     bounded        true/     options +      options +
   length,    range       range          false     allow_custom   bounds +
   format                                                        allow_custom

native ask_user, multi_select: false -> SingleChoice { options,
                                          allow_custom: true }
native ask_user, multi_select: true  -> MultiChoice  { options,
                                          1..=options.len(),
                                          allow_custom: true }
```

Two properties follow, and both are load-bearing:

- **the request declares the legal answer shape.** A Runtime Client never has
  to guess which answers are legal, and a free-text row exists only where a
  question sets `allow_custom`. A bounded MCP `enum` sets it to `false`, so
  the client offers no custom row and the protocol cannot express one — the
  old "type a custom answer to an enum and the whole tool call fails" outcome
  is gone by construction rather than by a special case.
- **response validation derives from those immutable request facts**, in
  `events::interaction`, which is the one authority the live coordinator, the
  durable store, and every producer share. A Runtime Client may reject
  obviously invalid input earlier for the user's benefit, but client-side
  validation is UX and runtime-side validation is authoritative. An invalid
  answer is refused while the interaction stays pending; it never fails the
  enclosing tool invocation.

###### The scalar domains

Each scalar shape names exactly **one** domain, and that same domain is used by
the request bound, the Runtime Client wire, the authoritative comparison, and
the value finally emitted to a provider. No stage widens or narrows, so there
is nowhere for a validated value and an emitted value to disagree.

**`Number` — the finite IEEE-754 binary64 (`FiniteNumber`), canonical binary64
text on the wire.**

MCP types an elicitation `number` bound as a binary64: rmcp's
`NumberSchema::minimum` and `maximum` are `Option<f64>`. A JavaScript `number`
is a binary64 too. Binary64 is therefore not a convenience — it is the widest
value every stage can hold without rounding, which is what makes it the one
domain all of them can share:

```text
MCP NumberSchema bound (f64)
  -> NumberAnswerSpecification bound (FiniteNumber)
  -> Runtime Client wire              "43e0000000000000"   (canonical binary64 text)
  -> client `number`, reconstructed exactly                (binary64)
  -> NumberAnswer value              (FiniteNumber)
  -> authoritative range comparison  (FiniteNumber)
  -> MCP accept.content JSON number  (the same FiniteNumber)
```

Three things are kept strictly apart, and conflating any two of them is the
whole failure class:

```text
human decimal spelling      client-local presentation ("1.5e3")
  -> finite binary64        the semantic value        (1500.0)
  -> canonical wire text    an exact encoding of the *value*
```

The wire carries the **value**, never the spelling. The protocol does not
preserve `1.5e3`, because lexical spelling is client-local presentation state;
it preserves the exact binary64 the client selected.

*Why the wire is not a JSON number.* A JSON number cannot carry binary64
identity across this protocol, because a JavaScript client serializes a
`number` through `JSON.stringify`, which prints the shortest decimal that
*round-trips* — not the exact value it denotes. The exact binary64 `2^63` is
the mathematical integer `9223372036854775808`, and `JSON.stringify` emits
`9223372036854776000`. Those are different integers. They happen to parse back
to the same binary64, but a reader that treats a JSON integer as an exact
decimal integer — which it must, to refuse a decimal binary64 cannot hold — has
to reject the second, and a question whose only legal answer is `2^63` becomes
publishable and unanswerable. **Binary64 identity must therefore not depend on
any language's decimal rendering of a `number`.**

*The canonical wire form.* A `FiniteNumber` crosses the Runtime Client protocol
— and is stored in the Event Journal — as its IEEE-754 bit pattern written as
exactly 16 lowercase hexadecimal digits, most significant first:

```text
2^63   -> "43e0000000000000"
-2^63  -> "c3e0000000000000"
0.1    -> "3fb999999999999a"
```

It is **exact** (the value's own bits, so `decode(encode(x)) == x` for every
finite binary64, with no decimal parser in the trust path); **canonical** (one
value has one spelling, byte for byte, in both languages — a shortest
round-tripping *decimal* is deterministic within one language but Rust and
JavaScript do not format it identically, so it could not carry the one-settled-
spelling property `ExactInteger` already holds rustX to); **bounded** (always
16 bytes, with no 700-digit subnormal expansion and no locale, grouping, or
exponent-notation variation); and **closed over the domain** — the wire
alphabet *is* the domain, so a decimal binary64 cannot hold, `9007199254740993`,
has no wire representation at all rather than one the runtime must detect and
refuse. `FiniteNumber::from_wire` is the one authoritative parse, and
`tui/src/protocol/number.ts` is the client's single conversion seam; every
bound and every answer on both sides goes through them.

The representation is internal to the Runtime Client protocol and the durable
audit. A human never sees it — the TUI reads a decimal draft, validates it with
`readNumberDraft`, and renders bounds as ordinary decimals — and an MCP server
never sees it either: `accept.content` carries the ordinary JSON number built
from the same bits.

*Admissibility is the client's statement, not its authority.* A whole decimal
binary64 cannot hold is refused by `readNumberDraft` before any bound is
consulted, on the *spelling* rather than on the rounded value:
`9007199254740993`, `9007199254740993.0` and `9.007199254740993e15` all denote
one refused integer, and `Number(...)` collapses all three onto `2^53` before
anything could tell them apart. That refusal is a statement of the domain for
the human's benefit. The enforcement is structural: such a value has no
canonical wire spelling to arrive in, so it can never reach the runtime's range
check at all — which is stronger than detecting it there.

*Negative zero.* rustX **canonicalizes** `-0.0` to `+0.0`. IEEE-754 gives the
two distinct bit patterns, but every comparison the domain takes part in — Rust
`Eq`, `PartialOrd`, and the authoritative range check — already treats them as
one value, so admitting two bit patterns would give one semantic value two
canonical wire spellings. The normalization happens at construction, in
`FiniteNumber::try_new` and in `finiteNumberToWire`, so no `-0.0` ever exists to
be serialized, and `"8000000000000000"` is refused on the wire as a
non-canonical spelling of `0.0` exactly as `ExactInteger` would refuse `"-0"`.
NaN and infinity are unrepresentable by construction rather than merely
rejected, which is what makes `Eq` sound: every value the domain holds is
reflexive.

**`Integer` — the exact `i64` (`ExactInteger`), decimal text on the wire.**

MCP types an elicitation `integer` bound as an `i64`: rmcp's
`IntegerSchema::minimum` and `maximum` are `Option<i64>`. So `i64` is exactly
the domain the protocol hands rustX, and no MCP integer schema can name a bound
outside it — refusing an out-of-domain integer schema is vacuous here by
construction, not missing.

The stage that *cannot* hold that domain is the Runtime Client protocol, whose
JavaScript `number` is a binary64 and loses whole numbers above `2^53`. A
question bounded to `9007199254740992..=9007199254740993` would then be
publishable by the runtime and unanswerable by any client — a published
question with no faithful response representation, which the typed interaction
contract forbids. So the value crosses the wire as canonical decimal **text**
and is parsed back exactly once, by the runtime:

```text
MCP IntegerSchema bound (i64)
  -> IntegerAnswerSpecification bound (ExactInteger)  "9007199254740993"
  -> Runtime Client JSON string                       "9007199254740993"
  -> TUI draft, edited as decimal text                 9007199254740993
  -> ExactInteger::parse — the one authoritative parse (i64)
  -> authoritative range comparison                    (i64)
  -> MCP accept.content JSON integer                   9007199254740993
```

`ExactInteger::parse` accepts an optional `-` followed by ASCII digits and
nothing else: `1.5`, `1e3`, `+1`, `-`, `NaN`, `Infinity`, and `12abc` are all
refused, as is any value outside `i64`. A settled value re-serializes from its
`i64`, so one value always has exactly one canonical spelling. The TUI compares
drafts with `BigInt`; it never uses `Number` or `Number.isSafeInteger` as the
semantic representation of an integer, because doing so would round away the
very values this representation exists to preserve.

**`Text` — an omitted answer and an explicit `Text("")` are different facts.**

A submission may leave a question unanswered; that is not the same as answering
it with the empty string. For a required MCP string property with
`minLength: 0`:

```text
explicit Text("")  -> { "action": "accept", "content": { "field": "" } }
omitted            -> { "action": "decline" }   (a required property is missing)
```

The empty string is legal whenever the question declares no `min_length` or a
`min_length` of `0`, and a positive `min_length` refuses it exactly as it
refuses any short answer. A client must therefore track answer **presence**
separately from draft length: the TUI carries an explicit per-question
committed bit, set by editing the field — typing a character and erasing it is
an explicit empty answer — or by pressing Enter on it, which commits the draft
as it stands. An untouched field is never committed, so a blank questionnaire
still submits nothing.

A response addresses a choice by its **zero-based option index**, never by its
display label. A label is presentation: it can repeat, collide with a
client-reserved row, or be forged, and none of that can change which value the
runtime settles on. (Two options a *human* could not tell apart are still
refused deterministically, because that ambiguity is real even when the wire
is not.)

Every Questionnaire also carries an `InteractionRequester` — the
registry-resolved `{ tool_id, tool_name, origin }` of the tool that asked. It
is canonical, provider-independent identity, never a display string and never
an MCP SDK value, and it flows unchanged through the live request, the Event
Journal subject, the Runtime Client projection, and the TUI. It is
deliberately orthogonal to `InteractionSource`: *where* an interaction came
from (primary or subagent) and *who* asked (a native tool, or an MCP server
named by `ToolOrigin::Mcp`) are two independent facts and are never collapsed
into one field.

The runtime control plane exposes `effective_approval_mode` and a pending
desired mode. A busy attempt freezes the effective mode it admitted; later
requests coalesce in `desired_approval_mode` and reconcile only after terminal
settlement, before the next attempt admission. Requesting `FullAccess` never
auto-answers a pending Approval, activates a disabled Tool, restores an
excluded Tool, or bypasses execution/concurrency restrictions. `ApprovalMode`
is current runtime configuration (`approvalMode`, default `policy`) and is not
Session history; resume uses the current configuration.

These are intentionally distinct runtime facts:

```text
availability != activation != approval != approval mode != execution != concurrency
```

An approval request contains only immutable, decision-relevant facts:
conversation/attempt/turn identity, `ToolCallId`, resolved `ToolId`, safe tool
name, origin, mode, validated arguments, and the policy reason. Conversation
identity is injected by the coordinator and attempt identity is supplied by
the owning execution at the narrow request boundary; neither is caller-
reported through approval facts. The response vocabulary is finite (`Allow`
or `Deny { reason }`) and contains no replacement arguments. Allow therefore
resumes the exact invocation that was already prepared; the Agent Loop checks
cancellation again at the existing start frontier before creating an executor
future.

The asynchronous policy boundary has one cancellation rule: after
`PreToolPolicy::evaluate()` settles, the Agent Loop checks cancellation before
consuming `Allow`, `Deny`, `Ask`, or a policy error. If cancellation is
observable, the decision is not consumed and the call receives the normal
cancelled result slot. An `Ask` response is subject to a second checkpoint;
`Responded(Allow)` is a rendezvous outcome, never tool-start authority.

The coordinator's pending state has one mutex-protected terminal transition:

```text
Pending --response--> Responded
Pending --owner cancellation/runtime drain--> Cancelled
```

The losing operation receives `not_pending` and cannot wake or resume the
owner. A terminal transition removes the live map entry, but its waiter keeps
a counted `LifecycleAdmission` until the owner consumes or drops the outcome.
The Runtime Client settled observation is published only after that waiter
authority is released and while a second counted settlement admission covers
the leaf observation callback. Thus an empty pending map is not quiescence,
and no interaction callback can begin after `Quiescent`.

`AgentCancellation` remains the sole cause authority for an attempt-owned
interaction. The coordinator retains only an `ExecutionCancellation` view to
consume the already-selected first-winner reason at this boundary; it never
receives the owner or performs cause arbitration of its own. The view cannot
expose the owner's signal; its `child_signal()` can only derive a subordinate
signal whose cancellation does not propagate upward. A response that arrives
after cancellation is observable cannot publish `Responded`; it is rejected as
`not_pending` after the matching `Cancelled { reason }` transition. During
drain,
`ConversationRuntime` requests `RuntimeShutdown`, reads the active attempt's
winner, and propagates that reason to every live pending interaction. A prior
`UserRequested` cause therefore remains `UserRequested`; absent an earlier
winner, all interaction, tool, and attempt cancellation facts report
`RuntimeShutdown`.

The runtime has one active `CurrentAttempt` slot. A runtime-created native
interaction is published only by that attempt's AgentExecution, so every live
pending interaction at drain belongs to the one cancellation authority being
propagated. `finish_attempt` clears the slot only after the attempt's semantic
settlement and final callback, while interaction waiter admissions keep
quiescence behind the same boundary.

Interaction IDs are derived as `{AttemptId}-interaction-{ordinal}`. Attempt
identities are recovered from durable history and never reused, so a process
restart cannot make a delayed pre-crash response name post-restart work. Live
pending interactions are process-owned observations, not durable workflow
records. Recovery does not replay or reconstruct an old approval request; a
new runtime starts with no phantom pending interaction.

### 1.3.1 Durable interaction audit (FND-04 / Issue #109)

The interaction plane owns two things that must never be confused:

```text
pending waiter / prompt lifecycle  = process-owned workflow state (never durable)
requested / settled semantic facts = durable audit evidence (Event Journal)
```

Only the second is persisted, as two low-frequency Event Journal facts:

```text
InteractionRequested { interaction_id, subject }
InteractionSettled   { interaction_id, settlement }
```

The coordinator reaches durability through the narrow
`ConversationInteractionAudit` capability, which commits exactly those two
facts and rejects every other payload. It receives no Ledger, Surface,
Request Snapshot, publication, or general Journal authority, so an audit seam
can never become a second way to authorize a side effect.

The Runtime Client/TUI boundary is fail-closed for cursor contradictions:
cursor absence is legal only for a hidden Context-kind User message. A visible
User, Assistant, or Tool message, and every visible inbound, must carry its
durable `TranscriptCursor`; a hidden Context carrying one is also invalid.
Protocol validation and the presentation reducer share this visibility rule,
so a malformed event cannot silently advance or rewrite accepted presentation
state.

`InteractionRequested` opens the `interaction:{id}` durable lifecycle and
`InteractionSettled` closes it exactly once, in the same shape the background
and subagent ownership lifecycles already use. The store rejects a duplicate
request, a duplicate or contradictory settlement, a settlement without its
request, and a settlement whose terminal its subject cannot produce (an
Approval cannot be questionnaire-submitted, a Questionnaire cannot be
approved; cancellation is the one terminal both share). Both facts carry a
canonical event identity
derived from the interaction identity, so the pair resolves through the unique
`event_id` index rather than a Journal scan.

The store enforces four further semantic invariants, because
`InteractionSubject` and `InteractionSettlement` are ordinary deserializable
payloads and a fact that bypassed the live coordinator must still be refused:

- `InteractionRequested` and `InteractionSettled` belong to the exact same
  conversation + attempt + turn envelope. The conversation comes free from the
  store's envelope check; the attempt and turn are compared against the
  committed requested fact, and an audit fact missing either its attempt or
  its turn is refused because it cannot be pinned to its pair.
- An Approval audit subject must match the canonical `ToolCall` it references
  *and* the generation that proposed it. A `ToolCallId` is
  request/publication-scoped, so equal content is not ownership; the store
  resolves `(call_id, attempt_id, turn_id)` through the retained FND-03
  publication owner — `publication_proposals` joined to `publication_streams`
  on a `canonical` settlement — to exactly one Assistant `message_id`, requires
  that message to still be on the active Surface and to contain the call, and
  then requires the frozen tool id, name, and argument digest to equal the
  subject's. A well-typed approval naming a call that was never proposed, a
  different tool, a different argument value, *or the same call from another
  turn or another attempt* is a semantically false audit record and is refused.
  There is no bare conversation-global `call_id` fallback: the Agent Loop
  refuses to commit an Assistant message without an open publication stream, so
  every real approval has a frozen `(attempt, turn, message_id)` owner and no
  lenient branch is needed for a state the runtime cannot produce.
- Interaction audit payload bounds are durable-store invariants. Questionnaire
  count, question/header text, option count/label/description/preview,
  question and option uniqueness, answer mode/index/length, approval request
  reason, denial reason, tool-name length, and the canonical lowercase-hex
  form of `arguments_digest` are all checked at the store. The limits live in
  one place, `events::interaction`, which both the coordinator's live
  validation and the store's durable validation call, so they cannot drift and
  a future PostgreSQL backend reuses the same contract.
- A Questionnaire settlement must satisfy the exact requested questionnaire
  contract, not merely carry a response variant: indices are unique and in
  range, single-select answers name one authored option or custom text, and
  multi-select answers name a non-empty unique authored set. Empty submission
  is the distinct decline settlement.

Two ordering rules make the plane observable rather than merely intended:

```text
InteractionRequested                   -> questionnaire is released to a client
InteractionSettled(Submitted/Declined) -> semantic waiter resumes
InteractionSettled(Approved)           -> ToolExecutionStarted -> external side effect
```

The requested fact commits inside the same critical section that admits the
pending entry and strictly before the publication callback runs, so a failed
commit publishes no prompt at all and fails closed before publication. Before
the root admission permit, an exact provider refusal is the ordinary
`Unavailable` contract; it creates no requested audit. After the exact permit
has crossed the publication frontier, however, the coordinator has admitted
semantic publication. A failed reliable `InteractionRequested` route is
therefore a supervised control-path failure, not provider absence: the child
does not receive an `Unavailable` result or continue normally, and the open
requested fact remains historical evidence of the interrupted execution.

The settled fact commits before the semantic waiter is released and before the
responding client is told its response was accepted, so a user-facing approval
response can never race ahead of the durable evidence that the approval
existed. If the reliable `InteractionSettled` route fails after the coordinator
has selected and committed its outcome, the selected outcome is not rewritten
and the route error is not swallowed: the waiter receives an internal control
failure and the owning Agent Loop cannot continue under a broken supervision
path. `InteractionSettled(Approved)` remains strictly before
`ToolExecutionStarted`, but a historical or undiscoverable settled frame never
grants recovery authority.

The hard invariant is that this is audit and nothing more:

> A historical `Approved` interaction is audit evidence only. It never grants
> execution authority after recovery/restart.

Recovery therefore has no interaction dimension at all. It takes the attempt
identity watermark these facts carry and nothing else: it reconstructs no
waiter, republishes no prompt, and never converts an old approval into
permission to run the tool it referred to. A call whose `ToolExecutionStarted`
is absent is simply a call that never started, and recovery settles it with
the ordinary pre-start cancelled canonical result slot. After a restart the
historical identity is durably spent in both directions, so a current runtime
that wants the same tool must reach a **new** live approval under a new
identity.

Payloads stay bounded. A Questionnaire subject is stored by value because the
questionnaire contract bounds every question, option description/preview, and
answer string; an Approval subject
names the call/tool identity and policy reason by value and pins the exact
model-issued argument value by SHA-256 digest, because that value is already
durable by-value in the canonical `ToolCall` the Message Ledger owns — which
is also what makes the pin verifiable rather than decorative. Keypresses,
focus changes, editing state, and TUI presentation details are not interaction
facts and never enter the Journal, so its size stays O(human decisions).

A pending interaction belongs to the already admitted attempt and its pinned
Runtime Resource Snapshot / `CapabilitySnapshot`. While a waiter owns the
attempt, `reload_resources` returns `Busy { reason: Interaction }` and the
complete old generation is retained: an external edit to `AGENTS.md`-style
files, Skills, extension instructions, or Tool configuration cannot change the
pending prompt, the approval subject, the Tool schema, or execution authority
underneath the waiter. Only after settlement and attempt completion may a
reload publish a new generation, and that generation affects a later admitted
attempt only.

The Runtime Client protocol carries the same semantic plane through
`interaction_respond`, typed acceptance/errors, `interaction_pending` and
`interaction_settled` events, and `snapshot.pending_interactions`. Snapshot
plus cursor and subscribe-after-cursor retain the existing repair invariant.
No bound publication provider fails approval closed as `Unavailable`. A loaded
App Server runtime remains capable with zero external attachments; disconnect
does not change pending Approval/Questionnaire ownership or publication admission.
Detaching an attachment does not close admission for future interactions and
does not answer, deny, or cancel an already-published request. A later
attachment can answer a still-live request from the authoritative runtime
projection.

For a parallel tool batch, every call resolves its own pre-tool decision in
canonical call order before any executor starts. A denied or cancelled call
gets exactly one normal Tool Plane result slot and no `ToolExecutionStarted`
fact or executor future. Canonical ToolCall/result order is independent of
response timing. A denied result is typed `ToolExecutionStatus::Denied`, not
executor `Failed`.

Cancellation is a typed canonical result with independent reason and phase:
`BeforeStart` means the accepted call owned a result slot but its
`CallSlot.executor_started` frontier was never crossed; `DuringExecution`
means that frontier was crossed and cancellation won before ordinary
completion. `DuringExecution` does not promise rollback or absence of side
effects. The Agent Loop selects the phase and settles the one result slot;
executors own physical work and cleanup and may report a provisional physical
cancellation status, but the owning scheduler or registry derives and
normalizes the canonical phase. Provider adapters, Runtime Client projection,
and TUI only project the already-authoritative typed fact. The
foreground projection has one at-most-once slot transition: a live execution
settlement closes it when that fact arrives, while a canonical ToolMessage
commit closes an otherwise-unsettled slot (including `BeforeStart`) without
inventing started or physical-completion events. Background execution uses
the same canonical phase vocabulary, with its registry owning the detached
runner frontier.

Foreground execution liveness is owned by the same generic lifecycle, never
by executors (Issue #204). One admitted foreground call runs under the
attempt-frozen `ToolExecutionDeadlinePolicy`: a hard deadline on total
execution lifetime and an optional idle-liveness window, both measured from
the call's executor-start frontier (the one clock read when the lifecycle
admits the invocation to its executor). The idle window additionally
requires the executor's declared `ToolProgressCapability::Meaningful`,
frozen into the prepared invocation at resolution: an executor without
honest progress evidence runs hard-deadline-only and is never
idle-cancelled. Executor progress reports through the existing
`ProgressReporter` seam refresh the idle window only; the hard deadline
never moves. The biased winner arbitration is one explicit linearization
point — attempt cancellation > hard deadline > idle deadline > physical
completion at equal readiness — and a deadline winner is cancellation
intent, not settlement: the lifecycle cancels exactly that call's child
cancellation signal and transitions to the executor's settlement authority.
The `ToolExecutor` boundary splits one started execution into
`ToolExecutionHandle { completion, settlement }` — the physical completion
plane and the independent cancellation/settlement control plane, both
executor-owned. Once intent wins, the lifecycle awaits only the settlement
plane, which runs all rustX-owned local cleanup (kill, wait, reap, join) to
its end and then returns typed `Confirmed`/`Unconfirmed` evidence; it is the
normal settlement mechanism, awaited without a timeout of its own.
`Unconfirmed` means local rustX execution ownership reached its terminal
cleanup boundary — no rustX-owned task or process remains — while
terminality past the external-effect frontier (a remote MCP call, a remote
HTTP operation, an external service's state) stays unprovable; it never
means "the lifecycle stopped waiting". `TOOL_SETTLEMENT_CONTROL_GUARD`
bounds only the wait for a broken executor whose settlement plane never
returns, and its expiry is a settlement control-plane failure, never
settlement evidence. The two `OutcomeUnknown` paths stay type-distinct:
executor-returned `Unconfirmed` journals
`ToolExecutionSettlementObserved { Unconfirmed }`, while guard expiry
journals `ToolExecutionSettlementControlFailed` and never a
settlement-observed fact. The evidence then selects
the canonical status under the Issue #202 contract: proven terminal
settlement after a deadline is `TimedOut`, explicit `Unconfirmed` evidence
or guard expiry is `OutcomeUnknown` — never derived from an unreturned or
dropped execution future — and a proven normal outcome that won the physical
race survives. Canonical settlement is absorbing: the closed call slot
guarantees that no late physical completion, residual executor-owned
physical cleanup, or repeated intent can publish a second result or a
post-terminal fact, so runtime drain transitively waits only for each
admitted execution's canonical settlement, and a per-call deadline never
strands its batch siblings. The durable typed facts of a cancelled call are
ordered `ToolExecutionStarted`, the retained progress facts,
`ToolExecutionDeadlineFired { kind }` when a deadline fired,
`ToolExecutionCancellationRequested { cause }`, exactly one settlement fact
(`ToolExecutionSettlementObserved { certainty }` or
`ToolExecutionSettlementControlFailed { reason }`), and the terminal
`ToolExecutionCompleted` last; the journal is observational evidence and the
canonical ToolResult remains the only outcome authority.

The Bash tool's explicit model-requested `timeout` is tool-owned business
input below this lifecycle: the executor settles it physically (process-group
kill, wait, reap) and reports the proven `TimedOut` itself. There is no
implicit executor-local default timeout — a foreground Bash invocation
without an explicit timeout is bounded by the generic hard deadline, which
drives the same proven physical settlement through cancellation.

The real `ConversationRuntime::shutdown()` path is covered by a deterministic
regression: it observes the Runtime Client pending event, linearizes
`Running -> Draining`, requests cancellation through runtime-owned
`AgentCancellation`, and remains incomplete while a test gate holds the
waiter handoff after pending-map removal. Only after the interaction waiter,
AgentExecution, attempt task, and projection settlement release their counted
authority may the lifecycle publish `Quiescent`.

### 1.3.2 Durable transcript history and paging (FND-05 / Issue #110)

The transcript is a derived read model, not a new conversation owner. The
ownership boundary is:

```text
Message Ledger bodies       canonical User / Assistant / Tool message facts
Publication audits          released non-canonical model output
Event Journal interaction   requested/settled human-decision audits
transcript_order            stable references and ordering only
Conversation Surface        current active model-context working set
Runtime Client / TUI        bounded projection and presentation only
```

`transcript_order` contains only a reference kind, reference identity, and a
monotonic position. Its identity is the composite `(reference_kind,
reference_id)` key: a MessageId, PublicationStreamId, and EventId may reuse
the same opaque string without colliding. The store appends that reference in
the same transaction as the owning durable fact. Reading a page resolves the body from its
canonical owner on demand, so the transcript cannot become a second body
store or an unbounded in-memory vector. Accepted inbound User content is not
displayable until its acceptance transaction commits; adoption into the
Ledger preserves the same durable identity and ordering reference.

Surface and transcript intentionally diverge. Surface owns the finite active
identity/order sent to the model and is replaced by compaction. Transcript
retains the durable readable history of visible canonical messages and audits
after those messages leave Surface. Normal transcript visibility includes
User, Assistant, Tool, and compaction-summary messages. All Context-kind User
messages, including Agent Status, runtime observations, and extension
environment facts, are hidden from normal chat history. The durable position
is allocated before the observation is released; every live visible message
or audit carries that exact `TranscriptCursor`. The Runtime Client event
cursor remains only an observation-stream position and is never a transcript
ordering input.

The Runtime Client snapshot contains a newest transcript page capped at 64
entries. `transcript_page_get` accepts an exclusive `before_cursor` and a
limit from 1 through 256; the response is chronological within the page and
returns the exclusive cursor for the next older page. The caller passes that
`next_cursor` unchanged on the next older-page request: it is the current
page's oldest boundary, not a newest-entry cursor. This transcript cursor is
independent from the live Runtime Client event cursor: snapshot plus
subscribe-after-cursor repairs live projection state, while transcript page
requests repair durable history and never move the live cursor. Every live
transcript-visible observation carries the cursor allocated by the same
durable transaction as its owning fact, so live, snapshot, reattach, headless,
cold reopen, and paging folds have one order. A detached, reattached,
headless, or cold reopened runtime reads the same durable pages and never
enumerates the whole conversation at bootstrap.

Publication audits render as explicitly non-canonical Assistant transcript
items. They are derived from the publication-audit owner rather than the
Message Ledger, and do not imply canonical acceptance or execution.
Incomplete and Unaccepted audits remain distinct. A model-proposed tool call
inside either audit is typed and rendered as proposed, unaccepted, and
unexecuted; it is never a Tool Plane invocation and never implies execution,
a result, or side effects. Historical interaction requested/settled audits
are visible evidence only: recovery never recreates their waiters or grants
authority. Resource discovery and reload likewise produce no transcript item;
old RequestSnapshots retain their exact System/resource bytes, while a cold
reopen loads a fresh resource generation for future requests.

The TUI therefore sends user input through the App Server and renders it
only after durable acceptance. It does not maintain an optimistic semantic
echo or a parallel transcript. Page-up reads older durable pages and merges
live and historical entries by their durable transcript cursors without
moving the live event cursor; identity only rejects the same fact twice.
Historical audits are non-actionable presentation rows.

## Plugins and Conversation state

Plugins are closed Rust-owned capabilities configured by `agent.plugins` or a
complete named Agent's `plugins` table. All default off. `NativeAgentExtensions`
is the internal typed representation; it is not a dynamic plugin API.

Root composition belongs to the immutable configuration generation. A successful
reload publishes Plugin configuration, Tool registrations, policies and context
contributors together. Each Attempt uses its admitted profile. Named Agents
compose independently, without a Root Tool or Plugin ceiling. Child scope
restrictions remain typed admission rules.

Todo and Goal current state belongs to the Conversation, not configuration.
Turning a Plugin off removes its model-facing capability and live presentation;
it does not rewrite canonical historical results or erase its domain state.
The native `effective_plugins` projection reports the published composition;
historical-only inspection does not invent current configuration.


## 2. Layer model

### Layer 0: Domain and protocol types

This layer contains runtime-owned data contracts only:

- Message blocks and content blocks
- Model requests and model events
- Tool definitions, calls, and results
- Runtime events
- Runtime manifest
- Attempt, turn, and capability identifiers

It must not depend on provider SDKs, MCP SDKs, databases, HTTP frameworks, or process implementations.

## 2.1 Implemented Layer 0 contracts (M1)

The canonical contracts defined in M1 live in the `src` module tree as follows:

```text
runtime/identity.rs        strong IDs (ConversationId, MessageId, AgentId,
                           AgentVersionId, AttemptId, TurnId, EventId, ToolId,
                           InteractionId, ToolCallId, ToolExecutionId,
                           McpServerId, SkillId, SkillVersionId, ArtifactId)
                           and CapabilityRevision
runtime/interaction.rs     provider-independent native Approval and bounded
                           Questionnaire requests, typed responses/outcomes,
                           coordinator pending registry, terminal rendezvous,
                           and Runtime Client observation
runtime/cancellation.rs   CancellationSignal: the one runtime-owned
                           cancellation primitive shared by model adapters,
                           compaction, foreground tool execution, and
                           background work; ExecutionCancellation observes
                           its owner and derives one-way child signals
runtime/types.rs           TokenMeasurement, TokenMeasurementSource,
                           CancellationReason, RuntimeError, RuntimeClock
runtime/inbound.rs         ConversationInboundMailbox (per-conversation
                           process-local coordination contract over the
                           durable Pending Inbound Inbox): InboundSequence,
                           InboundItem, InboundBatch, MailboxError
durable/inbox.rs           ConversationStore trait + domain types (InboundDraft,
                           AcceptedInbound, PendingInboundItem, PendingBatch):
                           the backend-independent acceptance/selection/adoption
                           operations, plus the fused `commit_model_turn_start`
                           contract (canonical User context + RequestSnapshot
                           with frozen Effective System Prompt +
                           ModelRequestStarted in one transaction)
durable/sqlite.rs          SqliteConversationStore: the M8 SQLite backend
                           (one semantic authority over Pending Inbound,
                           Message Ledger, Surface revisions, Request
                           Snapshots, Event Journal, and checkpoint metadata)
runtime/continuation.rs   ProviderContinuationState boundary (OpenAI Responses
                           stored/stateless, Anthropic opaque state)
message/content.rs         TextBlock, ImageReference, FileReference
message/types.rs           MessageBlock (User/Assistant/Tool), provenance
                           (UserSource, InboundKind),
                           UserMessageBlock.timestamp (persisted inbound
                           instant; absent for derived compaction summaries),
                           ContentBlockIndex, content enums per role
                           input schema, ToolDefinition with independent
                           execution/concurrency/approval axes,
                           ToolReplayPolicy, ToolOrigin), ModelToolDefinition
                           (the compiled model-facing definition), ToolCall,
                           ToolCallStart, ToolInvocation (stripped/validated
                           caller-neutral invocation with ToolInvocationId), ToolExecutionResult,
                           ToolExecutionStatus, ToolProgress, TruncationState
agent/lifecycle.rs          required PreToolPolicy / PreToolView seam and
                           AttemptLifecycle interaction rendezvous binding
tools/executor.rs          ToolExecutor boundary, ToolExecutionContext,
                           ProgressReporter, ToolRegistry (validating
                           definition/executor registry), PreflightOutcome
                           (both variants carry the registry-resolved ToolId
                           and ToolOrigin)
tools/schema.rs            JSON Schema validation, the reserved __rustx_
                           namespace, the model-facing schema compiler, and
                           reserved invocation metadata extraction
tools/workspace.rs         Workspace: the canonical runtime-owned workspace
                           boundary (canonicalized root)
tools/locator.rs           runtime-owned read locator authority for advertised
                           managed-output paths and unrelated runtime
                           invariants: absolute locators, explicit authorized
                           roots, lexical owning-root determination before
                           canonicalization, same-root canonical-target
                           authority, no symlink escape or cross-root
                           authority transfer
tools/managed_output.rs    ManagedToolOutput: the conversation-owned managed
                           tool-output store: lazy foreground result spills
                           (`results/result_N.txt`, monotonic sequence,
                           `create_new`) and the dispatch-allocated
                           background live-output channel
                           (`tasks/exec_N.output`); owns model-mutation
                           rejection for its runtime-owned namespace
tools/artifacts.rs         ArtifactStore: conversation-owned opaque monotonic
                           artifact ids with streaming spooling (genuine
                           semantic artifacts only — never textual overflow)
tools/environment.rs       ToolEnvironment: the explicit authorized child
                           environment (no wholesale parent inheritance)
tools/background.rs        ConversationBackgroundRegistry: conversation-owned
                           background executions (lifecycle state machine,
                           dispatch ownership commit, cancel-vs-complete
                           linearization, terminal inbound publication,
                           bounded progress snapshots)
runtime/subagent/          SubagentRegistry (conversation-owned one-shot
                           child runtimes: two-stage prepare/commit, driver
                           task as sole process owner, cancel/escalation,
                           exactly-once terminal publication), the bounded
                           framed control IPC, and process supervision
tools/todo.rs              ConversationTodoList: the conversation-owned task
                           list (id allocation, status machine, blocked_by
                           graph validation), staged per ToolResult batch
                           and rebuilt at construction from the newest
                           snapshot the conversation's own canonical
                           `todo` results committed. It also owns the
                           bounded read-only TodoStatusPresentation and its
                           fingerprint — the whole interface the Todo
                           extension offers another extension
tools/runtime.rs           ConversationToolRuntime: the per-conversation
                           bundle of workspace, artifacts, environment,
                           background registry, and — when the Todo Agent
                           Extension is composed — task list, handed to
                           AgentExecution
tools/native/             the native tool plane: one module per native
                           capability (read/, write/, edit/, glob/, grep/,
                           bash/, execution/), each owning its name,
                           description, typed input contract, generated
                           schema, executor, and private helpers;
                           registration.rs owns the NativeToolRegistration
                           and schema generation, input.rs the typed
                           input boundary, support.rs the shared failed/
                           success results and the one atomic file commit,
                           and mod.rs only composes the known native tools
tools/native/search/      the private native-search substrate shared by
                           Glob and Grep: the one workspace file-universe
                           policy (cwd-oriented root resolution, traversal, hidden-file
                           visibility, ignore-file behavior, symlink
                           policy, normalized relative paths, deterministic
                           enumeration) — not a tool, never registered,
                           never a generic search-provider framework
tools/native/bash/        the Bash subsystem: registration (mod.rs), the
                           invocation lifecycle executor (executor.rs), the
                           output capture half (capture.rs), and the
                           per-invocation process supervisor
                           (supervisor.rs) — the supervisor is not a
                           separate tool
tools/mcp/                MCP adapter: protocol-revision negotiation,
                           configured server runtime, paginated discovery,
                           list-change invalidation, canonical calls,
                           progress, cancellation, stdio
                           protocol-corruption observation (`framing.rs`)
tools/python.rs           managed FastMCP package discovery, fingerprint-keyed
                           uv environment preparation, and synthesis of the
                           generic MCP server bindings (`python:<folder>`)
model/types.rs             ModelRequest, ModelUsage, ModelProtocol, and the
                           provider-neutral request boundary. Model-visible
                           runtime context is canonical UserMessageBlock
                           history plus the frozen Effective System Prompt;
                           no semantic context attachment type crosses this
                           layer.
model/catalog.rs           the validated rustx.toml catalog: explicit
                           provider endpoints and credential sources,
                           redacted credentials, model definitions,
                           capabilities, reasoning profiles, bounded compat
model/invocation.rs        opaque requestParams and their shallow-overlay
                           contract, per-protocol protected wire keys,
                           effective-capability intersection,
                           ResolvedModelInvocation, ModelBindingRegistry
model/session.rs           SessionModelConfig (the session's authoritative
                           mutable model state), the summary policy, and the
                           immutable AttemptModelSnapshot
model/fixture.rs           fixture construction for tests over the public
                           catalog path (no runtime behaviour of its own)
local_runtime/             the local conversation runtime process: bounded
                           current runtime configuration, the one composition owner,
                           the startup argument contract, and the stdio
                           serving lifecycle
toml_authoring.rs           the one TOML reader behind rustx.toml and
                           rustx.toml: strict typed TOML, no
                           other relaxation, and no schema of its own
model/finish.rs            ModelFinishReason
model/error.rs             ModelError, ModelErrorKind
model/event.rs             ModelEvent (canonical arm of the adapter stream)
events/types.rs            RuntimeEventEnvelope, RuntimeEvent, AttemptOutcome,
                           AttemptFailure
protocol/manifest.rs       RuntimeManifest and capability/context/limit sections
model/adapter/traits.rs    ModelAdapter runtime-owned interface, ModelStream,
                           ModelStreamItem, and ephemeral ModelStreamProgress
(cancellation lives in runtime/cancellation.rs; model adapters receive
                           the shared CancellationSignal)
model/adapter/validation.rs    deterministic local capability validation
model/adapter/block_index.rs   provider-key to ContentBlockIndex allocator
model/adapter/openai/     OpenAI Chat Completions and Responses adapters
                           (async-openai, custom no-retry HTTP service)
model/adapter/anthropic/  Anthropic Messages adapter (direct HTTP/SSE,
                           no Anthropic SDK)
```

Dependency direction between the modules points inward toward the shared
runtime-owned types:

```text
protocol → model, runtime
events   → message, model, tools, runtime
model    → message, tools, runtime
message  → tools, runtime
tools    → runtime (and the tool plane reuses runtime-owned coordination)
```

Serialization conventions for persistence-facing types:

- Enums use explicit discriminators with stable snake_case values
  (`"role"` for `MessageBlock`, `"type"` for events and content blocks).
- Strong IDs serialize as transparent JSON strings; `CapabilityRevision`
  serializes as a plain JSON number.
- Timestamps are UTC RFC 3339 strings (`chrono::DateTime<Utc>`).
- Durations are integer milliseconds (`duration_ms`, `retry_after_ms`).
- `serde_json::Value` is used only for genuinely arbitrary JSON: JSON
  Schema, tool-call arguments, structured tool output, and opaque provider
  continuation payloads.
- Persistence-facing structures never use `HashMap`; ordering is explicit.

The three execution layers each consume these contracts: the agent kernel
operates on them, the context engine assembles them into provider context,
and the model plane translates them to and from provider protocols.

#### M1 contract corrections discovered by later milestones

`ModelRequest.max_output_tokens` is a required `u32`, not an
`Option<u32>`. Real Anthropic integration proved that an adapter cannot
faithfully represent "no runtime output limit" when the provider requires an
explicit generation maximum (`max_tokens`), and hiding an arbitrary
adapter-local default behind `None` was rejected as hidden runtime policy.
The runtime must therefore resolve an effective output-token limit before
entering the adapter boundary; no adapter-local default exists. This is a
deliberate pre-1.0 canonical correction, not a compatibility shim.

`ContextManifest` gained `context_window_tokens` in M4, and Issue #42 moved
its *ownership*: the context window belongs to the selected catalog model,
not to the process. The current runtime/project configuration supplies the
current `SessionContextPolicy` (reserve tokens, keep-recent target, summary
output cap) for each composition; durable Session state does not persist it.
Each attempt derives its `ContextConfig` from that current policy plus **its
own** immutable model snapshot.
The soft input limit is still
`context_window_tokens - reserve_tokens - max_output_tokens` (checked,
impossible configurations rejected), but an attempt on a 32k model never
plans compaction with a previously selected 128k window.

Issue #42 also retired the universal `ReasoningEffort` enum. Reasoning is a
model-declared *named profile* whose wire behaviour is exactly its configured
`requestParams`; the runtime assigns no meaning to a profile name and
synthesizes no reasoning field. `ModelManifest` therefore carries a catalog
`ModelRef`, the selected `ReasoningProfileId`, and the semantic
reasoning-enabled state.

### 2.2 Attempt settlement invariant

Normally exactly one terminal runtime event is durably committed for an
attempt, and each terminal event carries only the data valid for that state:

```text
AttemptCompleted      finish reason
AttemptCancelled      cancellation reason
AttemptTimedOut       -
AttemptLimitExceeded  exceeded limit
AttemptFailed         normalized AttemptFailure
```

`AttemptCompleted` never carries a failure outcome, and unknown event
payload fields are rejected on deserialization, so contradictory terminal
encodings are impossible by construction. The platform-level `AttemptOutcome`
type maps one-to-one with these terminal events via
`AttemptOutcome::from_terminal_event`. When an attempt fails because a model
request exhausted its retry policy, `AttemptFailure::Model` preserves the
normalized `ModelError` without degrading it to a runtime error string.

The Agent Loop settles its execution state before attempting the terminal
Event Journal append. If that required append fails, no terminal event is
published or fabricated; the typed execution result carries the settlement
candidate and the durable failure, and the owning runtime enters
`DurabilityFailed`.

### 2.3 Streaming assembly identity

`ModelEvent` (and the corresponding `RuntimeEvent` deltas) target content
blocks by the rustX-owned `ContentBlockIndex`: the position of the block
within the ordered `AssistantContentBlock[]` of the message being assembled.
Interleaved text, reasoning, refusal, tool-call, and provider
continuation-state streaming therefore assembles unambiguously without
exposing any provider block id type. Refusal streams as refusal
(`ModelEvent::RefusalDelta`, published as a `RefusalSuffix` publication frame)
and assembles into `AssistantContentBlock::Refusal`, never into plain text. `ToolCallStarted`
carries only the data known at start (`ToolCallStart`: call id, tool id,
name); raw argument fragments stream via `ToolCallArgumentsDelta`, and the
fully assembled `ToolCall` is emitted only at `ToolCallCompleted`.

The provider adapter normalizes protocol events into one `ModelStream` contract.
Its `Event(ModelEvent)` items are canonical output; its ephemeral
`Progress(Generation | Liveness)` items preserve provider-derived execution
progress that cannot yet be represented canonically. `ModelEventAssembler`
receives only the canonical event arm, then retains the canonical `Completed`
event it actually consumed as the sole successful terminal authority and
validates semantic coherence before the Agent Loop can act on the result. Its `finish()` operation
has no caller-supplied terminal override: the assembled turn carries the
consumed event's finish reason and terminal usage. In particular, a completed
turn satisfies the bidirectional tool-call rule: `ModelFinishReason::ToolCalls`
occurs if and only if the assembled turn contains at least one complete
canonical `ToolCall`. An empty `ToolCalls` turn and canonical tool calls under
any other finish reason are explicit `RuntimeError::ContractViolation`
failures, never successful no-tool turns or normalized finish reasons.

### 2.4 Tool execution event identity

Every tool execution event carries the executing tool call identity:
`ToolExecutionStarted`, `ToolExecutionProgress`, `ToolExecutionCompleted`,
and `ToolExecutionFailed` all carry `tool_call_id` and `tool_id`. With
parallel execution, completion order may differ from call order, and each
completion remains attributable to its originating call. `ToolExecutionResult`
itself stays reusable and carries no call identity; identity is attached at
the event and message boundary only.

### 2.5 Message content single source of truth

The durable Message Ledger (M8) is the only authoritative store for canonical
message content. `AssistantMessageCommitted` and `ToolMessageCommitted` are
execution facts that reference the committed message by its stable
`MessageId` and never embed the message body, so the Event Journal never
holds a competing copy.

A committed-message event is inserted in the same `ConversationStore`
transaction as the Ledger body it references. The same rule applies to
`CompactionCompleted` (summary plus exact Surface revision) and
`ModelRequestStarted` (snapshot plus referenced Surface revision). This makes
an orphan reference structurally impossible in the SQLite backend; a failed
transaction exposes neither side. Persist-before-publish then appends the
committed event before any observer or external projection sees it.

### 2.6 Durable Pending Inbound Inbox (Issue #63)

The durable authority split for inbound work:

```text
Pending Inbound Inbox     = accepted / not-yet-adopted inbound durability
                            (the one per-conversation InboundSequence
                            allocator; the acceptance linearization point)
ConversationInboundMailbox = process-local coordination / wakeup only
Message Ledger            = adopted canonical conversational facts
Conversation Surface      = current model-visible ordering/projection
ConversationRuntime       = admission + safe-boundary adoption owner
Event Journal             = execution facts
```

Two linearization points are defined exactly:

1. **Acceptance** ([`ConversationStore::accept_inbound`]): the durable
   per-conversation sequence allocation, the pending record, and any
   producer correlation/idempotency state commit in **one** transaction.
   Producer success is returned only after that commit. The process-local
   wake fires strictly after it and is a liveness optimization — a crash
   between the commit and the wake loses nothing. A successful acceptance
   and the coordinator's `shutdown` have one total ordering: the coordinator
   holds its one state lock across the lifecycle/shutdown decision **and**
   the durable acceptance, so shutdown linearizes either entirely before the
   acceptance (the acceptance then fails with `Shutdown` and commits nothing)
   or entirely after it.
2. **Adoption** ([`ConversationStore::adopt_pending_batch`]): the selected
   finite watermark batch is appended to the durable canonical Message
   Ledger, advanced through Surface Append operations, updates the current
   checkpoint, and removes its pending records in **one** transaction. Crash
   before the commit leaves the items pending; crash after it makes them
   canonical and Surface-visible exactly once, never independently
   re-adoptable.

`select_pending_batch` is a non-destructive finite-watermark snapshot: an
item accepted after the snapshot belongs to the next batch. The durable
store persists the **complete** canonical Message Ledger — adopted inbound
`User` messages, `Assistant` messages, `ToolResult`s, context facts, and
compaction summaries in canonical order — through the prepare →
durable-append → infallible-install seam. A complete `ToolResult` sibling
batch commits atomically (one durable transaction), so a partial tool-result
group can never become canonical. Background terminal notifications converge
on the same acceptance seam with a deterministic producer correlation, so a
retry with the same committed correlation can never publish a duplicate
notification; the durable terminal inbound commits **before** the background
record is exposed as terminal, and a durable publication failure retains the
terminal candidate in an explicit `PublishingTerminal` state rather than
faking `Running`. Retaining the candidate is not the settlement ownership
itself: the background runner drives the production settlement continuation
— publication attempt #1 inside `finish`, then exactly one registry-owned
retry under the same deterministic correlation (exactly-once even when
attempt #1 committed durably but observed an error). When that bounded
budget is exhausted, the candidate stays retained and the failure is
reported to the owning `ConversationRuntime` through the narrow
`BackgroundDurabilityFailureSink` seam, which places the owning runtime
into its explicit `DurabilityFailed` state; no runtime-owned execution can
leave its production settlement path without a guaranteed terminal
publication or that explicit degraded outcome. A standalone never-claimed
registry may retain an observable `PublishingTerminal` candidate when its
bounded budget is exhausted because it has no owning `ConversationRuntime`
durability-health sink.

A durable Ledger append is **not** by itself a resumable runtime safe
boundary. The durable store's complete Ledger ordering is the canonical
truth of committed message facts; normal live runtime resumption additionally
requires a structurally complete current Surface boundary. The conversation
domain's `recovery_safety` predicate answers that question fail-closed for an
incomplete tool turn (an `Assistant` tool call without its committed
`ToolResult` sibling). A compaction summary can no longer be durable without
its exact Surface Replace and checkpoint because M8 commits those facts in
one transaction.

M9a supersedes the M8 restart *gate* with a restart *contract*: an incomplete
tool turn is now repaired from durable evidence rather than refused, and
`recovery_safety` becomes the checked **post-condition** of reconciliation
instead of a construction-time veto. It remains the live admission guard, so a
failed tool-result batch during normal execution still fails closed. See
[Recovery model](#7-recovery-model).

### Layer 1: Agent kernel

The kernel owns deterministic execution semantics:

- Attempt state machine
- Turn lifecycle
- Model -> tool -> model loop
- Tool batch ordering
- Turn-boundary inbound message draining
- Attempt termination rules
- Retry and compaction decision points
- Typed lifecycle interception coordination (`PreStepPolicy`,
  `ToolResultObserver`), the deferred context buffer, and the split between
  lifecycle *timing* and semantic *ownership*

The kernel operates only on rustX canonical types and interfaces.

The runtime inbound boundary (`src/runtime/inbound.rs`, Layer 0) is
coordination only. Since Issue #63 the [`ConversationInboundMailbox`] is the
narrow acceptance/publisher seam over the **durable Pending Inbound Inbox**
(`src/durable`): it validates eligibility and lifecycle, durably accepts
through the [`ConversationStore`], then publishes the process-local wake and
observation. It owns **no** sequence allocator and **no** payload queue. The
kernel's `AgentExecution` selects and adopts exactly one finite batch per
safe turn boundary. Canonical history is the durable Message Ledger and the
Event Journal records execution facts; the mailbox is not a scheduler,
supervisor, or persistent service layer.

#### M3 implementation (agent loop)

The M3 implementation freezes the agent-loop boundary in `src/agent` and
the tool execution contract in `src/tools/executor.rs` (the provisional M3
`Tool` trait was replaced by the canonical M5 [`ToolExecutor`] boundary):

```text
canonical input state
        |
ModelAdapter (canonical ModelRequest in, ModelStreamItem stream out)
        |
ExecutionStateMachine: Idle -> RunningModel -> WaitingForTool -> RunningModel -> Completed
        |
ModelEventAssembler: stream validation + ordered AssistantMessageBlock assembly
        |
ToolRegistry preflight: resolve -> extract -> strip -> tool-owned normalize -> validate -> dispatch
        |
deterministic scheduling phases (sequential barriers, parallel groups)
        |
        durable Event Journal facts, ending in one terminal event when its durable append succeeds
```

The loop owns execution semantics, message assembly, tool execution,
continuation state, cancellation observation, and runtime-event emission.
The durable Event Journal owns historical execution facts; the observer is
only the live projection seam. Adapters own provider protocol translation only; the validating
[`ToolRegistry`] pairs canonical [`ToolDefinition`] values with
[`ToolExecutor`] implementations and never falls back id-first.
Continuation state propagates losslessly without fabrication, cancellation
always settles as a terminal cancellation candidate, and a normally settled
attempt commits exactly one terminal `RuntimeEvent`. A failed terminal append
is an explicit durable failure, not a fabricated event. See
`docs/agent-loop.md` for the full boundary description.

The Agent Loop test suites drive the loop with scripted fixture models and
tools (`tests/support/fake.rs`), assert behavior through the
recorded `RuntimeEvent` trace and the platform `AttemptOutcome`, and
reconstruct execution phases from traces and durable audits
(`tests/common/mod.rs`). See `tests/README.md` for the full test
architecture.

### Test seams are not published API

Substituting a runtime-owned dependency is a `#[cfg(test)] pub(crate)`
seam, never a published item:

```text
ModelBindingRegistry::new         one binding path; builds the three
                                  supported protocol adapters directly
ContextRuntime::for_attempt       one context-runtime constructor; derives
                                  the summarizer from the frozen snapshot
```

An external test binary can only reach `pub` items, so a seam usable from
`tests/*.rs` is necessarily a seam a consumer can call; `#[doc(hidden)]`
hides it from documentation without removing it. The suites that need a
scripted `ModelAdapter` or a scripted `ContextSummarizer` therefore compile
into the crate's own test build through `src/lib.rs`, with their sources
under `tests/` so `src/` carries production code only. Compilation placement
is not the semantic class: the deterministic contract suites live under
`tests/scripted/` (`scripted_suites::`), while in-crate suites whose
invariant is a real OS/process boundary live under `tests/boundary/`
(`boundary_suites::`) and are selected by that prefix in CI. The
integration targets under `tests/*/main.rs` use published API exclusively;
fixtures shared by both live in `tests/common/`, and fixtures that need a
seam live in `tests/support/`. `tests/README.md` documents the
domain ownership, the class/placement model, and the target topology.

### Three provider fixtures, three bounded purposes

```text
tests/support/model.rs             a scripted injected `ModelAdapter` behind
                                   a validated catalog binding. Internal
                                   state machines and units that need no
                                   network and no provider boundary.

tests/common/mod.rs FixtureServer  a raw Rust HTTP/1.1 fixture. One adapter
                                   in isolation: request serialization,
                                   stream parsing, error normalization,
                                   one-attempt/no-retry. No Agent Loop.

test-support/fake-provider         the canonical external provider-emulation
                                   boundary. Composed Agent Loop conformance
                                   across the real runtime and a real
                                   external provider process.
```

The third is the one that decides what "conformance" means. It is an
external Python 3.12 process (managed by uv, never a production runtime
dependency) that speaks the real HTTP/SSE provider protocols, and the
`conformance` integration target (`tests/conformance/`) composes the real
`LocalConversationRuntime` against it:

```text
test driver -> real catalog, binding, adapter, HTTP client, stream parser,
               Agent Loop, context engine, tool runtime, capability plane,
               Runtime Client projection
            -> real HTTP + SSE
            -> the scripted external provider
```

Nothing in rustX is substituted there — no fake adapter, no fake tool, no
fake Skill runtime, no second Agent Loop. Scenarios are strict ordered
scripts: request *N* meets step *N*, an unexpected or extra request fails by
default, and an unconsumed step fails the process. Race-sensitive tests are
ordered by named provider-side gates and an observation barrier rather than
by sleeps: a driver waits until the provider provably reached a point,
performs its runtime action, and releases. `test-support/fake-provider/README.md`
documents the process, control, and scenario contracts.

The lower two fixtures are retained deliberately. Routing an adapter
translation test through an external process, a Python toolchain, and a
scenario definition to assert one JSON field would add cost without adding
truth. They are not, however, an implementation of composed conformance: a
test that exercises the Agent Loop, the context engine, the tool runtime, or
the capability plane belongs on the external boundary.

The Issue #22 inbound batching integration is canonical:
`ConversationToolRuntime` owns the one conversation inbound mailbox, and at
every safe turn boundary the loop performs exactly one finite
watermark-bounded drain of `tool_runtime.mailbox()` and appends every
drained message as its own canonical `UserMessageBlock` before the next
model request. The loop and the background runtime provably share one
mailbox: an `AgentExecution` over a tool runtime of a different
conversation is rejected structurally at construction. Mailbox draining
adds the safe-boundary cancellation-before-selection rule; observable
cancellation before every model turn is a generic Agent Loop invariant for
all executions. See `docs/agent-loop.md` section 9 for the full boundary
description.

### Layer 2: Context engine

Issue #54 fixes the conversation boundary used by the context engine:

```text
System / User / Assistant / Tool
        ↓ canonical roles
Message Ledger
  append-only immutable facts
        ↓ current active MessageIds only
Conversation Surface @ SurfaceRevision
  sole authority for active identity, order, and visibility
        ↓ keyed reads of the finite current Surface
Context Engine
  projection, token pressure, retention, and compaction planning
```

The Context Engine owns only the finite projection of canonical conversation
and its token/retention/compaction behavior. Context Assembly, request
admission, RequestSnapshot creation, and provider translation are owned by
the Agent Loop and model plane respectively.

Compaction appends one canonical `User` message with
`UserSource::Runtime` and `InboundKind::CompactionSummary`, then applies one
complete-message Surface `Replace`. It never deletes or mutates Ledger facts.

#### Current context implementation

The Issue #55 implementation freezes the boundaries in src/context and
src/agent/execution.rs:

```text
claimed inbound
    ↓ finite ContributorInputSnapshot
ContextAssembly (native + certified extension proposals)
    ↓ validated AcceptedContext
Agent Loop staging (scratch validation, prepared canonical commits;
                    no durable effect)
    ↓
cancellation-vs-start arbitration (attempt start gate held; M9b)
    ↓ commit_model_turn_start: one transaction
canonical request-scoped User context + Surface state/reference +
RequestSnapshot (including the frozen Effective System Prompt) +
ModelRequestStarted
    ↓
ModelAdapter → provider
```

The engine is a deterministic pure function of the current Surface, keyed
Ledger results, tool definitions, the exact Effective System Prompt, and
observed provider usage: the same inputs always produce the same projection,
plan, and estimate. It owns no provider knowledge — token estimation is
pluggable (`TokenEstimator`, with a default
`ceil(bytes / 4)` formula), and the engine holds no model catalog.

Its configuration is split by ownership. The session owns the static
`SessionContextPolicy`; the *window* comes from the attempt's immutable model
snapshot. `ContextRuntime::for_attempt` derives one engine per attempt from
those two inputs, so a session model change between attempts changes the next
attempt's compaction arithmetic and never the running one's.

Key contracts:

- `ContextProjection` contains only complete canonical messages in current
  Surface order; it never creates a partial Assistant projection.
- Canonical history has only conversational User, Assistant, and Tool roles.
  `Assistant` owns `ToolCall` identity and arguments; `Tool` owns the result
  and references `ToolCallOccurrenceRef` (Assistant MessageId + block index),
  retaining `ToolCallId` only for provider correlation. A runtime compaction summary remains a `User`
  message with `UserSource::Runtime` and
  `InboundKind::CompactionSummary`.
  System authority is request-time state and therefore cannot be retired or
  resurrected by Surface replacement.
- Token measurements carry explicit provenance
  (`ProviderReported`/`ProviderAnchored`/`Estimated`), and estimates never
  become provider usage. A provider-reported `input_tokens` applies as
  `ProviderReported` only to the exact measured projection (deterministic
  fingerprint). It additionally applies as `ProviderAnchored` to any request
  context the measured one is an ordered **prefix** of, under unchanged
  non-conversation input (Effective System Prompt and tool definitions):
  the measured prefix keeps the provider's number and only the canonical
  messages appended since are estimated. A whole-conversation estimate
  compounds estimator error over every message ever sent, so anchoring is
  what keeps the soft-limit decision trustworthy on a long conversation; a
  compaction Surface rewrite destroys the prefix and the measurement is
  refused outright rather than patched with a guessed delta.
- `TokenMeasurement` and `TokenMeasurementSource` are Layer 0 value contracts
  owned by `runtime/types.rs`. The Context Engine owns the estimator,
  provider-observation validity, provenance application, and compaction
  accounting behavior in `context/tokens.rs`; it does not own the shared
  measurement data type.
- Cut selection is structural: a deterministic index of tool-call/result
  edges rejects orphan tool messages and never separates a call from its
  result. A candidate is always a whole-message span.
- `SurfaceRevision` is a stable reconstruction reference within one live
  conversation lineage. Historical Surface operations are retained for exact
  replay; later Appends and Replaces never change an earlier revision.
- Normal projection, planning, and compaction read only current Surface
  identities and keyed Ledger bodies. They do not enumerate the Ledger or
  scan the historical Surface operation log; current replacement generation
  is O(1) head metadata.
- The `ContextSummarizer` service is provider-neutral; the production
  `ModelBackedSummarizer` issues a canonical one-off `ModelRequest` (no
  tools, no Agent Status, no Skill catalog, no continuation) through the
  `SummaryRequest::model_input()` assembly shared with the planner. That
  assembly renders the retired span as a bounded plain-text transcript —
  truncating tool results, replayed reasoning, and tool-call arguments with an
  explicit notice — rather than embedding the canonical JSON encoding, so the
  summary request is always smaller than the history it replaces. The summary
  input limit covers the fixed instruction, the rendered transcript, and the
  canonical User wrapper, and is derived as the summary invocation's own
  effective context window minus the session reserve minus its output budget,
  never the primary model's window, through the
  existing `ModelAdapter` boundary. A summary model rejecting that request as
  oversized replans the compaction against a halved summary input budget
  (bounded and strictly decreasing) instead of failing, and a compaction
  recovering from a primary context overflow scales the soft input limit — and
  only that limit — by the measured `EstimateCorrection` for the rejected
  request. The correction never crosses into the summary input limit, same
  summary model or not: it measures one primary request, whose deviation can
  come from the continuation, the tool schemas, or the effective system
  prompt, none of which this request carries. The summary budget is bounded
  by the summary model's own rejection. It is constructed from the attempt's
  *frozen summary policy*, never from an independently injected summarizer:
  in `session` mode that is the attempt's own primary invocation, in
  `explicit` mode a separately resolved catalog model. The context plane's
  summary output safety cap is applied through the runtime-owned protected
  max-output field and never by mutating a reasoning profile or a
  request-parameter object.
- Primary model requests retain the attempt-pinned Runtime Resource Snapshot,
  CapabilitySnapshot, Effective System Prompt, active Surface, Tools, and
  compatible primary continuation. The compaction summary is a separate
  one-off side request containing only runtime-owned summary guidance and the
  exact planned retired messages. It does not inherit primary System/project/
  Skill/extension guidance, Tools, continuation, prompt-prefix/cache
  continuity, or Agent Loop execution. The guidance requires one fixed
  structured Markdown contract (Goal, Constraints & Preferences, Progress,
  Key Decisions, Next Steps, Critical Context), and the committed summary
  carries typed cumulative file-operation metadata extracted from the retired
  span's canonical native Read/Edit/Write calls (Issue #140).
  Runtime/Agent Status observations remain historical evidence and are never
  reconstructed as live state from summary text.
- The mandatory progress rule (coverage advances and projected estimate
  strictly decreases) is the anti-loop invariant; successful compaction
  invalidates the pending provider continuation, and
  `ContextWindowExceeded` is recovered through exactly one bounded
  compact-and-retry
  (`MAX_CONTEXT_OVERFLOW_RETRIES_PER_MODEL_TURN = 1`).
- Automatic overflow recovery and idle manual compaction call one canonical
  pipeline for planning, summary generation, exact fit validation, durable
  commit, and hot-state installation. Manual compaction freezes the current
  model/context/capability inputs at admission, including tool definitions and
  capability-derived Skill guidance and project instructions in the
  non-retirable Effective System Prompt used for fit accounting. These values
  come from the attempt's pinned Runtime Resource Snapshot; compaction never
  rediscovers them. It checks out the sole `ConversationState`;
  manual completion is client-visible only after that state is restored and
  the maintenance slot is clear. Pending inbound admits after restoration. It
  owns no attempt identity and is rejected while an attempt or another manual
  compaction is active.
- The Ledger remains immutable historical facts, the Surface is the active
  model working set, and RequestSnapshot is the exact historical request-time
  authority for System bytes and Tool definitions. Runtime Resource Snapshot
  is process-local executable authority, not durable compaction state;
  compaction never reloads it. Explicit reload and cold recreation remain
  separate lifecycle boundaries.
- Agent Status is an optional delivery opportunity, not an automatic emission
  rule. One logical primary step owns one finite
  `AgentStatusOpportunitySet`; its independent FreshInbound and PostToolBatch
  members may coexist. At preparation, execution freezes one finite Pre-Status
  Surface from active identities plus keyed Message Ledger hydration, samples
  the clock once, and captures one immutable authoritative Background and
  committed Todo snapshot. The closed engine evaluates each interested module
  once against that set, then admits any contributing sections as one
  canonical `UserSource::Runtime` context message with
  `InboundKind::Context(ContextKind::AgentStatus(metadata))`. The metadata is
  the durable typed membership/timestamp descriptor tied to that canonical
  message; neither the engine nor a projection parses renderer text. A
  complete tool batch marks PostToolBatch only after its canonical ToolResult
  batch commits; the marker is attempt-local and never creates or prolongs a
  model turn. RuntimeToolObservation remains a separate producer and precedes
  AgentStatus in Context Assembly. If no module contributes, no empty status
  message is emitted. Overflow compact-and-retry reuses the accepted
  generation without a second Surface scan, clock sample, authoritative
  capture, or trigger evaluation.
- Todo status reads only the bounded presentation `ConversationTodoList`
  derives from its own `committed()` snapshot, and only when the Todo
  extension is composed at all. The derivation happens at the runtime capture
  boundary — `ConversationToolRuntime::todo_status_presentation`, beside the
  owner — so the strongest Todo value the Agent Status engine can receive is
  `Option<TodoStatusPresentation>`: a finite immutable value carrying its own
  Todo-owned fingerprint. Production `context/status.rs` names no
  `ConversationTodoList`, `TodoSnapshot`, or `TodoWriter` at all, so Agent
  Status cannot read, derive, or influence Todo state; it decides only whether
  and how often to show what Todo already decided. It shows a
  bounded deterministic view of actionable tasks and uses semantic key
  `active_actionable` plus a SHA-256 fingerprint of that bounded view. A
  durable latest-emission head suppresses an identical fingerprint while
  fewer than four later newly committed first requests of logical primary
  model steps follow the reminder's store-assigned origin, then permits it
  again at exactly four. Changed state is eligible at the next opportunity.
  The head is updated atomically with the canonical status message and its
  `AgentStatusEmitted` fact at model-turn start. The bounded
  `todo_progress_sequence` advances once for each successful
  `retry_number == 0` start; same-start context/status, Time, Background,
  RuntimeToolObservation, compaction, and overflow retries do not advance or
  reset it.
- The initial-turn trigger is an explicit execution mode, never an `Option`
  used as a status switch: `AgentExecutionRequest` carries one
  `InitialTurnTrigger` — `FreshInbound(FreshInboundTurn)` makes validation and
  fresh-inbound compaction protection mandatory and offers the optional Agent
  Status opportunity; `Continuation` expresses an intentional pure
  continuation with no new inbound turn and therefore no Agent Status on the
  first request. There is no legacy no-context execution path. A settled tool
  batch adds the independent attempt-local PostToolBatch member to the next
  already-existing continuation step; one logical primary step owns at most
  one generation.
- A `FreshInboundTurn` is ordered according to canonical history/inbound
  sequence: `validate_against` requires the referenced messages to occur in
  strictly increasing canonical position in `message_ids` order
  (`OutOfCanonicalOrder` otherwise); the runtime never sorts or reinterprets
  a caller-supplied turn order.
- There is no provider registration seam. The engine validates the
  rustX-owned `Time <-> Temporal`, `Background <-> BackgroundExecution`, and
  `Todo <-> Todo` mapping, and a capture, evaluation, or validation failure
  quarantines only that module for the rest of the attempt. Surviving modules continue;
  quarantine is not persisted and a new attempt retries the module. These
  optional failures never become a context-preparation failure or alter model
  request count.
  An overflow whose recovery compaction fails still preserves the normalized
  `ContextWindowExceeded` as the final model failure with the compaction
  diagnostic carried by `CompactionFailed`.
- A fresh inbound turn that has not been observed may never be compacted
  away; when preserving it makes the projection impossible, planning fails
  with `CannotFit` rather than summarizing the unobserved instruction.

#### CFG3 generations, Skills and external sources

The complete [configuration generation](configuration.md#reload-and-frozen-work)
contains Providers, Models, policy, Root profile, resource catalogs, provenance,
source revisions and prepared finite demand. Off-side preparation cannot change
published state. The coordinator lock publishes one complete resource snapshot;
failure leaves the exact prior snapshot authoritative. Owned active work returns
busy. Admitted work retains its snapshot; subsequent admission uses the new one.

The two Skill roots are `~/rustx/.agents/skills` and
`<workspace>/.agents/skills`. Whole-package shadowing occurs before validation.
Selection is prompt visibility (`"all"`, exact names, or `[]`), not filesystem
access control. Guidance prints absolute root paths and selected names and
descriptions, never every absolute package path. Bodies remain progressively read.

MCP and Python discovery is inert. Root-selected demand prepares at composition;
child demand prepares at child admission. Workflow demand is owned by Workflow
admission. Definitions and source credentials are frozen together; no lower
shadowed definition can supply a missing member of a higher definition.


#### Issue #56 typed lifecycle interception

The Agent Loop remains the lifecycle owner. Issue #56 adds exactly two
phase-specific typed seams, carried by one required immutable
`AttemptLifecycle` value (`src/agent/lifecycle.rs`) per attempt:

```text
Context Assembly (deferred + native + extension proposals)
        |
PreStepPolicy               Enter | Reject(reason)
        |
staging (scratch validation, no durable effect)
        |
cancellation-vs-start arbitration   <- the one linearization point
        |                             (start gate held across check + commit)
commit_model_turn_start -> canonical User context + Ledger/Surface +
                           RequestSnapshot (frozen Effective System Prompt)
                           + ModelRequestStarted, in one transaction

Assistant(ToolCall A, ToolCall B) committed
        |
execute, settle every CallSlot, commit ToolResult A then ToolResult B
        |                                <- batch structural settlement point
cancellation checkpoint    <- before each observer, and again once it settles
        |
ToolResultObserver pass, in (canonical ToolCall order, producer order)
        |
validate count + content                 <- observer transaction boundary
        |
stamp the observer's bound producer reference
        |
Agent-Loop-owned deferred buffer (transient, not history)
        |
next Context Assembly -> resolve producer -> lane + provenance
        |
PreStepPolicy -> admission -> canonical User context, owned by its producer
```

`AttemptLifecycle::inert()` is the identity configuration, so no execution
path branches on whether a seam is attached. The `ConversationRuntime`
currently constructs the inert configuration, exactly as it constructs
`ContextRuntime::for_attempt` without certified contributors — a configured
owner arrives with the consumer that needs it, not as speculative plumbing.

**Lifecycle timing and semantic ownership are separate concerns.** The Agent
Loop owns *when* a proposal becomes eligible: "post-tool" means its owning
tool batch settled, so it enters the next primary step rather than this one.
Context Assembly owns *who* the fact belongs to: every staged proposal carries
the `DeferredContextProducer` the loop stamped from its observer's binding —
never from anything the observer returned — and assembly resolves that
reference before deriving lane, `UserSource`, and `ContextKind`, through the
same table it applies to that owner's request-time proposals. There is no rule
turning post-tool proposals into native runtime context: a certified extension
(#58) producing deferred post-tool context keeps its extension identity,
provenance, and lane.

**Binding is not admission.** `ContextAssembly::register_extension` is the one
semantic identity/provenance/attestation authority. The lifecycle seam exposes
only `with_native_tool_result_observer` and
`with_extension_tool_result_observer`, and the latter takes a logical key that
any caller can construct — a reference, not a credential. At assembly time the
native producer resolves to the rustX-owned runtime observation owner, and an
extension producer resolves to the matching **registered** extension, using
that registration's own generation and attestation. An unregistered key fails
the assembly with `ContextAssemblyError::UnregisteredContributor` before
admission: no lane, no `UserSource::Extension`, no synthesized generation. A
certified extension that only defers still resolves to its authoritative
generation. The lifecycle seam therefore cannot become a second registry.

`PreStepPolicy` observes the final immutable `AcceptedContext` and returns
`Enter` or `Reject`. It has one owner per attempt rather than a chain — a
chain would require a second ordering model on top of the Issue #55
lane/identity order, and no consumer needs several independent admission
decisions. It is the single downstream authority every proposal converges on,
so a rejection proves no proposed dynamic context committed, no Surface
revision advanced because of it, no `RequestSnapshot` was frozen, and no
provider request started. It owns no cancellation: a pending bounded
evaluation settles and the generic checkpoint still decides admission.

`ToolResultObserver` receives an immutable `ToolResultObservation` of one
finalized result — canonical batch position, `ToolCallId`, registry-resolved
`ToolId`, typed `ToolOrigin`, the committed `ToolExecutionResult`, and an
`ObservedToolInvocation` carrying the resolved `ToolInvocationMode` and the
**validated business arguments** of the call. The arguments are needed because
a result under-determines the fact it describes: native Read returns content,
while the path lives only in the invocation, and re-deriving it from history
would build a second drifting authority. They are read-only, metadata-stripped
and provider-payload-free, and absent entirely for a preflight-rejected call
that never resolved an invocation. The model-facing tool name is deliberately
absent, so recognizing the native rustX Read capability is a typed identity
question (`tool-read` + `ToolOrigin::Builtin`) rather than a name comparison;
an MCP tool (a managed Python package's tool included) publicly named `read`
can never be confused with it.
Both `PreflightOutcome` variants carry the registry-resolved identity and
origin from the same resolved `ToolDefinition`.

Observers are bound to a `DeferredContextProducer`, at most one per semantic
owner, so a native runtime owner and one or more certified extensions can each
own deferred context about the same settled call. They are invoked and ordered
by logical producer, giving the deferred order key `(ToolCall batch position,
producer identity, proposal FIFO)` with no registration-order term and no new
ordering model.

An observer returns bounded `UserMessageProposal` values only — not the full
`ContextProposal` vocabulary. A settled tool batch is a conversational fact,
and the only concrete requirement (including #58's `PostToolUse
additionalContext`) is deferred conversational context, so this seam cannot
change the Effective System Prompt of the following turn. That is enforced by
the return type, not by a runtime check.

The bounded return value is checked at the **observer transaction boundary** —
per-observation count against the established `MAX_PROPOSALS_PER_CONTRIBUTOR`
limit, running attempt total, and per-proposal content — before a single
proposal is staged, so an unbounded observation is rejected where it happens
rather than one step later.

Cancellation ownership stays with the Agent Loop: it is checked before each
observer starts and again once that observer settles, before its return value
is consumed. An in-flight bounded observation is allowed to settle, but once
cancellation is observable no later observer starts and neither an observer's
success nor its failure can decide the terminal outcome.

Any failure or cancellation in the pass discards every proposal of that pass
and clears the buffer, leaving no partial deferred state. The buffer is not a
second transcript, ledger, or Surface, and the observer is not a privileged
committer: a later pre-step rejection or cancellation prevents the deferred
context from ever becoming canonical.

Tool-execution wrappers/middleware, post-tool result replacement, pre-tool
argument or identity rewriting, generic question/form frameworks, generalized
permission/risk policy, subagent lifecycle observation (#60), and
turn-stopping/forced continuation are intentionally absent. The bounded
native Approval and Questionnaire seams are implemented by M9.2/#100 above; they do
not expand into those frameworks.
`docs/agent-loop.md` section 4.3 carries the full authority matrix.

#### M5 implementation (native tool plane)

The M5 implementation freezes the canonical tool plane boundary in
`src/tools` and replaces the provisional M3 `Tool` trait:

```text
canonical ToolDefinition (tool-owned schema + three policy axes)
        |
validating ToolRegistry (definition + Arc<dyn ToolExecutor>)
        |
preflight: resolve -> extract reserved metadata -> strip -> tool-owned business-argument
            normalize -> canonical JSON Schema validate
        |
ToolInvocation (caller-neutral identity + validated business arguments + resolved mode)
        |
shared native invocation -> ToolExecutor::start(ToolInvocation, ToolExecutionContext)
        |
ToolExecutionHandle (completion and physical settlement planes)
        |
ToolExecutionResult
```

The three policy axes are independent:

- [`ToolExecutionPolicy`] (`ForegroundOnly` / `BackgroundOnly` /
  `ModelSelectable`) decides ownership and settlement: foreground work is
  attempt-owned and physically cancellable, background work is
  conversation-owned and detached after accepted dispatch.
- [`ToolConcurrencyPolicy`] (`Sequential` / `Parallel`) decides scheduling
  within one tool-call batch: a `Sequential` invocation is an exclusive
  barrier, adjacent `Parallel` invocations run as one group.
- [`ToolApprovalPolicy`] (`Never` / `Always`) decides whether an eligible
  invocation publishes a native Approval before the executor starts. The
  runtime `ApprovalMode` is a separate control-plane override: `Policy`
  consults this axis and `FullAccess` makes effective approval `Never` only.

The canonical input schema is tool-owned and never mutated. For
`ModelSelectable` tools the model-facing compiler decorates a clone with the
required top-level `execution_mode` field
(`{"type": "string", "enum": ["foreground", "background"]}`, carrying a
model-facing description of the ownership decision) and appends a
runtime-owned reminder to the compiled description. The contract is:

```text
ModelSelectable
    ⇒ the model must explicitly choose execution_mode per invocation
    ⇒ preflight resolves ownership once
    ⇒ the runtime strips execution_mode
    ⇒ the executor never sees model-facing runtime metadata
```

The runtime extracts the field, resolves the canonical mode, strips it, and
validates the remaining business arguments against the original schema
before dispatch. A missing or invalid `execution_mode` is a deterministic
preflight rejection carrying the exact retry the model needs; it is never
defaulted to foreground. `ForegroundOnly`/`BackgroundOnly` definitions are
compiled verbatim — no synthetic field is injected and ownership resolves
from the fixed policy alone.

`ModelSelectable` is the one policy under which rustX must *write into* a
tool's root schema, so it is the one policy that constrains the root's shape.
Under it a canonical schema must match the **decoratable root profile**: the
root object's instance semantics are owned entirely by

```text
type   properties   required   additionalProperties
```

alongside any purely descriptive root keyword (`$schema`, `$id`, `$comment`,
`$defs`/`definitions`, `title`, `description`, `default`, `examples`,
`deprecated`, `readOnly`, `writeOnly`). Every other root keyword is refused,
whatever draft introduced it.

One further rule reaches past the root. Decoration is an *in-place* edit, so
it is only sound while the root schema is the sole description of the root
instance — and a reference can re-enter the decorated root from any depth. A
schema whose `child` property is `{"$ref": "#"}` would make the injected
selector propagate into nested business objects, so `$ref`, `$dynamicRef`,
and `$recursiveRef` are refused throughout a `ModelSelectable` canonical
schema. rustX refuses them outright instead of resolving URIs to decide which ones
reach the root: that decision is a JSON Schema reference resolver, and this
contract is meant to be checkable by inspection. The scan descends only
through positions JSON Schema defines as carrying subschemas, so a `$ref` key
that is really a property name (under `dependentRequired`, or the Draft-7
`dependencies` list shape) or annotation data under an unrecognized keyword
is left alone rather than misread as an applicator. Apart from references,
nested subschemas stay unrestricted — a business property may hold
composition, cardinality assertions, and an `execution_mode` of its own.

This is an allowlist on purpose. Injecting a required `execution_mode`
property changes what the root instance must look like, so *any* root
assertion rustX does not understand can silently contradict the injection —
`maxProperties` capping the object below the new required count, a root
`const`/`enum` pinning the whole object, a Draft-7 `dependencies` demanding
the stripped selector, a composition branch that never learned about it.
Every one of them produces the same fatal outcome: the tool registers, the
schema compiles, and no correct model call can ever exist. Enumerating
hazards could never be proven complete, so the profile enumerates what is
*safe* instead.

On top of the profile, rustX rejects a **claim on the reserved name**:
`execution_mode` declared in root `properties`, demanded in root `required`,
or both. This check is separate because `properties` and `required` are
inside the profile. The bare `required` entry matters as much as the declared
property — the runtime strips the selector *before* the canonical schema
validates anything, so such a tool would register successfully, receive a
perfectly correct model call, and reject it forever.

Every error tells the human to rename the business field, flatten the root to
the profile, inline the reference, or choose a non-`ModelSelectable` policy.
rustX never renames, shadows, merges, or reinterprets a collision.

Together the three rules give compilation a provable contract rather than a
safe-looking root syntax — the **projection equivalence** that "clone and
decorate the root" actually claims:

```text
canonical(B) ⇔ compiled(B + top-level execution_mode)
```

For any business arguments `B` and either mode the canonical and compiled
schemas must agree, and any invocation the compiled schema accepts must
satisfy the canonical schema once the top-level selector is stripped. Each
rejection class closes one way of breaking it: a root assertion decoration
contradicts, a claim stripping can never satisfy, and a reference that
carries the injected property into nested objects — which breaks the
equivalence in both directions at once.

None of the three rules applies under `ForegroundOnly`/`BackgroundOnly`,
which receive no injected field: an arbitrary composed, reference-heavy root
stays valid there, `execution_mode` included. That scoping is load-bearing —
the native `ask_user` questionnaire is a normal closed root object and MCP
servers may ship arbitrary JSON Schema — so the policy-unaware
`validate_canonical_schema` must stay permissive. The contract therefore lives
in the bounded layer that owns both the effective policy and the compiled
model-facing schema.

Separately, the `__rustx_` top-level property namespace remains reserved for
other runtime concerns under every policy: no canonical schema may claim one
and no invocation may carry one. Tool-owned argument normalization, where
present, runs between stripping and canonical validation. Native Edit's known
malformed argument spellings are handled there; provider adapters and the
Agent Loop remain unaware of them. `ModelRequest.tools` carries the compiled
[`ModelToolDefinition`] values only — provider adapters translate them
verbatim and never decide execution semantics, and no tool (Bash included)
implements `execution_mode` handling of its own.

The registry is a correctness boundary: duplicate `ToolId`s, duplicate
model-facing names, empty identities, invalid or non-root JSON Schema,
reserved `__rustx_*` collisions, invalid policy combinations, and
background-capable `execution` registrations are rejected; a
canonical call whose id and name disagree is a contract violation. Tool
definitions reach the model in deterministic registration order, and the
context engine accounts the exact compiled definitions.

One conversation owns one `ConversationToolRuntime`, constructed exactly
once from a bounded `ConversationRuntimeConfig` that binds one
`ConversationStoreBinding` (the mailbox capability is derived from it),
the clock, the event sink, the environment, the workspace, and the
artifact store; after construction the conversation background registry
identity and its execution records are stable and can never be replaced
or reset by a configuration change. The runtime owns the canonical
`Workspace` boundary (canonicalized root), the `ArtifactStore` (opaque
monotonic `artifact_N` ids, streaming spooling of genuine semantic
artifacts), and the `ManagedToolOutput` store (auxiliary runtime-owned
textual output storage addressed by absolute path, never a semantic
artifact: `tool-output/results/result_N.txt` lazy spill files of oversized
foreground textual tool output, and `tool-output/tasks/exec_N.output`
live-output files allocated at the background dispatch commit point and
reused by the terminal settlement message). The
artifact root and the workspace root must be disjoint filesystem regions:
equal roots, nested roots, and symlink-resolved overlap are rejected at
construction, so runtime-private output files are not included in the default
cwd-based Glob/Grep traversal. An explicit absolute host path remains subject
to the ordinary native file-tool contract.
The explicit `ToolEnvironment` and the authoritative
`ConversationBackgroundRegistry` complete the bundle. Background
executions own a deterministic `exec_N` `ToolExecutionId`, a lifecycle
state machine (`Starting -> Running -> Cancelling -> terminal`), a
two-stage dispatch with an explicit ownership commit linearization
point, a cancel-vs-completion linearization rule, bounded latest progress
snapshots, and exactly-once terminal inbound mailbox publication
(`background-exec_N-terminal`). The `execution` intrinsic
(foreground-only, sequential) is the **single model-facing observation,
steering, and cancellation control plane** for conversation-owned
asynchronous executions (Issue #162, steering from Issue #193): every
creation result returns a typed execution handle (`kind` + `id`), and
`execution(status|cancel|steer)` routes an explicit `kind = tool` target
only to `ConversationBackgroundRegistry` and a `kind = subagent` target
only to `SubagentRegistry`. There is exactly one model-facing handle type
and exactly one model-facing control tool; steering adds an action, never a
second handle and never a second tool. The intrinsic owns no lifecycle
state — the domain registries remain the sole authorities for lifecycle,
cancellation, durability, settlement, and terminal publication — and it
never infers a kind from an id string or falls through from one domain to
another. `steer` is subagent-only, and `kind = tool` + `action = steer` is
refused as an unsupported kind/action combination before either authority
is consulted.

`execution(steer)` is a **control acknowledgement plane, never a child
result channel**. It owns the model-facing schema, the explicit action
dispatch, the target-kind validation, and the minimal acknowledgement
projection (`execution`, `state`, `accepted`); every semantic decision —
whether this child may still accept guidance, how accepted guidance is
ordered, how acceptance linearizes against cancellation intent and
terminal authority, and how the message reaches the child's Agent Loop —
belongs to `SubagentRegistry::steer` and, below it, to the child
conversation's own coordinator — including the ownership refusal of a
Workflow-owned `AgentRun`, whose semantic input is authored by the compiled
Workflow program through the `WorkflowRuntime` and never by this control
plane. A child's final report continues to arrive exactly once through the
canonical parent inbound publication.

As a foreground `ToolCall`, `execution(steer)` participates honestly in
the generic Issue #204 cancellation/settlement lifecycle through the shared
`ToolExecutionHandle::settled_by_operation` ownership mechanism. There is no
private steer operation slot or second lifecycle owner. The steer domain's
`SteerToolEnd` classifies its **effect frontier**: before admission, typed
`Cancelled` yields confirmed no-effect settlement; after admission with the
child undecided, typed `OutcomeUnknown` maps through the shared handle to
`Unconfirmed`; a known child decision yields a confirmed typed steer result.
The child registry owns guidance acceptance; the generic Tool lifecycle owns
canonical ToolResult selection and commit.
Cancelling the steer ToolCall is deliberately not subagent cancellation:
it never invokes `SubagentRegistry::cancel`, and the child subagent keeps
running under its own lifecycle.

The bundle also owns the conversation's `ConversationTodoList` — when the
frozen composition includes the **Todo Agent Extension** (Issue #259). That is
the extension's state half: composing Todo composes the list here, the `todo`
Tool in the Tool Plane, the bounded status presentation Agent Status may
consume, and the Runtime Client projection, all together. A composition without
it has no list at all, reads no Todo history, and publishes no `todo` Tool,
while every canonical `todo` fact the conversation already holds stays exactly
where it is.

The list is deliberately *not* a second persistence path. Every settled `todo` call publishes the complete
post-call snapshot as the structured content of its own canonical tool
result, so the durable record of the list is ordinary conversation
history; `ConversationToolRuntime` construction rebuilds the list by
taking the newest such snapshot from the canonical Ledger, and a rejected
call publishes nothing because it mutated nothing. A list therefore
survives a process restart, a Session resume, and a compaction exactly as
far as the conversation history that carries it does — and survives a
Session clone, fork, and tree branch because a lineage copy copies that
canonical history and the Surface operations that project it, not only
the current projection (see *A lineage copy is a copy of the conversation,
not of its Surface* in `docs/invariants.md`, and `durable::LineageSeed`).

That is only true while the in-memory list cannot run ahead of the
Ledger, so a `todo` call writes *staged* state owned by a `TodoBatch`
the Agent Loop opens before the batch runs. Settling installs what that
batch's own canonical results published — not whatever is staged — so a
stage nothing committed can be discarded but never promoted, and a batch
always starts from the committed authority rather than inheriting one.
Later calls of one batch read what earlier ones staged; dropping the
token, which is what every non-commit exit does, discards them.

The batch is the *only* mutation authority, and it is exclusive. A second
`open_batch` is refused while one is open, because silently replacing the
running batch would leave it committing results and then settling a list
it no longer owned. Mutations are made through the `TodoWriter` that
batch hands its own invocations — the `todo` executor holds no list and
receives the writer per invocation, the way `ask_user` receives its
Questionnaire requester — so a dispatch outside the Agent Loop is refused
as an ordinary failed ToolResult rather than staging a list some other
batch would publish, and one batch's stage is unreadable to every other.
Settlement reports only what the batch meant — the list became what its
committed results published, or it did not move — because a live batch *is*
the open batch, so "the list was taken away" is not an outcome a caller has
to branch on.

None of that authority is exported. The list, the batch, the writer, and
the context seam that binds a writer to an invocation are crate-private,
because settling a batch asserts that canonical history already carries
the list being installed and only the loop that commits the batch can
assert it. The published surface is the derived list —
`ConversationToolRuntime::todo_snapshot` and
`RuntimeClientSnapshot.todos` — plus the rules for reading a list out of
canonical history (`published_snapshot`, `TodoSnapshot::validate`).

Every mutation is also checked against the rule a rebuild applies before
it is staged, so the authority cannot publish a list it could not read
back, and a rebuild that finds the newest committed result unusable fails
construction rather than adopting an older, already superseded list.

The runtime runs the same derivation over the whole Ledger and carries
the result in `RuntimeClientSnapshot.todos`, so a reference client
reconstructs the list from the runtime's own projection plus the
committed results it observes live — never by scanning the bounded
transcript page it happens to hold, and without any client-side task
state. The tool is fixed foreground-only, sequential,
approval-never: one list cannot be mutated by a detached execution, two
concurrent mutations would publish racing snapshots, and there is nothing
in a task list for a human to approve. A subagent child composes its own list
over its own conversation and its own Ledger, when its frozen extension set
includes Todo — so parent/child and child/child isolation is structural rather
than a check, and a child that composes no Todo extension has neither a list
nor a `todo` registration at all.

The dispatch ownership commit is the background linearization point: the
registry synchronization boundary is acquired first and the final
attempt-cancellation observation happens at that same protected boundary.
Cancellation observable there rolls the prepared dispatch back completely
(no published record, no accepted result, the runner never begins);
ownership wins commits exactly once and a later attempt cancellation can
never reclaim the detached execution. Since M9a the commit also writes the
durable `BackgroundExecutionCommitted` fact — the execution identity, its
owning `ToolCall`/tool, and the frozen tool name — **before** releasing the
runner's start gate, so no detached external side effect can begin without
durable evidence a restart can classify. A durable failure there rolls the
dispatch back completely and returns `BackgroundDispatchError::Durable`;
nothing is detached. The fact opens the `background:{execution_id}` durable
lifecycle that the one terminal publication closes. Cancellation intent that commits
first retains its reason in the non-terminal `Cancelling` state, but the request
alone is not a confirmed cancellation: the executor owns the physical terminal
outcome. Only an executor-proven cancellation is canonicalized to `Cancelled`
with the retained reason; an executor-proven success settles as `Succeeded`,
and failure, timeout, and the honest unknown survive under their own truthful
terminal states.
All progress entering runtime state and events passes through one shared
UTF-8-safe bound (`bound_tool_progress`) used by both foreground and
background paths. Foreground progress is additionally cardinality-bounded
per active call (`MAX_PROGRESS_EVENTS_PER_FOREGROUND_CALL`): one invocation
retains at most that many normalized observations before structural
settlement — the first `MAX - 1` observations pinned, the final slot
tracking the newest observation — and only the retained observations become
durable `ToolExecutionProgress` Event Journal facts at batch commit;
coalesced observations never cross the durable commit point. Background
progress retains only the latest bounded snapshot per execution record.

Agent Status owns the runtime `background_execution` built-in section:
executing attempt captures one read-only active snapshot from the background
registry, the closed Background module bounds and evaluates it, and the
renderer shows retained active executions in allocation order with an
`omitted_count` when necessary. Time, Background, and Todo are compile-time-
owned contributors; there is no extension provider registration seam.

The native tool plane implements Read, Write, Edit, Glob, Grep, and Bash as
ordinary registrations under the concrete bounded `NativeToolPolicies`
configuration: each ordinary native tool independently selects its
execution and concurrency policy (foreground-only sequential by default,
with `BackgroundOnly` and `ModelSelectable` as legal per-tool choices).
The only intentionally fixed policy remains the runtime intrinsic
`execution`.

One native capability owns one module boundary. A native tool module owns
its name, description, typed input contract, generated schema, executor,
and private helpers, and constructs itself through its own
`registration(policy)` function returning a `NativeToolRegistration`
(definition + executor + tool-owned argument normalizer); `tools/native/mod.rs` only composes the known
native tools. Composition stays explicit and deterministic: no discovery,
no plugin loading, no registration macros, no generic tool factory.

##### Model-facing ordinary native tool contracts

The model-facing schemas of the six *ordinary* native tools follow
established Pi coding-agent conventions rather than rustX-specific
parameter vocabulary, so a model trained around modern coding agents
recognizes the surface immediately:

```text
read   { path, offset?, limit? }              offset is 1-based (default 1),
                                              zero normalizes to one; no page default
write  { path, content }                      creates missing parent directories
edit   { path, edits: [{ oldText, newText }] }
glob   { pattern, path?, limit? }              omitted path = execution cwd
grep   { pattern, path?, glob?, ignoreCase?, literal?, context?, limit? }
bash   { command, timeout? }                  timeout is in seconds
```

For Read, Write, Edit, Grep, and Glob, a relative model path is interpreted
against and lexically normalized from the authoritative execution cwd
(`Workspace::root()` in the current runtime); an absolute path is likewise
lexically normalized as an ordinary host filesystem path. `.` and `..` are
resolved before filesystem existence or symlink behavior, so missing
intermediate components cannot change path meaning. These five tools do not
impose the locator workspace-containment policy. `Workspace` remains the
runtime/Bash cwd authority. `tools/locator.rs` resolves runtime-advertised
managed-output paths for reads, while `ManagedToolOutput` owns the narrow
model-mutation rejection; neither is a hidden second policy for ordinary
native file paths. Final-component symlinks
are followed for atomic Write/Edit commits so the link itself is not
replaced.

Adopting those conventions is a *schema* decision only. It does not import
Pi's runtime, subprocess model, permission system, ignore behavior, result
ordering, or remote-operations abstractions: execution semantics stay
explicitly rustX-owned, and where a rustX contract and an external
implementation disagree, the rustX contract wins.

Three consequences are load-bearing:
- **Edit is an atomic multi-edit against one original file snapshot.** One
  invocation reads one snapshot, resolves *every* `oldText` against that
  same snapshot (never against the result of an earlier edit in the same
  call), prefers exact matching, and uses a NFKC-based fuzzy fallback only
  when exact matching fails. Fuzzy matching removes per-line trailing
  whitespace and normalizes smart quotes, dashes, and special spaces. Each
  effective oldText must be unique using non-overlapping occurrence
  counting; intersecting, nested, and coinciding ranges are rejected before
  one atomic commit. BOM and the original LF/CRLF/CR line-ending style are
  restored, and any validation failure leaves the file unchanged.
- **Glob and Grep share one search substrate.** `tools/native/search/` owns
  the single file-universe policy both observe: search roots are resolved
  against cwd or used as absolute host paths, a single file is a legal Grep
  root, hidden files are visible,
  ignore files (`.gitignore`, `.ignore`, git global excludes,
  `.git/info/exclude`) deliberately *not* applied, symlinks never followed
  (so neither a directory symlink recursion nor a file symlink target can
  enter the universe), normalized root-relative paths, and deterministic
  lexical enumeration. A caller filter — Glob's `pattern`, Grep's optional
  `glob` — only ever narrows that shared set. The `ignore`, `globset`,
  `grep-regex`, and `grep-searcher` crates are implementation dependencies
  of that substrate and of the Grep engine; no `rg` executable is ever
  spawned, none of those crates' defaults are part of the tool contract, and
  `grep-searcher` never owns workspace traversal.
- **Bash converts its unit at the tool boundary.** The model-facing
  `timeout` is measured in seconds and is converted to the internal
  `Duration` in the Bash input contract. Nothing below that boundary — the
  executor, the supervisor, process-group lifecycle, cancellation, timeout
  settlement, descendant termination, output capture — changes, and no unit
  conversion spreads into the process plane.

Deterministic ordering and bounded output remain rustX-owned semantics in
all cases. Read returns a contiguous complete-line head of at most 2000
lines/50KB and reports the exact continuation offset. Grep and Glob return
plain text, use a complete-line 50KB head, and report either the requested
match/result limit or the byte limit with actionable guidance. Grep shortens
individual lines to 500 Unicode characters and says how to use Read for the
full line. These tool-owned projections remain below the global 64KB runtime
safety boundary; the global limiter is not changed and remains the last
resort for Bash, MCP, and other result types.

`execution` is a runtime intrinsic that happens to participate in the
common tool execution plane. It is not an ordinary native tool: it is the
single model-facing observation, discovery, and cancellation control plane
for conversation-owned asynchronous executions (Issues #162 and #180). Its
contract and runtime semantics are outside the ordinary-native-tool contract
alignment, and it is never moved, renamed, or re-schema'd to make
`tools/native/` look uniform. The closed role of the control plane is:

```text
creation APIs      -> return an ExecutionHandle { kind, id }
execution(status)  -> inspect one execution through its owning domain
execution(cancel)  -> request cancellation through its owning domain
execution(list)    -> discover bounded conversation-owned execution handles
terminal results   -> remain on their existing domain result channels
```

It owns only routing: every request is dispatched by explicit `kind` to the
owning domain registry (`ConversationBackgroundRegistry` for `kind = tool`,
`SubagentRegistry` for `kind = subagent`), which returns its authoritative
snapshot or its own authoritative bounded listing; the intrinsic projects
those into a bounded tagged model-facing representation. The tool status
projection carries the `BackgroundExecutionSnapshot`; the subagent status
projection is the minimal control contract of Issue #192 (typed handle,
named agent, lifecycle state, `publication_abandoned`, the committed
cancellation reason when one exists, and the semantic
`isolated_changes_retained` fact when settlement retained changed isolated
work) and deliberately excludes everything else the authoritative
`SubagentSnapshot` carries — the registry's internal `detail` (diagnostics-only
since Issue #178, when the successful child answer stopped entering it),
the observation-plane `observation`/`execution_profile` fields, the child
agent/conversation correlation, the delegating tool call, the definition
digest, and every physical workspace fact — so the canonical inbound
child-agent message remains the **only**
child-result delivery channel, `execution` never becomes a result channel,
and observing a child never enlarges parent model context. The intrinsic
never guesses a kind from an id, never tries one registry and falls through
to another, and never owns lifecycle state, a registry, a cache,
cancellation implementation, durability, or result publication.

The model-facing input contract is action-tagged, so the action determines
which fields exist rather than leaving a target optional:

```json
{"action": "status", "target": {"kind": "tool | subagent", "id": "..."}}
{"action": "cancel", "target": {"kind": "tool | subagent", "id": "..."}}
{"action": "list",   "filter": {"kind": "tool | subagent", "active_only": true}}
```

A `target` on `list`, a missing `target` on `status`/`cancel`, a `filter`
outside `list`, and any unknown field are all input-contract violations
under the existing strict-schema policy; there is no compatibility spelling
for the pre-#180 shape. The filter vocabulary is deliberately the smallest
useful one — an optional `kind` and an optional `active_only`, where
omission is the only spelling of "do not filter on this axis". There is no
query language, sort key, cursor, label selector, or conversation selector,
and no `wait`, `output`, `logs`, `poll_result`, `transcript`, `restart`, or
`delete` action.

Read-model ownership runs one way only. Each domain authority owns its own
bounded discovery read model, and the model-facing control plane consumes
them; no domain depends on the control plane that consumes it:

```text
ConversationBackgroundRegistry::listing(active_only, limit)
    -> BackgroundExecutionListing   (owned by the background domain)

SubagentRegistry::listing(active_only, limit)
    -> SubagentListing              (owned by the subagent domain)

execution(list)
    -> requests a bounded read model from each selected domain
    -> converts, merges, and projects them
    -> applies the one global MAX_LISTED_EXECUTIONS response bound
    -> returns ExecutionListingResponse
```

The split of authority is the point. A registry owns which executions
exist, their lifecycle classification, its own authoritative intra-domain
order, the matching count, and finite snapshot construction from an
explicit `limit` its caller supplies. The intrinsic owns filter routing
between domains, the cross-domain merge policy, the global response limit,
the truncation metadata, and the model-facing projection. `tools/execution`
retains only the shared model-facing identity envelope — `ExecutionKind`,
`ExecutionHandle`, and the `MAX_LISTED_EXECUTIONS` response bound — and no
read model at all: `MAX_LISTED_EXECUTIONS` is a property of the
`execution(list)` *response*, never a domain invariant, and a registry
bounds only how much it materializes.

`execution(list)` semantics:

- **Scope.** Discovery is conversation-scoped *by construction*, not by
  filtering: the intrinsic holds the registries this conversation owns, so
  another conversation's execution is unreachable rather than hidden, and
  remains indistinguishable from absence — even when the two conversations
  allocated structurally identical ids.
- **Kind isolation.** The kind filter selects which domain authority is
  consulted at all, so it can never fall through into the other domain.
- **Lifecycle filter.** `active_only` uses each owning domain's own
  classification (`BackgroundLifecycle::is_active`,
  `SubagentState::is_active`), under which `PublishingTerminal` is
  non-terminal. Omitting the field — the default — lists active and
  terminal executions alike.
- **Ordering.** Each domain returns its matching records most recently
  allocated first, in its own authoritative allocation order. The intrinsic
  merges the two by strict alternation starting with the tool domain
  (tool, subagent, tool, subagent, ...); when one domain runs out the
  remainder of the other follows in order. The two domains allocate from
  independent sequences and share no ordinal or clock, so alternation — not
  concatenation — is what keeps one domain's overflow from starving the
  other out of a single global bound. Ordering never depends on timestamps.
  The merged result is therefore deterministic but deliberately **not**
  globally most-recent-first: newest-first holds *within each domain*, and
  no cross-domain chronological claim is made — nor could one be, since the
  domains share no ordinal or clock. The model-facing tool description says
  exactly this and claims no global recency.
- **Bound and truncation.** The response is truncated to the single global
  `MAX_LISTED_EXECUTIONS` constant; there are no per-domain quotas, so the
  externally visible bound is exactly one number. Every response carries
  `returned`, `matched` (how many matched the filter before the bound),
  `truncated`, and `limit`, so the shape does not change with the data.
  Truncation keeps the deterministic prefix of the order, and repeating an
  identical request against unchanged registries returns identical entries
  and identical metadata.
- **Observation only.** Listing takes each registry's ordinary read path and
  mutates nothing: no lifecycle, no cancellation, no settlement, no terminal
  notification, no capacity accounting, no ordering, and — for subagents —
  no observation-plane revision or latest value.
- **No result retrieval.** A listing entry carries the typed handle, the
  owning domain's own lifecycle state, and the few identity facts that make
  it recognizable (`tool_name`; `agent`, `started_at`,
  `publication_abandoned`). It carries no detached tool `result` or
  `progress`, no subagent `detail`, no answer content, and no child
  history. Full child history belongs to the child's own conversation.
- **Lifecycle vs. activity.** The `state` of an entry is the owning
  domain's authoritative lifecycle vocabulary, so `list` and `status`
  project the same lifecycle facts for the same execution. Issue #178's
  live activity projection is deliberately *not* part of either: it is an
  observational read model that enters no model context, `execution(status)`
  already drops it, and a listing that carried it would make one action of
  the same intrinsic expose what another withholds.

Native tool input schemas are generated from tool-owned Rust input types,
so the typed contract is the single source of truth for the model-facing
arguments:

```text
native:   Rust input type -> generated schema -> ToolDefinition
MCP:      MCP schema                          -> ToolDefinition
Python:   FastMCP-derived MCP schema          -> ToolDefinition
```

Managed Python preparation uses the MCP adapter internally; FastMCP derives
its Tool schemas from decorated function names, docstrings, and type hints.
The published Tools retain Managed Python source provenance.

All three converge at the same registry boundary, and the runtime keeps
validating every invocation against the stored canonical schema before
dispatch. An optional native property means an *absent* property: the
native schema-generation boundary collapses the nullable union that
`Option<T>` would otherwise produce for a field that only expresses
omission, so `{"timeout": null}` is a business argument violation
rejected at preflight rather than a second spelling of omission. This is a
rule about implicit nullability, not a restriction on the schema language:
a native contract that genuinely needs a composite or nullable model-facing
shape states it explicitly.

The executor ABI is unchanged by this: `ToolRegistry` validates
model-issued business arguments against the generated canonical schema
before dispatch, and a native executor receives the validated canonical
`ToolInvocation` — whose `arguments` remain canonical JSON — and
immediately decodes them into its tool-owned typed input before any
tool-specific filesystem, process, or other business work begins.
`ToolExecutor` never carries typed generics.

```text
model JSON
    -> ToolRegistry schema preflight
    -> validated ToolInvocation
    -> native executor boundary
    -> typed input decode
    -> tool-specific semantic validation
    -> actual tool work
```

Required fields, type correctness, and schema constraints belong to the
input contract, while workspace permission, filesystem existence, pattern
compilation, and process lifecycle rules remain execution concerns.
Outputs are deliberately untyped: the canonical `ToolExecutionResult` stays
the only tool result contract, so the agent loop never learns tool-specific
result types. Within it, ownership is explicit: `content` is tool-owned
(`ToolResultContent::Json` is arbitrary tool-owned structured data — rustX
reserves no ordinary JSON field names and no generic runtime code infers
semantics from property names such as `full_output`, `partial_output`, or
`note`), `artifacts` holds genuine semantic artifacts, and
`managed_output` is the rustX-owned typed continuation metadata of managed
textual output (absolute read-only locator plus typed complete/partial
state). A failing result is passed back to the model as correction evidence:
the typed status remains authoritative, while
`ToolExecutionResult::model_facing_projection` combines tool-owned content,
runtime status feedback, and managed-output continuation into one
provider-independent representation. That complete projection — including
structurally retained continuation and bounded diagnostics — never exceeds
`MAX_MODEL_TOOL_RESULT_BYTES`; provider adapters only translate it to their
wire formats. The generic background terminal publication consumes the same
projection, and tool-owned JSON remains ordinary tool-owned content.

Bash treats one invocation as one complete lifecycle:
spawn one per-invocation supervisor, capture stdout/stderr/combined, let
the supervisor own the invocation's process group to its kernel-mediated
terminal state, and settle only when the shell's terminal status is
known, the owned group's terminal report arrived, AND the output capture
is settled — shell-parent exit is not by itself the Bash settlement
boundary, so a descendant that remains in the owned group after the shell
exits (holding the pipes or having redirected them away) can never escape
the timeout/cancellation contract. The child runs with an explicit
`env_clear()`-based environment, per-stream incremental UTF-8 decoding
before multiplexing (every advertised output path holds valid text for
Read/Grep), bounded head/tail previews per stream, and mode-dependent
output storage in the conversation's managed tool-output store: foreground
output spills lazily into `results/result_N.txt` only once the preview
bound is crossed, while a background execution streams into its
dispatch-allocated live-output file `tasks/exec_N.output` from the first
byte on (the absolute path is runtime-owned typed continuation metadata —
`ToolExecutionResult::managed_output` — which the canonical model-facing
projection presents exactly once; the result's `artifacts` stay empty — text
overflow is not an artifact),
`TERM -> BASH_TERM_GRACE -> KILL` cancellation driven by the supervisor,
typed result semantics (zero exit success, non-zero exit failed with the
code preserved, timeout as `TimedOut`, cancellation as `Cancelled`),
explicit spill-capture failures (a spill that cannot be allocated or
written fails the invocation explicitly rather than silently losing full
output), and explicit process-control failures (supervisor setup, shell
spawning, waiting/reaping, signaling, and IPC failures settle as `Failed`,
never as a silent `Success`, `Cancelled`, or `TimedOut`) — never a silent
success that lost the retained output.

##### Tool Plane result normalization and output ownership

Native and MCP executors produce a logical result; they do not
choose independent oversized-result policies. The shared Tool Plane seam in
`src/tools/output.rs` owns capture, deterministic previews, UTF-8 handling,
managed-output retention, and typed `TruncationState`/
`ManagedOutputContinuation` publication. The canonical result projection in
`src/tools/types.rs` owns the model-facing status/content/continuation
composition and the one aggregate byte bound. The origin adapters remain
protocol translators: MCP translates `CallToolResult`, and Python tool
packages are served through that same MCP adapter, so they inherit the MCP
translation; model providers only translate
the already-decided canonical projection.

The limits have deliberately different meanings:

- `FOREGROUND_TOOL_RESULT_PREVIEW_BYTES` (currently 16 KiB) is the shared
  foreground projection threshold;
- `MAX_MODEL_TOOL_RESULT_BYTES` (currently 64 KiB) is the absolute
  canonical/model-facing safety bound for every complete
  `ToolExecutionResult::model_facing_projection`, including tool-owned
  content, runtime status feedback, and managed continuation;
- managed output is auxiliary storage for the complete logical text (or an
  explicitly partial prefix after a storage failure), not canonical history
  and not a semantic artifact/File result.

In foreground mode, a complete deterministic representation at or below the
shared preview threshold remains a direct result and creates no result spill.
Crossing the threshold lazily allocates exactly one
`results/result_N.txt`, streams the complete representation there, and
publishes a bounded deterministic preview plus typed `Complete` continuation.
An allocation failure publishes `Unavailable`; a write/read failure retains
the locator as `Partial`. Result size alone never changes semantic success to
failure; an output-storage failure is a separate explicit failure fact.
MCP content blocks are budgeted collectively, so the canonical preview is
always valid UTF-8 rather than malformed truncated JSON.

In background mode, `prepare_dispatch` allocates exactly one
`tasks/exec_N.output` before the accepted result advertises it. Bash streams
into it while running; MCP writes its complete final logical
representation to it at result normalization/settlement. The accepted and
terminal locators are therefore identical, no secondary `results/` spill is
created, and terminal publication keeps the locator and fixed Read/Grep
guidance structurally while bounding the canonical body by
`MAX_MODEL_TOOL_RESULT_BYTES`. Failed retention is `Partial`, never false
`Complete`.

The background write/settlement linearization is also explicit: the origin
owns its sink until its executor future returns, the runner invokes registry
terminal settlement only after that return, and terminal candidate/publication
then claims the registry's structural winner. No origin-owned writer remains
after settlement can win, so cancellation cannot be followed by a late
MCP result write that mutates settled result state.

**The Bash invocation ownership boundary is its dedicated process group.**
On both supported platforms the inner supervisor creates a fresh session and
process group, and `TERM`/`KILL` are issued with `killpg` while the retained
inner pid proves that the numeric group id is still allocated to the
invocation. The outer supervisor reports the canonical `AllChildrenReaped`
event only after its group-scoped `waitid(Id::PGid)` gate reaches `ECHILD`.

Linux strengthens that base lifecycle with an inherited seccomp policy that
rejects descendant `setsid(2)`/`setpgid(2)` calls, plus child-subreaper
adoption for orphaned descendants. Those two primitives make the group wait
a complete whole-group terminal proof, including shell-backgrounding and
supervisor-loss fallback; the filter uses syscall numbers from the compiled
Linux ABI and rejects x32 execution on x86-64.

macOS has the same real process-group and `waitid` lifecycle, using the
platform libc adapter because `nix` does not expose `waitid` on Apple
targets. It has no seccomp or child-subreaper equivalent, so a descendant
that outlives the shell is reparented to launchd and becomes invisible to
the supervisor's group-scoped wait. macOS therefore does **not** treat a
group-scoped `ECHILD` as a whole-group terminal proof. Instead:

- Bash is wrapped with an EXIT `wait` as a **best-effort convenience** so
  ordinary background jobs finish naturally; it is not an ownership
  boundary and the user command may legally replace it;
- when the shell is reaped, the inner supervisor escalates to the outer's
  fallback containment (`SIGKILL` to the retained group), and the outer
  reports terminality only after issuing that containment signal and then
  proving the group absent with a `killpg(pgid, 0)` probe reaching `ESRCH`;
- a containment signal whose result is `EPERM` is never itself terminal:
  `EPERM` proves only that the signal operation was not authorized, so the
  group's absence is proven independently by the `killpg(pgid, 0)` probe
  rather than inferred from `EPERM`. (On macOS the kernel also reports a
  zombie-only group as `EPERM`, which is indistinguishable from an
  unauthorized live member, so neither is ever treated as a terminal fact.)

A command that deliberately creates a new session leaves the macOS
process group and thereby exits rustX's ownership domain: rustX does not
track, contain, reap, or wait for such a descendant, and settlement of the
owned group does not imply it terminated. A lost outer supervisor that
leaves no waitable anchor is reported as unproven rather than converted
into a false terminal proof. macOS terminal settlement therefore proves the
owned process group was actively terminated — not that every descendant
was reaped, which rustX cannot prove on macOS. `/proc` is never the source
of truth for ownership or quiescence on either platform.

**The inner supervisor pid is an ownership anchor with exactly one
reaping owner.** The outer supervisor's dedicated anchor path is the only
code allowed to observe the anchor's terminal state (`waitid` with
`WNOWAIT`: observation only, never consumption) and the only code allowed
to reap it (the group-scoped gate, strictly after any fallback
containment signal). The outer therefore has **no generic `waitpid(-1)`
reaping loop**: every child of the outer is either the anchor or an
in-group adopted descendant, so the gate reaps the whole child domain and
a generic loop could only ever consume the anchor and lose the
abnormal-exit fallback-containment decision. An `ECHILD` from the
dedicated anchor observation is an ownership invariant violation, never a
terminal observation: the outer reports it and fails safely — it never
derives owned-group terminality from an anchor `ECHILD`, never signals a
numeric group id without the retained anchor, and never reports the
canonical terminal event. The inner supervisor's own reaping hygiene
consumes only its own children (bash and adopted in-group descendants),
never an anchor of another owner.

The OS ownership commit is the successful `/bin/bash` spawn after the inner
has created the invocation session/group and installed the platform's
membership policy (seccomp on Linux; an explicit no-op on macOS). Protocol
state makes this explicit: the inner reports `AnchorReady`, rustX retains the
possible ownership identity and replies `Start`, then the inner reports
`OwnershipEstablished` after spawning Bash. If communication fails after
`Start`, rustX conservatively assumes ownership may exist. Pre-gate setup
failure reports `NoOwnership` and may settle without a Bash domain. The
`Start` gate is a recognition point, not a reader boundary: the inner's
rustX-facing control direction owns one `FrameReader` for the whole
invocation, shared by the gate and the owned control loop, so a `Terminate`
that the kernel delivered in the same `read()` as `Start` still drives the
ordinary `TERM` -> grace -> `KILL` path (the shared control-frame ownership
invariant, identical to the interactive unit's gates).
On Linux, catastrophic fallback authority is a pre-ownership prerequisite:
the runtime child-subreaper primitive is consulted (once per process,
idempotently) before the supervisor unit spawns, so `START` — which
authorizes the Bash spawn — is never sent before rustX can own catastrophic
containment. macOS has no equivalent orphan-adoption primitive; its normal
path uses direct-child and process-group ownership, and a lost outer without
a waitable anchor remains explicitly unproven.

Control-channel EOF is never a post-ownership process-terminal event. Normal
terminality linearizes at the outer's group-scoped `ECHILD` and its
`AllChildrenReaped` frame. On Linux, catastrophic loss of both supervisors
uses the runtime's **process-level kernel coordination primitive** — the
child-subreaper capability owned by `src/runtime/process_supervision.rs`,
with lazy one-time, idempotent, sticky activation — to retain the adopted
inner anchor, contain its group, and reach a second group-scoped `ECHILD`.
Kernel adoption does not assign arbitrary children to Bash lifecycle
ownership, and rustX implements no generic unknown-child reaper. On macOS,
the outer's descendants are not adopted by rustX; if the anchor is not
waitable, emergency containment reports `AnchorUnavailable` and remains
unproven rather than committing a result. Thus EOF changes communication
state and failure intent, while process lifecycle remains independently
`PreOwnership`, `OwnershipPossible`/`Owned`, or `Terminal`.

Every Bash result status — `Success`, `Failed`, `Cancelled`, and
`TimedOut` — is terminal with respect to the invocation-owned process
group: no invocation-owned Bash process remains capable of executing work
before any result is returned. A detected process-control/runtime failure
determines the eventual result status but does not itself settle the
invocation lifecycle: failures before any Bash tree was established may
return `Failed` immediately, while failures after ownership exists (signal
failure, wait/reap failure, IPC failure, control-channel abandonment)
follow the containment lifecycle — the failure is remembered, the outer
supervisor becomes the active containment authority (it observes the
inner's terminal state via `waitid(WNOWAIT)` without releasing the
structural anchor, sends one fallback `SIGKILL` to the still-proven-owned
group, and releases the anchor only through the group-scoped wait), the
capture is finalized, and only then is `Failed` returned.
`BASH_TERMINATION_CONFIRMATION` is a process-confirmation watchdog: expiry
records `QuiescenceTimeout` failure intent but does not authorize result
commit. After process terminality, a separate capture deadline may force-
finalize wedged readers and return `Failed(CaptureTimeout)`. The
outer supervisor also un-wedges a `SIGSTOP`-frozen inner anchor with
`SIGKILL`, so a stopped containment chain cannot strand the owned group;
the only residual state in which rustX cannot prove owned-group
terminality from outside the unit (a unit frozen beyond the outer
supervisor's reach) cannot be truthfully converted into a terminal result.
Control-channel abandonment remains fail-safe through the normal supervisor
unit. If the unit itself is lost, the rustX-held subreaper authority above is
the independent fallback. The anchor is released only by the final reap
after the last signal, so a numeric group id whose allocation has ended is
never signaled.

### Layer 3: Model plane

The model plane implements protocol adapters:

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages

The M2 implementation freezes the model-plane boundary:

```text
Provider HTTP / SDK
        |
adapter-private provider representation
        |
ModelAdapter
        |
ModelEvent
        |
M3 Agent Loop
```

#### Provider, Model and selection ownership

Provider and Model are independently named atomic domains in `rustx.toml`.
A Model names its Provider and declares protocol, limits, capabilities, reasoning,
compatibility and opaque native request defaults. Identity spelling infers none
of those semantics. A Workspace replacement discards the entire lower object,
including credentials. Root model selection is another atomic semantic unit.

An explicit Session model is deliberate durable intent. The next admission
resolves it against the published generation; cold resume revalidates it against
freshly read current sources. Named Agents with no model inherit the invoking
Attempt's already-frozen effective model, not current Root defaults.


#### The ToolCall acceptance boundary (Issue #201)

Model tool intent is non-authoritative wire state until it crosses one
explicit acceptance point:

```text
provider/model stream
    -> provider-specific proposal assembly   (adapter, protocol-aware)
    -> proposal identity resolution          (src/model/adapter/proposal.rs)
        -> ModelEvent::ToolCallStarted, argument streaming
    -> complete argument validation          (same module)
    -> ToolCall ACCEPTANCE  <- the one linearization point
        -> canonical ToolCall in ModelEvent::ToolCallCompleted
    -> Agent Loop Tool path                  (preflight, approval, execution,
                                              exactly-once ToolResult)
```

Every adapter assembles a proposal in its own protocol terms — provider
envelopes, chunk indexes, snapshot merges, block ids, lossless wire
normalization such as retaining an already-established streamed identity when
later continuation deltas omit it — and then presents the assembled proposal
to the shared functions exactly once.

The two stages are deliberately not the same event. **Identity resolution**
establishes that a proposal is attributable — a usable correlation identity
and a tool identity that resolves to one declared `ToolId` — and that is what
licenses the canonical `ModelEvent::ToolCallStarted` and its argument deltas.
`ToolCallStarted` is a canonical normalized model-stream event of the
adapter→kernel protocol, but it carries a `ToolCallStart`, not a `ToolCall`:
nothing is executable yet. **Argument acceptance** is the acceptance
linearization point; the complete argument representation is parsed exactly
once, and only on success does the full canonical
`ToolCall { id, tool_id, name, arguments }` exist, carried by
`ToolCallCompleted`. There is therefore exactly one unambiguous point at
which an executable canonical `ToolCall` comes into being.

Neither stage invents intent, and neither performs Tool schema validation,
which belongs to Tool preflight after the canonical call exists.

Everything a provider can do to make a proposal untrustworthy is recognized
below this line and normalized above it into one class,
`ModelErrorKind::MalformedToolProposal`, with typed provider-independent
provenance: `ProviderDeclared` (the provider terminated declaring its own
call malformed), `StreamAssembly` (the stream never delivered an identity),
`AdapterStructural` (an undeclared tool name, or an argument representation
that is not one complete JSON value), and `ReservedProtocolLeak` (reserved
in-band tool markup leaked into ordinary output while no structured call was
produced).

Reserved-markup recognition is the only Qwen-shaped protocol knowledge in the
runtime, and it lives in the Chat Completions adapter beside every other
dialect difference (`src/model/adapter/openai/qwen_xml.rs`). It is opt-in and
narrow: the model must declare the dialect through `compat.chatToolProtocol`,
tools must actually have been exposed in that request, the generation must
have produced no structured call, the provider must have terminated as a
complete normal generation, and the output must contain an actual protocol
*emission*. Under the default `native` profile generated text is never
inspected at all. Nothing is inferred from a model name, a provider name, or
a hostname.

Recognizing an emission is deliberately not a substring test. The reserved
bytes of the dialect are exactly what a correct answer contains when the user
asks how the dialect works, so `contains(open) && contains(close)` would
classify a good answer as malformed tool intent. The recognizer instead
requires the structure a real vLLM/Qwen emission has and a discussion of it
does not.

That structure is the dialect's *reserved grammar*, never a pretty-printed
layout. Upstream drives the dialect from reserved markers — `<tool_call>`,
`<function=`, `<parameter=` and their closers — and matches parameter
regions with a `re.DOTALL` value group, trimming at most one wrapping
newline; newline placement is therefore template decoration and carries no
protocol meaning. `<parameter=path>notes.txt</parameter>` inline, a fully
compact `<tool_call><function=…><parameter=…>…</parameter></function></tool_call>`,
and the pretty-printed emission are one and the same region, and all three
are recognized. Recognition runs on the fully assembled generated output, so
provider chunk boundaries cannot change the outcome either.

What separates that structure from a discussion of it is *ownership*, not
formatting. A generation that leaks the dialect is one the serving stack was
supposed to consume whole: its output **is** the reserved region. A
generation that explains the dialect is writing a document, and the reserved
bytes are material that document introduces, quotes and discusses. So the
scan walks the assembled output from its start and stays in the protocol
only while everything behind it is reserved markup or the payload of an open
reserved envelope; the first thing that is not — ordinary words outside every
envelope, or a Markdown code fence — hands ownership to the document, and the
scan ends there. Inside the protocol the region must still be real
structure: an opener matched by its own closer, in order, with a payload that
is a plausible function/parameter identifier rather than an illustrative
ellipsis.

Ownership is a property of the output, not of its layout, and it never
begins again at a line break, so

> `The exact parameter syntax is: <parameter=path>notes.txt</parameter>`

and the same answer with that space replaced by a newline are one answer and
classify identically. The invariant is explicit: **reserved-protocol
classification must not change solely because ordinary explanatory prose and
the exact same quoted syntax are separated by a newline rather than a space**,
and more generally a raw newline is never the structural basis for treating a
region as emitted tool intent. The one layout marker that does carry meaning
is the code fence, because a fence is the author declaring the block quoted;
lines otherwise exist for the scan only as the unit a fence is recognized on
and as the place a pretty-printed emission puts its tags. The consequence is
deliberate and conservative: reserved bytes produced after a generation has
begun writing a document are not classified as a leak, because no structural
evidence separates "here is the syntax:" from a leak that follows a sentence,
and enumerating English phrasings would be a vocabulary rule rather than a
grammar. The recognizer is one bounded forward pass with constant state: no
backtracking, no materialized regions, no general XML parsing, and it prefers
missing a speculative shape to reclassifying a correct answer. Recognition is
one-directional — it proves a leak and produces one refused proposal; the
leaked region is never parsed back into a `ToolCall`, because a call this
runtime had to infer is precisely the invented model intent the acceptance
boundary exists to refuse.

Above the adapter the Agent Loop reacts to the class and never to the
evidence. It owns the bounded one-regeneration recovery of that class
(`docs/agent-loop.md`), and the acceptance boundary keeps the canonical
`ModelEventAssembler` unchanged: the assembler validates a stream that has
already crossed acceptance, so its rejections stay fail-closed
`RuntimeError::ContractViolation` failures and are never retryable.

#### Single-generation safety: liveness, budget, integrity (Issue #203)

One physical model generation is subject to three different contracts, and
conflating them is what lets a generation run forever:

```text
transport liveness      !=   generation budget      !=   generation integrity
src/model/deadline.rs        src/model/generation.rs     src/model/generation.rs
"is the provider still        "has this generation        "is this still a
 producing anything?"          produced too much?"         generation at all?"
```

A stream that emits reasoning forever is perfectly *live*, so the deadline
cannot bound it. A stream stuck in a repetition loop is live and inside its
budget for a long time, so the budget cannot diagnose it. Each contract needs
its own mechanism, and none of them is provider-specific.

`src/model/generation.rs` owns the last two as one request-local
`GenerationGuard`, which is execution state exactly like
`ModelRequestDeadline`: it observes the *normalized* `ModelStreamItem` stream,
never provider wire data; it never inspects a provider name, a model name, a
hostname, or sampling configuration; it never calls another model; and it
decides nothing. It reports one typed `GenerationFailure` and its caller — the
Agent Loop for a primary request, the context-plane summarizer for a summary
request — owns discarding the generation, cancellation arbitration, the
recovery budget, and the canonical commit decision.

The separation is in the type system, not only in this prose. Ownership is:

```text
src/model/deadline.rs      transport liveness: the timeout policy, the
                           request-local phase machine, and ModelTimeoutPhase
src/model/generation.rs    generation budget + generation integrity: the
                           GenerationSafetyPolicy contract, the byte
                           accounting, and the degeneration detector
src/model/invocation.rs    the policy-resolution seam: what the runtime's
                           safety bounds actually are for one invocation
src/model/error.rs         normalized ModelError aggregation
Agent Loop                 semantic recovery, cancellation, commit/discard
```

`generation.rs` defines and re-exports no liveness vocabulary and does not
depend on `deadline.rs`; a transport timeout is representable only as its own
typed fact and never as a `GenerationFailure`. It also decides none of its own
limits: it defines the contract it enforces and takes a resolved policy as an
input.

**Deterministic degeneration detection.** Each generated text channel gets a
bounded repetition detector. Its whole state is fixed and complete: a 2 KiB
ring window, one prefix-function table of 2049 `u16` entries, one 64-byte
checkpoint block, and three counters — about 6.1 KiB per channel, and
nothing that grows with the length of the generation.

Detection is exact suffix periodicity, computed once per checkpoint rather
than per candidate. A scan runs the Knuth–Morris–Pratt prefix function over
the *reversed* window, whose prefixes are the window's suffixes: for a string
of length `L` the smallest period is exactly `L - pi[L]`, so one linear pass
answers "what is the primitive period of the last `L` bytes?" for every `L` at
once. Every candidate period is then answered from that table in constant
time, because the evidence threshold forces `p <= L / 4`, which puts the span
inside the periodicity lemma: the periods of the span are exactly the
multiples of its smallest period, so `p` is a period **iff** that smallest
period divides it.

The work bound is therefore exact rather than typical, and it is worth stating
precisely instead of calling it "practically linear":

> One scan costs fewer than `2 * 2048` byte comparisons — the prefix-function
> construction — plus at most 509 constant-time table lookups, one per
> candidate period. Scans happen once per 64 generated bytes, so the cost per
> generated byte is under 70 byte comparisons **for every input**. No
> candidate can cost more than another, and no input can make a scan
> expensive.

There is no sampling, no hashing, no probabilistic filter, no verification
budget that could cause a miss, and no backtracking; adversarial near-periodic
output costs exactly what ordinary prose costs, which is asserted by a
deterministic work-count regression rather than by timing.

The evidence threshold is explicit:

> at least **four consecutive exact repetitions** of a unit whose **primitive
> period is at least four bytes**, spanning at least **1024 bytes**.

The primitive-period rule is what keeps ordinary structure out: a run of `,`
or `},` or `}\n` is periodic at every multiple of its own length, and all of
those multiples are rejected because the shortest period is a delimiter, not a
repeating unit. Ordinary repetitive code, JSON, and XML are not exactly
periodic in the first place, because their payload changes while their syntax
repeats. A repeated unit longer than 512 bytes is not classified at all: the
detector prefers missing an ambiguous shape to rejecting a valid generation,
and it is a strong-evidence guard, never a quality judge.

Scanning happens at cumulative-byte checkpoints of the channel, never at
provider delta boundaries: the detector buffers a partial checkpoint and
commits only whole 64-byte blocks, so the scanned prefix at checkpoint `k` is
exactly the first `k * 64` bytes of the channel however the provider chunked
them. `"foo " "foo " "foo "` and `"foo foo " "foo" " foo"` are therefore the
same generation and classify identically.

**Channel attribution.** Reasoning and visible content are tracked
separately, so the typed fact states *where* the generation degenerated.
Refusal deltas belong to the content channel, explicitly: a refusal is visible
model-generated output that assembles into the canonical Assistant message, so
a refusal loop is exactly as degenerate as a text loop. Tool-call argument
streams are deliberately not a channel — structured wire data legitimately
repeats, and tool intent belongs to the `ToolCall` acceptance contract above —
though their bytes are still charged against the total-output safeguard, since
an unbounded argument stream is still an unbounded generation. Usage updates,
continuation metadata, lifecycle events, and ephemeral progress are charged to
nothing and inspected for nothing.

**Output and reasoning budgets.** These are independent policy dimensions:

```text
max total output   !=   max reasoning
```

means two bounds a policy owner sets separately, **not** one derived from the
other. A model may legitimately answer at length, and it may legitimately
think before answering, but "may think without bound" does not follow from
"may answer at length" — and neither does "may think exactly three quarters as
much".

The provider-neutral output limit already exists and is already configured —
`max_output_tokens` in catalog TOML, overridable per Session, resolved into
`ResolvedModelInvocation` before the adapter boundary — and provider-native
reasoning controls already have a home in a model's declared reasoning
profiles, authored through `request_params`, whose decoded parameters an
adapter maps to its own API. Issue #203
adds no second output-limit mechanism and no new configuration.

What it adds is a runtime-side *safeguard*, because correctness must not
depend on a self-hosted backend honoring its own request limit. Its shape is
`GenerationSafetyPolicy`, and its two fields are the two dimensions above:

```text
GenerationSafetyPolicy {
    max_generated_bytes: u64,           // always resolved
    max_reasoning_bytes: Option<u64>,   // a separate bound, when one is resolved
}
```

`None` has explicit semantics: **no separate runtime reasoning bound**.
Reasoning bytes still count against `max_generated_bytes` like every other
generated byte, so the generation stays bounded; what is absent is the
tighter, separately attributed reasoning limit. It is not "unlimited
reasoning", and it is not zero.

The guard *enforces* a resolved policy and never invents one. Resolution
belongs to `ResolvedModelInvocation::generation_safety_policy`, which is the
smallest existing owner that can answer the question: the effective output
budget is already resolved there from the catalog model, the session override,
and the context plane's summary cap, and the result is already frozen for the
whole attempt. The guarantee the guard then provides is stated exactly:

> One physical generation cannot stream more than `max_generated_bytes` of
> normalized generated data, nor — when a reasoning bound is present — more
> than `max_reasoning_bytes` of normalized reasoning text, before the runtime
> terminates it. Both numbers are UTF-8 **byte** counts of the adapter's
> normalized output, **not model tokens**, and neither replaces the
> provider-enforced `max_output_tokens`.

Both bounds currently come from one documented **runtime fallback policy**,
`runtime_fallback_generation_safety_policy`: `32 bytes per resolved output
token`, floored at 256 KiB, with reasoning granted three quarters of the
result. That share is a current default of the resolution layer, not a
model-independent invariant — there is no semantic law making a reasoning
budget three quarters of an output budget, and nothing that *enforces* the
bounds knows the number. When a product requirement asks for a separately
configured reasoning bound, it is resolved there, beside the output budget,
and nothing downstream changes.

The multiplier is deliberately generous — real output runs nearer four bytes
per token — because this bound exists to stop a runaway stream, not to shape
an answer: a bound that could plausibly reject a legitimate long generation
would be the worse error. The repetition detector, not this floor, is what
catches the failure mode Issue #203 observed, at roughly a kilobyte of
evidence.

**Provider length termination.** `ModelFinishReason::Length` is the provider
stating that it stopped because of its limit rather than because the answer
was finished, so the generation is incomplete by construction. It is
classified as a typed budget fact and fails closed **before** the canonical
commit boundary; it never becomes an ordinary successful assistant completion.
The typed fact carries no measurement, because the provider knows the token
count and this runtime does not, and inventing one would be false precision.

**Typed facts, bounded by construction.** Every outcome normalizes into the
existing model-error vocabulary rather than into new top-level runtime events,
and the two contracts occupy two different fields of `ModelError`:

| Condition | `ModelErrorKind` | typed detail |
| --- | --- | --- |
| repetition loop | `GenerationDegenerated` | `generation: Degenerated { channel, period_bytes, repetitions, span_bytes }` |
| provider token limit | `GenerationBudgetExceeded` | `generation: ProviderLengthLimit` |
| runtime byte safeguard | `GenerationBudgetExceeded` | `generation: RuntimeBudgetExceeded { budget, limit_bytes, observed_bytes }` |
| expired deadline | `Timeout` | `timeout_phase: ResponseStart \| StreamIdle` |

A timeout carries a `timeout_phase` and no `generation`; a generation defect
carries a `generation` and no `timeout_phase`. Two constructors enforce that:
`ModelError::timeout` produces the transient liveness shape and
`ModelError::generation_failure` the non-retryable generation shape, and
neither can produce the other's.

The phase a timeout carries can only come from
`ModelRequestDeadline::pending`, which yields a `(phase, instant)` pair only
while a deadline actually exists. A terminal request owns no deadline, so it
yields nothing — there is no phase for a caller to convert, and no default to
fall back to. The impossible expiry is unrepresentable rather than normalized
into a plausible one.

Every payload is an enumeration or an integer, so provider output can never
author unbounded durable diagnostic data through these types: the bound is
structural rather than a truncation rule, and the rendered message is
runtime-authored. `ModelRequestFailed` carries the fact into the Event
Journal; no new `RuntimeEvent` variant was needed. No consumer above the model
layer reads prose to learn which contract was violated.

The summarizer is the model plane's other consumer and carries the same guard,
mapped through its own summary-failure boundary rather than into generic model
retry — exactly as it already does for a deadline. A truncated or looping
summary is the dangerous case, because compaction *replaces* retired history.

#### Logical model steps and actual requests (Issue #134)

The Agent Loop owns one logical primary model step, which begins with one
Context Assembly/admission generation and may contain several actual provider
requests. Every actual request crosses the ordinary stage/finalize,
cancellation/start-arbitration, durable `RequestSnapshot` +
`ModelRequestStarted`, reconstruction, and adapter frontier. It has its own
request and publication identities, outcome, and settlement. The shared
`RequestIdentity.retry_number` is the single actual-request ordinal across
transient recovery and context-overflow recovery; it is not an event-position
inference and there is no speculative next request ID.

Transient recovery replays the frozen admitted request state. It does not
reassemble live context, sample Agent Status, consume new `FreshInbound`, run
contributors, or read new mailbox entries. Context overflow remains a separate
Context Engine recovery boundary: estimator correction, compaction, fit
validation, and a new post-compaction frozen state. A transient failure after
compaction replays that post-compaction state. The Agent Loop settles the
previous publication before scheduling or starting another request. Failed
partial output is durable noncanonical audit evidence and cannot authorize Tool
Plane execution; only a successful actual request can commit the canonical
Assistant.

The transient budget is three retries, the overflow budget is one, and the
**semantic corrective-generation** budget is one corrective *generation* for
the whole logical step, for an additive maximum of six primary provider
requests per logical step. A corrective generation may itself be realized by
several actual requests when transport or overflow recovery intervenes; the
bounded corrective context it owns is carried by all of them and is cleared
only when that generation resolves. The three budgets are keyed on disjoint
error classes, so none of them resets or multiplies another. The semantic
budget is one budget for *all* generation defects — a malformed tool proposal
(Issue #201), a degenerate channel, an exhausted generation budget (Issue
#203) — rather than one per class, so no ordering of those anomalies can
produce a third semantic generation for the same step. Backoff
uses the runtime-owned monotonic clock: 2, 4, and 8 seconds, overridden by an
adapter-normalized `retry_after_ms` hint capped at 60 seconds. Provider
adapters classify provider failures with `ModelRetryDisposition`; a runtime
owner may assign the disposition for a normalized runtime failure such as a
request timeout. The Agent Loop always owns retryability policy, budget,
scheduling, and execution, and never derives it from provider strings or HTTP
prose. `ModelRequestFailed.usage` retains only the latest trustworthy
cumulative pre-failure evidence for that request, or `None`.

Issue #137 adds one fixed request-only input item to this boundary. After a
logical step is terminally unresolved, live settlement and startup recovery
derive the last durably started `RequestIdentity`, walk its ordinals from the
highest through zero, and keyed-load only the identities derived from each
candidate. The shared selector chooses at most one meaningful `Incomplete` or
`Unaccepted` Publication Audit; it does not scan, concatenate, or choose a
conversation-wide latest audit. An eventually successful internal retry makes
all earlier retry audits internal generations and installs no pending source.

The durable producer transition commits the attempt terminal and the
replacement/clear of `pending_unresolved_output_stream_id` together. The
first eligible primary start is the consumer: its start transaction freezes
the selected bounded rendering and `RequestOnlyInsertionAnchor` in the
Request Snapshot and clears the pointer in the same commit. A cancellation
before that commit preserves the pointer. Transient retries reuse the frozen
snapshot semantics; overflow only moves the request-only value down its
full → reduced → metadata-only → omitted ladder.

#### OpenAI adapters (async-openai)

Both OpenAI adapters use the `async-openai` crate for typed request types,
the SDK client plumbing, and SSE stream consumption. Two properties are
enforced by construction:

- Automatic retry is bypassed. The SDK's default executor wraps the plain
  transport in `OpenAIRetryLayer`; the adapters install a rustX-owned custom
  HTTP service (`NoRetryService`) that executes exactly one `reqwest` request
  per call and performs no retry. One adapter invocation is exactly one
  provider request attempt.
- The Chat Completions response stream and the entire Responses protocol use
  the SDK's BYOT (bring-your-own-type) facility as raw JSON, so unknown
  finish reasons, unknown event fields, and future item shapes are tolerated,
  and preserved Responses continuation items round-trip losslessly.

The no-retry service also captures the provider HTTP status, `Retry-After`
header, and error payload at the transport boundary, because the SDK's typed
error drops response headers.

Provider context-window failures are normalized through one shared semantic
classifier across OpenAI and Anthropic HTTP/SSE paths. It accepts both the
standard nested error envelope and compatible providers' top-level
`message`/`type`/`code` object. Explicit overflow stop reasons such as
`model_context_window_exceeded` terminate as
`Failed(ContextWindowExceeded)`, not a successful `Length` completion, so
the agent loop's bounded compact-and-retry path owns recovery consistently.
Ambiguous request-size codes (`request_too_large`, `string_too_long`) do not
establish context pressure by themselves: only a message with independent
token/context evidence upgrades them. A generic HTTP byte-size failure remains
`InvalidRequest`/`ProviderError` and never authorizes Surface compaction.

#### Anthropic Messages (direct HTTP/SSE)

Anthropic has no official Rust SDK, and the evaluated community SDK
(`anthropic-sdk-rust` 0.1.x) has stale typed stop-reason coverage relative to
the current Messages API. The Anthropic adapter therefore talks to
`/v1/messages` directly with `reqwest` and `eventsource-stream`:

- correct current streaming semantics (incremental `text_delta`,
  `thinking_delta`, `signature_delta`, and `input_json_delta` deltas emit
  canonical events as they arrive; cumulative `message_delta` usage; a
  protocol `ping` is exposed only as ephemeral Liveness progress;
  `pause_turn`; `model_context_window_exceeded`);
- explicit rejection of server-side fallback: a provider `fallback` block is
  `Unsupported` (never silently discarded), because its position carries
  replay semantics rustX cannot preserve losslessly with the current
  canonical continuation model;
- current request semantics (`redacted_thinking.data` preserved losslessly
  as opaque provider state; `thinking` and `output_config` are provider-owned
  fields the *selected reasoning profile* declares — the adapter synthesizes
  neither);
- current refusal semantics (`stop_reason = refusal` with top-level
  `stop_details`; a human-readable `explanation` streams as `RefusalDelta`
  before `Completed(Refusal)`, never as plain text);
- current stop-reason coverage (`end_turn`, `stop_sequence`, `tool_use`,
  `max_tokens`, `model_context_window_exceeded`, `refusal`, `pause_turn`);
- forward-compatible event parsing (unknown top-level events never crash the
  parser; content-block events with a missing or invalid `index` are hard
  provider protocol errors, never reinterpreted as `0`);
- transparent retry ownership (the transport performs exactly one HTTP
  request per invocation; no retry, no reconnect, no failover);
- no SDK type leakage (there is no Anthropic SDK dependency at all).

The Anthropic wire representation is private to
`src/model/adapter/anthropic/wire.rs`; no alternative canonical Anthropic
model exists.

Canonical deltas are provisional adapter output: M2 reports what the provider
actually streamed, including partial output that a later refusal may
invalidate. Whether provisional content becomes a completed canonical
`AssistantMessageBlock` is owned by the future Agent Loop, never by M2, so no
adapter-local terminal buffering exists for Anthropic text or thinking.

#### Normalization rules

- `ContentBlockIndex` is assigned by rustX, never by the provider. A
  provider-index-to-canonical-index allocator maps provider block identity to
  canonical positions in first-appearance order, so provider tool indexes and
  different content-part layers never shift canonical indexes. Anthropic
  server-side fallback blocks are rejected as `Unsupported` before any
  canonical allocation: their provider positional/replay semantics cannot be
  preserved losslessly with the current canonical continuation model, so
  they are never silently dropped.
- Tool names resolve deterministically to canonical `ToolId` values before a
  request is sent; duplicate model-facing names are rejected before any
  provider request. Provider call ids remain `ToolCallId`; they are never
  synthesized from `ToolId` or from array position.
- Tool argument fragments stream raw (`ToolCallArgumentsDelta`) and the
  complete JSON is parsed exactly once at completion. Malformed completed
  JSON terminates the invocation with a normalized failure.
- When a provider emits both incremental deltas and a cumulative snapshot for
  the same semantic text, reasoning, or refusal value, the adapter accumulates
  the exact streamed value and requires the snapshot to match it. Matching
  snapshots are deduplicated; snapshot-only values are recovered; a
  contradiction fails the invocation rather than being repaired heuristically.
- Continuation state is emitted through the canonical
  `ProviderContinuationState` boundary, never kept in hidden adapter memory.
  OpenAI Responses supports both `Stored` (provider storage,
  `previous_response_id`) and `Stateless` (`store: false`, preserved output
  items including opaque encrypted reasoning). Anthropic thinking signatures
  and `redacted_thinking.data` are preserved as rustX-owned opaque JSON on
  the reasoning block and replayed verbatim; canonical reasoning text alone
  is never sufficient to reconstruct a provider reasoning item (OpenAI
  Responses fails with `Unsupported` instead of fabricating one).
- Usage is normalized without inventing counts: Anthropic effective input is
  `input_tokens + cache_creation_input_tokens + cache_read_input_tokens`
  from the latest cumulative snapshot (never summed over time), reported
  `output_tokens_details.thinking_tokens` maps to
  `UsageDetails::reasoning_tokens`, and `cache_read_input_tokens` maps to
  `UsageDetails::cached_input_tokens` without double counting.
- Cancellation is the runtime-owned `CancellationSignal` flowing through
  the common interface; the network-opening await itself is
  cancellation-aware (a cancellation while waiting for response headers
  aborts the in-flight request), an in-flight invocation stops consuming the
  provider stream, no retry ever occurs, and the invocation terminates with
  `Failed(Cancelled)`.
- Live integration tests are opt-in (`#[ignore]`): ordinary CI runs
  `cargo test --all-targets --all-features` without credentials or network.
  A developer CLI smoke tool (`examples/model_smoke.rs`) streams one response
  per protocol against production credentials.

### Layer 4: Tool plane

The tool plane exposes a single runtime-owned execution contract and multiple executor implementations:

- Native tools
- External source Tools: configured MCP sources and Managed Python packages,
  with separate materialization owners and a shared ordinary Tool contract
- Platform communication tools such as durable message sending

Execution implementations may depend on `rmcp`, process APIs, `uv`, or other libraries. The agent kernel may not.

#### M7 implementation (one external-capability tool plane)

Every model-visible tool is one canonical `ToolDefinition` paired with one
`Arc<dyn ToolExecutor>`. Native Tools and external source Tools from MCP and Managed Python use the same
registry preflight, reserved `__rustx_*` stripping, JSON Schema validation,
execution policy, concurrency policy, progress event, cancellation signal,
foreground/background ownership, and result types. The Agent Loop has no
origin-specific dispatch path.

The base/native/runtime registry is immutable input to capability preparation.
Each candidate composes it with MCP definitions in `McpServerId` order (the
configured server set is a `BTreeMap` keyed by that identity, so the order is
structural rather than a sorting pass) and then remote name; the synthesized
`python:<folder>` servers are entries of that same map, so Python tools take
their place in the same identity order with no origin-specific pass.
The candidate owns a new `ToolRegistry`; a committed `CapabilitySnapshot`
owns that exact registry. A duplicate model-facing name rejects the complete
candidate.

`McpServerRuntime` owns one configured server's rmcp peer, transport, progress
dispatcher, list-change invalidation mechanism, and supervised stdio owner
when used. It is constructed from an `McpServerId` and an identity-free
`McpServerBinding`: the coordinator's `BTreeMap<McpServerId,
McpServerBinding>` key is the one authoritative server identity, so duplicate
identity is structurally impossible and no ordering pass exists.

Connection setup negotiates a protocol revision rather than requiring one.
rustX offers the resolved rmcp build's complete `ProtocolVersion::
KNOWN_VERSIONS`, newest first, through `ClientLifecycleMode::Auto`: rmcp
probes the MCP 2026-07-28 inline `server/discover` lifecycle, walks the
offered list down whenever the peer answers `UNSUPPORTED_PROTOCOL_VERSION`,
and falls back to the legacy `initialize` handshake only when the peer proves
it does not know `server/discover`. Negotiation is for peers whose revision
rustX does not know. A managed Python server — any id in the reserved
`python:` namespace — instead uses the explicit
`ClientLifecycleMode::Discover` lifecycle, because rustX materialized it
against a pinned inline-lifecycle FastMCP and already knows the revision it
speaks: exactly one handshake request is issued and no probe window applies.
Every other peer retains `Auto` unchanged. (See
[pr-321-mcp-handshake-flake.md](pr-321-mcp-handshake-flake.md) for the rmcp
defect this also avoids; it is rationale, not contract.) rustX then
validates the negotiated
revision against its own offered set — the legacy handshake lets a server
echo any revision — and a peer with no shared revision fails with a bounded
`McpError::ProtocolCompatibility` naming both sides. The negotiated revision
also selects the invalidation mechanism: `subscriptions/listen` from
2026-07-28 onwards, the plain `notifications/tools/list_changed` client
callback before it. At most one invalidation mechanism is installed per
connection; when the server advertises `tools.listChanged`, exactly one
revision-appropriate mechanism is installed.

Stdio protocol corruption is a rustX runtime fact, not peer-only traffic.
rmcp's stdio framing deliberately keeps decode failures to itself — plain
non-JSON noise is ignored, and well-formed JSON that is not a valid
MCP/JSON-RPC message earns only a bounded `Invalid Request` reply to the
peer — so rustX installs a narrow observation seam around the unmodified
rmcp transport (`src/tools/mcp/framing.rs`): a read-side tee on the child
stdout pipe that classifies each completed line with rmcp's own codec
(same accept set, one framing authority — the tee never delivers,
correlates, or handles a message itself). On a confirmed structurally
invalid message the seam records a bounded `McpError::ProtocolViolation`
fact and ends the byte stream right after the offending line, which fails
the in-flight connect/discovery/`tools/call` operation structurally and
poisons the connection generation: it never serves another call as
healthy, and physical settlement still belongs to the ordinary runtime
close and generation-retirement machinery. The capability plane records
the failure on the typed `ToolSourceId` belonging to the materialization
owner, so a managed Python package's stdout corruption is diagnosed with
Managed Python provenance by the capability coordinator — there is no
Python-specific parser and FastMCP has no separate framing contract.
#### MCP liveness, connection generations, and recovery (Issue #205)

MCP is a Tool executor and transport adapter. The shared native invocation
lifecycle (extracted from Issue #204 by WF-02) is the generic deadline and
settlement consumer. The Agent Loop alone commits canonical results; MCP owns physical execution, external-effect
certainty, connection health, protocol cancellation, and capability
acquisition.

**The external-effect frontier is `send_cancellable_request` returning
`Ok`.** rmcp fails that call under exactly one condition — its service event
loop is gone, so the request was never enqueued — which makes an `Err` a
proof that nothing was serialized or written. `Ok` proves only enqueueing:
the write happens in rmcp's own send task, and no observation distinguishes
"not yet written" from "written and executing". Everything before that point
therefore settles as an ordinary outcome (`Failed`, or a proven `Cancelled`
when cancellation intent is observed at the pre-dispatch checkpoint, in which
case no request is dispatched at all). After it, only a **correlated remote
response** — a `CallToolResult`, or a JSON-RPC error answering this request
id — proves terminality; transport loss, a poisoned generation, an
acknowledged cancellation notification, and an abandoned response channel all
settle as `OutcomeUnknown`.

Cancellation past the frontier has **two planes, and only one of them holds
settlement authority**:

```text
remote control plane            local ownership plane
  notifications/cancelled         this request's in-flight HTTP request
  best-effort, unacknowledged     (Streamable HTTP only)
  MAY never complete              terminated and proven released by
                                  rustX-owned state alone
```

`notifications/cancelled` is the strongest cancellation the negotiated
protocol defines, and the protocol acknowledges it in no way, so sending it
never proves the remote stopped. It is also never an unbounded prerequisite
of settlement. Over Streamable HTTP a dispatched `tools/call` *is* a live
local HTTP request — its POST future while the response headers are
outstanding, then its SSE response body — and rmcp's client cannot send while
that request is the work it is doing, so awaiting the cancellation send would
leave the branch pending on the very request it is trying to cancel.

rustX therefore supplies the HTTP client the transport posts through
(`src/tools/mcp/streamable_http.rs`) and models each tool invocation's local
half as an **explicit request lifecycle**, keyed by the exact JSON-RPC request
id rmcp put on the wire — never a parallel correlation id, and never inferred
from a record being absent:

```text
   admit(id)                          begin_dispatch(id)
   the executor, one statement        the outbound seam, inside
   after its effect frontier          Transport::send's synchronous prologue
            \                        /
             v                      v     whichever arrives first creates it
         +--------------------------------+
         |        AwaitingDispatch        |  on rmcp's peer outbound queue:
         +--------------------------------+  no local activity has begun and
             |                        |      the outbound participant has not
             |                        |      reached the seam
   the prologue takes ownership   the generation's outbound seam ends,
             |                    so that participant can never arrive
             v                        |
         +-----------------+          |
         |  DispatchOwned  |          |   the send future owns it and has not
         +-----------------+          |   handed it to the inner transport
             |          |             |
   the POST  |          | it refuses, resolves, or is dropped
   registers |          |             |
             v          |             |
         +-------------+|             |
         |  HttpOwned  ||             |   the POST future, then the SSE
         +-------------+|             |   response body it produced
             |          |             |
   every HTTP owner dropped           |
             |          |             |
             v          v             v
         +--------------------------------+
         |            Released            |  terminal: no local participant
         +--------------------------------+  can dispatch it or hold HTTP
                        |                    activity for it again
         the invocation's admission guard drops
                        v
                   (forgotten)
```

Ownership is a **baton, never two parallel owners**: the outbound participant
owns the window in which the POST has not started but is still going to, and
hands that ownership to the POST the moment it registers — holding both would
make local settlement wait for rmcp to resolve an outbound send, which is the
dependency this contract exists to remove.

**`AwaitingDispatch` is an ownership state, not a gap.**
`Peer::send_cancellable_request` returning `Ok` only enqueues the request;
rmcp's service loop dequeues it later and *calls* `Transport::send`, and only
then spawns the future that call returned. Treating that interval as "nothing
local began" is what let a cancelled invocation settle, drop its admission,
and have its entry forgotten — after which the still-live send future
recreated a fresh, uncancelled entry and dispatched a `tools/call` whose
canonical terminal result already existed. Dispatch ownership is therefore
taken in `Transport::send`'s **synchronous prologue**, before the returned
future exists, and one further check immediately before that future could
poll the inner send drops it unpolled if a termination landed in between.

The contract this establishes is stated as one ordering rule:

> Terminal Tool settlement happens-after every local participant capable of
> later dispatching that request id has become terminal.

and one lifecycle rule:

> Request lifecycle authority is created once per request id per connection
> generation and may only move toward terminality. It is never resurrected.

Cancellation reads that phase instead of guessing it, and does something
different in each:

- **`AwaitingDispatch`** — the request's token is cancelled, and settlement
  awaits the outbound participant's own decision. That participant reaches the
  seam, observes the terminal lifecycle state, refuses the request, and
  publishes the release proof; if the generation's outbound seam ends first,
  `Transport::close` (or the transport's own drop) publishes that the
  participant can never arrive and releases the proof instead. Either way the
  request never reaches the network;
- **`DispatchOwned` / `HttpOwned`** — that exact local activity is terminated
  and settlement awaits an explicit release proof: the wrapper drops the send
  future's inner send, the POST future, or the response body **first** and
  releases the latch **afterwards**, so the latch is real ownership evidence
  rather than a restatement of "we stopped waiting";
- **`Released`** — every local participant already finished. This is the race
  the earlier shape could not see: a cancellation landing after the POST
  released but before rmcp delivered the correlated response used to look
  identical to "the POST has not started", so it created a pre-termination
  record for a request that could never register again, and that record then
  survived until the connection generation closed. Now nothing is terminated
  and nothing is recorded; the executor only arbitrates the response that is
  already on its way.

A tracked POST registration **requires the dispatch baton**. `Released ->
HttpOwned` is therefore impossible, and so are `HttpOwned -> HttpOwned` and
`AwaitingDispatch -> HttpOwned`: each is refused with a pre-cancelled,
untracked guard that can neither reach the network nor release ownership that
is not its own.

That proof depends on no remote response, no protocol acknowledgement, and no
timer — timing a future out, or dropping one and calling the drop a proof, is
never settlement evidence here. Every entry has a **request-local forget
point** — its invocation's admission guard *together with* its outbound
participant's decision — so normal completion cleans up its own state with the
connection still open; the memory bound is the in-flight tool-call count,
never the count of requests the generation has ever raced. An entry is removed
only once every participant is terminal, which is precisely what stops a late
participant from finding its entry missing and minting a fresh one.
Over stdio an outbound write owns no resource that outlives it, so there is no
local half at all and the cancellation send is awaited exactly as before.

The response channel is retained across the whole path, so a **correlated
remote response always wins**: rmcp resolves the request's local responder in
the same event-loop step in which it reports the notification's send outcome,
and rustX's own local termination resolves it through the ordinary
transport-send failure path, so after either fact the channel is *checked*
without waiting. A call that terminated its own transport-level request does
not read the resulting transport-send failure as evidence about the
connection: the HTTP session survives an aborted request. Nothing is reported
until the local ownership proof resolves, which is what keeps `Unconfirmed`
honest — no task, no process, no in-flight HTTP request, and no response body
of that invocation remains. Connection close terminates every request the
generation still owns locally before joining rmcp's transport worker, so no
per-request control primitive outlives drain. MCP never selects a canonical
terminal status; the lifecycle maps confirmed settlement to `Cancelled` or
`TimedOut` from the winning cause.

Remote progress notifications flow through the one generic
`ProgressReporter` seam and refresh only the idle watchdog — never the
immutable hard deadline — and the executor fabricates no heartbeats. Because
progress is *liveness evidence*, the router's guarantee is stated precisely:
for every admitted in-flight request, once genuine remote progress has been
observed, no bound in the router erases the fact that it occurred before the
dispatching call can consume it. Payload detail is explicitly not
guaranteed — payloads coalesce and a subscriber's queue drops on full — but
losing the *occurrence* would turn real progress into a false idle timeout.

"Has observed" marks where the adapter's ownership begins: rmcp hands
inbound notifications to the handler on a spawned task while resolving a
response's local responder inline, so a notification sent immediately before
its response may not have reached the router when that response settles the
call. rustX cannot preserve evidence it was never given, and that window is
not a liveness hazard — it can only lose a notification when the correlated
response has already arrived, so the call settles at once and no idle
watchdog is consulted.

Three windows inside the adapter could lose it. rmcp mints a request's
progress token *inside* `send_cancellable_request`, so the dispatching call
can only subscribe after the request is enqueued and a server answering
immediately races that subscription; because a correlated response outranks
progress in the executor's biased arbitration, a ready response would
otherwise end the call with notifications still queued; and a response
arriving inside that same pre-subscription window is not evidence that the
call is over.

The subscription window is closed by **ownership, not by capacity**. Bounding
a speculative pre-subscription cache cannot be correct: nothing in the
architecture bounds how many admitted requests are inside that window at once,
so any capacity there makes a legitimate live request's only liveness
occurrence an eviction candidate — including at the hands of another
legitimate request. rustX therefore hands rmcp its own transport wrapper
(`src/tools/mcp/dispatch.rs`) and registers a tool invocation's
`(RequestId, ProgressToken)` pair **synchronously inside `Transport::send`**,
before the message reaches the wire. That registration *happens-before* the
server can receive the request, which happens-before the server can emit
progress for its token, so there is no unowned request-token window at all:

```text
send_cancellable_request -> Ok
  Transport::send prologue  ->  the token becomes known and live
    bytes on the wire
      the server receives the request
        the server emits progress for that token
          the router delivers it            <-- always after registration
executor -> subscribe(...)                  <-- may be anywhere after Ok
```

Progress therefore has exactly **two classes**, and only one is evictable:

| | known live request token | unknown / unsolicited token |
|---|---|---|
| origin | registered by the dispatch seam | any other token a peer emits |
| storage | O(1) per token | none at all |
| eviction | **never** by capacity | counted, then dropped |
| removal | the request's own terminal forget point | n/a |

They do not share a container, so peer-controlled traffic has no capacity to
consume and nothing of a live request's to displace. A known token's state is
one coalesced entry — the latest payload plus an occurrence count — handed
over on subscription; the executor drains its subscription before classifying
a response or settling a cancellation; and the live subscription set is
bounded by the in-flight call set rather than by any router constant, removed
by its owner's drop. A full delivery queue cannot erase an occurrence either,
because it is full precisely when that many undelivered proofs are already in
the subscriber's hands. **Payloads may be coalesced; the occurrence may not be
lost.** The router only reorders and coalesces notifications the peer
genuinely sent, and fabricates nothing.

The third window is closed by keeping two facts apart. **A correlated
response ends remote execution but does not end the dispatching caller's
progress-consumer ownership** — the peer can answer while the call is still
between its effect frontier and its subscription, and reading that answer as
proof that no caller remains deletes exactly the evidence the guarantee is
about. Each dispatched request therefore carries three independent facts:
whether a correlated answer can still arrive (it cannot, once the peer
answered or rustX refused to dispatch the request), and whether the
dispatching call still owns progress (awaiting subscription, subscribed, or
relinquished), plus whether outbound admission has been consumed. **A live
caller owns progress until it consumes or relinquishes it. After caller
relinquishment, request state is retained only while needed to prevent a
not-yet-consumed outbound admission from recreating ownership.** This
payload-free tombstone has no delivery index; late admission consumes and
forgets it without recreating an owner. Remote terminality also removes the
tombstone. Once admission has happened, relinquishment forgets immediately,
even if the remote server never answers. Remote execution uncertainty does
not by itself retain progress-router state: the canonical `OutcomeUnknown`
Tool result owns that uncertainty. Router cardinality is O(live callers +
unresolved outbound admissions), never historical ambiguous calls.
A subscription is
never refused because its request is already answered; it claims the
buffered occurrence atomically and the executor reports it before the
terminal result. The caller dimension is one RAII lease taken in the same
await-free step that admits the request locally, so cancellation, a deadline,
transport loss, protocol corruption, a refused dispatch and ordinary
completion all relinquish through one path rather than through per-branch
cleanup.

**`McpConnection` (`src/tools/mcp/connection.rs`) is the stable connection
owner** of one configured server, and it is what a published capability
generation and every discovered executor bind to. Underneath it, one
*connection generation* is one concrete negotiated transport/session
authority: one spawned stdio unit or HTTP session, its handshake, its
negotiated revision, its peer, and its own corruption seam. Executors resolve
the authoritative generation at dispatch, so a replaced transport is served
immediately by every already-admitted tool. A generation dies only on proof
(closed runtime, confirmed protocol violation, transport-class rmcp failure),
that proof is recorded monotonically so repeated loss signals retire it once,
and replacement is bounded by construction: **at most one connect attempt per
dispatch**, driven inline by the dispatching execution future, before that
dispatch crosses its own frontier. There is no reconnect loop, no backoff
timer, and no detached reconnect task to own.

**Reconnect is therefore never replay.** Reconnection is reachable only from
a new dispatch's transport resolution; there is no in-flight queue, no
correlation-id carry-over, and no resubmission path. A request that crossed
the frontier lives entirely inside its own execution future and has already
reached a terminal classification before any replacement generation exists.

**Configuration reload publishes only a complete successful candidate.** A
selected-source failure rejects the entire candidate, including changed binding
metadata and policy. The exact prior generation stays authoritative. No
last-known-good executor is installed under a replacement endpoint or policy.
Physical transport recovery within an unchanged frozen binding remains owned by
the existing connection lifecycle; it is separate from configuration publication.


Drain closes publication authority: closing a connection cancels its
ownership root first (so a connect in flight settles its own process and
returns), then drives every generation it established to physical
settlement. Afterwards no acquire can establish a generation, no reconnection
can spawn a server, and a later dispatch through a stale handle settles as an
ordinary pre-frontier failure.

Executors capture an `Arc` to the server's connection, not to one transport
generation. The observed remote tool surface
and binding are immutable for a capability revision, but rustX does not claim
to snapshot the implementation behavior of the independent remote server.
`tools/list_changed` epoch mutation and capability snapshot activation share
exactly one synchronization boundary — the mutex-protected MCP invalidation
state: notification epoch advancement, preparation epoch snapshots, and the
commit's final epoch validation plus snapshot swap all serialize through the
same guard. If the notification wins first, the prepared candidate cannot
commit and the active snapshot is unchanged; if the commit wins first, the
notification belongs to a future refresh and can never retroactively
invalidate the already-committed snapshot. Lock ordering is explicit and
documented (capability state lock -> MCP invalidation guard; the notification
path holds only the guard), so no cycle exists. Preparation rejects a catalog
that changes during discovery, and commit rejects a candidate whose epoch
changed before the protected snapshot swap.

MCP stdio uses the runtime-owned interactive supervisor unit, whose control
sockets are separate from the server's stdin/stdout protocol pair. The unit
is the M5 Bash supervisor shape applied to a long-lived server, composed
from the same shared structural ownership core
(`src/runtime/supervised_unit`): an inner supervisor calls `setsid()`,
applies the fixed-membership restriction on Linux (macOS has no seccomp
equivalent), and issues `TERM -> grace -> KILL` with `killpg` against its
own process group; an outer supervisor is the reaper of last resort with the
single-owner anchor discipline and the authoritative terminal report. The
kernel-mediated terminal proof is the group-scoped wait
(`waitid(Id::PGid)` returning `ECHILD`) — never a `/proc` scan and never a
`killpg(0)` probe **on Linux, where child-subreaper adoption plus the
fixed-membership restriction make that a complete whole-group proof**. On
macOS that `ECHILD` only proves the waiting supervisor has no waitable group
child left, so macOS instead escalates to the outer's fallback containment
`SIGKILL` and proves the group absent with a bounded `killpg(pgid, 0)` probe
reaching `ESRCH` — never a fabricated whole-group emptiness claim. rustX's
detached driver task owns physical settlement
from the moment the supervisor spawn succeeds, drains the server's stderr
until EOF (bounded preview), reaps the direct supervisor child before
publishing settlement, and runs Linux's adopted-anchor emergency containment
when the unit is lost; macOS reports the lost-anchor case as unproven.
Startup is
ownership-gated in both directions: the outer supervisor may create the unit
hierarchy only after rustX accepted and retained its control connection
(`MSG_OWNER_ATTACHED`), and the outer attaches its inner supervisor with a
bounded pre-ownership state machine (inner connection, inner exit, upstream
loss) instead of a blocking accept. `MSG_ANCHOR_READY` is then the anchor
commit point: only a unique, positive, pid-matching announcement gives the
inner pid its second meaning as the owned process-group id. Before it the
outer owns the inner strictly as a direct child pid — no group-scoped wait
may run against that pid — and reports `NoOwnership` only after proving
that direct child reaped; an unprovable pre-anchor reap reports a
process-control failure and no `NoOwnership`. After the startup gate has
opened, bare pre-ownership plus control loss is therefore never a terminal
proof. Both gates are pure recognition points, not reader boundaries: each
stream direction has exactly one buffered control-frame reader for the whole
connection lifetime (rustX -> outer across the startup gate, the pre-inner
drain, inner attachment, the pre-anchor phase and the anchored relay loop;
outer -> inner across the START gate and the owned inner control loop), and
every phase drains what is already buffered before it waits for another
read. `NoOwnership` itself is parsed by one strict, fail-closed grammar
(empty payload, or a four-byte positive reaped pid; anything else is a
protocol error). Physical settlement is published only
with proven terminality; an unproven terminal state is returned as an
explicit error from `wait_for_settlement`/`McpServerRuntime::close`, never as
a successful settlement. Streamable HTTP
uses the current rmcp client transport with explicit static headers and no
`Mcp-Session-Id` compatibility state.

Custom Python packages are discovered only from
`<workspace>/.agents/tools/<package>/` (Issue #174). One folder is one
package; its contract is two files: `server.py` (the FastMCP server, launched
through the fixed entrypoint `server.py:mcp`) and `requirements.txt` (required
even when empty; declaring `fastmcp` itself is rejected with a
package-identifying diagnostic, because the FastMCP build is rustX-pinned at
exactly `MANAGED_FASTMCP_VERSION` and the MCP protocol peer's identity is
never workspace-declared). Every `@mcp.tool` function in the server becomes
one tool; FastMCP derives name, description, and input schema from the
function's name, docstring, and type hints, so those are the schema authority.
Candidate preparation freezes the package bytes in memory and computes one
fingerprint over the material inputs: the package identity (the
synthesized `python:<folder>` MCP server identity), every package file
(path and bytes, including `requirements.txt`), the probed Python
interpreter identity (path + version), the probed uv identity (path +
version), the FastMCP pin, and OS/arch — no timestamps, and no host paths:
the folder name is the logical identity, so relocating the workspace never
changes a package's identity, while two distinct folders never share one
prepared environment even when every user-authored byte is identical
(cross-package environment deduplication is explicitly out of scope). The
preparation then publishes one immutable
prepared state per fingerprint under the runtime-private store
(`<environment-store>/python-tools/packages/<fingerprint>/`): the frozen
`source/` copy, the rustX-generated `pyproject.toml` (the package's
requirements plus `fastmcp==<pin>` and a `requires-python` tracking the probed
interpreter series), the rustX-generated `uv.lock`, the `venv/` built with
`UV_PROJECT_ENVIRONMENT`, and the rustX-owned `manifest.json` recording every
identity input and the frozen source digest. A build runs `uv lock` then
`uv sync --frozen` in a sibling staging directory and publishes with one
atomic rename; same-fingerprint in-flight builds coalesce behind one
store-owned owner task (callers only wait; owner failure publishes a terminal
error and removes the in-flight entry). Reuse is fail-closed: a state whose
manifest is missing or invalid, whose identity inputs no longer match (the
fingerprint, the package identity, the fixed entrypoint, the managed FastMCP
pin, the probed Python/uv identities, OS/arch), whose
frozen source no longer hashes back, or whose venv interpreter is gone is an
explicit preparation failure, never a silent reuse and never a repair. The
manifest's `origin` field is deliberately non-authoritative host provenance
for diagnostics; reuse never depends on it. A
changed fingerprint prepares a new state directory; the old one is left
untouched (no GC exists). The interpreter whose identity enters the
fingerprint is pinned to uv via `UV_PYTHON`, managed Python downloads stay
disabled, and every preparation command has a finite deadline (a timeout is an
explicit preparation failure). The environment isolates dependencies, not
filesystem, network, or security permissions. The `PythonToolStore` is
initialized lazily — Python is optional, so construction belongs to Python
preparation and a failure degrades availability without poisoning anything —
but once initialized it is owned for the `CapabilityCoordinator` lifetime and
is the single process-local coordination domain for Python build coalescing;
it is never reconstructed per preparation.

A prepared package synthesizes exactly one generic `McpServerBinding` with the
server identity `python:<folder>`: a stdio launch of the prepared venv's
interpreter running `python -m fastmcp.cli run <state>/source/server.py:mcp
--skip-env --no-banner` with banner, update check, and bytecode caches
silenced so stdout stays reserved for the MCP wire. The launch never
re-resolves dependencies — it names the venv interpreter directly, never
`uv run`, and never re-enters the store. From that binding on, everything —
connect, `tools/list`, the frozen catalog epoch and `tools/call` — uses the
existing MCP adapter. The coordinator publishes availability and ordinary
Tool provenance as `ToolSourceId::ManagedPython(package)`. Commit, leases
and the frozen child crossing preserve that semantic source identity; the
transport binding never becomes the Agent/Workflow selection vocabulary. The revision a managed
package speaks is therefore a property of that generic connection, not of the
package: the rustX-owned peer (FastMCP 4, Issue #241) answers the modern
`server/discover` probe, so a managed child is offered
`ClientLifecycleMode::Discover` and negotiates MCP `2026-07-28` with one
handshake request and no probe deadline, and rustX's negotiated fallback to
genuinely older external peers is unchanged and unrelated to this pin. One
folder is one server
identity, and multiple tools of one folder arrive through one `tools/list`;
no uv command and no process spawn happens per `tools/call` — the prepared
venv interpreter is launched once per committed generation and reused. The
generic MCP connect + `tools/list` that follows preparation is the server
validation, and the candidate-generation machinery guarantees a failure
leaves the previously committed generation intact.

#### MCP multi-round-trip tool calls (Issue #242)

MCP `2026-07-28` (SEP-2322) allows a server to answer `tools/call` with an
`InputRequiredResult` instead of a `CallToolResult`. rustX adopts the subset
that maps cleanly onto its existing human-interaction ownership —
**Elicitation** — and treats that answer as an intermediate state of the
already-admitted invocation:

```text
model ToolCall A
    |
    v
rustX ToolInvocation A ──── approval evaluated once, before ToolExecutor::start
    |
    +--> tools/call round 1 --------------------------+
    |        |                                        |
    |        +--> CallToolResult -------------------->-+---> exactly one
    |        |                                        |      terminal
    |        +--> InputRequiredResult                 |      ToolExecutionResult
    |                 |                               |
    |                 v                               |
    |        one runtime-owned Questionnaire          |
    |        (the same InteractionCoordinator that    |
    |         serves native ask_user)                 |
    |                 |                               |
    +--> tools/call round N+1 ----------------------->+
```

The whole loop lives inside the single operation future of
`ToolExecutionHandle::settled_by_operation`, on the one connection generation
resolved before the first round. So one model `ToolCall` remains one
`ToolExecutor::start`, one `ToolInvocationId`, one `ToolExecutionId` (when
detached), one server binding, one execution lease, one remote tool name, one
set of business arguments, one progress stream, and one terminal result — and
the Agent Loop, the background registry, and the provider adapters see nothing
new. `src/tools/mcp/mrtr.rs` owns protocol translation only; the round driver
composes the existing dispatch frontier, cancellation arbitration, progress
ownership, local release proof, and protocol poisoning rather than replacing
any of them.

**Interaction authority is one narrow crate-private seam, now shared.** The
MCP adapter is rustX-owned code and consumes the same
`QuestionnaireRequester` that native `ask_user` does:

```text
                InteractionCoordinator        (the only human-interaction owner)
                        ^
                        |
              QuestionnaireRequester          (crate-private, attempt-bound,
                   |          |                publish-and-await only)
              ask_user     MCP MRTR
```

It carries the attempt identity, a read-only cancellation view, and the
conversation-owned coordinator, and can do exactly one thing: publish one
bounded Questionnaire and await its typed response. It cannot trigger
cancellation, run a model, settle Approval, mutate canonical history, or
create any other kind of interaction. `ToolExecutionContext` still exposes no
generic interaction capability, so an externally registered `ToolExecutor`
cannot acquire one.

**The capability is advertised per request, because that is the granularity
of the authority.** rmcp populates `_meta` client capabilities (SEP-2575) on
every request of a `2026-07-28` connection, so rustX declares
`elicitation { form }` on a `tools/call` exactly when that invocation holds
the Questionnaire capability — foreground Agent Loop dispatch and Workflow
native invocation do; a detached background execution does not. The
connection handshake itself is untouched (`ClientCapabilities::default()` on
both the legacy `initialize` path and the inline `server/discover` probe), so
rustX never claims the legacy server-initiated `elicitation/create` callback
it does not implement, and legacy connections see no request `_meta`
capabilities at all.

That is also the honest answer for background work. rustX has no background
human-interaction domain: `ask_user` is foreground-only and an interaction is
owned by a live attempt, whose identity a detached execution does not have.
Rather than invent a background waiter, attribute an interaction to an
already-terminal attempt, or widen the interaction protocol for a domain that
does not exist, a background MCP invocation tells the server the truth and
refuses an `input_required` answer with a bounded diagnostic — under its own
`ToolExecutionId`, with exactly one settlement. Background MCP execution
itself is unchanged; only server-driven elicitation inside it is refused.

**Sampling and Roots are refused, not implemented.** A `sampling/createMessage`
input request would make an MCP server an initiator of rustX model execution;
a `roots/list` input request would expose host authority through a deprecated
surface that duplicates rustX's Workspace ownership. Both produce a bounded
unsupported-feature diagnostic, with no model call and no workspace
disclosure. A round mixing supported and unsupported requests fails as a
whole, before any prompt is published.

**The Elicitation form maps onto the typed question vocabulary, constraint by
constraint.** One MCP property becomes one rustX question, and the core
invariant of `src/tools/mcp/mrtr.rs` is:

> rustX never emits an MCP `accept` whose `content` has not been validated
> against every constraint of the original requested schema that rustX claims
> to support — and it never claims to support a schema shape whose
> constraints it would then discard.

So every supported field of every rmcp schema type is either **preserved and
validated**, or its presence **refuses that schema instance**:

```text
StringSchema   title -> question header, description -> prompt,
               minLength / maxLength     -> Text { min_length, max_length }
               format date|date-time|uri -> Text { format }        validated
               format email              -> REFUSED (no faithful validator)
NumberSchema   minimum / maximum (f64)  -> Number { minimum, maximum }
                                          finite binary64, the identity
                                          translation
IntegerSchema  minimum / maximum (i64)  -> Integer { minimum, maximum }
                                          exact i64, the identity translation
BooleanSchema  (no constraints)          -> Boolean
enum single    enum | oneOf | enumNames  -> SingleChoice { options,
                                              allow_custom: false }
enum multi     enum | anyOf,
               minItems / maxItems       -> MultiChoice { options,
                                              min_selected, max_selected,
                                              allow_custom: false }
URL elicitation                          -> REFUSED (not a questionnaire)
```

`default` is deliberately **not** a constraint: it is an authoring hint, and
rustX does not pre-fill an answer on a human's behalf, so ignoring it can
never produce schema-invalid content. That decision is documented rather than
silent. `maxLength` above rustX's own 4096-scalar answer bound is narrowed to
that bound, which is strictly stricter than the server's constraint and
therefore still schema-valid; a `minLength` above it is impossible to satisfy
and refuses the schema.

Because the bounds live in the **provider-independent** question
specification, the authoritative validator is the shared one: a multi-select
with `minItems: 2, maxItems: 2` rejects one and three selections and accepts
two, and a fractional value is not even a spellable answer to an `integer`
question. No response that violates a declared bound can reach an MCP
continuation.

Question order is `(input-request key, schema property order)`, both
deterministic, and answers map back by server-assigned key rather than by
position. A whole-questionnaire decline, or an unanswered required property,
becomes the protocol's own `decline` action. Provider unavailability keeps the
existing coordinator contract and is never reported as a human decline.

Two failure classes stay strictly separate, which is what makes the human
experience recoverable:

```text
the server requested a schema rustX cannot represent
    -> deterministic unsupported-feature failure of the invocation
a human typed something the declared answer shape refuses
    -> the interaction response is refused, the interaction stays pending
```

**MCP-originated prompts name their server.** The Questionnaire's
`InteractionRequester` is built from the registry-resolved invocation and this
runtime's own `McpServerId`, so a Runtime Client renders
`Requested by MCP server: <id>` / `Tool: <name>` from canonical facts. A
native `ask_user` prompt carries `ToolOrigin::Builtin` and is never labelled
as MCP. The routed source (primary or subagent) remains a separate field.

**`requestState` stays protocol-owned.** It is retained on the executor's
stack for the lifetime of the invocation, returned byte for byte on the next
round, never parsed or re-encoded through a rustX schema, and dropped at
terminal settlement. It never reaches canonical history, the model-issued
`ToolCall` arguments, the Event Journal, or Goal/Workflow/Subagent durable
state, and a process restart never resumes one. The Event Journal does record
the ordinary requested/settled facts of any Questionnaire a round published —
those are runtime execution facts, and they are audit evidence rather than
continuation authority.

**The bound is fixed and rustX-owned.** `MCP_MRTR_MAX_ROUNDS = 10` counts the
initial call as round 1, matching the convention rmcp's own
`DEFAULT_MRTR_MAX_ROUNDS` uses, and is checked the moment an intermediate
result arrives — before translation and before publication — so a server that
never terminates cannot even create pending interaction state on its way to
being refused. Every payload is bounded too: at most four input requests per
round, at most four derived questions (the shared `ask_user` bound), 8 KiB of
`requestState`, 64 KiB of input requests, 16 KiB of `inputResponses`. There is
no configuration switch and no generic limits framework.

**Cancellation composes rather than competes.** The continuation dispatch
frontier *is* the existing pre-dispatch cancellation checkpoint: observable
cancellation there means no new round is dispatched, so a human response that
lost the race can never create fresh remote ambiguity. Everything past the
frontier keeps its existing meaning — a correlated remote response wins,
everything else is `OutcomeUnknown`, local request ownership is terminated and
proven released before anything is reported, and a poisoned generation stays
poisoned. One case is new and is resolved in the same spirit: a correlated
`InputRequiredResult` observed after cancellation intent has already won
settles as a proven `Cancelled`, because that round's remote outcome *is*
known and rustX will not start another.

#### MCP Tasks (Issue #243)

MCP `2026-07-28` also carries the **Tasks extension**
(`io.modelcontextprotocol/tasks`, SEP-2663): a server may answer `tools/call`
with a `CreateTaskResult` (`resultType: "task"`) instead of a result. rustX
adopts it as an **adapter-local remote sub-lifecycle of one already-admitted
`ToolInvocation`**, never as a rustX task:

```text
remote MCP Task = execution-local protocol state inside one ToolInvocation
remote MCP Task != rustX background task, Scheduler job, WorkflowRun,
                   Subagent, Goal, Todo, durable runtime entity, or a second
                   ToolExecutionId
```

```text
model ToolCall A
    |
    v
rustX ToolInvocation A ──── approval evaluated once, before ToolExecutor::start
    |
    +--> tools/call round 1..N (the SEP-2322 MRTR loop above)
    |        |
    |        +--> CallToolResult -----------------------------> exactly one
    |        |                                                   terminal
    |        +--> CreateTaskResult                                ToolExecution
    |                 |                                           Result
    |                 v
    |          RemoteTaskActive  (no tools/call is ever sent again)
    |                 |
    |                 +--> tasks/get -> working
    |                 |        bounded, cancellation-aware wait, then poll
    |                 +--> tasks/get -> input_required
    |                 |        one runtime-owned Questionnaire, then
    |                 |        tasks/update, then keep polling
    |                 +--> tasks/get -> completed / failed / cancelled ------->+
    |                 +--> tasks/cancel (best effort, only when rustX
    |                          cancellation or a deadline won)
```

A task may be materialized by **any** round, including an MRTR continuation:
`InputRequiredResult -> Questionnaire -> tools/call -> CreateTaskResult` is a
valid flow. Once a task exists the round loop is left for good, so the
original call is never replayed and a second task identity cannot appear.

**Capability advertisement is truthful and per request.** On a `2026-07-28`
connection every request of an invocation declares
`extensions { io.modelcontextprotocol/tasks }` in its own `_meta`, because
driving a task needs no human and rustX implements the whole client half.
`elicitation { form }` is declared *additionally* and *only* when that
invocation holds the runtime-owned `QuestionnaireRequester`. The two are
independent: a detached background invocation drives
`CreateTaskResult -> working -> completed` while advertising no elicitation at
all. Legacy peers are unchanged — the handshake still carries
`ClientCapabilities::default()` and legacy requests carry no `_meta`
capabilities. The server side of the contract is kept too: a peer that did not
advertise the extension in its negotiated capabilities does not acquire the
right to answer with a `CreateTaskResult`, and one that does is refused.

The polling and ACK contracts follow the current
[2026-07-28 Tasks specification](https://github.com/modelcontextprotocol/ext-tasks/blob/main/specification/2026-07-28/tasks.md)
and the resolved rmcp 3.2.0 `model/task.rs`. Creation carries the same
polling hint as later snapshots, with no first-poll exception. rmcp's
convenience update/cancel helpers also accept `EmptyResult`; rustX uses the
strict negotiated `TaskAckResult` through its existing request owner.

**The polling owner is the operation future, not a task.** `tasks/get` runs in
the same `ToolExecutionHandle::settled_by_operation` future as everything
else. Nothing is spawned and no timer outlives the invocation, so termination
stays with the existing cancellation signal and the generic Issue #204
deadline. `pollIntervalMs` uses a 25 ms minimum floor with no maximum
clamp (default 500 ms). Every poll, including the first after creation and
the next after an update, honors the hint. The wait is a `select!` on the invocation's own cancellation signal, so a
server can create neither a busy loop nor a wait that defeats cancellation.
Timer construction uses checked instant arithmetic; an unrepresentable next
instant leaves the wait pending until local cancellation/deadline, never
causing an earlier poll.
`ttlMs` is read as the server's retention metadata and never as a second local
deadline. Polling is transport activity: another `tasks/get` is **not**
reported as Tool progress, so no liveness is fabricated.

**One dispatch owner for four methods.** `tools/call`, `tasks/get`,
`tasks/update`, and `tasks/cancel` all go through the same private
`dispatch_owned_request`, which owns the pre-frontier rejections, the
`send_cancellable_request` effect frontier, request-lifecycle admission,
progress ownership, cancellation arbitration, the local release proof,
protocol poisoning, and transport-loss recording. It reports *facts*; each
caller applies its own result vocabulary. The outbound seam's ownership was
widened to match, so a task request over Streamable HTTP is a rustX-owned
local participant that a cancellation can terminate and prove released.

**In-task input reuses the #242 translation exactly.** A task's
`inputRequests` are the very same rmcp type an `InputRequiredResult` carries,
so `src/tools/mcp/mrtr.rs` plans them unchanged: one bounded typed
Questionnaire through the one crate-private `QuestionnaireRequester`, one
typed response, one `inputResponses` map. There is no task-specific
interaction coordinator, no task-specific question vocabulary, and no public
interaction seam. Sampling, Roots, and URL-mode elicitation stay unsupported
for the same ownership reasons.

**`tasks/update` is eventually consistent, and rustX answers a key once.**
The specification acknowledges an update before the next snapshot must
reflect it, so a later `tasks/get` may legitimately repeat a key that was just
answered. `RemoteTask` remembers every answered key for the task's lifetime —
recorded at the update *dispatch frontier*, before the request exists — so a
stale repeat publishes no second interaction and sends no second update, while
a genuinely new key in the same snapshot is still processed. The set is
bounded at 32 distinct keys, so a server cannot grow execution-local state
without limit.

**Cancellation composes, and `tasks/cancel` proves nothing.** Local
cancellation is checked at each task-request dispatch frontier and in the poll
wait, so no further `tasks/get` or `tasks/update` is dispatched after the
terminal decision. An in-flight task request is terminated and proven released
exactly like a `tools/call`. rustX then sends at most one cooperative
`tasks/cancel` — bounded by a fixed local wait on its acknowledgement, since
the acknowledgement is not evidence — and settles. Two mechanisms stay
distinct: `notifications/cancelled` cancels one in-flight JSON-RPC request and
says nothing about the task, while `tasks/cancel` signals intent about the
task and still does not prove the remote effect stopped.

rustX stops driving an addressable active task only after at most one
best-effort `tasks/cancel`. Missing Interaction authority, unsupported input,
local continuation failures, and method/result mismatches follow this rule.
A closed, failed, or poisoned generation and an invalid/untrusted task id
cannot carry a cancellation; rustX settles without replay or resumption.
`tasks/update` and `tasks/cancel` accept exactly rmcp 3.2.0's `TaskAckResult`
(`resultType: "complete"`, optional `_meta`, no other fields). An unrelated
successful result is a method/result mismatch, not an ACK or connection
poison. Even a valid cancellation ACK leaves the task outcome unknown.

**Effect certainty after materialization.** A `CreateTaskResult` *is* remote
work in progress, so every outcome other than the task's own terminal state
settles as `OutcomeUnknown` — including a cancellation rustX itself decided.
Claiming a proven `Cancelled` there would assert something no part of the
protocol can establish. The task's own terminal states are different: a
`completed` task is projected through the ordinary MCP result path (so
`isError: true` stays a completed task with a failed tool), a `failed` task
becomes one bounded `Failed` carrying the protocol's correlated error, and a
`cancelled` task becomes a `Failed` that names the remote authority rather
than inventing a local `CancellationReason`.

**Generation loss is fail-closed.** The remote task belongs to the invocation
and to the connection generation that created it. rustX never persists a task
id, never replays the original `tools/call`, never reacquires a generation to
keep polling, and never resumes a task across a process restart. A task that
outlives rustX's connection continues on the server, and rustX reports an
unknown outcome rather than pretending otherwise.

### Layer 5: Agent resources

`.agents` defines Skills, named Agents, Workflows, Managed Python packages and
MCP servers at exactly User and Workspace roots. Each same-name Workspace
identity replaces one whole User resource before validation. Invalid winners
stay invalid; they cannot expose lower definitions through fallback.

[CFG3 configuration](configuration.md) specifies paths, selection, bounded
ordered diagnostics, native authoring, semantic overlay and demand. Resource
existence is independent of Root authority. Skill prompts expose metadata and
absolute roots for progressive disclosure. Python and MCP lifecycle owners
materialize only admitted finite demand.


### Layer 6: Runtime services

This layer owns execution infrastructure:

- Cancellation hierarchy
- Runtime event writer
- Message store interface
- ConversationStore integration for Ledger, Surface, Request Snapshot, and
  Event Journal durability (the in-memory model is only a bounded hot read
  model)
- Capability revision management
- Capability mutation guard
- Process supervision
- Background shell session management
- Durable recovery evidence, the (M9a) startup recovery pipeline that
  classifies and reconciles it, the M9b model-start arbitration, and the M9c
  runtime supervision/quiescence drain contract

#### M6 implementation (capability coordination)

M6 implements the concrete capability snapshot/mutation semantics required
for Skills in a narrow coordination layer (`src/capabilities`), not a
generic runtime supervisor:

- **Immutable attempt capability snapshot.** One `CapabilitySnapshot`
  holds the monotonic `CapabilityRevision`, the immutable `ToolRegistry`
  handle, the immutable Skill snapshot/catalog with its
  `SkillId` + `SkillVersionId` bindings, the Python/Node environment
  identity and path when present, and the effective `ToolEnvironment`
  (base authorized environment plus the deterministic Skill environment
  overlay).
- **Capability owner identity.** A `CapabilityCoordinator` is explicitly
  conversation-owned and records the canonical Workspace root with its
  `ConversationId`. An attempt lease can only be passed to a
  `ConversationToolRuntime` with the same conversation/workspace ownership
  domain; construction rejects a mismatch before model or tool execution.
- **Attempt capability lease.** An `AgentExecution` structurally holds one
  RAII lease pinning one immutable snapshot for its complete lifetime; no
  model turn re-discovers Skills or re-queries the conversation capability
  pointer. There is no capability-free constructor.
- **Quiescent commit.** Candidate preparation (discovery, dependency
  merge, environment materialization) runs independently; activation is a
  quiescent atomic commit legal only when zero attempt leases are active
  for the conversation. Acquisition and commit serialize through one
  synchronization boundary; an identical candidate is a no-op that never
  fabricates a revision; a stale candidate (prepared from an obsolete base
  revision) is rejected; failed preparation/commit leaves the active
  revision authoritative. Conversation-owned detached background
  executions do not hold attempt leases and never block a capability
  commit.
- **Executable identity is part of the no-op equivalence.** A candidate is
  a no-op only when the model-visible capability contract (tool
  definitions, available catalog, Skills, environment digests) **and** the
  effective executable/runtime binding identity (the frozen MCP server
  bindings: command/args/env/cwd/endpoint/headers/policy) are both
  equivalent. A changed executable binding — a configured server whose
  launch changed, or a managed Python package whose source edit moved its
  prepared state to a new fingerprint-keyed directory and changed the
  synthesized launch program — is a real publication even when the
  `tools/list` schema is byte-identical; after a successful commit, newly
  admitted executions use the new executable generation. An
  already-admitted execution keeps its old generation (and the retired old
  MCP runtime closes only after its leases settle).
- **Background environment capture.** The effective environment is
  captured at background dispatch prepare time (before the ownership
  commit) and retained by the detached execution; later revision
  activations never mutate it.

#### M7 capability additions

Capability preparation now owns the full composition transaction:

```text
base/native/runtime tools + prepared MCP (managed Python packages included)
    -> candidate ToolRegistry -> candidate CapabilitySnapshot -> commit
```

There is no mutable process-global active registry. An attempt lease captures
one snapshot and its exact registry for all turns. Detached background work
captures the exact executor before ownership transfer; an old MCP call keeps
its `McpServerRuntime` — a Python tool's call included, since the prepared
venv state it launches is immutable on disk — across later capability
revisions. Environment GC metadata is
written deterministically for future ownership, but M7 implements no GC.

The shared supervised process runner (`src/runtime/process_runner`) is the
M5 Bash process-group lifecycle extracted so native Bash and Skill
environment materialization share one owned supervisor/process-group
domain: same child-subreaper contract, same cancellation/timeout physical
settlement, same catastrophic containment, explicit cwd and child
environment, finite timeout, bounded diagnostics, and no generic
`waitpid(-1)` reaper.

### Layer 7: Interfaces and projections

The outermost layer exposes the runtime to humans and other systems:

- App Server protocol (versioned, transport-neutral public client boundary)
- Local interactive CLI
- Runtime command interface
- Runtime projection/event streaming

See [App Server protocol v12](app-server-protocol.md) for the method vocabulary,
generated client schemas, connection multiplexing, weak attachment lifetime and
headless interaction ownership. #36 binds the same endpoint to stdio JSONL for a
local TUI-owned child and WebSocket for browser/remote/existing-server clients;
it does not introduce another semantic endpoint. #290 consumes those bindings:
`rustx-tui` is now an App Server client in both modes, and the ordinary local
mode uses stdio JSONL to a TUI-owned child rather than a loopback WebSocket. See
[TUI App Server client](tui-app-server.md). No REST or AG-UI frontend protocol is
implemented.

#### Runtime Client protocol implementation (Issue #37, revised by Issues #131, #130, #136, #140, and #144)

Issue #37 established the native projection machinery in `src/runtime_client`.
Issue #288 reuses it beneath the one public App Server protocol:

```text
canonical runtime state / internal RuntimeEvent
                |
                v
 deterministic Runtime Client projection
                |
                v
 RuntimeClientEvent / RuntimeClientSnapshot
                |
                v
       App Server protocol v12
```

The governing invariant is that all authoritative execution and
conversation state originates from rustX Runtime; external clients observe
deterministic projections and never become a second authority. The
internal `RuntimeEvent` vocabulary is an execution-fact vocabulary, **not**
the wire contract: `RuntimeClientEvent` and `RuntimeClientSnapshot` are
explicit runtime-owned projection types. Their public wire contract is versioned
by `APP_SERVER_PROTOCOL_VERSION`, independently of journal, manifest, crate and
the existing local TUI protocol version. Snapshot/cursor/attachment authority
stays in the host; App Server does not copy its projection database. Both stdio
JSONL and WebSocket in #36 bind App Server, including its standalone process entry
point. Neither binding owns domain semantics. The existing `src/protocol` boundary remains the compiled
`RuntimeManifest` protocol; it is not a frontend protocol.

The following version history describes the local Runtime Client stdio contract,
which after #290 has no external client: `rustx-tui` speaks App Server v12, and
`src/runtime_client` is an internal projection foundation the App Server reuses.
App Server clients never negotiate or nest it. Its local version is
`RUNTIME_CLIENT_PROTOCOL_VERSION`.

Runtime Client protocol 34 removes the obsolete global `SessionSummaryView.active`
field. Strict initialization rejects v33 clients; Session route changes report
reattachment requirements against the installed single-runtime attachment.

Version 24 added the typed question
vocabulary, its canonical scalar domains — a finite-binary64 `Number` carried
as canonical binary64 text and an `Integer` carried as canonical decimal text,
neither of them as a JSON number a JavaScript client would re-spell — and
canonical Questionnaire requester identity (Issue #242) on top of version 23, which added
the former source-state/unprepared projections. Version 22 added the
[native Workflow projection and cursor handoff](workflow-run-projection.md).
Version 21 adds Review and
Questionnaire invocation correlation. Version 20 adds borrowed
Workflow run identity to child workspace facts. Version 19 adds typed deadline
interruption of approval waits. Version 18 distinguishes
Agent and Workflow approval invocation identity. Version 17 preserves
`denied` in background lifecycle projections. Version 16 introduced the
Tool outcome certainty vocabulary. It carries Issue #202's
explicit tool outcome certainty: the canonical `ToolExecutionStatus`
replaces `interrupted` with `outcome_unknown` (a bounded producer-owned
`detail` accompanies it and is never parsed for semantics), the background
terminal state `interrupted` becomes `outcome_unknown`, `timed_out` now
claims proven terminal settlement rather than mere deadline expiry, the
background lifecycle's terminal vocabulary is the honest `succeeded`,
`failed`, `denied`, `cancelled`, `timed_out`, and `outcome_unknown` — an unknown
outcome is never collapsed into `failed` — and
every terminal non-success status carries bounded model-facing feedback
through the canonical projection. Version 15 added the explicit
retained-workspace disposal resource lifecycle, including the
`PreservedUnresolved` state and pending partial-settlement outcome.

#### Retained workspace resource lifecycle (Issue #190)

The logical child lifecycle and its post-terminal physical workspace lifecycle
are separate authorities. The logical lifecycle remains the closed
`Succeeded`/`Failed`/`Cancelled`/`Interrupted` set. The workspace resource
projection is the following bounded state machine:

```text
None                         no runtime-owned isolated worktree
Retained { handoff }         exact current handoff is proven
PreservedUnresolved          a runtime-created workspace may still remain;
                             settlement proof is incomplete
        |
        +-- exact re-proof + durable intent --> DisposalInProgress
                                                   |
                                                   +-- worktree removed
                                                       --> WorktreeRemoved
                                                            |
                                                            +-- compare-delete
                                                                + durable
                                                                settlement
                                                                --> Disposed
```

`SubagentOwnershipCommitted` durably retains the immutable `WorkspaceSnapshot`
(source repository, logical relative workspace, deterministic physical root,
runtime branch, base commit, and subagent identity). The terminal event adds a
typed resource disposition: `None`, `Retained { handoff }`, or
`PreservedUnresolved { reason, detail }`. The unresolved form deliberately
does not manufacture a `WorkspaceHandoff`; its snapshot and typed reason keep
ownership visible across restart while preserving the stronger proof boundary.

For ordinary `Retained` disposal, the workspace manager re-proves source
repository identity, deterministic allocation, exact Git registration,
worktree HEAD, branch attachment, branch ref HEAD, and the recorded handoff
before the first destructive command. A later disposal of
`PreservedUnresolved` performs that same exact proof to derive a fresh handoff;
missing or changed facts fail closed and leave the resource unresolved. Since
the unresolved form has no durable terminal handoff `HEAD`, that re-proof also
requires both current heads to equal the immutable snapshot base; a changed
commit cannot be guessed into a disposable handoff.
Unresolved nested process containment is stricter: Git facts cannot prove that
the process boundary is safe, so the runtime refuses destructive disposal
until that separate containment authority is resolved.

The durable disposal intent is the authorization commit point. The exact
`git worktree remove --force` is the destructive physical linearization point.
After it succeeds, branch cleanup is a compare-delete of
`refs/heads/<recorded branch>` with the recorded expected HEAD. A moved or
otherwise unprovable branch is preserved and the resource settles as
`WorktreeRemoved`; it is never deleted unconditionally. A successful branch
settlement followed by the final durable event reaches `Disposed`.

Recovery folds only durable facts. An intent with an intact resource resumes
the exact authorized operation; an authorized missing worktree with a
residual expected ref continues at branch settlement; a durable partial fact
restores `WorktreeRemoved`; and an intent whose exact physical resources are
already gone converges to `Disposed`/`AlreadyDisposed` and can append the
missing final settlement. An unresolved terminal fact restores
`PreservedUnresolved` with the original snapshot and no handoff. A missing
intent is not inferred from filesystem absence, so an externally disappeared
worktree remains an ownership mismatch rather than a fabricated success.
Every durable transition is monotonic; duplicate facts are idempotent and
conflicting phase/order facts are rejected.

The runtime serializes its own disposal requests and performs the final proof
immediately before invoking Git, but the proof and Git command are separate
process operations. rustX therefore makes no atomic check/use claim against
an external actor concurrently mutating Git. The compare-delete boundary
still prevents deletion of a moved branch, and all remaining ambiguity fails
closed.

Version 14 adds the Agent Status
contextual annotation projection (Issue #194): the snapshot's latest-only
`status` is replaced by the bounded composition window `statuses`, each status
opportunity carries the durable identity it was established against, and
`agent_status_composed` carries one complete window transition — the admitted
composition plus the eviction that admission caused — rather than only the new
composition. Placement is therefore a runtime-published fact — the composed
status identity, the eligible opportunities, and the durable position each of
them froze — while how a client draws a status at that place stays
presentation. Version 13 introduced the Issue #187 subagent workspace
representation that separates logical child project authority from physical
Git worktree ownership: the subagent workspace projection carries
`logical_workspace` plus a tagged `isolation` (`shared` or `git_worktree` with
the source repository root, repository-relative workspace, physical worktree
root, base commit, branch, and parent dirty fact), and a retained handoff
exposes `logical_workspace` and `physical_worktree_root`. Version 12 added
routed interaction projection for the root human surface. Version 11
added the
subagent live-activity projection; version 10 added subagent workspace facts
and preserved-worktree handoff metadata; and version 9 added `interrupted` to the
closed `SubagentState` vocabulary: `RuntimeClientSubagent` now carries the
child's latest-value `observation` (revision, activity, timestamp,
counters), the redacted `execution_profile` (wire key `execution_profile`;
the bare `profile` key stays retired), and `started_at`. Rust snapshots and
the maintained TUI mirror agree that an unexpected child process/control-plane
loss has an unknown outcome, and that the activity projection is
diagnostics-only — the closed `SubagentState` lifecycle remains the only
authority on whether a child is alive, settling, or settled. Superseded
Runtime Client versions are rejected explicitly;
there is no compatibility decoder.

Version 12 carries a set of pending Approval and Questionnaire projections
from the primary conversation and every live child. Each projection has an
InteractionRef containing the conversation_id and conversation-local
interaction_id, plus source metadata, so a response never relies on TUI
focus or on a bare local identifier.

Module ownership:

```text
runtime/                      the semantic conversation runtime
conversation_runtime.rs       ConversationRuntime: the conversation
                              coordinator (Issue #61) — session model
                              authority, attempt-id allocation, the
                              current-attempt slot, attempt admission,
                              between-attempt ConversationState,
                              RequestHistory, the mailbox/admission
                              relationship, the lifecycle/drain authority,
                              settlement handoff, the
                              inactive/running/draining/quiescent lifecycle
                              boundary, and the adapter
                              bootstrap handshake; publishes semantic
                              observations
runtime/observation.rs        the runtime-owned semantic observation
                              contract (Issue #61): ConversationObservation
                              (semantic source types only), the primary
                              PendingObservations queue, and a bounded
                              pre-activation fan-out for existing local
                              observers. The runtime keeps no second durable
                              or Runtime Client fold: the Runtime Client
                              projection is the one full client fold
runtime/request_history.rs    append-only in-memory owner of frozen
                              settled RequestSnapshots and reconstruction
                              lookup (owned by ConversationRuntime);
                              never a message transcript
runtime/inbound.rs            ConversationInboundMailbox: inbound ordering
                              and finite batching authority, with the
                              shared admission wake handle
runtime_client/types.rs        protocol version, cursor, attachment/request
                               ids, the typed request/response/event
                               envelope, method results, typed errors
runtime_client/event.rs        RuntimeClientEvent (external vocabulary)
runtime_client/snapshot.rs     RuntimeClientSnapshot read model
runtime_client/projection.rs   RuntimeClientProjection: the client read
                               model linearization owner (fold, cursor
                               allocation, bounded replay, subscribers)
                               and the translation of semantic
                               observations into the client vocabulary
runtime_client/host.rs         RuntimeClientHost: the projection + control
                               + attachment adapter over ConversationRuntime
                               — attachment admission, snapshot/cursor
                               reads, event subscriptions, protocol
                               adaptation; it owns no canonical
                               conversation/session/admission state
runtime_client/attachment.rs   RuntimeAttachment: one control attachment
                               plus read-only inspection attachments,
                               RAII/explicit detach, request dispatch,
                               event subscription delivery
runtime_client/endpoint.rs     RuntimeClientEndpoint: the transport-neutral
                               semantic entry point that dispatches every
                               Runtime Client request, `initialize` included
runtime_client/transport/      byte-stream adapters beneath the semantic
                               layer (Issue #38); `stdio.rs` is the strict
                               stdio/JSONL transport
```

Issue #61 extracted the conversation runtime coordinator from this
boundary. The layering is:

```text
ConversationRuntime semantic facts
        |
        v
ConversationObservation (runtime-owned vocabulary)
        |
        v
runtime observation fan-out
        |
        +--> primary PendingObservations
        |       -> RuntimeClientProjection
        +--> bounded local observation subscribers
                (existing bounded activity surfaces only)

RuntimeClientProjection (translation, fold, cursor, replay, subscribers)
        |
        v
RuntimeClientHost (attachment / control adapter)
        |
        v
RuntimeClientEndpoint -> AppServerConnection -> stdio JSONL / WebSocket -> clients
```

The runtime never emits Runtime Client projection types: the observation
vocabulary carries runtime-owned source types, and the projection owns the
translation into `RuntimeClientEvent`/`RuntimeClientSnapshot`.

A conversation runs the exact same admission/execution path with zero
Runtime Client attachments: the coordinator is the semantic owner, and the
Runtime Client is a projection/control/attachment adapter over it.

- **The semantic endpoint owns `initialize`.** `RuntimeClientEndpoint` is
  the boundary a transport wraps. It starts unattached and accepts every
  Runtime Client request; `initialize` performs version negotiation,
  control/read-only attachment admission, `AttachmentId` allocation, and the linearized initial
  snapshot, storing the resulting attachment. Non-`initialize` requests
  before that are `not_attached`; a successful `detach` (or dropping the
  endpoint) returns it to the unattached state. `RuntimeClientHost::attach`
  remains an internal primitive that the endpoint invokes — it is not the
  protocol entry point. Issue #38 therefore reduces to framing:

  ```text
  JSONL line -> RuntimeClientRequest -> endpoint.handle_request
             -> RuntimeClientResponse -> JSONL line
  endpoint.next_event -> RuntimeClientProtocolEvent -> JSONL line
  ```

  No transport implements negotiation, admission, identity creation, or
  replacement/rejection semantics, and none needs an out-of-band attach
  operation.

- **Two synchronization boundaries, one per authority.** The conversation
  coordinator guards its admission state (session model, between-attempt
  canonical state, current-attempt slot, lifecycle/drain authority, inbound/attempt
  identity counters) with one lock; the Runtime Client host guards its
  projection state (snapshot read model, cursor allocation, bounded
  replay, subscribers, attachment admission/detach) with a second lock.
  The coordinator publishes every semantic transition as a
  runtime-owned `ConversationObservation` into the shared leaf queue,
  and every host lock acquisition drains that queue first, so the
  projection folds observations in the coordinator's commit order.
  Snapshot/cursor, cancel-current, terminal settlement, and admission
  therefore still linearize by synchronization, never by timing — the
  coordinator's admission linearization is one documented point, and the
  projection's snapshot/cursor linearization is another.
- **Native interactions use the same semantic boundary.** A pending Approval
  or Questionnaire is folded from `ConversationObservation::InteractionPending`
  into `RuntimeClientSnapshot.pending_interactions`; its terminal outcome
  folds through `InteractionSettled` and removes only that live entry.
  The originating conversation's `InteractionCoordinator` remains the
  semantic owner. The root Runtime Client is only the human-facing surface:
  it aggregates primary and live-child projections and forwards a response
  addressed by the full `InteractionRef`. It never creates a parent
  interaction, wakes the parent model, settles a child, or writes child
  content into primary history. A stale, duplicate, pre-crash, or
  post-quiescent response is the typed `interaction_not_pending` error
  and has no semantic effect.
- **Attachment availability is bounded and non-semantic.** The one control
  attachment represents the 0.1 interaction provider; read-only inspection
  attachments never do. Publication admission is checked by the root host
  under the same `ClientState` mutex that installs/removes that attachment.
  The child must receive an ephemeral permit for its exact `InteractionRef`
  before its coordinator commits `InteractionRequested`; therefore a detach
  that wins the host mutex rejects publication as `Unavailable` without a
  requested audit, while a permit that wins remains valid for that one
  originating publication. The propagated availability watch is an early
  fail-closed hint only, not this authority. Detaching after publication only
  closes admission for future requests: it does not answer or cancel a live
  interaction, and a reattached client repairs from the runtime snapshot and
  cursor rather than from local prompt state.
- **Interaction routing is reliable semantic control.** Child
  publication-admission requests/results, InteractionRequested,
  InteractionSettled, and response acknowledgements use the reliable
  bidirectional child control protocol. They do not use the disposable
  activity/observation lane, whose latest-value loss and backpressure remain
  irrelevant to execution control.
- **Human-provider availability is separate from capability selection.** A
  root control attachment makes the root human surface available to live
  children; detaching after publication leaves the originating waiter
  pending, and reconnect rebuilds presentation from live authoritative state.
  This route does not grant ask_user: only a frozen subagent definition that
  explicitly selects that capability receives it. Process recovery never
  recreates a waiter or treats historical approval as execution authority.
- **The Runtime Client host binds before activation.** A conversation
  runtime has four lifecycle states and one explicit admission/drain
  authority:
  them:

  ```text
  ConversationRuntime::new(..)         -> runtime-owned / inactive
      [optional] RuntimeClientHost::new(..)     bind the client adapter
  ConversationRuntime::activate()      -> Running: execution may begin
  ConversationRuntime::shutdown()      -> Draining -> Quiescent
  ```

  An **inactive** runtime is inert, and this is enforced, not merely
  documented: its mailbox refuses `enqueue` with
  `MailboxError::ConversationInactive`, `submit_inbound` fails with
  `InboundAdmissionError::Inactive`, `model_set` fails with the typed
  `ModelUpdateError::Inactive`, `shutdown` fails with the typed
  `ShutdownError::Inactive`, the background registry refuses
  `commit_dispatch` with `BackgroundDispatchError::ConversationInactive`,
  and the capability coordinator refuses a runtime-owned ordinary `commit`
  with `CapabilityCommitError::RuntimePublicationRequired`; live capability
  mutation must use the configuration reload publication owner. No admission worker
  exists, `admit_next_attempt` is a no-op, and an inactive runtime
  therefore publishes no observation at all.

  Capability candidate preparation is a composition/readiness operation and
  may run while inactive. It is counted through activation and drain, while
  the revision swap remains refused until a live commit observes `Running`.

  There is exactly **one authoritative lifecycle state**: the shared
  `ConversationLifecycle` token composed by the runtime and read by every
  runtime-owned semantic boundary. The mailbox keeps no lifecycle flag
  (runtime ownership is the lifecycle handle itself), the capability
  coordinator keeps no lifecycle flag (the handle is attached at its
  claim), the coordinator keeps no copy, and the background registry reads
  the same gate through its mailbox. `activate` performs the single
  `Inactive -> Running` transition and `shutdown` performs the single
  `Running -> Draining` transition of that one token. Activation's worker
  spawn and initial admission kick are the one-time post-transition work of
  its winning caller; drain's settlement and `Quiescent` publication are the
  one shared runtime-owned completion. Because there is no subsystem-specific
  intermediate lifecycle state, background and capability commits can never
  observe contradictory lifecycle states in one real-time history.

  Binding a client host is a **composition decision, not a hot
  operation**. A bind after activation is refused with the typed
  `HostConstructionError::RuntimeAlreadyActivated`; rustX does not
  promise that a first host installed after semantic execution has begun
  would reconstruct the read state a continuously attached client would
  have. A headless runtime (Issue #60 subagents, every zero-client
  regression) simply never constructs a host.

Runtime Client **attachments** stay fully dynamic after activation —
attach, detach mid-attempt, reattach — because attachment lifetime and
host-binding lifetime are different axes.

#### Conversation attachment and inspection (Issue #179)

Runtime Client attachment is a generic conversation-identity capability, not
a subagent transcript API. One known identity resolves to the best available
normal Runtime Client read path:

```text
child_conversation_id
        |
        +-- child runtime owns its live endpoint
        |       -> read-only attachment to the child's actual projection
        |
        +-- child runtime owns a live liveness lease, but endpoint setup failed
        |       -> explicit live-inspection-unavailable diagnostic
        |
        +-- no live liveness lease
                -> RuntimeClientHost::new_durable over the stable store
```

`child_conversation_id` resolves to the child's live Runtime Client read
projection while the runtime exists and falls back to the same conversation's
durable authorities after the live runtime is gone. The local runtime first
probes the identity-derived child-owned Unix socket (whose short
collision-resistant filename keeps the local endpoint within platform socket
pathname limits). A successful connection is process-routing state only; the
child remains the owner of the semantic Runtime Client endpoint and projection.
If the probe cannot connect, the local runtime checks the child's disposable
identity-derived liveness lease. A held lease means that the child is still
live but its optional inspection transport is unavailable, so inspection
returns an explicit bounded diagnostic rather than presenting a durable
snapshot as the live projection. Only when no live lease is held does the
local runtime open the stable child store and `RuntimeClientHost::new_durable`
build the ordinary projection from that conversation's durable Surface,
Message Ledger, Request Snapshots, transcript ordering spine, and Event
Journal. The lease is an OS-lifetime routing marker, not a registry or
conversation fact; the child removes it normally, and its lock disappears on
abnormal death. The wire protocol remains the normal `initialize`,
`snapshot_get`, `transcript_page_get`, and subscription shapes. There is no
transcript identifier, inspection identifier, child-specific payload, or
protocol compatibility mode.

The child's own conversation is the durable authority for its transcript and
execution history. A child spawn keeps its stable `conversation.sqlite` under
the launch runtime root's identity-derived semantic child directory while its
physical incarnation/artifact root remains disposable. Settlement removes
only the physical execution root, so a completed, failed, cancelled, or
interrupted child can be reopened by the same `child_conversation_id` after
its runtime process is gone. The parent subagent surface keeps identity,
lifecycle, terminal metadata, and bounded live observation; it never stores
the child's canonical messages or execution facts.

The live child endpoint admits an explicitly read-only Runtime Client
attachment to the same projection the child owns. It can observe assistant
output, current model/attempt state, foreground tool execution and progress,
interactions, compaction, and terminal transitions, including state that has
not yet reached durable settlement. It cannot submit inbound, cancel, answer
interactions, execute tools, mutate models or Session state, settle lifecycle,
or shut down the child. The host's one control attachment remains separate
from read-only subscribers; a child inspector never takes execution or
interaction ownership.

The root control-capable Runtime Client is the one human-facing interaction
surface for the supervised tree. A child still owns every Approval and
Questionnaire through its own InteractionCoordinator; the root only projects
the child's live request, adds Subagent source metadata, and forwards the
response using InteractionRef. This is a presentation route, not parent
mediation: no parent interaction, model prompt/result, canonical-history
entry, or replacement invocation is created. Child death removes its pending
projections without synthesizing a terminal answer, while pending authority
in another live conversation is unchanged.

The durable host has no `ConversationRuntime`, mailbox, subagent registry,
provider adapter, or lifecycle handle. Its attachment linearizes at the
durable bootstrap reads used to construct the projection. Durable Event
Journal facts are folded into that read model without allocating a live
Runtime Client cursor or publishing an event. `snapshot_get` and a fresh
attachment rebuild from the durable authorities, so a skipped projection or
reconnect repairs from authoritative state rather than client-owned history.
The durable fallback does not replace or reconstruct #181's disposable live
observation state; running-child inspection reads the live conversation
projection directly. Detaching is only subscriber/client lifecycle: it cannot
cancel, settle, retry, execute tools, change Event Journal ordering, or affect
the parent.

Live inspection resolution linearizes at a successful local socket connection.
At T0, that connection selects the child's live read projection. If the child
begins terminal settlement at T1, the attachment remains valid through the
terminal transition. At T2, when the child runtime/process closes the
endpoint, the inspector transport closes cleanly; the terminal durable
settlement is the authority used for reopening. At T3, or on any later open,
the same `child_conversation_id` probes again and resolves the durable store.
If the socket is unavailable at T0, the liveness lease is the second
linearization check. A held lease returns the explicit degraded-observation
diagnostic; an absent or unlocked lease selects durable bootstrap. Thus a
running child with a failed endpoint never masquerades as a durable live view,
while a stale lease/socket cannot become a permanent dependency. Once the
child is reaped and its live lease is gone, the same `child_conversation_id`
resolves to the durable store; a removed physical incarnation does not change
the conversation identity.

The TUI keeps parent/child frames, the selected row, and the current
conversation label as presentation state only. `Ctrl+Up`/`Ctrl+Down` selects
a known subagent row, `Enter` opens its exact `child_conversation_id` in a
read-only ordinary conversation view, and `Esc` detaches that inspection and
restores the still-live parent attachment. Inspection is not parent-model
context transfer: opening a child changes neither the parent's transcript nor
the next parent provider request. The #178/#181 observation plane is reused
only as bounded parent-side live observation; it is disposable and is not a
transcript or execution-history authority.

- **Adapter bootstrap is one global cut.**
  `ConversationRuntime::install_observation_bridge` is the one fallible
  step after the binding claim. It runs entirely under the one
  coordinator lock — the same lock `activate` takes, which is what makes
  the lifecycle rejection atomic — and captures the seed in this order:

  ```text
  T0  coordinator lock; reject if activated; install the observation queue;
      capture shutting_down / canonical messages / session model
  T1  background registry: install observer + capture snapshots
  T2  mailbox:             install observer + capture pending
  R   capability:          install observer + capture snapshot   <- the cut
      coordinator lock released
  ```

  > **Invariant.** The bootstrap cut `R` is a real global state of the
  > runtime: the initial snapshot contains every projected runtime fact
  > committed through `R`, every projected transition after `R` is
  > delivered exactly once through the live observation stream in
  > semantic publication order, and no transition before `R` is
  > published as a post-`R` event.

  This is a proof, not four independent cuts glued together. Every
  captured value is still its authority's live value at `R`:

  - coordinator facts cannot move — every mutator (`model_set`,
    `shutdown`, `submit_inbound`, admission, settlement) takes the
    coordinator lock, held across `[T0, R]`;
  - the background plane is pristine by construction — the
    `ConversationToolRuntime -> ConversationRuntime` ownership transfer
    requires no prepared dispatch and no committed record, and the
    registry then refuses `commit_dispatch` while its mailbox is bound
    inactive — so no background record exists across `[T0, R]` and none
    can be created;
  - the mailbox refuses `enqueue` while its bound runtime is inactive,
    so the pending queue is frozen across `[T0, R]`;
  - the capability coordinator refuses a runtime-owned ordinary `commit`
    before activation, and the capability snapshot is captured *at* `R`.

  And because each authority installs its observer in the same lock
  section that captures its seed, no transition can be both seeded and
  queued, and none can be neither.

  **Bootstrap state never fabricates a live event.** The projection
  installs every seeded fact — canonical history, session model,
  capability snapshot, and pending inbound — as snapshot state through
  `RuntimeClientProjection::bootstrap`. Nothing is routed through
  `apply`, so bootstrap publishes no `RuntimeClientEvent` and allocates
  no `RuntimeClientCursor`: `{ snapshot, cursor 0 }` is the state at `R`,
  and the first cursor belongs to a real post-activation transition (the
  background seed is provably empty by the ownership-transfer invariant).
  The bootstrap cut `R` **precedes** the activation transition: the
  handshake completes over the inert runtime and the shared
  `ConversationLifecycle` `Inactive -> Running` CAS happens afterwards.
  Because the runtime remains semantically inert from `R` until that
  transition — mailbox, background, capability, and coordinator mutations
  are all inactive-gated — no projected semantic fact can appear in the
  interval `[R, activation)`, so the live stream carries every
  observation the runtime ever emits.

  There is deliberately **no** runtime-side mirror of the client attempt
  view. The runtime does not fold `ConversationObservation` a second
  time; the client projection is the single fold.
- **One conversation runtime per identity, one host per runtime.** One
  `ConversationToolRuntime` identity is bound to at most one
  `ConversationRuntime` and at most one `RuntimeClientHost` for that
  identity's lifetime. `ConversationRuntime::new` performs one
  **tool-runtime ownership transfer** and claims the capability
  coordinator binding; `RuntimeClientHost::new` claims a second, client
  binding on the same handles; both are `Clone` and every clone shares one
  binding, so a cloned runtime bundle is not a second bindable identity. A
  second coordinator is rejected with
  `ConversationRuntimeError::RuntimeAlreadyBound` and a second host with
  `HostConstructionError::RuntimeClientAlreadyBound`.

  The ownership transfer is one real synchronization contract, not three
  independent steps. Under the background registry lock — the same
  boundary the dispatch ownership commit linearizes at — it requires a
  pristine background plane (no prepared dispatch, no committed record),
  claims the coordinator binding, and binds the canonical mailbox
  runtime-owned with a fresh `Inactive` shared lifecycle, all at one
  point:

  ```text
  standalone ConversationToolRuntime
      |
      |  ownership transfer (one registry critical section)
      |    1. require pristine background (no prepared, no committed)
      |    2. claim the coordinator binding
      |    3. bind the mailbox runtime-owned + shared Inactive lifecycle
      v
  ConversationRuntime-owned / inactive
      |
      |  background commit -> BackgroundDispatchError::ConversationInactive
      v
  ConversationRuntime::activate()   (the shared lifecycle Inactive -> Running)
  ```

  Either a standalone background commit wins the section first — the
  transfer is refused typed with
  `ConversationRuntimeError::ToolRuntimeNotQuiescent` and consumes
  nothing — or the transfer wins first and every later background commit
  fails `ConversationInactive`. A `ConversationRuntime` can therefore
  never be constructed over a tool runtime that already contains staged
  or committed background work, and the inactive phase can never inherit
  a detached semantic transition that would later advance the Runtime
  Client cursor before activation. Construction is transactional: if the
  capability claim fails after the transfer, the mailbox is unbound and
  the coordinator claim released again, restoring the exact previous
  standalone state.

  The ownership transfer (`standalone -> runtime-owned/inactive`) and
  activation (`Inactive -> Running`) is a distinct commit point after the
  transfer establishes runtime ownership plus the `Inactive` lifecycle
  relationship, and `activate` later performs the one lifecycle
  transition.

  This is a runtime ownership invariant, not a caller convention. Two
  coordinators over one authoritative runtime would each admit attempts
  from the same mailbox over competing canonical state, and each
  subsystem carries exactly one observer slot, so the second construction
  would silently unhook the first. The headless conversation runtime
  (zero hosts) is fully supported: it installs no observation seams and
  admits asynchronous inbound through the mailbox's shared wake handle.

  Every fallible validation runs before the claim, the binding claim is
  the ownership-commit boundary, and the only fallible step after it is
  the bridge handshake — on whose failure the claim is released again. A
  rejected construction therefore has no semantic side effect: no
  observer is replaced, no worker starts, no mailbox, background, or
  capability state moves, and no claimed-but-invalid binding remains.
- **Conversation runtime activation is explicit.** `ConversationRuntime::new`
  requires a Tokio execution runtime and rejects construction outside
  one with the typed `ConversationRuntimeError::NoExecutionRuntime`
  error, so `activate` can always spawn the admission worker. Activation
  is the composition's own explicit step — never a side effect of
  constructing a Runtime Client host — so the admission worker exists at
  exactly the same lifecycle point for a headless runtime and an
  interactive one, and native producers never depend on a Runtime Client
  call to activate admission.
- **One conversation authority.** The `ConversationToolRuntime` owns the
  `ConversationId`, the canonical mailbox, the authoritative background
  registry, and both binding identities; the conversation runtime
  *derives* its identity from it, and the Runtime Client host derives
  everything it reports from the conversation runtime.
  `RuntimeConversationConfig` and `RuntimeClientHostConfig` therefore
  carry no conversation id field of their own — a second configured
  identity could disagree with the runtime, and a coordinator that runs
  one runtime while naming another conversation would issue
  `AgentExecutionRequest`s the runtime rejects, after having already
  admitted the attempt. Structural absence removes that state instead of
  checking for it. The capability coordinator remains a separate
  authoritative identity, so it is still validated explicitly against the
  runtime before the coordinator binding claim.

  **Host lifetime is not attachment lifetime.** Reconnect replaces the
  attachment on the same host (detach, then a fresh `RuntimeClientEndpoint`
  `initialize` yielding a new `AttachmentId`); it never reconstructs the
  host. The binding is deliberately not released when the bound host is
  dropped: rebinding a surviving runtime bundle would require a recovery
  model for canonical history, pending mailbox projection, and cursor
  continuity that the Runtime Client projection does not own. Recreating a host over the same
  runtime bundle is **not** supported by the current recovery contract — a new host requires a
  new `ConversationToolRuntime` identity. Observer installation on the
  mailbox, background registry, and capability coordinator is crate-private
  for the same reason: it is a runtime coordination seam, not a public
  extension point.
- **Ownership: observation edges are non-owning.** The graph is:

  ```text
  semantic owner ─────────► Arc<RuntimeInner>
  (ConversationRuntime and clones, the host adapter, a running attempt
   task — the task is a bounded owner that releases at settlement)

  RuntimeInner ──► authoritative subsystems (tool runtime, mailbox,
                   capability coordinator)
  RuntimeInner ──► observation fan-out ──► primary PendingObservations
                                   └──────► bounded local subscribers
  RuntimeInner ──► current ModelTimeoutPolicy + shared MonotonicClock

  attempt/manual admission ──► frozen policy/clock copies
                              ├─► AgentExecution
                              └─► ContextRuntime ──► ModelBackedSummarizer

  RuntimeClientHost ──► Arc<ClientInner>
  ClientInner ──► Arc<ConversationRuntime> (control + bootstrap reads)
             ──► projection state
             ──► primary Arc<PendingObservations>

  authoritative subsystem ──► Arc<RuntimeObserver>
  RuntimeObserver ─────────► Weak<RuntimeInner>

  admission worker ────────► Weak<RuntimeInner> + Arc<WakeGate>
  projection worker ───────► Weak<ClientInner> + Arc<PendingObservations>
  ```

  The low-level construction seams are crate-private and require those
  explicit admitted values. Neither `AgentExecution` nor `ContextRuntime`
  creates a fallback timeout policy or an independent monotonic clock, so a
  provider-backed primary request and summary request cannot silently enter
  different elapsed-time semantics.

  Subsystem observer slots keep owning `Arc<dyn InboundObserver>` and
  friends; the concrete `RuntimeObserver` is non-owning, so installing a
  seam cannot create the cycle
  `RuntimeInner -> subsystem -> Arc<RuntimeObserver> -> RuntimeInner`. Each
  callback upgrades the weak handle and returns without publishing when the
  upgrade fails — the conversation runtime no longer exists, which is never
  an error for the subsystem. The admission worker holds only a weak
  runtime handle plus the wake gate it waits on, and the projection worker
  holds only a weak client handle plus the queue it waits on; neither holds
  a strong handle across an await.

  `RuntimeInner` is therefore destroyed when its last semantic owner is
  released, not at process exit. `RuntimeInner::drop` closes the wake gate
  (the admission worker's terminal condition) and the observation fan-out;
  `ClientInner::drop` closes only the primary queue (the projection worker's
  terminal condition); all closes are idempotent. Teardown takes no lock,
  joins nothing, and publishes nothing. A running attempt task is a
  deliberate *bounded* strong owner — an admitted attempt must reach
  settlement, and the task releases the runtime when it does. Attachment
  detach remains unrelated to runtime or host lifetime.
- **Lock order.** The graph is acyclic by construction:

  ```text
  CoordinatorState ──► ConversationInboundMailbox ──► ObservationFanout
  CoordinatorState ──► ObservationFanout
  ClientState ──────► primary PendingObservations
  ConversationBackgroundRegistry ───► ObservationFanout
  SubagentRegistry ────────────────► ObservationFanout (observer only)
  CapabilityCoordinator ───────────► ObservationFanout
  AgentExecution (attempt task, holds no lock) ──► ObservationFanout

  bootstrap (one section, coordinator lock held throughout):
    CoordinatorState ──► ConversationBackgroundRegistry
                    ──► ConversationInboundMailbox
                    ──► CapabilityCoordinator

  mailbox wake / WakeGate ─────────► (leaf Notify only)
  ```

  `ObservationFanout` is the runtime-owned leaf: one primary
  `PendingObservations` queue plus weak, bounded subscriber queues. Every
  authoritative subsystem fires its observer *while holding its own lock*,
  and every such observer does exactly one thing: append an immutable
  observation to the fan-out. The primary queue is the Runtime Client's one
  fold; local subscribers are disposable observation surfaces, never a second
  history or Runtime Client fold. No subsystem ever acquires
  `CoordinatorState` or `ClientState`. Since Issue #61's revision there is no
  runtime semantic record in the graph at all — the runtime performs no fold,
  so there is no second intermediate lock.

  The subagent terminal-durability sink is a separate rule: the registry
  copies the sink while its mutex is held, releases that mutex, and only then
  calls the owning `ConversationRuntime`, which may acquire `CoordinatorState`
  and publish `DurabilityFailed`. Thus the normal ownership direction is
  `CoordinatorState -> SubagentRegistry`; there is no held-lock reverse edge.
  The registry never waits for a process or performs async work while its
  mutex is held. Runtime Client projection callbacks remain leaf queue
  writes, and the driver task owns the process handle independently of both
  logical locks.

  All downward edges out of `CoordinatorState` point the same way. The
  `CoordinatorState -> mailbox` edge exists in `admit_next_attempt`,
  which drains under the coordinator lock so the drain fact, the history
  commits, and the attempt publication linearize together. The bootstrap
  handshake adds `CoordinatorState -> {background, mailbox, capability}`
  in that same direction, held as one section, which is what makes the
  bootstrap cut global. No reverse edge exists, so the graph stays
  acyclic.

  The mailbox's shared wake handle notifies the admission worker at every
  enqueue publication — a leaf signal, never a lock — so idle
  asynchronous inbound is admitted without any client request.
  Consequently an authoritative commit never waits on the client lock,
  and subscriber notification can never block authoritative runtime
  state. The `AgentExecutionObserver` callbacks append to the leaf queue;
  that adds no incoming edge because `AgentExecution` is owned by its
  attempt task and holds no lock when it observes. Every client lock
  acquisition drains the pending queue first, so queued observations fold
  in the coordinator's commit order.
- **Snapshot/cursor invariant.** `snapshot_get` returns `{ snapshot,
  cursor }` where the snapshot describes all Runtime Client state through
  cursor C, and a subscription after C observes every subsequently
  published event or fails explicitly with `resync_required`.   This holds
  by construction (one boundary), not by luck. At bootstrap the same
  invariant holds at cursor 0: the seed is installed as snapshot state,
  never replayed through `apply`, so no pre-existing runtime fact
  allocates a cursor or publishes an event — and, by the ownership-transfer
  invariant, no background execution can even exist at bootstrap (the
  registry is pristine at construction and refuses dispatch commits while
  its mailbox is bound inactive).
- **RuntimeEvent mapping policy.** Every internal event is classified
  PROJECT / FOLD INTO CLIENT STATE ONLY / INTERNAL in the projection
  owner: attempt lifecycle/settlement, streaming output, tool-call
  assembly, and foreground/background tool lifecycle project; turn
  counting and final usage fold; model request mechanics stay internal;
  compaction start, failure, and committed completion project with optional
  attempt attribution and update the shared context read model. Internal
  `RuntimeEvent` evolution therefore cannot silently break the Runtime Client
  protocol.
- **Streaming repair.** The snapshot carries an in-flight Assistant output
  view (accumulated blocks) and foreground tool views keyed by the
  logical tool-call identity, so a client repairing after `resync`
  reconstructs every client-visible effect without duplicated or missing
  semantic output. Parallel physical completion never corrupts logical
  identities.
- **The client replay ring is a projection cache, not durability.** A finite
  in-memory ring (`RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT = 4096`,
  configurable) holds recent projected events; expired or ahead-of-stream
  cursors fail with `resync_required`. Reconnect/bootstrap rebuilds the
  projection from the ConversationStore's current head and paged Event
  Journal when historical inspection is requested. The ring never supplies
  recovery facts and is not the Event Journal.
- **Cursor-driven subscriptions (no second backlog).** A subscription is
  a consumed `RuntimeClientCursor` into that one ring plus an
  edge-triggered, payload-free wakeup. Publication pushes into the ring,
  evicts beyond the retention limit, and wakes subscribers; a consumer
  pulls the next retained entry after its cursor, one per poll. A stalled
  consumer therefore costs one cursor rather than a queue, total retained
  memory stays bounded by `replay_limit` no matter how far behind any
  consumer is, and the publisher never blocks on a slow consumer. A
  consumer that falls behind retention receives the explicit
  `EventDelivery::ResyncRequired` — a stable, terminal verdict — instead
  of a silently non-contiguous stream, so cursor contiguity within a
  subscription is guaranteed. `EventDelivery` distinguishes `Event`,
  `Pending`, `Closed`, `ResyncRequired`, and `Exhausted`, which is what
  lets Issue #38 implement deterministic bounded transport backpressure
  with no unbounded queue hidden underneath it.
- **Explicit projection failure.** Cursor allocation uses a checked add:
  overflow sets an exhausted flag rather than wrapping, publication
  stops, and every read (`snapshot_get`, `capability_get`, `initialize`,
  subscribe, and subscription polls) then fails with
  `projection_exhausted`. A read never hands back a model that silently
  stopped folding authoritative transitions.
- **Attachment lifecycle.** A live Runtime Client host admits one control
  attachment and any number of explicitly read-only observation attachments.
  A second control attach fails with `attachment_in_use` and never evicts the
  first; read-only inspection attaches to the same projection without
  acquiring control authority. Detach (explicit or RAII drop) releases only
  that attachment, reconnects receive a fresh attachment identity, and
  request ids are attachment-scoped. Detach changes only attachment state:
  it never cancels the attempt, never cancels conversation-owned background
  work, never drains the mailbox, and never mutates canonical history or
  capability state.
- **Current-attempt coordination.** The conversation runtime owns the
  current-attempt slot and the exact `AgentCancellation` the attempt task
  runs against; `cancel_current_attempt` requests cancellation through the
  coordinator, which verifies under its own lock that the named attempt is
  still the current one, so a settlement/admission race can never cancel a
  newer attempt. The acceptance response is never terminal settlement (the
  Agent Loop owns settlement, observed asynchronously). Neither the
  coordinator nor the host owns a second attempt state machine.
- **ConversationState: one owner at a time.** Ownership transfers by move; it
  is never cloned or shared as a second mutable authority:

  ```text
  idle        ConversationRuntime owns ConversationState
  admission   ConversationState moves into AgentExecution, which is the
              sole authority while the attempt runs
  running     the runtime never mutates a competing copy; asynchronous
              inbound stays mailbox-owned until the loop commits it, and
              RuntimeClientSnapshot.messages is projection only
  settlement  AgentExecutionResult moves ConversationState back to the
              ConversationRuntime for the next idle/admission boundary
  ```

  The settlement-path equivalence between the projection mirror and the
  authoritative ledger is covered deterministically by regression tests;
  there is only ever one mutable authority.

  This move-based runtime ↔ AgentExecution boundary is the bounded #54
  design; Issue #61 extracted the enclosing `ConversationRuntime`.
- **Admission: one authority.** `ConversationRuntime` is the one
  next-attempt admission owner. Every ordinary inbound producer — the
  Runtime Client human submit path, runtime/agent inbound, background
  terminal notifications, future subagent/fleet/external producers —
  publishes into the authoritative mailbox; the mailbox's shared wake
  handle notifies the coordinator's admission worker; and
  `admit_next_attempt` observes idle + gate, performs one finite
  watermark-bounded drain, commits the drained messages into canonical
  history, allocates the attempt id, freezes the model snapshot, and
  publishes the current attempt — all under the one coordinator lock.
  While an attempt is running, enqueued messages wait for the loop's
  safe-boundary drain inside the same attempt, and the settlement handoff
  admits the next attempt exactly once. Success of `submit_inbound` means
  accepted/published, never assistant-finished. No producer ever starts an
  `AgentExecution` itself.
- **Mailbox diagnostics.** The projection mirrors enqueue/drain facts
  (pending items in `InboundSequence` order, latest drain watermark and
  count) from an observation seam fired at the mailbox linearization
  points; the conversation runtime's observer queues observations (the
  coordinator drains the mailbox under its own lock) and a worker task
  plus every projection lock acquisition applies them in total order.
  `RuntimeClientCursor` remains a distinct domain from `InboundSequence`;
  clients can never drain or mutate the mailbox. Background terminal
  notifications enqueue through the same semantic path as every other
  mailbox state.
- **Background projection.** The authoritative
  `ConversationBackgroundRegistry` is projected through a read-only
  observation seam: `BackgroundExecutionUpdated` events and the snapshot
  background section carry execution identity, tool identity/name,
  lifecycle, latest bounded progress, and terminal result. Detached work
  survives attempt termination and client detach; protocol
  `background_status`/`background_cancel` use the registry authority, and
  cancel acceptance is distinct from terminal settlement.
- **Capability/tool/Skill inspection.** One semantic projection derives
  from the active `CapabilitySnapshot`: the revision, the deterministic
  active model-visible Tool catalog, the complete available Tool catalog
  (including inactive definitions), and a deterministic model-visible Skill
  catalog (identity, version, name, description, host `SKILL.md` location).
  Normal agent composition guarantees canonical native Read, so the catalog is
  non-empty whenever that immutable snapshot has visible Skills. Executors,
  environment paths, package-manager state, and `SKILL.md`
  bodies never appear; ordering is deterministic; inspection never mutates
  the capability set. Available
  and active Tools are distinct fields, and provider requests use only the
  active field.
- **Agent Status projection: one frozen generation.** One primary-step
  preparation traverses the closed engine's source-owned `Time -> Background ->
  Todo` modules once against one finite opportunity set. Each interested module
  captures one immutable snapshot and evaluates it once; the accepted typed
  sections feed both `render_agent_status` for the canonical Runtime context
  UserMessageBlock and, after successful model-turn-start commit,
  `observe_status` for the Runtime Client projection. Overflow retry reuses the
  generation, and the client path never recomposes or parses rendered context
  text. A module failure is quarantined for the current attempt and does not
  fail preparation. The optional `opportunities.post_tool_batch` field carries
  the batch-level eligibility fact together with the durable position of the
  `ToolResult` batch that established it; it is omitted unless that production
  opportunity actually existed.
- **Agent Status placement: a runtime fact, bounded and historical.** The
  snapshot projects a bounded window of compositions (`statuses`, oldest
  first), not a latest value: a composed status is a historical fact of the
  conversation, so a later attempt neither retracts nor relocates it. Each
  view is placed by its eligible opportunities, and each opportunity carries
  the durable identity it was established against: `FreshInbound` the exact
  inbound message (`target_message_id`), `PostToolBatch` the durable
  transcript position of its settled `ToolResult` batch
  (`transcript_anchor`). The Agent Status Context message is request-scoped
  model history with no transcript cursor of its own, which is exactly why
  placement is published rather than left to a client to infer from arrival
  order or screen adjacency. Identity is `status_message_id`, so re-observing
  one composition is idempotent. Placement is runtime-owned; presentation —
  where a client draws a status and how subordinate it looks — is not, and no
  layout instruction crosses this boundary.

  **Where the anchor is frozen matters as much as who publishes it.** The
  Agent Loop's status linearization is `canonical ToolResult batch commits ->
  pending PostToolBatch opportunity -> next primary-step preparation -> Agent
  Status composed -> durable model-turn-start commit -> observe_status`.
  Inbound acceptance is an independent durable boundary and may legally commit
  anywhere in that window. The batch's own position is therefore frozen with
  the opportunity, at the batch commit, and travels through the composition
  into the published view; the Runtime Client projects that fact and never
  reads a current maximum transcript cursor to reconstruct it. A projection
  that sampled the frontier when it folded the status observation would place
  a `PostToolBatch` status after an unrelated inbound turn that merely
  happened to be accepted first.

  **Retention has exactly one owner.** The window bound
  (`AGENT_STATUS_WINDOW`) is Runtime Client projection policy and is not part
  of the wire contract. `agent_status_composed` therefore describes the whole
  transition: the admitted composition and, when the window was full, the
  `evicted_status_message_id` that admission pushed out. A client removes that
  identity and applies the identity-keyed admission — nothing more. This is
  what makes folding every event through cursor `C` and replacing state from
  the authoritative snapshot at `C` produce the same window past the bound as
  well as below it; two retention owners would be two policies, and a client
  that trimmed on its own would keep compositions a later snapshot repair
  silently dropped. A replayed observation admits nothing and so evicts
  nothing, which keeps replay idempotent in a full window.
- **Protocol envelope.** A transport-neutral JSON-RPC-style envelope:
  `request(id, method + typed params)`, `response(id, result | error)`,
  and `event(cursor + typed payload)` with no request ids on
  notifications. Every Runtime Client method is client-initiated
  (`initialize`, `submit_inbound`, `cancel_current_attempt`,
  `snapshot_get`, `subscribe_events`, `capability_get`,
  `background_status`, `background_cancel`, `detach`, `shutdown`).
  Typed errors distinguish `unsupported_protocol_version`,
  `attachment_in_use`, `not_attached`, `invalid_request`,
  `no_current_attempt`, `unknown_background_execution`,
  `resync_required`, `runtime_shutdown`, `invalid_state`,
  `projection_exhausted`, and `runtime_failure` — provider SDK errors and
  internal synchronization failures are never exposed.
- **Shutdown vs detach.** `shutdown` starts the one runtime drain, cancels
  the current attempt and conversation-owned work, and resolves only after
  quiescence. It is not detach. Detach and transport loss leave semantic
  runtime work running.

#### Runtime Client transports: stdio JSONL (Issue #38)

The existing local TUI transport lives in its own namespace. This diagram is
specific to that application pending #290, not the public App Server topology:

```text
rustX Runtime
      |
      v
Runtime Client projection
      |
      v
Runtime Client protocol          semantic; Issue #37/#131/#130/#136/#140/#144
      |
      v
transport adapters                framing only; src/runtime_client/transport
      |
      +-- stdio / strict JSONL    Issue #38
      |
      +-- TUI                     pending #290 migration to App Server
      |
      v
clients
```

Issue #38 added `src/runtime_client/transport/stdio.rs` for this temporary local
Runtime Client contract. #36 instead binds the App Server endpoint to first-class
stdio JSONL and WebSocket transports; #290 replaces the TUI's old wire semantics,
not stdio as a transport choice. The following describes the current #38 adapter.

- **The endpoint remains the semantic owner.** A transport calls
  `RuntimeClientEndpoint::handle_request` and forwards
  `EventSubscription` deliveries. It implements no protocol-version
  negotiation, no attachment admission, no `AttachmentId` allocation, and
  no snapshot, cancellation, replay, or shutdown semantics. The governing
  transport invariant is that only a complete, valid, in-bound-size
  framed request may cross into `handle_request`.
- **One session owns framing and I/O.** `serve_stdio_jsonl_with_io` is
  one async loop owning the endpoint, the bounded reader, the writer, and
  the framing state; `serve_stdio_jsonl` is the process-stdio composition
  of it over `tokio::io::stdin()`/`stdout()`. There are no transport
  tasks, no channels, and no ownership cycle back into the host. Dropping
  the endpoint on return is the RAII detach.
- **Record limit.** `STDIO_JSONL_MAX_RECORD_BYTES` is 8 MiB and applies
  in both directions. It bounds one record's JSON payload: the
  terminating LF is not counted, and a trailing CR is counted when CRLF
  was used on input. Inbound records are accumulated out of a fixed
  `STDIO_JSONL_READ_CHUNK_BYTES` chunk with the bound checked before
  every append; outbound records are serialized into a size-limited sink
  so an oversized record is refused mid-serialization rather than built
  and then measured. The bound is on logical record retention: each
  transport buffer holds at most one record's bytes and no reservation
  above the limit is ever requested. Allocator rounding of such a
  request is outside this contract, so `Vec::capacity()` itself is not
  claimed to be bounded by the limit.
- **LF, and accepted CRLF.** LF is the sole record delimiter. One
  physical LF terminates one record, so an escaped `\n` inside a JSON
  string stays in one record and multiline pretty-printed JSON is not
  supported. CRLF input is accepted by removing exactly one `\r` before
  the terminating LF; no other whitespace is touched.
- **Malformed and oversized input is transport-fatal.** The Runtime Client protocol has
  no uncorrelated error envelope, and a malformed frame may not even
  carry a request id, so the transport invents none. Any complete
  in-bound-size record that does not deserialize to the exact Runtime Client request
  type — malformed JSON, unknown method, unknown field, wrong parameter
  type, empty or whitespace-only record — ends the session with a
  framing error, applies nothing, and writes no protocol record. An
  oversized record is session-fatal immediately: no further buffering, no
  discard/recovery state machine, and never a partially applied request.
- **Zero outbound backlog.** The transport queues no protocol records. At
  most one outbound record is being serialized and written at a time, and
  the next input record or event is selected only after that write
  completed. The projection's bounded replay ring stays the one retained
  Runtime Client event backlog: there is no second transport history and
  no reconnect log.
- **A slow consumer stalls the transport, not the runtime.** A blocked
  output parks the transport's current write and stops it consuming
  input. Attempt execution, event publication, mailbox activity,
  background execution, and capability state continue under their own
  owners, and no projection lock is held across any transport await.
- **Active-subscription lag closes the transport.** After a stall the
  subscription may fall behind the bounded replay ring. The Runtime Client protocol has
  no uncorrelated stream-error record, so the session ends with a typed
  local `SubscriptionLagged` error carrying the cursor information and
  the client repairs from an authoritative snapshot after reconnecting.
  The semantic `subscribe_events` → `resync_required` path is unchanged.
- **EOF and broken pipe detach only.** Clean EOF at a record boundary and
  an output `BrokenPipe` are normal session ends; a partial record at EOF
  is a typed truncation error. All of them drop the endpoint and detach,
  and none cancels the current attempt, settles anything, drains the
  mailbox, mutates canonical history, or shuts the runtime down. A failed
  write is never retried, because it may have partially reached the peer.
- **Semantic shutdown does not close the transport.** A successful
  `shutdown` is answered like any other request and the session keeps
  serving: reads still work, further inbound gets the typed
  `runtime_shutdown` error, and only a later EOF or detach ends the byte
  stream.
- **Transport errors are not protocol errors.** `StdioTransportError` and
  `StdioSessionEnd` are local to the transport; nothing transport-shaped
  enters `RuntimeClientError`, and the transport writes no human or
  operator logging to its output sink — failures are returned to the
  caller for a process-composition layer to report.
- **Conformance is transport-independent.** The Issue #38 scenario suite
  (`tests/support/runtime_client_conformance.rs`) drives one set of
  semantic scenarios through a direct-endpoint driver and the stdio
  driver for the old Runtime Client contract. The new App Server parity scenario
  is `tests/support/app_server_conformance.rs`: #36 supplies stdio and WebSocket
  drivers for that shared semantic scenario, not the old protocol's scenarios.
  Byte-level framing tests stay transport-specific.

#### Runtime Client configuration projection

App Server V7 projects authored source revisions, published effective facts,
provenance, selected versus defined resources and admitted Attempt differences.
Clients never resolve configuration. Source CAS commits and full configuration
reload are separate operations. Reconnect reconstructs authoritative state and
never replays an uncertain mutation. See [the protocol](app-server-protocol.md).


### Layer 8: CFG3 composition and durable Sessions

[Configuration](configuration.md) is the authoritative authoring, overlay,
resource, publication and runtime storage reference. [Agent profiles](agent-profiles.md)
and [Web Settings](web-settings.md) describe the shared native contracts.

`UserConfigManager` captures process bindings and current source bytes. The typed
`RuntimeLayer` overlays independently named Providers/Models and explicit atomic
semantic units. Domain resource readers shadow Workspace identities before
parsing. Discovery is inert; finite Root/child/Workflow admission drives the
existing MCP/Python lifecycle owners.

`RuntimeConfiguration` and `RuntimeResourceSnapshot` form one immutable generation.
Reload builds off-side, validates all selected demand, and publishes under the
coordinator's one state lock. Active Attempt, background, child, Workflow and
maintenance ownership refuses publication with typed busy. Already-admitted work
keeps its frozen snapshots; source writes alone change no runtime.

Session persistence owns cwd and explicit model intent. Cold load rereads current
files/startup bindings and revalidates that intent. `SessionRuntimeManager` owns
residency, not a second configuration generation. The runtime root is startup-only
`~/rustx/runtime` (or explicit binding). Sessions own Conversation directories,
each with one SQLite store and managed Tool output. Typed UUIDv7 identities are
independent of explicit ordering metadata. No effective configuration is persisted
as future authority.


### Layer 9: TUI and Web

Both are thin App Server V7 clients. TUI `/settings`, `/model` and `/reload`
project/control native facts. Web [Settings](web-settings.md) separates read-only
Effective from authored User and Workspace drafts. Structured editors cover
Providers/Models, policies, Root selections, Plugins, MCP and complete named
Agents. Save is revision-fenced source publication; Reload publishes a generation.
Drafts survive CAS conflict. Uncertain outcomes trigger reads, never blind replay.


## Native YAML WorkflowRuntime (M9.5 / Issue #83)

The native Workflow layer is a bounded program boundary over the named
Subagent runtime. YAML is authoring serialization, never an execution AST:

```text
rustx.toml
    |
    +--> Agent main/workflow admission
    +--> Workflow main exposure
                  |
                  v
      .agents/workflows/<id>.yaml (discovered by filename)
                  |
                  v
         WorkflowDefinition
                  |
          compile + validate
                  |
          immutable WorkflowProgram
                  |
                  v
           WorkflowRuntime
                  |
                  v
          SubagentRuntime
                  |
                  v
      ordinary Agent Loop / Tool Plane
```

The ownership boundary is intentional. Configuration loading and candidate
publication own definition/admission, exact YAML resolution, capability
validation, and the one generation-wide publication point. The compiler owns
the canonical `WorkflowDefinition` to `WorkflowProgram` transition: it
validates the explicit entry, stable node ids, dangling edges, reachability,
acyclicity, termination, complete Branch ports, profile admission, schemas,
explicit references, and path availability. The resulting program is
immutable. `WorkflowRuntime` owns only one run's orchestration state,
workflow-local structured values, deterministic control-flow progression,
parallel joins, run budgets/cancellation, and terminal settlement.

`SubagentRuntime`/`SubagentRegistry` owns child admission, the frozen named
profile/model/capability/Skill/resource composition, retry and timeout
behaviour, approval and interaction composition, process containment,
workspace/worktree acquisition, handoff, physical child settlement, and
capacity. The Agent Loop owns model turns and canonical child conversation
semantics. The Tool Plane owns ordinary tools. There is no second Agent Loop,
Tool Plane, Subagent runtime, Workflow scheduler, or approval authority.

### Configuration and identity

Workflow YAML resources use the two `.agents/workflows` roots and complete
identity shadowing. Root `agent.workflows` is an explicit allowlist. Unused
invalid resources warn; selected invalid resources fail admission. A Workflow's
static dependencies do not grant Root direct Tool capability. A run freezes its
admitted configuration generation and complete bounded compiled program.


### Program vocabulary and typed values

The v1 program has only `Agent`, `Branch`, `Parallel`, and `Return`. Ordinary
graph edges express sequential flow; there is no Sequence/Pipeline primitive.
An Agent invokes one already admitted named profile with a fixed static task,
a frozen output schema, and one optional trusted static `override` replacing
that child's `tools`, `skills`, or `extensions` (SUB-OVR / Issue #258). It
cannot override model, instructions, workspace policy, approval authority,
execution mode, or retry policy, and its `override` is compiled program data
rather than anything model output or node input values can reach. The root and every fixed Parallel branch contain the same compiled
lexical block. Inputs and Returns use tagged reference, literal, object and
array values, including `{type: reference, path: [review, blockers]}`.
There is no interpolation or expression-string language.
Only committed schema-bound values cross node boundaries; transcripts,
reasoning, diagnostics, usage, timing, provider identity, and parent history
remain outside workflow-local values.

Branch consumes a typed boolean/equality/composition predicate and selects
exactly one true/false successor. Parallel is a finite definition-keyed set
of private multi-step blocks using the root's executor. Ordinary failure is
all-settle; native capacity saturation waits cancellably at the registry
commit boundary. Return validates and completes only its owning block.
The parent consumes declared exports, never sibling private values.

The authoring grammar, conservative type proof, static-versus-instance
identity, cancellation frontiers, aggregate budgets and retirement points are
specified in [Fixed scoped Workflow programs](workflow-programs.md).

Workflow v1 uses a deliberately closed recursive JSON Schema vocabulary. A
schema must declare one of `array`, `boolean`, `integer`, `null`, `number`,
`object`, or `string`, and may use only `type`, `properties`, `required`,
`additionalProperties` (boolean), `items` (one nested schema), `enum`, and
`const`, excluding numeric const/enum refinements. These rules apply identically to Workflow input/output schemas,
Agent output contracts, and Parallel branch outputs. Unsupported
value-constraining keywords—including length/range/item constraints and
`oneOf`/`anyOf`/`allOf`—are rejected recursively. Compatibility is proven
conservatively over this vocabulary; finite `enum`/`const` producers are
checked value-by-value, and an unknown or unprovable relationship rejects the
program rather than relying on runtime validation.

### `workflow_output` and terminality

`workflow_output(value)` is a reserved Workflow/Agent terminal protocol. It
may be represented as a tool-shaped provider call, but the canonical Tool
registry rejects that model-facing name for every ordinary Builtin and
MCP registration. A Workflow child therefore receives exactly one
provider-visible `workflow_output` definition. The Agent Loop consumes it
before ordinary Tool Plane preflight or dispatch, so it has no
ordinary `ToolExecutionId`, no business-tool side effect, and no normal child
ToolResult continuation. A valid value is checked against the frozen Agent
output schema and commits exactly once; a normal final Assistant message is
not successful Workflow Agent completion. Invalid output commits nothing and
gets bounded schema feedback. A successful protocol call is the sole
tool-shaped call in its Assistant turn: mixed ordinary calls or duplicate
terminal calls execute no ordinary side effect, commit no value, and return
bounded protocol feedback for a later model turn.

The output latch and native cancellation authority share one explicit
linearization point. If cancellation wins, later output is stale; if valid
output commits first, later cancellation cannot rewrite completion. The
Workflow run reaches terminal settlement only after all owned child work has
reached the native settlement point. Runtime drain proves zero
Workflow-owned active work; no detached child survives successful quiescence.

### Publication, reload, and recovery

Startup and reload construct one complete candidate: configuration, exact
discovered YAML, frozen Workflow admission, Agent selections, capability
availability, and native Tool registrations. Validation happens off-side and
one publication boundary makes the coherent generation visible. An invalid
candidate leaves the previous valid generation untouched. A foreground
Workflow Tool captures the immutable program at registration; a run therefore
retains its program snapshot across later reloads, while reload affects only
future runs.

Ordinary Workflow lifecycle and join events (`started`, child admission,
Branch selection, Parallel admission/settlement, and terminal
completion/failure/cancellation) are best-effort observability facts appended
through the existing Event Journal. An ordinary lifecycle/join append failure
does not become Workflow state authority, change control flow, or rewrite a
terminal result. The successful child value is different: its
`WorkflowAgentOutputCommitted` fact is committed atomically with
`SubagentTerminalSettled`; a failed or cancelled child uses the dedicated
terminal-settlement transition. Neither path creates a parent inbound
notification or a durable `settled -> delivered` phase; the facts are
lifecycle evidence for recovery, while the live `WorkflowRun` remains the
execution authority. The Workflow Tool returns one bounded parent ToolResult;
intermediate values and child transcripts are not injected into parent
canonical history. There is no durable Workflow resume or crash replay: a
pre-crash nonterminal run is not silently rerun, because Agent and Tool work
may be non-idempotent.

Workflow child composition uses the existing capability projection,
`ApprovalMode`, `ToolApprovalPolicy`, and `InteractionCoordinator`. FullAccess
can affect approval only for already-authorized capabilities; it cannot widen
the selected profile. `ask_user` remains an ordinary admitted capability and
WorkflowRuntime never manufactures it or calls the TUI. Named Subagent
worktree/workspace ownership and `WorkspaceHandoff` remain native boundaries.
A future Canvas may edit the same `WorkflowDefinition`; Canvas, layout
metadata, and UI scheduling are out of scope.

## 3. Dependency rule

Dependencies point inward.

```text
Interfaces / projections
        |
Runtime services
        |
Model / Tool / Skill implementations
        |
Context engine
        |
Agent kernel
        |
Domain and protocol types
```

Forbidden dependencies include:

```text
Agent kernel -> OpenAI SDK
Agent kernel -> Anthropic SDK
Agent kernel -> rmcp
Agent kernel -> database client
Agent kernel -> HTTP framework
Agent kernel -> control-plane schema
```

## 4. Message model

The canonical conversation model contains exactly three message roles:

```text
UserMessageBlock
AssistantMessageBlock
ToolMessageBlock
```

Semantics:

- `UserMessageBlock`: inbound information supplied to the current agent. The source may be a human, another agent, the control plane, or an external system.
- `AssistantMessageBlock`: model output produced by the current agent.
- `ToolMessageBlock`: result of a tool call produced by the current agent.

System instructions are not conversation messages. The runtime assembles
typed request-time System Sections and renders their only provider-neutral
authority into `ModelRequest.effective_system_prompt`.

Identity and provenance are metadata. Message role does not encode real-world identity.

The Message Ledger is append-only immutable canonical fact storage. The
Conversation Surface is the sole authority for active identity, order, and
visibility. `SurfaceRevision` is a stable reconstruction reference within
the single conversation lineage; later mutations never alter an earlier
revision. Normal projection uses current Surface identities and keyed Ledger
reads only, never full Ledger or Surface-history scans.

Provenance is implemented as typed runtime-owned metadata: `UserSource`
distinguishes human, agent, fleet, external-system, and runtime sources. A
runtime compaction summary is represented as a
`UserMessageBlock` with runtime provenance and `InboundKind::CompactionSummary`;
no fifth message role exists. Ordinary inbound messages carry their
persisted UTC instant on `UserMessageBlock.timestamp` (supplied by the
producer, never fabricated); derived compaction summaries carry `None`.

Agent-to-agent communication uses a durable mailbox model. A `send_message` tool result reports only whether delivery was durably accepted or rejected. The recipient later receives the content as a `UserMessageBlock`.

## 5. Turn model

A turn is:

```text
one model response
+ all tool calls emitted by that response
+ all corresponding tool results
```

Tool execution may be parallel or sequential. Runtime execution events may follow actual completion order, while canonical tool-result ordering follows the original tool-call order for deterministic context construction.

Inbound mailbox messages may arrive at any time but are injected only at safe turn boundaries.

## 6. Durability model

The runtime distinguishes execution events from conversation messages:

```text
RuntimeEvent = execution fact
MessageBlock = model-context fact
```

Runtime events are append-only. In production, a successful Event Journal
append must commit before the event is published to external subscribers. A
failed required terminal append publishes neither the terminal event nor a
synthetic replacement; the owning runtime reports the durable failure.

A canonical `AssistantMessageBlock` is committed only when a complete model
response has been assembled. The model plane communicates through the one
normalized `ModelStreamItem` protocol. Canonical `Event(ModelEvent)` items are
adapter-to-kernel facts; the ephemeral `Progress(Generation | Liveness)` arm
is provider-derived execution evidence only. Neither arm is inserted directly
into canonical history: the agent kernel assembles one `AssistantMessageBlock`
from canonical events, while progress is consumed only by request-local
deadline state.

Partial model deltas are **not** Event Journal facts. They belong to the
durable publication plane described in section 6.1, which owns the user-facing
release contract and its own bounded write policy. The Event Journal keeps the
low-frequency recovery-significant semantic facts only, so its size is
O(execution facts) rather than O(provider deltas).

Normally exactly one terminal runtime event settles an attempt (see section
2.2). If its required append fails, the attempt has a typed settlement
candidate but no terminal Journal fact. Committed-message events reference
the message by identity only: canonical message content exists solely in the
Message Ledger, and the Event Journal records the commit fact (see section
2.5).

Persist-before-publish is the frozen event-publication invariant:

```text
generate RuntimeEvent
→ durably append / commit sequence
→ publish externally
```

It applies to every externally published runtime fact. Facts that reference a
Ledger body, Surface revision, or Request Snapshot use the shared
ConversationStore transaction so publication never outruns its authority.
Successful publication does not add the committed event to an attempt-local
trace: the observer is a live projection seam, while the durable Event
Journal remains the historical authority.

## 6.1 Durable user-facing publication (FND-03 / Issue #108)

Publication durability is a **separate plane** from provider outcome and from
canonical conversation acceptance. It exists to hold one user-facing contract:

> No semantic output is released to a user-facing Runtime Client before rustX
> has durably committed that publication.

### 6.1.1 Three linearization points

Any request that emits user-facing model output has three distinct commit
points, owned by three distinct planes:

```text
P — Provider outcome         ModelRequestCompleted durable   (Event Journal)
U — Publication outcome      final frame + terminal marker   (publication plane)
C — Conversation acceptance  canonical Assistant durable     (Message Ledger)
```

The required commit ordering is:

```text
P < U < C
```

and the durable store — not only Agent Loop control flow — enforces the
implication:

```text
C => U => P
```

The store proves this chain on every dependent transition. U and C reload and
decode the exact Request Snapshot, verify its durable `ModelRequestStarted`
envelope, and re-check the successful `ModelRequestCompleted` fact for that
same request. C additionally requires the frozen provisional Assistant
`MessageId`, an `AssistantMessageCommitted` event whose conversation,
attempt, and turn envelope equal the stream, and an event payload naming that
same message. These checks happen before the compound transaction can change
the Ledger, Surface, Journal, staging, proposal, or settlement state.

P and U are deliberately never combined into one transaction. "The provider
finished" and "rustX committed this output for release" are different facts,
and a crash between them must stay distinguishable. Likewise
`ModelRequestCompleted` is never merged with the Assistant commit: provider
completion remains an external execution fact even when canonicalization later
fails.

### 6.1.2 Pipeline

Provider chunk size is not the publication unit:

```text
Provider ModelStreamItem::Event canonical delta
  -> in-memory assembler            (canonical message assembly)
  -> bounded publication coalescer  (bytes / latency / structure / terminal)
  -> typed publication frame
  -> durable publication staging
  -> user-facing release
```

The coalescer (`src/publication/coalescer.rs`) flushes on a bounded
deterministic policy: a maximum byte threshold, a structural boundary (a
tool-call proposal start or completion), or the stream terminal. When the
first payload enters an empty buffer, it owns one absolute monotonic deadline
`oldest_pending_time + max_latency`; later canonical provider events never reset it. The
coalescer owns that deadline and asks the runtime monotonic clock for the
wake-up future, so a quiet provider still flushes at the deadline and the
Agent Loop never starts a fresh full-duration debounce timer. Deterministic
tests install a manually advanced clock; no wall-clock sleep decides a flush.

### 6.1.3 The U transaction

There is deliberately no "write the final frame, publish, then mark the stream
complete" sequence to crash inside. When P has committed and provider
completion is structurally accepted, the remaining publication payload and the
publication terminal marker commit in **one** transaction; only then is the
final buffered payload released. When no payload remains, a terminal-only
frame still carries the terminal transition, so nothing visible is delayed
that does not exist.

### 6.1.4 Three mutually exclusive settlements

One publication stream settles exactly once:

```text
Canonical                   U reached, C reached — the Ledger is the authority
UnacceptedPublicationAudit  U reached, C never   — complete output, never accepted
IncompletePublicationAudit  U never reached      — publication has no durable terminal
```

Incomplete is defined on the **publication** boundary, never the provider
boundary:

> Incomplete Publication means user-facing publication did not reach its own
> durable terminal boundary. It does not imply that the provider necessarily
> failed to reach transport termination.

So a stream whose `ModelRequestCompleted` is durable but whose U never
committed is Incomplete, and a structural `assembler.finish()` rejection after
frames were already released is Incomplete (no P exists at all).

Once either audit commits, canonical Assistant acceptance of that stream is
permanently forbidden, and once canonical acceptance commits, no audit may be
created for it. The canonical transition validates that the exact stream is
publication-complete, appends the Ledger fact, advances the Surface, records
`AssistantMessageCommitted`, and clears the stream's publication staging — all
in one transaction.

The first `open_publication_stream` transition applies the same proof before
inserting anything. A missing or malformed Request Snapshot, foreign request,
attempt, turn, message, or derived stream identity is rejected. Identical
reopens remain idempotent only after that proof succeeds.

### 6.1.5 Durable lifecycle staging versus immutable audit

While a stream is in flight, its frames are **transient lifecycle staging**.
Settlement removes that staging in both directions: canonical acceptance
deletes it, and audit terminalization consolidates it into one bounded
immutable audit object. A stream that staged ten thousand frames therefore
leaves either nothing or one bounded record, never O(number-of-frames)
permanent history.

### 6.1.6 Proposal staging state machine

`publication_proposals` is the store-owned proposal state machine; the
assembler and Agent Loop may reject the same malformed sequence earlier, but
they are not the authority. A `ProposedToolCallStarted` frame creates exactly
one `(stream_id, call_id)` owner and freezes its block index, tool ID, and
name. An arguments suffix requires that same stream-local owner, the frozen
block index, and the `started` state. A completion requires the same owner,
block index, tool ID, and name, and changes `started` to `completed` exactly
once. Duplicate starts, duplicate completions, completion without start,
foreign stream ownership, and suffixes after completion are typed durable
violations. The complete frame batch is preflighted before any frame, owner,
sequence, or terminal marker is changed, and the same validator is used by
ordinary staging and U terminal staging.

Audit consolidation may only materialize a proposal that has this durable
owner and matching state. C performs the reverse check as well: every
canonical Assistant `ToolCall` must match a frozen stream-local owner and a
durable `completed` state, every current-stream `completed` owner must appear
exactly once in that Assistant, and no `started`-only owner may remain. The
comparison covers `call_id`, block index, tool ID, and name before the compound
C transaction begins. Thus a provider may reuse a raw call ID in a later
publication, but ownership can never be silently reassigned to an earlier
stream or silently omitted at canonicalization.

### 6.1.7 Audit semantics

A publication audit records the semantic output rustX durably committed **for
release**. It is an upper bound on what may have been displayed and never
proof of perception; rustX adds no Runtime Client ACK protocol.

### 6.1.7a One-shot unresolved-output carryover (Issue #137)

Publication Audit remains the sole body authority. A terminally unresolved
audit may be selected once as a bounded `UnresolvedOutputCarryover` for the
first later eligible primary model start, but that projection is explicitly
request-only. The durable root stores only
`pending_unresolved_output_stream_id`; the bounded rendering is frozen by
value in the consuming Request Snapshot. No canonical Assistant or User
message, Ledger row, Surface identity, fabricated `MessageId`, lineage seed,
transcript entry, Runtime Client field, or carryover event is created.

The provider-neutral request type is:

```rust
enum ModelInputMessage {
    Canonical(MessageBlock),
    RequestOnly(RequestOnlyModelContext),
}

enum RequestOnlyModelContext {
    UnresolvedOutputCarryover(RenderedUnresolvedOutputCarryover),
}

struct RenderedUnresolvedOutputCarryover {
    source_stream_id: PublicationStreamId,
    source_settlement: UnresolvedOutputSettlement,
    records: Vec<RenderedCarryoverRecord>,
    omitted_blocks: CarryoverOmissionCounts,
}

enum UnresolvedOutputSettlement {
    Incomplete,
    Unaccepted,
}
```

Canonical identities remain real Ledger identities; the request-only variant
has no canonical identity. The Agent Loop assembles the order before adapter
translation. For FreshInbound it inserts the carryover immediately before the
first canonical message of the pending fresh turn. For a Continuation with no
fresh inbound it inserts it after the existing canonical projection and before
newly staged current context. This anchor and the exact admitted bounded
representation are frozen in the Request Snapshot, so reconstruction does not
consult the current pointer, audit, Surface head, or runtime.

The publication boundary converts `PublicationAuditKind::Incomplete` or
`PublicationAuditKind::Unaccepted` once into the model-input-owned
`UnresolvedOutputSettlement`. The frozen representation preserves that
settlement and renders `source_settlement=incomplete` or
`source_settlement=unaccepted` in Full, Reduced, and MetadataOnly forms.
Historical reconstruction copies the frozen value without another audit load;
only Omitted removes the request-only item.

The shared selector takes the last durably started `RequestIdentity` and
checks retry ordinals `N, N-1, ..., 0`, deriving each request, provisional
message, and publication-stream identity before a keyed audit load. It accepts
only `Incomplete`/`Unaccepted` audits with meaningful renderable content,
falls back from an empty latest audit, and never scans or concatenates audits.
Live settlement and crash recovery call this same implementation. An internal
retry that eventually reaches canonical Assistant acceptance installs no
carryover from its earlier failed generations.

Producer and consumer boundaries are separate durable linearization points:

```text
durable publication evidence
  → shared selector
  → one transaction: attempt terminal + replace/clear pending source

eligible primary start
  → one transaction: freeze snapshot representation/anchor
                  + commit ModelRequestStarted semantics
                  + clear pending source
```

If the producer transaction does not commit, neither terminal nor pointer
change is visible. Cancellation before the consumer start commit preserves the
pointer; a committed start consumes it exactly once. Actual-request retries
reuse frozen logical-step semantics and never reread the pointer or audit.
Overflow fit may only degrade carryover from full to reduced, metadata-only,
or omitted. It is auxiliary best-effort context: it cannot force compaction,
cause `CannotFit`, evict protected fresh inbound, or move back toward detail.

Rendering uses a 4096-byte final UTF-8 bound, 2048-byte per-text-block tail
bound, and 512-byte whole-tool-argument bound. Whole records are admitted
newest-first and restored to source order with structural omission metadata.
Reasoning is runtime-authored narration, never provider-thinking continuation;
tool proposals remain explicitly complete/incomplete, unaccepted, and not
executed. Payload is escaped as data inside deterministic non-closable records.
Carryover is excluded structurally from `ModelBackedSummarizer` input and from
`LineageSeed`; it has no relationship to Agent Status or provider continuation
state. The Issue #137 durable-state change was local to development schema
version 15; `EVENT_SCHEMA_VERSION` remains 1 because no Event Journal
envelope changed, and that carryover-only change did not alter the Runtime
Client protocol because carryover has no wire field or consumption event.

### 6.1.8 Model-proposed tool calls versus Tool Plane execution

A tool call appearing in a publication frame or audit is only a **model
proposal**. The vocabulary names it so (`ProposedToolCallStarted`,
`ProposedToolCallArgumentsSuffix`, `ProposedToolCallCompleted`), and the
durable store enforces the hard invariant:

> No tool proposal from an Incomplete or Unaccepted publication may have a
> dependent `ToolExecutionStarted`, `ToolResult`, or side-effect
> authorization.

This is one store-layer owner, reused for foreground execution start,
progress, completion and failure, single and batch canonical ToolResult
commits, recovery ToolResult repair, background authorization, and subagent
ownership. Whenever a transition carries a tool ID, the owner compares that
ID with the proposal or canonical Assistant owner frozen for the call; a
matching bare `call_id` is not enough. The proposal table owns
`(stream_id, ToolCallId)` rather than a conversation-global bare call ID; a
provider reuse in another publication is a distinct proposal, never a silent
reassignment. Canonical C retains the accepted ownership row and marks it
canonical so later Tool Plane transitions resolve the exact accepted proposal.
Audited rows remain permanently barred.

Transcript and UI consumers therefore distinguish a released proposal from a
real Tool Plane invocation fact by which plane it came from.

### 6.1.9 Request-pinned resource generation

A publication stream is pinned to the exact attempt, turn, request, and
provisional message identity that opened it. FND-01 (Issue #106) owns resource
loading and reload; this plane preserves that boundary:

- external edits to project instructions, Skills, or extension Tool
  configuration during streaming cannot alter the in-flight provider request,
  the model Tool schemas, preflight authority, publication classification, or
  the later canonical Assistant of that stream;
- the public reload operation returns `Busy` while the attempt owns the
  session; it never aborts or splices a new generation into publication;
- after the attempt ends, a successful reload may affect a later admitted
  attempt only;
- recovery classifies P/U/C and tool-proposal state from rustX-owned durable
  evidence, never by re-reading current resources or the current Tool registry.

A process death followed by a cold reopen may load current resources for
future requests, but the old stream's publication settlement stays tied to its
frozen historical request.

### 6.1.10 Intentional tail-latency tradeoff

Any user-facing payload still buffered when provider completion is accepted
waits for P to commit and then for U to commit before it is released. If no
payload remains, a terminal-only U frame still commits but no visible text is
delayed. This tail latency is the cost of correct ordering and honest audit
classification, and it is intentional.

## 7. Recovery model

Runtime process memory is disposable.

> **Durability says what happened. Recovery classification says what can
> safely happen next.**

M8 answered *what durably happened*. M9a (Issue #12) answers *given exactly
what durably happened, what state is this conversation in after restart, and
what is safe to do next*. The governing invariant is:

> Recovery reconstructs what durably happened; it never invents success, never
> silently replays an ambiguous external side effect, and never regenerates
> historical request/context from current configuration.

and, three times over:

```text
exact historical reconstruction  !=  safe replay permission
started + outcome unknown        !=  safe retry
started + outcome known          !=  never externally started
```

The recovery evidence model keeps the **external execution lifecycle** and
the **canonical structure lifecycle** on separate axes. Only an attempt with
zero durable external-start evidence — no `ModelRequestStarted`, no
`ToolExecutionStarted`, ever — may be classified as the safe Class-B
continuation case; a crash/restart/recovery cycle never turns historical
external-start evidence into a later claim that no external work started.

### 7.1 Owner

Recovery **policy** is owned by `ConversationRuntime` (`src/runtime/recovery.rs`,
driven from `ConversationRuntime::new`) and consumes `ConversationStore`
evidence. The store exposes durable facts and semantic transactions; it never
decides whether an ambiguous request is replayable. No recovery policy lives in
the SQLite backend, the Runtime Client, a provider adapter, the mailbox, the
TUI, or a background producer.

Recovery runs **after** the tool-runtime ownership transfer and **before** the
runtime object exists. Both halves matter: a construction that loses the
ownership race must leave no trace, so it must never have reconciled anything;
and the claim's pristine-background-plane precondition is what proves a
durably-owned-but-unpublished background execution has no live in-process
record. Because no coordinator exists yet, no recovery SQLite work can ever run
under the admission mutex, and activation/admission cannot race an unfinished
reconciliation.

### 7.2 The four phases

```text
durable facts
    -> reconstruct   RecoveryEvidence::reconstruct  (read only)
    -> classify      RecoveryPlan::classify         (pure)
    -> reconcile     RecoveryPlan::reconcile        (atomic durable commits)
    -> recovered runtime state
    -> resume        ResumeDisposition              (a permission, not a replay)
```

- **Reconstruct** reads the durable Surface head and its active bodies,
  Pending Inbound, and a paged fold of the Event Journal. It commits nothing,
  invokes no provider or tool, fabricates no observation, and executes no
  context contributor.
- **Classify** is a pure function of that evidence. It never depends on
  wall-clock timing, current provider availability, current plugin/config
  state, whether a Runtime Client is attached, or a random retry decision.
- **Reconcile** commits the new recovery facts, each atomically.
- **Resume** is a typed permission the runtime consumes at activation.

### 7.3 Evidence sources

Startup may consume only rustX-owned durable authority:

| Source | Used for |
| --- | --- |
| Conversation Surface head + checkpoint metadata | the recovered active working set |
| Message Ledger (keyed reads) | active message bodies, canonical structure |
| Pending Inbound Inbox | accepted-but-unadopted work |
| Request Snapshots | exact historical request reconstruction |
| Event Journal (paged fold) | attempt/turn/model/tool/background lifecycle |
| Publication streams (unsettled rows + staged frames) | publication settlement classification (FND-03) |

Historical truth is never reconstructed from a Runtime Client snapshot or
cache, TUI cards, current DSH state, current Skill discovery, current Agent
Status, current filesystem state, current `rustx.toml`, a live
`ContextContributor` run, regenerated dynamic context, or old process-memory
registry contents. Current configuration configures **future** work only.

### 7.4 Classification matrix

| Class | Durable evidence | Recovery action | Resume |
| --- | --- | --- | --- |
| **A — not started** | no attempt fact at all | none | ordinary Pending Inbound admission |
| **B — admitted, no external start** | `AttemptStarted`, **no `ModelRequestStarted` ever**, **no `ToolExecutionStarted` ever** | one interrupted attempt terminal | ordinary Pending Inbound admission |
| **C — external start committed, outcome unknown** | `ModelRequestStarted` with no durable outcome, and/or `ToolExecutionStarted` with no durable outcome | canonical tool-turn repair, then one interrupted attempt terminal | blocked: recovery starts nothing |
| **D — durable terminal exists** | one terminal attempt fact | none (absorbing) | ordinary Pending Inbound admission |
| **E — external start committed, outcome durably known, settlement incomplete** | a `ModelRequestStarted` followed by `ModelRequestCompleted`/`ModelRequestFailed`, and/or `ToolExecutionStarted` followed by a durable outcome — with no attempt terminal | canonical tool-turn repair (exact durable result), then one interrupted attempt terminal | ordinary Pending Inbound admission; **no** automatic resend or replay |

The attempt class answers "what happened to the external plane", and nothing
else. **Whether a turn is still owed an answer is a separate durable
question**, answered by the answer obligation below, so every class except C
can continue an unanswered turn and none of them continues an answered one.

A settled ToolResult batch without a new `InboundTurnAdopted` fact does not
open another answer obligation. This is the deliberate post-tool recovery
contract: the earlier `ToolExecutionStarted` evidence proves that external
work crossed its start boundary, so a dead attempt is terminalized with
`PendingInboundOnly` rather than replaying a continuation model request. The
attempt-local `PostToolBatch` marker is never recovered and recovery never
creates a model step merely to consume it. A fresh inbound batch adopted at a
safe boundary still follows the separate durable answer-obligation contract.

Class B is the **only** state whose meaning is "no external work started" *for
an attempt that exists*: it requires durable proof that **zero** external-start
commits ever occurred for this attempt. A resolved outcome is not "never started" — the two facts live
on separate axes and never collapse:

```text
started + outcome known   !=  never started
canonical ToolResult committed  !=  historical ToolExecutionStarted erased
```

Per plane:

- **Model.** The request lifecycle is monotonic: `NeverStarted` →
  `StartedOutcomeUnknown` → `StartedOutcomeKnown`. `ModelRequestCompleted` or
  `ModelRequestFailed` never moves an attempt back to "no request started".
  `ModelRequestStarted` + no outcome means the provider may have received and
  executed the request: recovery reconstructs the exact provider-neutral
  request for diagnosis, classification, and audit — and performs **zero**
  automatic resend. A durably known request outcome (Class E) is preserved as
  a durable fact: the attempt settles honestly, but the canonical Assistant
  message never committed, so **no response body is fabricated** from
  `ModelRequestCompleted`, and **nothing is resent**. A durably **failed**
  request is never converted into a silent retry: M9a has no generic retry
  engine, and the historical failure stays durable. Request ambiguity and
  attempt settlement are different facts: the request outcome stays unknown
  while the attempt settles.
- **Foreground tools.** External execution history and canonical repair
  evidence are separate axes with separate owners. Each unanswered call on
  the current Surface is answered from durable evidence only: a durably
  known outcome is used verbatim; a started call with no outcome becomes
  `ToolExecutionStatus::OutcomeUnknown`; a call with no start evidence at all
  becomes `ToolExecutionStatus::Cancelled { reason: ParentCancelled,
  phase: BeforeStart }` because nothing external happened.
  A committed canonical `ToolResult` releases the call's detailed per-call
  repair evidence; the owning attempt's **bounded external summary**
  independently keeps proving the historical `ToolExecutionStarted`, so a
  crash between the repair commit and the attempt terminal can never
  reclassify an indeterminate attempt as Class B. Tool repair evidence is
  keyed by owning attempt **and** call id: the durable authority does not
  guarantee `ToolCallId` uniqueness across the conversation lifetime
  (providers mint call ids; each provider response rejects duplicates), so
  historical attempts can never alias the current unresolved call. No tool
  is re-executed. The missing siblings of one Assistant turn commit as one
  atomic batch in canonical model-call order, so no durable prefix of a
  sibling batch is ever observable.
- **Background.** A committed async background execution survives the starting
  *attempt*, not the *process*. A durably owned, never-published execution is
  terminalized as `BackgroundTerminalState::OutcomeUnknown` — never `Failed`,
  never relaunched — and its model-visible notification is published through
  the one Pending Inbound authority in the same atomic transition as the
  `BackgroundTerminalPublished` fact.

### 7.5 Recovery-generated durable transitions

| Transition | Before the commit | After the commit |
| --- | --- | --- |
| `append_canonical_batch_with_events` (tool-turn repair) | the turn is structurally incomplete; no recovered result exists | every issued call owns exactly one committed `ToolResult`; the turn can form a valid later model request |
| `append_event(AttemptFailed { RestartInterrupted })` | the attempt is durably non-terminal | the attempt is absorbing; a second reconciliation is refused by the durable lifecycle |
| `accept_inbound_with_event(terminal notification, BackgroundTerminalPublished)` | no model-visible terminal exists; recovery owns publication | the notification and the terminal fact both exist, exactly once |
| `accept_subagent_terminal(no notice, interruption message, SubagentTerminalPublished)` | an owned normal subagent child never settled; recovery owns the interruption publication | the runtime-authored interruption message and the terminal fact both exist, exactly once, through the same transition the live path uses (Issue #192) |
| `terminalize_publication_audit` | a publication stream is unsettled staging | the stream holds one bounded immutable audit, its staging rows are gone, and canonical acceptance of it is permanently forbidden |

Recovery-generated canonical facts carry **no** attempt or turn identity: they
are facts of the startup recovery phase, never retroactive claims about what
the dead attempt did. The one exception is the attempt terminal itself, which
must name the attempt whose lifecycle it closes.

If a reconciliation transaction fails, recovery fails closed: no fabricated
success is published, no runtime is constructed, and nothing is admitted as
though recovery had completed. The same applies to a durable authority that
holds two concurrently non-terminal attempts: that contradicts the
one-active-attempt admission invariant, so recovery reports it instead of
settling whichever attempt sorted first and silently leaving the other
unresolved.

### 7.5.1 Publication settlement classification (FND-03 / Issue #108)

Recovery reconciles publication staging without consulting the current
provider or workspace. The classification is entirely durable:

```text
staging + no U   -> IncompletePublicationAudit
U + no C         -> UnacceptedPublicationAudit
C                -> canonical authority; staging must not survive
```

The audit kind is derived by the durable store from the P/U evidence alone, so
no control-flow path — live settlement or recovery — can mislabel an
Incomplete publication as Unaccepted or the reverse. Terminalization
consolidates the transient frames into one bounded immutable audit and removes
the staging rows, so a stream that staged thousands of frames leaves one
bounded object behind.

Publication settlement runs **before** tool-turn repair and the attempt
terminal, so a crash inside the remaining reconciliation still leaves a state
the next startup classifies exactly as truthfully (see section 7.11).

An audit is never a canonical Assistant message: recovery produces no Ledger
row, no Surface advance, and no canonical model-visible context from it. Any
tool proposal it records may never acquire a dependent Tool Plane execution
fact. The narrow Issue #137 exception is the later request-only carryover
projection described below; it is runtime-authored context, not the audit
body becoming canonical or provider continuation.

For Issue #137, an unresolved logical model step may also leave one pending
carryover source. After publication streams are terminalized, recovery loads
the Request Snapshot of the last durably started request and invokes the same
keyed descending-ordinal selector used by live settlement. If a later retry
already committed the canonical Assistant, recovery selects no older audit;
otherwise it selects the highest meaningful `Incomplete`/`Unaccepted` audit,
falling back deterministically from an empty latest generation. The source is
never selected by a conversation-wide audit query or by hot memory alone.

The selected source (including the explicit `None` result) is passed to one
durable semantic terminal transition that commits the recovery attempt
terminal and replaces/clears `pending_unresolved_output_stream_id` together.
If recovery crashes before that commit, the audits and Request Snapshot facts
remain durable but no half-transition is exposed. A second recovery derives
the same request identity, loads the same keyed audits, selects the same
source, and can commit the same terminal/pointer transition. Every committed
recovery prefix is therefore a valid input to the next recovery.

### 7.6 Terminal uniqueness and repeated-restart idempotence

Terminal uniqueness is owned by the durable `lifecycle_state` table, never by
an in-memory flag. `attempt:{id}` and `background:{execution_id}` accept
exactly one terminal fact; a second is a typed `TerminalViolation`. After the
first successful recovery the classification is Class D with no unpublished
background work, so every later restart commits nothing and durable state stops
changing.

### 7.7 Identity recovery

Two process-local ordinals could otherwise collide with durable history after a
restart:

- `AttemptId` — allocated by `ConversationRuntime` as
  `AttemptId::for_conversation(conversation, n)`, an explicit bijection with a
  conversation-scoped ordinal. Recovery folds durable attempt facts back
  through `AttemptId::conversation_ordinal` and reseeds the allocator past
  every ordinal in durable authority. Independently, the Event Journal refuses
  a second `AttemptStarted` for one identity.
- `ToolExecutionId` (`exec_N`) — reseeded from the durable
  `BackgroundExecutionCommitted` facts before the runtime activates, while the
  background plane is provably pristine.

### 7.8 Pending Inbound across a restart

Still-pending stays pending with its exact `InboundSequence`, `MessageId`,
provenance, content, timestamp, and correlation. Already-adopted is a canonical
Ledger fact, is not pending, and is never re-adopted — identity, not content
equality, is the idempotency key. Finite watermark semantics are unchanged.
There is deliberately no separate "recovery queue": the durable Pending Inbound
Inbox *is* the queue of accepted-but-unadopted work, and an idle recovered
runtime admits it at activation with **zero** Runtime Client attachments.

### 7.9 Durability health after recovery

A successful recovery starts a fresh admission cycle. A previous process's
crash never poisons a runtime whose classification and reconciliation
succeeded, and an unresolved durable inconsistency is never silently converted
into a healthy state: it fails construction instead. Recovery failure and the
bounded live admission retry are distinct concepts; neither is overloaded into
the other.

### 7.9.0 The durable answer obligation

Recovery must continue exactly the turns a live runtime would still owe an
answer for — no more, no fewer. That question is **not** derivable from
canonical shape (a trailing human message looks identical whether it was
answered, cancelled, supplied as a fork seed, or accepted one millisecond ago)
and it is not derivable from the attempt class either. It is therefore its own
durable fact.

`RuntimeEvent::InboundTurnAdopted` is committed **inside the canonical adoption
transaction**, naming exactly the messages that transaction adopts. It is the
one durable statement that says "rustX accepted this work", and the durable
authority rejects an adoption whose obligation names anything else, so a
canonical `UserMessage` and the obligation to answer it can never disagree.

The obligation is **consumed** — never re-derived — by the first of two later
facts:

```text
adoption ──▶ obligation open
                │
                ├─ ModelRequestStarted ──▶ consumed: the turn reached the
                │                          provider; the external-outcome
                │                          plane owns it from here
                └─ attempt terminal ─────▶ consumed: the runtime concluded the
                                           turn (completed, cancelled, failed,
                                           timed out, limited)
```

This is what makes the ownership chain explicit across the three transitions
that can strand a turn:

- adoption commits **before** `AttemptStarted`, so a process that dies in that
  window leaves an adopted turn with *zero* attempt evidence;
- adoption also happens **mid-attempt**, at the Agent Loop's safe boundary,
  where the attempt's own request plane still reports the *previous* request's
  outcome;
- a conversation's second and later turns are adopted while the journal already
  holds complete, settled attempts.

Recovery resumes `ContinueAdoptedTurn` exactly when an obligation is open and
no external outcome is indeterminate; indeterminacy dominates. Supplied
bootstrap history — a fork or clone seed, a tree node, a persona lineage —
enters through `initialize`, which is not an adoption and commits no
obligation, so a reopened seeded lineage answers nothing it never accepted.
Recovery reads nothing but the obligation's own yes/no answer, so the evidence
stays O(1) however large the lineage or the adopted batch is.

### 7.9.1 Real process-death conformance (FND-06 / Issue #111)

The recovery contract above is proved against an actual `SIGKILL` of an actual
process running the actual runtime stack, not against a dropped store handle:

```text
parent test process
  -> spawns a child running the real runtime stack over a real durable file
  -> child reaches one named durable boundary and freezes there
  -> parent SIGKILLs the child's whole process group
  -> parent reopens the durable authority and runs real recovery
```

A boundary is a durable linearization point, never a wall-clock moment. The
`cfg(test)`-only seam `crate::runtime::process_death` parks a child before or
after one durable transition *while it holds the store's connection mutex*, so
a parked process is incapable of committing anything else from any thread. The
second rendezvous kind is a control socket the parent uses to edit resources
underneath a live runtime, or to kill a process while a compaction summary side
request is in flight. Ordering claims are read from the Event Journal by
sequence; nothing sleeps or polls to reach a state.

The complete boundary matrix — durable facts before the kill, allowed and
forbidden post-reopen state, recovery action, and, for the resource cases, the
loaded generation and the exact old/new model API context — is
`docs/process-death-conformance.md`.

### 7.10 Bounded working set

The evidence fold pages the Event Journal and retains only the *unresolved*
state. Reads are O(history); hot memory is O(unresolved work):

```text
recovery hot memory =
    O(nonterminal attempt summaries)      (at most one by the admission invariant)
  + O(canonical tool repairs outstanding) (only while a ToolResult is missing)
  + O(unpublished background executions)  (bounded by background policy)
  + O(active Surface attribution)         (bounded by the active working set)
```

A resolved entry is dropped the moment its resolving fact is read, so
complete Event Journal, Request Snapshot, and Ledger history are never
materialized as recovery state.

The tool plane is split across two owners on purpose:

- **Attempt-level external summary.** `AttemptEvidence` owns a bounded
  summary of the attempt's foreground-tool external history — did external
  execution happen, is any external outcome unknown, is any known. It
  survives the release of every detailed entry and is removed only by the
  attempt's own terminal.
- **Per-call repair evidence.** The repair map holds the exact
  `ToolExecutionResult` (or the honest unknown) needed to rebuild a missing
  canonical `ToolResult`, and only while that repair is outstanding. A
  committed `ToolMessageCommitted` releases the entry **whatever the owning
  attempt's terminal state**; absence from the map means "this call needs no
  further canonical repair". An attempt terminal alone never destroys a
  still-needed entry: the terminal-before-repair shape (Class D) keeps its
  per-call evidence until the canonical result commits.

So the retention rule is: detailed per-tool recovery evidence exists only
while that tool call may still require canonical repair; durable historical
external-start knowledge needed for attempt classification is represented
independently in bounded attempt-level state. A long attempt with 10,000
previously settled/canonicalized foreground tools retains zero detailed tool
results while its one bounded summary keeps classifying honestly.

### 7.11 Recovery-prefix invariant

> Every successfully committed prefix of recovery reconciliation is itself a
> valid, truth-preserving input to a subsequent recovery.

Reconciliation commits tool-turn repair, the attempt recovery terminal, and
background terminal publication as **separate** atomic transitions on
purpose; each is a useful semantic commit point. A crash between any two of
them must leave a durable state that the next startup classifies exactly as
truthfully as the first did. In particular, a `ToolMessageCommitted`
committed by a repair — with the attempt terminal still absent — keeps the
attempt's external-start evidence intact, so the next recovery still sees an
indeterminate (or known-outcome) attempt and never reclassifies it as
Class B.

### 7.12 Replay policy

`ToolReplayPolicy::Idempotent` remains metadata. M9a implements no replay
engine, no retry framework, no configurable recovery strategy, and no
user-selectable replay mode. The rule is unconditional in this slice: an
ambiguous tool/process side effect is never automatically replayed. The safe
default is to commit an `OutcomeUnknown` tool result and let the model
decide what to do next.

## 8. Compatibility policy

Before 1.0, rustX intentionally does not preserve compatibility with previous runtimes or flawed abstractions. Breaking changes are preferred when they materially improve correctness, separation of concerns, or long-term maintainability.
## WF-03 native candidate ownership

The shared native owner is `runtime::workspace`, not a Subagent-specific Git manager. A Workflow holds a logical `CandidateScope`; its native state owns exactly one retained `WorkspaceLease`. Exclusive `WorkspaceAccess` transfers to the ordinary Agent process driver or Tool invocation until physical descendants settle. Child exit returns access rather than disposing the run lease. All candidate consumers serialize before child capacity/Tool scheduling. Matching frozen isolated profile policy is required; unsupported bindings fail before Git acquisition. See [the complete candidate contract](workflow-programs.md#run-scoped-candidate-workspace-wf-03) for content identity, mutation detection, cancellation and exact commit points.

SQLite development schema 28 adds a distinct recovery guard to the Workflow
resource settlement facts introduced in schema 27. Child IPC 18 adds
borrowed-run association; Runtime Client/TUI 20 mirrors it. Superseded schemas
are rejected without migration. Resource persistence is not Workflow
continuation. Providers have no workspace orchestration responsibilities.

The interpreter's internal CommittedValue pairs JSON with one optional exact
CandidateReference. Tool applicability returns directly from the native
settlement path. Candidate Agent applicability originates at
`WorkspaceAccess::finish(false)` after child and nested physical settlement,
flows through `WorkspaceUseSettlement.candidate` and
`PhysicalSettlement.candidate`, then the registry stores it alongside JSON in
process-local `WorkflowAgentOutput` at the unique terminal outcome. Workflow
Agent settlement commits both into `CommittedValue` only after successful
terminal publication. It never reads a later `CandidateScope::current` to infer
the reference: a writer's output binds its post-write B, and a machine-review
output for A becomes stale after another writer. Inspection/containment failure
cannot publish a successful candidate-bound local value. Value
constructions/references/Parallel merge it and reject incompatible references.
Consumption checks live CandidateScope state, with expected-reference checks
inside queued Tool/Agent admission. Journal correlation remains historical
evidence. No user JSON field, provenance graph or second physical owner is
introduced.

Mutation admission traverses existing directories without following symlinks,
including empty directories, under entry/depth/watch bounds. Linux directory
creation and macOS directory-entry notifications invalidate incomplete coverage.
Typed PhysicalSettlement preserves the resource but retires ended active
ownership. Without a trusted durable terminal HEAD, Workflow disposal re-proof
requires checkout and branch HEAD to remain at acquisition base, plus exact
source equality with its durable last-proven recovery guard. Advanced-HEAD
unresolved candidates need explicit user/manual recovery; readable Git facts
cannot supply missing terminal authority. NestedContainment preserves the
stricter process authority. The native disposal owner checks candidate content
only when the exact worktree remains present. Committed exact intent permits
continuation after worktree removal and failed durable append, including
branch-only completion or AlreadyDisposed; absence without intent fails closed.
`WorkflowWorkspaceSettled.candidate` remains the exact proven terminal
candidate. A separate `recovery_guard: Option<CandidateRecoveryGuard>` stores
the last proven `reference` copied from native `CandidateScope.state.current`
for PhysicalSettlement. It does not certify final state, supply Workflow
applicability, or enter authored JSON. A failed writer inspection cannot advance
the guard; successful node proof of B followed by failed final inspection guards
B. Native disposal rehashes via `inspect_source` and compares the guard before
removal; changed bytes or a missing guard fail closed. No second disposal state
machine or Workflow Git owner exists. See the candidate contract for exact
limits and crash windows. SQLite schema 28 rejects older stores without
migration; IPC and client mirrors need no new fields.


## WF-04 human interaction ownership

See [Workflow human review and its exact frontiers](workflow-programs.md#human-questions-and-business-review-wf-04).
Workflow owns Review progression and local values; the conversation's existing
InteractionCoordinator owns requests, waiters, validation and durable settlement.
CandidateFreeze belongs to the native workspace plane and retains a real
WorkspaceAccess borrow. It is not another interaction or approval manager.
Candidate-dependent downstream admission transfers that same borrow into native
Tool/Agent work. Native Tool(ask_user) uses the existing requester, now bound to
the invocation driver's cancellation scope. Approval remains permission for a
prepared invocation; FullAccess changes only that permission gate.

Runtime Client 21 and child IPC 19 carry the coherent human-step vocabulary.
SQLite development schema 30 retains its schema-29 audit payloads and adds Loop
facts. Event envelope framing remains version 1. No pending recovery or
compatibility protocol is introduced.

WF-04 Review decisions/feedback are immutable business data, separate from
run-local accepted-candidate authority. Reject A remains readable after writer B;
explicit A-derived inputs remain stale. Plan and context facts preserve candidate
applicability in the Review specification/digest/audit, and all dependencies share
the native freeze. Native ask_user is workspace-independent under WorkspaceUse:
it never holds a CandidateScope borrow while waiting.
## CFG3 external source demand and credential authority

Definitions and discovery are inert. Both canonical `.agents` roots contribute
whole resources, with Workspace shadowing before validation. Selection establishes
finite demand; admission resolves winning credentials and enters the existing
source lifecycle owner. Unused failures warn, selected invalid sources fail.
Provider replacement never inherits a lower credential. See [configuration](configuration.md).


### Crash-safe Session deletion control

Protocol 28 routes finite deletion preview, revision-bound execution and explicit
recovery through LocalSessionAttachment. Catalog schema 7 owns both live membership
and pending frozen deletion records in one generation-checked atomic publication.
Completed cleanup records are durably removed; native high-water marks prevent
identity reuse independently. Runtime Client owns bounded deletion DTOs mapped
explicitly by the supervisor. Confirmed parent-directory
durability precedes blocking cleanup outside the catalog mutex; startup reconciles
pending work before composing any live Conversation. ConversationAccess consults
this same authority, so residual private files cannot revive deleted identities.
See [Session deletion lifecycle](session-deletion-lifecycle.md) for the state machine,
visibility/durability distinction and deterministic crash/concurrency evidence.

### Agent capability ownership and frozen demand

Root and named Agents independently own complete capability profiles. Root's
explicit named-Agent allowlist controls delegation, with no generic child
Tool/Plugin ceiling. The invoking Attempt's frozen effective model is inherited
when the named profile omits model selection. Child admission uses captured
resources and finite demand; no current-file reread occurs.

Agent Tool selection and global invocation policy remain different domains.
Skills are prompt visibility. Closed Plugins default off. See the exact typed
units and admission contracts in [configuration](configuration.md).


### Read-only Trace presentation

[`runtime_client::trace::TraceProjection`](trace.md) joins bounded native facts for
client presentation. It owns no durable data and is never read by execution,
settlement, cancellation or recovery. Its dedicated Trace cursor and newest
snapshot window remain separate from canonical messages, transcript paging and
live observation cursors.

See [Session-owned workspace uploads](session-uploads.md) for receipt admission, model paths, fork copies and durable cleanup.

### Pending inbound mutation and committed claim receipts

The exact pending controls described in [App Server protocol v12](app-server-protocol.md#exact-pending-inbound-controls-web-06)
remain native `ConversationStore` transitions. Sequence + MessageId identify one
occurrence, and a monotonic pending revision prevents lost updates. The durable
mutation transaction and canonical adoption transaction are the only ownership
frontier: neither App Server request order nor a browser state machine decides
whether an item remains mutable.

The selected batch supplies an upper sequence bound and prevalidation identities.
It supplies no authoritative post-commit payload. Adoption constructs its answer
obligation from the exact rows it reads and returns those rows as a committed
receipt. Both runtime admission paths install and observe that receipt. A removed
selection may yield an empty receipt, without admitting an empty turn or losing a
recovered continuation. The mailbox orders post-commit publication but owns no
accepted queue. Removing pending work does not cancel execution or delete
canonical history.

### CFG3 source authoring

Rust owns bounded semantic mutations, validation, canonical serialization,
revisions, CAS, redaction and publication. Clients own drafts and presentation.
A source Save commits bytes independently of Reload. Stale writes fail without
overwriting or dropping drafts; uncertain outcomes require authoritative reread.
See [Web Settings](web-settings.md) and [configuration](configuration.md).

## Canonical Tool ownership and lineage (#349)

`ToolCallId` is an opaque, provider-issued correlation string, not a rustX global
identity. It remains unchanged on OpenAI Chat tool results, Responses
`function_call_output.call_id`, and Anthropic `tool_result.tool_use_id`.
`ToolCallOccurrenceRef { assistant_message_id: MessageId, block_index: ContentBlockIndex }`
is the canonical rustX identity. Every `ToolMessageBlock` requires `occurrence`
alongside its own `id`, provider `tool_call_id`, native `tool_id`, and `result`.
`ToolExecutionId` is a separate runtime-owned UUID for detached execution; it is
neither provider correlation nor canonical occurrence identity.

The Agent supplies occurrence ownership from its committed Assistant blocks before
the result commit. Recovery retains AttemptId + ToolCallId execution evidence and
resolves missing results against exact canonical Assistant blocks; synthesized
results carry that occurrence before commit. Canonical history alone therefore
contains every call/result relationship, without source execution events.

SQLite schema **42** retains `canonical_tool_calls` only as a derived index. Its
primary key is `(assistant_message_id, block_index)`; Assistant/call and result
MessageId uniqueness constraints prevent duplicate provider IDs within one
Assistant and duplicate settlement. Tool commits validate the exact indexed
occurrence, call ID and Tool ID, then insert the canonical result and link its
MessageId atomically. The index is reproducible from canonical messages. No active
Surface search discovers result ownership. Schemas 38/39 are refused without
migration, dual decoding, backfill or fallback JSON scanning.

Clone, fork and tree copies remap canonical MessageIds and each result's
`occurrence.assistant_message_id`, preserving block positions and provider IDs.
These IDs remain historical provider correlation values, not new invocations;
rewriting them has no native ownership purpose. Reuse across Assistant messages is
valid, including within the retained Surface. Fork cuts and compaction boundaries
validate exact occurrence relationships and cannot retain only one side of a pair.


### Native Subagent transcript inspection

The parent's addressed runtime resolves an exact SubagentId through its native
registry to the owned child Conversation. Existing allocation access and an
identity-validated read-only store feed the shared Runtime Client durable
transcript projector and completed-response decorator. App Server
`subagent/transcript` translates this bounded read only; it never reconstructs
messages, creates child Sessions or grants a child controller. The child Ledger,
Surface and Journal remain the sole canonical authorities. Terminal lifecycle
and retained workspace facts do not manufacture transcript completion facts.

The TUI has one disposable child page, fenced by parent attachment epoch and
child selection/read generation. Reconnect reconstructs from current authority;
Esc closes presentation without runtime mutation. Child HITL remains routed to
the existing root interaction owner. See [the protocol](app-server-protocol.md#read-only-native-subagent-conversations-v12).

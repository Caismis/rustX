# Jobs and continuable Agents

Background is an execution mode, not a universal identity domain. A finite
background Tool operation and a durable child Agent have separate identities,
owners, read models and controls. They share cancellation, process supervision,
physical settlement, bounded output, watches and Event Journal primitives.

| Domain | Identity | Owner | Lifecycle | Model tools |
| --- | --- | --- | --- | --- |
| Job | `ToolExecutionId`, exposed as `job_id` | Conversation's `ConversationBackgroundRegistry` | Active → terminal, permanently | `job_list`, `job_status`, `job_wait`, `job_cancel` |
| Agent | `AgentId`, exposed as `agent_id` | Parent Conversation's `SubagentRegistry` | Inactive → Admitting → Active → Stopping → Inactive; unresolved authority → Unavailable | `subagent`, `list_agents`, `send_message`, `wait_agent`, `interrupt_agent` |
| Activation | Internal `SubagentId`, exposed as `activation_id` | One Agent and the shared finite child supervisor | Admission → running → stopping/settlement → terminal | Controlled through its owning Agent |

An ordinary foreground Tool invocation is not a Job. One Job ID identifies one
admitted detached invocation and is never reused. An Agent owns a stable child
ConversationId, parent lineage, immutable resolved profile/resources and its
workspace authority. Each activation has a fresh identity and process incarnation.
The same Agent can complete, be interrupted and resume without replacing its
conversation, history, selection identity or admitted authority.

## Job boundaries

`commit_dispatch` transfers a prepared detached runner into the conversation
registry under its admission gate. The Job owner decides cancellation versus
terminal commit under the existing registry synchronization boundary. Cancellation
intent is not physical settlement: an executor that proves success despite a
racing request retains that truthful success. Terminal publication uses the
canonical durable inbound path exactly once before observable terminal state.

`job_status` takes an immediate authoritative snapshot. `job_wait` captures the
exact immutable Job ID and uses the registry watch to await terminal physical
settlement, without polling; an unknown ID fails immediately. `job_cancel`
requests cancellation and waits through the same physical settlement contract.
`job_list` is bounded to 64, ordered deterministically by admission order (newest
first), and reports omission. No call observes or controls another conversation.
Completion is proactive; waiting never consumes or suppresses its notification.

## Agent boundaries

The registry mutex owns current-activation selection, message admission, closing
admission, cancellation intent and activation transitions. There is zero or one
active activation per Agent; no tool, transport or client makes that decision.

`subagent` commits the durable Agent identity and first activation with frozen
resolved authority. Creation starts a fresh child conversation; its optional
context is explicit supplied input, not an implicit copy of parent history.
Creation returns admission immediately; `wait_agent` supplies blocking behavior.
There is no foreground/background Agent lifecycle switch or separate fork tool. Later `send_message(agent_id, message)` performs one owner
operation under that mutex:

- **Active:** admit the message to this activation's ordered guidance lane. The
  child commits it through its own canonical durable inbound authority and makes
  it available at the next legal Agent Loop boundary.
- **Admitting or Stopping:** reject transiently. No message is secretly retained to restart
  the Agent after settlement.
- **Unavailable:** reject with typed `Settlement` / `agent_settlement`. Failed admission
  reservations, abandoned publication, unproven terminal containment and poisoned
  workspace authority require explicit reconciliation or repair; retrying input
  cannot settle them.
- **Inactive:** reserve exactly one next activation and its input, retaining the
  same AgentId and child ConversationId. Preparation is explicitly Admitting;
  another caller cannot reserve a competing activation. Failed preparation
  releases the reservation only after proven physical rollback and its durable
  commit. Unproven rollback retains that target as Unavailable without destroying
  the Agent. The reservation carries
  its exact activation generation, so cleanup from an earlier activation cannot
  clear a later reservation. Preparation retains a counted runtime admission
  lease and a shutdown-connected cancellation signal through commit or rollback;
  shutdown cannot finish while that detached preparation still owns resources.

Message delivery and cancellation share an ordered unbounded in-process command
lane. Individual messages remain bounded; an Active admission cannot fail merely
because sixteen earlier commands have not yet been consumed. Owner mutex order
establishes FIFO order, and cancellation enqueue is distinct from physical
settlement.

The child must request `SealRequested` before closing its guidance inbox. Under
the same registry lock, the parent changes Active to Stopping. The process driver
routes all previously admitted FIFO messages before `SealGranted`. Only then may
the child seal its durable inbox. Only a normally Completed attempt may reopen
this activation to process already accepted guidance. Failed (including timeout
and turn-limit failure), Cancelled, orphaned and Workflow-owned attempts are final.
Their accepted pending guidance remains in the canonical child inbox for a later
explicit activation; it is not erased or used to overwrite the first failure.
For normal completion, if accepted work remains, the child sends `SealOpen`;
the parent restores Active under the owner mutex and acknowledges
`AdmissionReopened`. The child coordinator holds the next turn until that
acknowledgement. If cancellation won, no reopening is granted. The child carries cancellation as an
absorbing fact from terminal observation through close, seal, reopen and publication.
A Completed observation committed before Cancel cannot authorize another turn;
accepted pending guidance remains canonical input for a later explicit activation. Thus an externally Active Agent cannot already have
closed message admission. A message that loses this boundary sees Stopping; a
message that wins stays with that exact activation. Interruption or physical loss
can still prevent later model observation, and cannot erase durable acceptance.

An ordinary result retains the concluding `AttemptCompleted.attempt_id` through
terminal handling and guidance draining. The last `AssistantMessageCommitted`
Journal fact scoped to that exact attempt selects one canonical MessageId; only
that committed Assistant message supplies ordinary Text. Refusal-only or otherwise
unsupported terminal content reports the explicit no-final-answer failure.
Earlier activation answers, earlier guidance attempts, tool narration and provisional
stream text cannot substitute. Reads use durable storage even before coordinator
state restoration. Workflow children retain their committed output-latch contract.

`interrupt_agent` captures the reserved or current activation, requests cancellation
through the admission signal or shared supervisor and waits for physical settlement.
The same mutex orders admission commit versus interruption; no control gap exists. The Agent then
becomes Inactive and can resume. It never deletes the durable Agent.

`wait_agent` captures the current activation under the registry lock. It waits
for that activation to settle and release ownership; a later activation cannot
retarget or prolong the wait. Inactive returns immediately with no activation
target. During Admitting it captures the reserved generation and waits through
commit or physical rollback. A rolled-back reservation returns that activation ID
with no execution outcome; failed rollback returns a settlement error. A lost
client wait response must not be automatically retried, since a fresh operation
could capture a different activation. Physical settlement or canonical-publication
abandonment returns an explicit settlement error, never successful wait/interrupt
completion while the Agent remains Unavailable.

`send_message` succeeds only after the child accepts input through canonical
inbound. Active Guidance uses `GuidanceResult::Accepted`; resumed input uses
`DelegateAccepted` on the activation's uniquely owned control channel. Neither
ownership commit nor writing either input frame proves acceptance. A positive pre-write
loss is not-delivered; a child `Refused` response is explicit refusal. A lost acknowledgement
returns an explicit unknown-delivery error after settlement, never accepted;
callers must not blindly retry an ambiguous effect. Restart reconciles the old
activation without replaying Delegate, while the child's canonical inbox remains
authoritative. Parent Tool cancellation can stop preparation before commit; once
activation ownership commits the Agent owns settlement. Cancelling the waiting
Tool releases its waiter, not that owned activation or already accepted input.

`list_agents` orders durable identities by their first ownership admission
sequence, newest first, bounds output at the shared `MAX_AGENT_LIST_LIMIT` (64),
and reports returned/matched/limit/truncated. It captures every returned
`(AgentSnapshot, latest SubagentSnapshot)` pair under one registry lock; clients
never reacquire status separately for each row. Repeated activation does not change an Agent's creation order.

## Authority, history and replay

The resolved model, Tools, Skills, Plugins, resources, policies and workspace
are frozen for the durable Agent at creation and reused by later activations.
Configuration reload or resource reconciliation and named-profile edits do not silently change it.
`DurableAgentAuthority` contains resolved authority and inherited execution/
approval policy only. `ActivationAdmission` separately owns task/context, terminal
contract and typed provenance. `CreationTool` and `MessageTool` identify their
actual ToolCalls; `ClientControl` has no fabricated ToolCall. Finite Workflow
children carry their real Workflow node origin. Activation admission supplies
new input and execution identity, not a new profile lookup. Workflow-owned finite AgentRuns remain governed by WorkflowRuntime and
are not exposed as resumable native Agents.

Canonical child conversation history is authoritative after commit. Each
activation's final report reaches the parent exactly once through canonical
inbound/history, correlated by stable Agent identity and activation identity.
Activity snapshots, control acknowledgements and wait results are not parallel
report/history channels. The Event Journal records ownership and execution facts;
it does not replace canonical child content.

A resumed activation first reserves an exact generation under the registry mutex.
The detached admission owner installs and fsyncs a recoverable physical authority
before publishing that activation ID or appending `AgentActivationAdmission::Reserved`.
The complete recovered workspace verification runs supervised Git under that physical
owner before Reserved. Startup discovers every allocation in the durable Agent's
physical namespace, including allocations without an admission event, and prevents
workspace reuse until all their helpers have positive settlement proof.
Live failure before Reserved transfers the exact consumed allocation into
`recovery_unreserved` / `recovery_pending` before clearing the admission reservation.
The workspace recovery fence then keeps the Agent Unavailable; pending recovery
blocks Goal idle and runtime drain. Dropping the parent handle is not proof.
Reconciliation must acquire and retain exact authority/continuation proof through
the in-memory release. No Reserved, RolledBack, or logical activation event is
invented for that unadmitted allocation. Session destructive exclusion also retains
native proof for every consumed allocation, including those absent from the journal.
The consumed ordinal remains unavailable for reuse after success or restart.
Recovery execution is shared, but admission completion observes only its exact
activation. Settling one allocation does not await another Agent's pending proof;
Goal idle and whole-runtime drain still account for all remaining obligations.

The append runs outside the mutex; interruption still captures the same reservation.
Ownership must consume that same Agent, activation and origin. After Reserved has committed, conclusive rollback records `RolledBack` with an explicit
physical-settlement proof. Recovery never treats the prior activation's terminal
fact as proof about a later reserved generation: an unresolved reservation or
unproven rollback keeps the Agent Unavailable with the exact reserved target and its
workspace unavailable. Wait reports a settlement error, and Session deletion
cannot remove that Agent's resources. The sequence watermark includes both committed reservations and positive physical-authority
allocations across the Session, including staging that preceded ownership, so a later
process never reuses a consumed ID. These facts contain execution correlation, never message bodies or private authority. The
owner publishes each receipt as one combined Agent-and-activation observation
after installing that whole transition under its mutex. One journal receipt
releases exactly that complete projection cut; two partial observations must not
share and prematurely release the same receipt. This releases the observation
journal frontier; later canonical reports cannot be stranded behind an unpublished
admission fact.

App Server v30 exposes separate `jobs` and `agents` snapshots and `job_updated`
and `agent_updated` events. Agent rows carry `agent_id`, `parent_agent_id`, child
ConversationId, `current_activation`, latest `activation_id`, `activation_state`
and explicit Admitting/Active/Stopping/Inactive/Unavailable state. Replay folds activations into the
same durable Agent identity; reconnect uses the same native projection.

TUI and WebUI key Agent rows and child inspection by AgentId, not activation ID.
Active → Inactive → resumed updates one row and preserves selection. Job cards
retain terminal state and output. Both clients route controls to the owner and
render canonical final reports without inventing lifecycle decisions. Runtime
Client bootstrap receives current Agent snapshots directly from the registry's
atomic owner cut, never by interpreting activation-history iteration order.
Every live lifecycle transition uses that same combined cut. Clients never infer
AgentState from Starting, terminal naming, or an activation cleanup flag;
Starting projects the owner's Admitting and terminal-unproven projects Unavailable.
Wait and interrupt both return `agent_wait`: captured activation/outcome plus the
latest Agent snapshot.

The TUI command dispatcher owns one pending Agent message submission per
attachment. Repeated Enter while its preserved draft awaits an outcome sends
nothing. Classified failure retains the draft without replay; success clears only
the unchanged submitted draft. Other controls remain usable.

Both typed clients bound outstanding observations separately from control traffic.
The TUI admits at most four waits, two Agent message admissions, two controls, and
eight ordinary RPCs per connection (the server's sixteen-request budget). Excess
requests fail locally before transmission; no queue or timeout recaptures a later
activation. Closing a waiter never implicitly cancels domain work. Real connection
loss classifies every transmitted request once, without replay.

Recovery uses durable `physical_settlement_proven`, not terminal naming. A live
Interrupted outcome with proven containment remains resumable after reopen.
Crash reconciliation initially records unproven settlement and fails closed;
clean Git state does not supply missing physical proof. Logical terminal outcome,
terminal publication and physical containment are independent dimensions. Later
physical proof does not rewrite Interrupted into success or replay the old input.

## Release gate audit

[Issue #15](https://github.com/Caismis/rustX/issues/15) was inspected during #411.
Its `#60 native async one-shot subagents`, `SubagentRuntime = async one-shot child
runtimes`, and `continuable/resumable subagents` exclusion describe the superseded
release contract. Issue #411 replaces those requirements with this document's
finite Job and durable Agent domains, distinct activation identity, deterministic
message/seal arbitration, frozen authority, replay and both client projections.
The release gate still requires canonical inbound durability, terminal uniqueness,
physical quiescence, Workflow ownership and complete CI. The historical issue text
was not edited; release tracking should link #411 and this current contract.

### Private admitted authority

SQLite schema 49 stores the whole executable `DurableAgentAuthority` and its
credential capture in the parent Conversation's private `agent_authorities`
table. Admission atomically commits that private row and a public AgentId
reference. Event Journal ownership facts cannot serialize profiles, Tool
environments, provider configuration, MCP arguments, headers or endpoints:
the event type carries only the opaque reference and public execution facts.
Recovery loads the private authority and captured credentials without reading
current configuration or environment. Missing private state fails closed.
Repeated activations reuse the same record; lineage copies omit it, archive
exports do not read it, and deleting the owning Session removes its database.
Existing local-store filesystem permissions protect these values.

## Agent workspace lifetime

`AgentRetained` retains the workspace for the durable Agent lifetime across
finite activations. It is not the finite Workflow `Retained` disposal handoff.
`subagent/disposeWorkspace` cannot delete a still-existing Agent's workspace.
An independent Agent-deletion lifecycle is a future owner, outside this repair;
Session deletion retains its existing full ownership and containment checks.

Workflow checks logical terminal, committed publication/value, and physical proof
separately. A valid value with unproven containment fails as physical settlement
uncertainty; committed publication is never described as an unpublished terminal.

## Recovery physical settlement owner

The activation authority lives at
`conversations/<conversation>/physical-settlement/<activation>/`, independently of
the disposable runtime incarnation. The storage owner locks `.allocation-owner`
in that conversation's physical-settlement namespace before creating
`.pending-<activation>`. Fsync of that directory and its linking ancestors durably
consumes the activation ordinal, even if no lease or receipt can be initialized.
Both private and published names reseed the allocator.

Before durable Reserved, the parent initializes and fsyncs the exclusive
`physical-owner` lease and exact `Unstarted` receipt inside the private directory.
Atomic rename to `<activation>` publishes complete authority; fsync of the parent
precedes returning any authority handle. An error after rename still leaves a
published obligation. Recovery takes the same namespace lock before classifying
private allocations, so no surviving initializer can publish after recovery.
It renames an abandoned private allocation to `.abandoned-<activation>` and fsyncs
the parent; that positive consumed name never authorizes another allocation.
No logical Agent, Reserved, rollback, or terminal event is invented for it.
Published allocations always require exact physical proof, even if initialization
returned an error. The typed allocation error carries the positive consumption
fact into the live registry's existing pending reconciliation set immediately. Missing/corrupt published evidence never means unpublished.
The initial published receipt is `Unstarted`. The parent passes the same open-file description
on fd 2 into the child; closing the parent's descriptor cannot release a live child's
copy. Before composition the child duplicates it CLOEXEC, restores diagnostics on
stderr, and durably changes the receipt to `Running`.

One child lifetime epilogue joins composition rollback or runtime native drain for
all normal exits. Only positive physical containment permits `Quiescent`; logical
publication failure cannot erase containment already proved. The parent may also
publish this proof after explicit direct and retained-native containment. Runtime
cleanup never removes the authority or receipt. Evidence remains until Session
deletion, hence survives terminal/rollback publication failure or parent death.

All asynchronous workspace Git commands use the existing native command supervisor,
so child-side preparation registers a retained process anchor before START.
Fresh activation workspace acquisition, recovered Agent verification before Reserved,
and parent-side workspace cleanup all carry activation physical authority.
Cleanup can launch Git after child settlement.
Each such command reserves its own continuation authority under the activation,
before spawning the existing trusted command supervisor. The supervisor owns the
inherited lease and publishes proof only after its complete containment gate,
including parent-control EOF. Commands cannot inherit or reuse the private authority.
A helper's `.pending-` directory is private under the already-published parent
lease: no helper spawn permit has escaped. Only atomically published helper
directories can be executable authorities. Helper UUIDs do not allocate activation
ordinals; activation-private directories use the separate consumption protocol above.

Recovery takes the parent authority lock and every published continuation lock.
It accepts exact `Quiescent` receipts. `Unstarted` plus the released inherited lock
is also positive proof: no reserving parent or executable prelude can still cross
that authority's resource boundary. `Running`, missing or invalid evidence remains
unproven. No process disappearance, free lock by itself, clean Git tree, or timeout
supplies containment proof.

The registry captures immutable obligations and claims them under its mutex, probes
files and locks and commits SQLite outside it, then revalidates the exact generation
and publishes one complete owner snapshot. A concurrent pass skips claimed work.
Startup performs a bounded pass and one watch-backed owner probes for up to 15
seconds; Goal idle performs a fresh pass, and shutdown joins that owner before
classifying unresolved resources. The bound limits work and never proves settlement.
Later reopen retries outstanding evidence without replaying input.

Native proof commits `SubagentPhysicalSettlementProven` for a terminal activation.
For a reserved generation it commits the original provenance with
`RolledBack { physical_settlement_proven: true }`. Proof locks remain held through
that durable append and its snapshot cut. A physical allocation without Reserved has
no semantic admission to roll back: recovery seals its private allocation or
validates its published Unstarted/Quiescent receipts and retains proof locks
through workspace release. The consumed ID
and inert physical evidence remain durable. An earlier unproven rollback cannot close
that obligation. Independent workspace poison is not cleared by physical proof.
Logical Interrupted and its canonical parent notice remain unchanged; the old
activation is never reattached. Session deletion folds subsequent physical facts
rather than treating a historical false flag as permanent.

Goal idle uses short claims in both execution registries: durable callbacks run
outside the mutex while claims exclude new ownership. Private Job preparations
also exclude idle; their rollback wakes the existing Goal coordinator.

Finite Workflow recovery preserves durable ownership and physical proof for shared
as well as retained workspaces. Restored execution history grants no executable
terminal protocol or resumed Workflow authority; physical reconciliation never
replays a Workflow node.

Job observation captures one immutable Job ID. Wait and cancel finish only with
its durable terminal snapshot or the typed `job_publication_abandoned` failure
once that Job's owner exhausts terminal publication and finishes its final
failure callback. Publication failure leaves the logical lifecycle at
`publishing_terminal`; clients display the failure without inventing terminal
success. Cancelling the observing Tool alone stops observation.

Job discovery uses the Job domain's `MAX_JOB_LIST_LIMIT` (64). Model and client
lists report `jobs`, `returned`, `matched`, `limit`, and `truncated` from one
registry cut, newest first. The TUI `/jobs` command preserves those counts.

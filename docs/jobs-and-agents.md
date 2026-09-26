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
acknowledgement. If cancellation won, no reopening is granted. Thus an externally Active Agent cannot already have
closed message admission. A message that loses this boundary sees Stopping; a
message that wins stays with that exact activation. Interruption or physical loss
can still prevent later model observation, and cannot erase durable acceptance.

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
inbound. Active guidance uses its existing acknowledgement; resumed input uses
`DelegateAccepted` on the activation's uniquely owned control channel. Neither
ownership commit nor writing Delegate proves acceptance. A lost acknowledgement
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

A resumed activation first commits an `AgentActivationAdmission::Reserved` fact
before staging any physical child. Ownership must consume that same Agent,
activation and origin. Conclusive rollback records `RolledBack` with an explicit
physical-settlement proof. Recovery never treats the prior activation's terminal
fact as proof about a later reserved generation: an unresolved reservation or
unproven rollback keeps the Agent Unavailable with the exact reserved target and its
workspace unavailable. Wait reports a settlement error, and Session deletion
cannot remove that Agent's resources. The sequence watermark includes reservations
so a later process never reuses their IDs. These facts contain execution correlation, never message bodies or private authority. The
owner publishes each receipt as one combined Agent-and-activation observation
after installing that whole transition under its mutex. One journal receipt
releases exactly that complete projection cut; two partial observations must not
share and prematurely release the same receipt. This releases the observation
journal frontier; later canonical reports cannot be stranded behind an unpublished
admission fact.

App Server v25 exposes separate `jobs` and `agents` snapshots and `job_updated`
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

SQLite schema 47 stores the whole executable `DurableAgentAuthority` and its
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

Before composing capabilities or accepting Delegate, each child holds a mandatory
exclusive lease in its unique incarnation directory. Parent-control EOF first
releases pending child interactions as ControlLost, then uses the ordinary native
runtime shutdown to contain nested processes and all other owned execution. Only
Quiescent permits an exact activation/conversation receipt. The recovering
registry requires both that receipt and acquisition of the released exclusive
lease. A free lease alone, PID absence, elapsed time or clean workspace is no proof.

The registry retains unresolved activation and reservation IDs and their child namespaces as
concrete reconciliation obligations. Startup performs a bounded pass and one
watch-backed owner probes for up to 15 seconds; Goal idle performs a fresh pass,
and shutdown joins that owner before classifying unresolved resources. The bound
limits work, never establishes containment. Later reopen retries outstanding
proof. Missing or invalid evidence remains fail-closed and requires explicit
repair; repeating send_message is not a reconciliation protocol.

Native proof commits `SubagentPhysicalSettlementProven` for a terminal activation.
For a reserved generation it commits the exact original provenance with
`RolledBack { physical_settlement_proven: true }`. An earlier unproven rollback
is a separate resource fact and cannot close that obligation. The registry then
releases recovery exclusion and publishes the complete owner projection under
one mutex, waking the existing Goal idle coordinator without inventing input. Independent
workspace poison is never cleared by physical proof. Logical Interrupted and its
canonical parent notice remain unchanged; the dead activation is never reattached
or replayed. Inert incarnation evidence remains until Session deletion, which
folds the later proof instead of treating a historical false flag as permanent.

Finite Workflow recovery preserves durable ownership and physical proof for shared
as well as retained workspaces. Restored execution history grants no executable
terminal protocol or resumed Workflow authority; physical reconciliation never
replays a Workflow node.

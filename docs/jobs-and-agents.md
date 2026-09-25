# Jobs and continuable Agents

Background is an execution mode, not a universal identity domain. A finite
background Tool operation and a durable child Agent have separate identities,
owners, read models and controls. They share cancellation, process supervision,
physical settlement, bounded output, watches and Event Journal primitives.

| Domain | Identity | Owner | Lifecycle | Model tools |
| --- | --- | --- | --- | --- |
| Job | `ToolExecutionId`, exposed as `job_id` | Conversation's `ConversationBackgroundRegistry` | Active → terminal, permanently | `job_list`, `job_status`, `job_wait`, `job_cancel` |
| Agent | `AgentId`, exposed as `agent_id` | Parent Conversation's `SubagentRegistry` | Active → Stopping → Inactive → later Active | `subagent`, `list_agents`, `send_message`, `wait_agent`, `interrupt_agent` |
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
resolved authority. Later `send_message(agent_id, message)` performs one owner
operation under that mutex:

- **Active:** admit the message to this activation's ordered guidance lane. The
  child commits it through its own canonical durable inbound authority and makes
  it available at the next legal Agent Loop boundary.
- **Stopping:** reject transiently. No message is secretly retained to restart
  the Agent after settlement.
- **Inactive:** reserve exactly one next activation and its input, retaining the
  same AgentId and child ConversationId. Preparation is represented as Stopping;
  another caller cannot reserve a competing activation. Failed preparation
  releases the reservation without destroying the Agent. The reservation carries
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
the child seal its durable inbox, after draining any accepted guidance through
ordinary child processing. Thus an externally Active Agent cannot already have
closed message admission. A message that loses this boundary sees Stopping; a
message that wins stays with that exact activation. Interruption or physical loss
can still prevent later model observation, and cannot erase durable acceptance.

`interrupt_agent` captures only the current activation, requests cancellation
through the shared supervisor and waits for physical settlement. The Agent then
becomes Inactive and can resume. It never deletes the durable Agent.

`wait_agent` captures the current activation under the registry lock. It waits
for that activation to settle and release ownership; a later activation cannot
retarget or prolong the wait. Inactive returns immediately with no activation
target. A resume reservation is transiently rejected rather than guessed. A lost
client wait response must not be automatically retried, since a fresh operation
could capture a different activation. Physical settlement or canonical-publication
abandonment returns an explicit settlement error, never successful wait/interrupt
completion while the Agent remains Stopping.

## Authority, history and replay

The resolved model, Tools, Skills, Plugins, resources, policies and workspace
are frozen for the durable Agent at creation and reused by later activations.
Configuration reload or resource reconciliation and named-profile edits do not silently change it.
Activation admission supplies new input and execution identity, not a new profile
lookup. Workflow-owned finite AgentRuns remain governed by WorkflowRuntime and
are not exposed as resumable native Agents.

Canonical child conversation history is authoritative after commit. Each
activation's final report reaches the parent exactly once through canonical
inbound/history, correlated by stable Agent identity and activation identity.
Activity snapshots, control acknowledgements and wait results are not parallel
report/history channels. The Event Journal records ownership and execution facts;
it does not replace canonical child content.

App Server v22 exposes separate `jobs` and `agents` snapshots and `job_updated`
and `agent_updated` events. Agent rows carry `agent_id`, `parent_agent_id`, child
ConversationId, `current_activation`, latest `activation_id`, `activation_state`
and explicit Active/Stopping/Inactive state. Replay folds activations into the
same durable Agent identity; reconnect uses the same native projection.

TUI and WebUI key Agent rows and child inspection by AgentId, not activation ID.
Active → Inactive → resumed updates one row and preserves selection. Job cards
retain terminal state and output. Both clients route controls to the owner and
render canonical final reports without inventing lifecycle decisions.

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

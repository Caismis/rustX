## Named Agent admission

See [Agent profiles](agent-profiles.md) and [CFG3 configuration](configuration.md).
User and Workspace definitions are complete named profiles. Resource identity
exists before selection; a higher malformed duplicate shadows the lower source.
Unused invalid definitions produce bounded ordered diagnostics. Admission of a
selected invalid profile fails with its typed diagnostic.

Root controls delegation through `agent.agents`. The child profile independently
selects Native Tools, external sources, Skills and closed Plugins. Missing child
model selection inherits the invoking Attempt's frozen effective model. Child
resolution never rereads current files, and preparation materializes only finite
admitted demand. Exact model/Tool/source/Skill bindings freeze before child
ownership commits. Global invocation policies remain global, not profile prose.

## Invocation overrides and frozen child contracts

`ResolvedSubagentSpec` is the complete immutable child contract. An invocation
may replace the supported Tool, Skill or Plugin dimensions as a whole, including
with an explicit empty selection. Missing dimensions use the named profile.
There is no generic Root Tool or Plugin ceiling and no recursive object merge.
Unknown references, unsupported child scope, invalid selections and impossible
materialization fail before ownership commits. Overrides cannot change global
execution/approval policies or create a persistent configuration layer.

The profile digest covers effective execution semantics. Canonical Tool identity,
model binding, Skill version and closed Plugin behavior are frozen before process
staging. Equivalent resolved profiles have the same digest; routing prose and
unselected definitions do not create execution authority. A `send_message` input is
ordinary inbound task content, never a way to change that admitted profile.

## Durable ownership

Each child owns a typed UUIDv7 Conversation identity and a Conversation-local
SQLite store under its owning Session. `AgentId` is the durable child identity. Internal `SubagentId` names one finite
activation and retains parent-scoped ordinal ordering; it is not the Conversation
locator and is never reused for a later activation.
Tool outputs belong to that Conversation allocation. Parent/child ownership
commits, execution settlement, process supervision and retained-worktree disposal
remain native runtime responsibilities. See [Session deletion ownership](session-deletion-ownership.md).

## Durable Agent authority across activations

The resolved contract freezes at Agent creation, not each resume. The durable
Agent owns the same child ConversationId and workspace authority across multiple
finite activation IDs. `send_message` to an inactive Agent reuses that admitted
profile, model, Tools, Skills, resources and policies; it does not consult current
settings or reread a named definition. Parent resource/configuration reload cannot
mutate existing Agent authority. See [Jobs and continuable Agents](jobs-and-agents.md).

Resume also requires proven physical containment of the prior activation. An
explicit `interrupt_agent` settles as Cancelled after native physical settlement
and leaves the Agent resumable. Crash reconciliation records Interrupted without
proof that the old direct child and nested processes settled. It preserves the
same Agent identity and history, projects Unavailable, and refuses another physical activation;
clean Git inspection or a shared workspace does not supply that missing proof.
Disposed or unresolved workspace authority likewise cannot be reacquired by resume.

Resume preparation has its own durable `AgentActivationAdmission` obligation.
Reserved commits before staging, under the same product ownership fence used by
Session management. Committed activation ownership consumes that reservation;
rollback closes it only with an explicit containment result. An unresolved or
unproven rollback reconstructs Unavailable with the exact reserved activation ID,
blocks further admission, and prevents Session deletion from removing resources.
A prior successful activation cannot settle this later physical obligation.

`AgentRetained` is Agent-lifetime ownership, not an activation-level `Retained`
handoff. Normal completion neither deletes nor disposes this workspace. Finite
`subagent/disposeWorkspace` cannot delete it; a future Agent-deletion lifecycle
would need its own explicit owner and is not introduced here.

Publication and containment remain independent: a committed Interrupted terminal
may later acquire durable physical proof without changing its unknown logical
outcome. Only native process-incarnation evidence can discharge that obligation;
workspace inspection alone cannot grant resume authority.

Recovery retains exact incarnation lease/receipt evidence until Session deletion.
The registry's bounded reconciliation owner commits
`SubagentPhysicalSettlementProven` only after native Quiescent evidence and lease
release exclude the old writer. This releases only the recovered physical
exclusion; an independently poisoned workspace remains unavailable. Session
deletion folds later proof before deciding whether containment is still unresolved.

Recovery also retains a reserved generation that never committed activation
ownership. Its exact native receipt can later commit proven RolledBack and release
only that reservation's physical exclusion. An unproven rollback fact leaves this
obligation open. Finite Workflow resource records retain their durable ownership
and physical evidence; retained workspace metadata never supplies execution proof.

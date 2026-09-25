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

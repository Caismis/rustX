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
commits, execution settlement and process supervision remain native runtime
responsibilities. Every child runs in the Session's shared workspace and owns
no workspace resource (Issue #462). See [Session deletion ownership](session-deletion-ownership.md).

## Durable Agent authority across activations

The resolved contract freezes at Agent creation, not each resume. The durable
Agent owns the same child ConversationId and frozen Session workspace binding
across multiple finite activation IDs. `send_message` to an inactive Agent reuses that admitted
profile, model, Tools, Skills, resources and policies; it does not consult current
settings or reread a named definition. Parent resource/configuration reload cannot
mutate existing Agent authority. See [Jobs and continuable Agents](jobs-and-agents.md).

Resume also requires proven physical containment of the prior activation. An
explicit `interrupt_agent` settles as Cancelled after native physical settlement
and leaves the Agent resumable. Crash reconciliation records Interrupted without
proof that the old direct child and nested processes settled. It preserves the
same Agent identity and history, projects Unavailable, and refuses another physical activation;
inspecting the shared workspace does not supply that missing proof. A missing or
unusable workspace binding fails resume before allocation, without falling back
to another directory.

Resume preparation has its own durable `AgentActivationAdmission` obligation.
Reserved commits before staging, under the same product ownership fence used by
Session management. Committed activation ownership consumes that reservation;
rollback closes it only with an explicit containment result. An unresolved or
unproven rollback reconstructs Unavailable with the exact reserved activation ID,
blocks further admission, and prevents Session deletion from removing resources.
A prior successful activation cannot settle this later physical obligation.

Publication and containment remain independent: a committed Interrupted terminal
may later acquire durable physical proof without changing its unknown logical
outcome. Only native process-incarnation evidence can discharge that obligation.

Recovery retains exact incarnation lease/receipt evidence until Session deletion.
The registry's bounded reconciliation owner commits
`SubagentPhysicalSettlementProven` only after native Quiescent evidence and lease
release exclude the old writer. This releases only that exact activation's
physical exclusion; an independent publication or admission failure keeps the
Agent unavailable. Session
deletion folds later proof before deciding whether containment is still unresolved.

Recovery also retains a reserved generation that never committed activation
ownership. Its exact native receipt can later commit proven RolledBack and release
only that reservation's physical exclusion. An unproven rollback fact leaves this
obligation open.

## Terminal control evidence

A child Result is one semantic terminal candidate, not physical settlement. The
parent continues reading the reliable control channel through the physical
epilogue: AnchorReleased and non-semantic Diagnostic frames remain admissible.
Duplicate Results and other post-Result frames are protocol violations and take
precedence over the earlier candidate. They settle through the existing Lost
outcome, never successful terminal publication. A valid anchor offered after
Result is retained for containment without an acknowledgement that could admit
new work. If the anchor cannot be retained, the existing ControlFailure outcome
refuses physical settlement proof. The driver still reaps the direct
process and contains every outstanding nested anchor or reports it unresolved
before the registry can publish and release counted capacity. Existing
cancellation commit points and bounded drain/termination escalation remain in
force; a kill signal alone is not settlement proof.

# Issue 411 design and implementation evidence

Starting main: 8b59e770225cf8ae39bfdfe2a6e50b0137bfa146.

Job authority remains ConversationBackgroundRegistry. ToolExecutionId identifies
only an admitted detached call. commit_dispatch owns admission; finish and
cancel_with_reason share the registry mutex and durability gate. Terminal commit
follows physical settlement and canonical inbound publication. Status snapshots
never await. Wait subscribes before observing the exact ID, and never retargets.
List consumes bounded allocation-order registry output. job_cancel waits for the
same terminal settlement rather than returning success for emitted intent.

SubagentRegistry must own durable Agent records, separately from finite activation
records. Existing process driver, cancellation, staged ownership, containment and
workspace settlement remain the finite activation substrate, including Workflow
AgentRuns. AgentId and child ConversationId are stable; activation IDs are fresh.
A durable Agent owns its frozen resolved profile/resources/policies, lineage,
workspace continuity and zero or one activation reservation. Resume must never
resolve a named profile from current configuration.

The same owner arbitration gate must order active message admission, inactive
activation reservation, interrupt and terminal sealing. Active delivery commits
through child canonical inbound, before the child terminal seal can close its
inbox. Stopping/starting rejects transiently. Concurrent messages receive owner
sequence numbers; delivery follows that order. Inactive send reserves precisely
one activation and stages from retained child authority/history. Failed staging
rolls back that reservation without deleting Agent history. Interrupt commits
stopping for the captured activation; normal completion cannot overwrite the
winner. Inactive is published only after physical settlement and final canonical
publication. wait_agent captures an activation ID/watch under owner arbitration;
it may resolve after that activation's inactive edge even if a later activation
has already started.

Journal execution facts carry Agent-to-activation correlation. Canonical child
history stays in its existing SQLite conversation. Parent reports retain the
existing atomic inbound + event commit, with deduplication keys scoped to the
activation, not AgentId. Replay folds activation events into stable Agent rows.
Runtime Client, App Server, TUI and Web remain projections/adapters.

Integration constraints identified before implementation (resolved by the continuation):
- child seal currently closes guidance independently of parent registry state;
  Active delivery/terminal arbitration must address this, not status-then-steer.
- child allocation currently refuses existing conversation identity and rollback
  deletes staged history; resume needs an ownership-safe existing-history path.
- isolated workspace retention/disposal is currently activation-owned; resuming
  must retain/reacquire exact Agent workspace authority, never silently use parent.
- recovery currently reconstructs terminal execution records, not frozen Agent
  admission; durable authority reconstruction must be added explicitly.
- Workflow typed output must remain workflow-local, never parent inbound.

## Inherited checkpoint

The continuation began with six uncommitted files: `src/tools/background.rs`,
`src/tools/native/jobs.rs`, `src/tools/native/mod.rs`,
`tests/scripted/background/jobs.rs`, `tests/scripted/background/mod.rs`, and this
working note. The initial Job tools, exact-ID watch fix, and gate-based tests were
retained. Dual registration was removed when the unified execution tool was
deleted. No changes in the primary checkout were touched.

The inherited checkpoint was not a completed implementation. The sections below
record the subsequent ownership decisions and actual source comparison.

## Source comparison evidence

These are source locations actually inspected.
Local reference repositories were read without modification. Kimi and ZCode were
cloned into task-specific /tmp directories from their authoritative GitHub repos.

### DeepSeek Harness

Checkout: /home/caismis/Documents/codes/deepseek-harness
HEAD: 477b4f420553e8a52c2fbccc464d7561b239c443

Read:
- packages/subagent/subagent/src/continuation-activation.ts
- packages/subagent/subagent/src/control.ts
- packages/subagent/tool-subagent-control/src/index.ts
- packages/jobs/tool-jobs/src/index.ts
- packages/client/ui-subagent/src/client/sidebar-chat/index.tsx

Adapted: child-specific delivery/settlement serialization; service-owned
send/interrupt decisions; separate Job tools; child-conversation-addressed sidebar.
Rejected: one-shot/continuable mode tags, caller-selected steer/queue mode, optional
completion wake policies. Reason: #411 requires one Agent lifecycle and native
canonical inbound publication, with no additional policy framework.

### Codex CLI

Checkout: /home/caismis/Documents/codes/codex
HEAD: 654b0a77d0d2f81aa21f61caf7af4be88fe550bb

Read under codex-rs/core/src/tools/handlers/multi_agents_v2/:
- send_message.rs
- followup_task.rs
- interrupt_agent.rs
- wait.rs

Adapted: distinct Agent control verbs and delegation to agent_control.
Rejected: separate QueueOnly send_message and TriggerTurn followup_task contracts;
activity-wide wait semantics. Reason: rustX send_message must atomically choose
active delivery/inactive activation and wait_agent must capture one activation.
Spawn, list and process-control inspection is recorded in the expanded audit below.

### Claude Code

Checkout: /home/caismis/Documents/codes/claude-code
HEAD: a371abbe75ffa0d0a3c92290e2bbf56a7ef54367

Read:
- src/tools/SendMessageTool/SendMessageTool.ts (local-agent dispatch)
- src/tools/AgentTool/resumeAgent.ts (transcript and authority reconstruction)

Adapted: messages can continue an inactive child from its transcript.
Rejected: current-definition lookup with general-purpose fallback, worktree-path
fallback to parent cwd, and app-state read followed by resume. Reason: frozen
admission authority, exact workspace ownership and owner-linearized arbitration
are stronger requirements here. Foreground/background presentation is covered in the expanded audit below.

### Kimi Code

Checkout: /tmp/rustx-411-kimi
Origin: https://github.com/MoonshotAI/kimi-code.git
HEAD: be7d5f5fea7800778e4660cd5f36780ba783bddd

Read under packages/agent-core-v2/src/agent/tools/task/:
- task-list/taskListTool.ts
- task-output/taskOutputTool.ts
- task-stop/taskStopTool.ts
- task-wait/taskWaitTool.ts

Adapted in the Job adapters: separate bounded listing and nonblocking snapshots,
with an explicit wait tool. Rejected: suppressing terminal notifications after
TaskStop or WaitFor, the WaitFor feature flag, and any-task waits. Reason: #411
requires proactive exactly-once publication and an immutable exact wait target.
The existing rustX watch supplies synchronization; no polling/progress timer was
copied.

### ZCode

Checkout: /tmp/rustx-411-zcode
Origin: https://github.com/zai-org/ZCode.git
HEAD: 29628c9acdb81b703bbd4080c207a0e7ce5e276e

Read:
- apps/zcode-cli/packages/core/src/subagent/runner.ts
  (sendMessageToLocalAgent, deliverMessageToRunningAgent,
   resumeTerminalAgentInBackground)
- apps/zcode-cli/packages/core/src/runtime-task/registry.ts (targeted search)

Adapted: Agent-specific message/resume semantics above runtime task
bookkeeping. Rejected: taskId == agentId, re-registering a terminal task as running,
and looking up the current profile on resume. Reason: Agent and activation identity
must differ; a Job's terminal state is absorbing; authority must stay frozen.

## Repository dependency audit

The initial stale-concept search found 158 affected/source-related paths. Important
additional owners beyond the issue's initial list include:
- src/local_runtime/subagent_child.rs: canonical inbound delivery and terminal seal
- src/local_runtime/composition.rs: frozen child construction and durable binding
- src/durable/inbox.rs and src/durable/sqlite.rs: narrow ownership/terminal commits
- src/runtime/recovery.rs: recovery correlation and interruption publication
- src/runtime/workspace.rs and submodules: retained physical resource authority
- src/runtime/workflow/: finite AgentRun output and workspace contracts
- src/local_runtime/session_ownership.rs, session deletion/archive code
- src/runtime_client/trace/: activation/trajectory correlation

GitHub #15 was read. It still names native one-shot Subagents and excludes
continuable/resumable Subagents from 0.1. No issue text was edited. Its current
release contract is superseded explicitly by `docs/jobs-and-agents.md`;
historical closed issues should remain unchanged.

## Repository validation contract

`.github/workflows/ci.yml` was inspected at the starting main SHA and again after
implementation. In addition to Rust format/lint/build/unit/contracts, Linux CI
requires provider-emulator `uv sync --frozen` and pytest; in-crate boundaries;
external durable/process/subagent/tools/conformance/cfg3 suites; strict TUI tests;
protocol generation/typecheck; dev launcher tests; WebUI unit/provenance/build and
the digest-pinned browser container. macOS execution remains a GitHub CI lane.

Current generated artifacts are App Server v24 and native Runtime Client50.
Repository policy keeps only the current generated version. Initial v22 was
superseded after integrating origin/main #409 (v23); v24 was generated from both
changes, without hand-editing generated types.

## Continuation implementation decisions

The continuation inherited all six files above and preserved their Job groundwork.
The initial origin fetch remained at 8b59e770225cf8ae39bfdfe2a6e50b0137bfa146.

- Existing AgentId is the durable child identity. Existing SubagentId is the
  finite activation correlation; no duplicate durable identity wrapper is added.
- SubagentRegistry owns the Agent map and finite activation records under the
  same mutex. The Agent retains its frozen resolved authority, child conversation,
  and exclusive workspace scope. Workflow-owned finite children remain separate
  from native durable Agent creation.
- Active requires an installed control handle. Private Starting and cancellation,
  sealing, publication transitions project Stopping. Inactive means the previous
  activation's physical and canonical terminal settlement completed.
- send_message chooses Active admission or Inactive resume reservation under that
  mutex. Active envelopes enter the driver's FIFO before the lock is released.
  Inactive reserves one prepare/commit operation; other send/wait/interrupt calls
  during preparation receive a deterministic transient Stopping rejection.
- Child SealRequested commits Running -> Stopping at the same parent mutex.
  The driver drains envelopes admitted before that commit before SealGranted.
  Only then may the child seal its canonical inbox. Already accepted input can
  require another ordinary Agent Loop attempt in the same finite activation.
- Child cancellation must not independently reject a message that won parent
  admission. The child's canonical inbox remains the durable content owner.
- Agent workspace retention is above activation lifetime. Each physical
  activation takes exclusive access and returns it after physical settlement;
  unresolved containment poisons later workspace admission. Staged first-creation
  rollback still settles its never-admitted lease.
- wait_agent captures one immutable activation ID under the owner mutex; an
  inactive Agent returns immediately with no target. Later resume cannot retarget
  that wait. interrupt_agent uses the same capture and existing cancel/settlement
  substrate; it never removes the Agent.
- Frozen semantic authority is committed with initial Agent ownership; subsequent
  activations do not re-resolve current resources. Credential caches continue to
  obey the existing non-serialization contract.
- The obsolete ExecutionKind/ExecutionHandle module and native execution mega-tool
  have been deleted, as have their dedicated compatibility-oriented routing suites.
  Replacement deterministic domain regressions prove the new owner contracts.

Passed targeted continuation regressions (not full CI):
- seal_arbitrates_with_message_admission_and_drains_the_winner: synchronous owner
  admission and real control frames prove both sides of the seal boundary.
- agent_resume_preserves_identity_and_wait_captures_only_one_activation: first
  future poll captures activation A, terminal watch proves A settlement, activation
  B starts before the original waiter is polled again; same Agent/conversation,
  different activation, original waiter completes while B remains Active.

Parallel work was authorized explicitly by the user after implementation began:
protocol/App Server, TUI (also current documentation), and WebUI. Core ownership,
integration, complete validation, commits, publication remain the primary agent's
responsibility. Publication and complete validation are recorded in the PR report.

## Expanded source-reference audit (continuation run, 2026-09-25)

All reference checkouts were read-only. The SHAs above still identify the inspected
source. This audit fills the earlier spawn/list/process/notification gaps; it does
not treat a reference implementation as authority over #411's stricter contract.

### DeepSeek Harness: creation, roster, settlement, UI

Additional exact files inspected:
- `packages/subagent/tool-subagent/src/index.ts`: foreground/background dispatch,
  the historical `backgroundMode` switch, and continuable creation admission.
- `packages/subagent/subagent/src/continuation.ts`: `startContinuable`, frozen
  descriptor/delegated policy capture before awaits, child-specific lock and
  materialization, direct-child message/interrupt authority.
- `packages/subagent/subagent/src/continuation-activation.ts`: final child drain,
  `notifySettlement`, ownership release, resident removal and final-state flush.
- `packages/subagent/tool-subagent-control/src/list-agents.ts`: child identities,
  running/inactive roster and parent/depth projection.
- `packages/jobs/jobs/src/index.ts`: owner fences, first-wins settlement, exact
  waiter release, producer teardown and bounded output-ring contract.
- `packages/client/ui-subagent/src/client/sidebar-chat/index.tsx`:
  child-Session-addressed detail selection and shared canonical conversation UI.
- `packages/client/ui-subagent/src/client/SubagentHeaderLineage.tsx` and
  `packages/client/ui-subagent/src/client/index.ts`: durable lineage/catalog rows,
  activity separate from Session identity, historical composer takeover.
- `packages/client/ui-jobs/src/client/index.ts` and
  `packages/client/ui-jobs/src/client/JobListAction.tsx`: separate Job roster,
  native control service, retained output and running/terminal presentation.

Adapted: durable child identity above finite activation; owner-controlled admission;
separate finite Job registry; stable child detail and native snapshot-driven UI.
Rejected: one-shot/continuable mode split, foreground disposal semantics, suppression
of completion publication for waited Jobs, best-effort-only child persistence.
Reason: rustX has one continuable Agent model, canonical durable history, physical
settlement, and exactly-once proactive publication regardless of readers.

### Codex CLI: spawn/list/interrupt and separate process substrate

Additional exact files inspected:
- `codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs`: explicit creation
  policy, role/model authority construction, child thread/depth/path identity.
- `codex-rs/core/src/tools/handlers/multi_agents_v2/list_agents.rs`: Agent-specific
  roster through `agent_control`, not a filtered process registry.
- `codex-rs/core/src/tools/handlers/multi_agents_v2/interrupt_agent.rs` and
  `codex-rs/core/src/tools/handlers/multi_agents_v2/wait.rs`: thread-specific
  interrupt versus mailbox/activity-wide wait contract.
- `codex-rs/core/src/agent/control.rs`: root-tree-scoped AgentControl registry,
  `start_or_steer_turn`, inter-agent communication and current-task interrupt.
- `codex-rs/core/src/tools/handlers/unified_exec/exec_command.rs`: separately
  allocated process ID, interactive process handling, finite one-shot lifetime.

Adapted: distinct Agent verbs, durable thread/conversation identity, shared owner
control and lower-level process supervision kept separate from Agent semantics.
Rejected: separate queue-only send/follow-up-trigger contracts, mailbox-wide wait,
compatibility submission receipts and interactive process identity as an Agent ID.
Reason: one rustX send operation must arbitrate Active/Inactive itself, and wait
must capture one exact activation; Tool process control is a different domain.

### Claude Code: local source provenance and authoritative current contract

The local checkout's origin is
`https://github.com/yasasbanukaofficial/claude-code.git`, a source mirror, **not**
the official Anthropic source repository. Its inspected SHA is the one above.
Additional exact files inspected:
- `src/tools/AgentTool/AgentTool.tsx`: early stable Agent ID, foreground versus
  background wait/presentation, foreground task registration/background handoff.
- `src/tools/SendMessageTool/SendMessageTool.ts`: local-Agent route, running input,
  stopped continuation and transcript recovery after in-memory task eviction.
- `src/tools/AgentTool/resumeAgent.ts`: canonical transcript loading and resumed
  background activation under the prior Agent ID.
- `src/tools/TaskStopTool/TaskStopTool.ts` and
  `src/tasks/LocalAgentTask/LocalAgentTask.tsx`: task stop, cancellation signals,
  task/UI terminal projection and completion notification.

Authoritative current contract also read directly on 2026-09-25:
[Claude Code subagents](https://code.claude.com/docs/en/sub-agents), specifically
“Run subagents in foreground or background” and “Resume subagents”. It documents
SendMessage continuation, retained conversation history, a new run under the same
Agent ID, and continuation after model-requested stop once that run exits.

Adapted: Agent completion need not end identity; messaging can continue canonical
history; foreground/background controls waiting/presentation, not identity.
Rejected: mirror's app-state read-then-resume branch, current profile fallback,
terminal-on-signal task projection, task eviction timing and built-in one-shot
exceptions. Reason: #411 requires one synchronized runtime authority, frozen
admission resources, physical settlement and one native Agent lifecycle. No source
mirror implementation detail is represented as a verified current official API.

### Kimi Code: finite Task inspection, waits and terminal notification

Additional exact files inspected:
- `packages/agent-core-v2/src/agent/tools/task/task-output/taskOutputTool.ts`:
  nonblocking snapshot, bounded output preview and recorded full-output locator.
- `packages/agent-core-v2/src/agent/tools/task/task-wait/task-wait.ts`:
  explicit blocking-wait input and optional any-task behavior.
- `packages/agent-core-v2/src/agent/task/taskService.ts`: exact task entry waiters,
  unknown-target behavior, cancel/grace/force-stop chain, persistence queues,
  notification origin keys and delivery/suppression markers.
The earlier list/stop/wait tool files remain part of the inspected set.

Adapted: snapshot and wait are separate operations, bounded retained output,
identity-scoped waiter registration and proactive completion bookkeeping.
Rejected: any-task waits, notification suppression after observing results,
feature-flagged wait surface and force-terminal bookkeeping after best-effort stop.
Reason: rustX already has exact watch-based waits and physical-settlement owners;
its completion notice is independent of whether a caller read or waited.

### ZCode: explicit Agent messaging above runtime task bookkeeping

Additional exact files inspected under `apps/zcode-cli/packages/core/src/`:
- `subagent/runner.ts`: `sendMessageToLocalAgent`,
  `deliverMessageToRunningAgent`, `resumeTerminalAgentInBackground`,
  `createRuntimeTaskSnapshot` and the Agent-ID/task-ID shortcut.
- `subagent/runtime-task-registry.ts` and `runtime-task/registry.ts`: common
  bookkeeping re-export, registration/replacement and exact terminal waiters.
- `tool/handlers/agent.ts`: explicit Agent creation and foreground-completed versus
  background-launched result projection.
- `tool/handlers/send-message.ts`: Agent-specific message/resume contract.
- `tool/handlers/task-output.ts`: nonblocking read versus blocking collection,
  fallback polling constant and result-notification claim.
- `tool/handlers/task-stop.ts`: model-initiated stop through control port.

Adapted: Agent-specific messaging and continuation above shared supervision;
finite output/status/wait concepts remain separate from canonical conversation.
Rejected: `taskId == agentId`, replacing a terminal record with Running, failed
active sends silently queued, dynamic profile lookup, polling fallback and legacy
TaskStop aliases. Reason: terminal Job identity is absorbing, Agent activation
identity differs from durable identity, and Stopping admission must reject rather
than silently arrange future work.

## WebUI completion and validation (2026-09-25)

The browser projection now uses v22 `jobs` and durable `agents` independently.
Agent rows key on `agent_id`; open canonical transcript and message draft survive
activation changes. `sendMessage` is the single native delivery/resume operation.
Wait captures only the returned activation notice and refreshes the authoritative
snapshot; a delayed A wait cannot replace streamed resumed B. Separate pending
wait controls leave interrupt/cancel available. Stopping preserves rejected drafts
and never falls back to an automatic resume. Workflow-owned finite children stay
under bounded native Workflow instances and do not receive durable Agent controls.
Trace displays durable Agent and finite activation correlations independently.
Child artifacts remain inert in the child transcript because current `artifact/read`
authorizes the parent artifact store; the UI never guesses child artifact ownership.

Deterministic coverage uses held fake transport replies and explicit snapshot
replacement, with no sleeps proving lifecycle order: `test/activity-controls.test.tsx`
(active/inactive single message operation, interruptible wait, Job cancellation,
delayed wait versus resumed B, canonical transcript refresh, Stopping rejection),
`test/chat.test.tsx` (stable Agent identity through resume/reconnect and irreversible
Job projection), and `test/trajectory.test.tsx` (two activations correlated to one Agent).
`test/e2e/activity.spec.ts` covers controls, transcript retention, Job terminal output,
mobile/desktop overflow, console errors, and actual rendered visual evidence.

Commands run from `web-console`:
- `pnpm install --frozen-lockfile`: passed.
- `pnpm typecheck`: passed against regenerated v22 including `agent_stopping`.
- `pnpm test`: passed, 952 tests across 54 files.
- `pnpm check:provenance`: passed, 135 source records and 131 package notices.
- `pnpm build`: passed; existing >500 kB bundle advisory remains.
- `CONTAINER_ENGINE=podman pnpm test:e2e`: passed, 90/90 tests in the pinned
  Playwright image; `/tmp/411-web-e2e-full3.log`. Podman is the available container
  engine supported by the repository script. No browser lane skipped.
- Final workflow node-key refinement: `pnpm typecheck`, targeted Chat/activity
  tests (14/14), production build, and pinned Workflow browser test (1/1) passed.
- `git diff --check`: passed.

Additional current Web CI dependency lane from `dev`: `pnpm install --frozen-lockfile`,
`pnpm typecheck`, and `pnpm test` passed (37 tests, no skips).
Only `desktop-right-panel-linux.png` changed among screenshot baselines, reflecting
the explicit Jobs/Agents/Workflows Inspector label. It was regenerated with the
repository-pinned Playwright image and visually inspected. New lifecycle evidence
was also inspected at `/tmp/rustx-411-web-activity-desktop.png` and
`/tmp/rustx-411-web-activity-mobile.png`.

## Real-process continuation and recovery containment follow-up

`tests/subagent/continuation.rs` (registered under `end_to_end`) now executes three
real child processes/activations across one durable Agent and child Conversation,
including clean parent shutdown/reopen before the third activation. Explicit
provider `HeaderGate` watch boundaries hold each child before completion. The test
proves distinct activation IDs, canonical prior answers in subsequent model input,
one final parent inbound for each activation, canonical child transcript retention,
and frozen instructions/profile/model temperature despite changing both the named
Agent file and model configuration on disk. Snapshot round trips separately await
parent canonical inbound consumption; terminal settlement is not confused with
parent Agent Loop consumption. No sleeps prove ordering.

Recovery audit found that `restore_agents` restored initial workspace authority
without folding later unresolved/disposal facts. It now permanently revokes resume
authority for unresolved containment, disposal fences/settlement, or an ownership
fact lacking reconciled terminal evidence. It folds the exact post-terminal resource
states without turning disposed resources back into retained ones. First ownership
must carry frozen authority; later activations cannot replace authority or bind the
same Agent to another child conversation, parent, or workspace. The workspace
lease rechecks the revocation flag after exclusive acquisition.

Deterministic registry tests in `registry/agent_recovery_tests.rs` reuse the existing
staged child and nested containment fixtures: shared and isolated unresolved
settlement cannot regain resume authority; an unreconciled orphan cannot admit a
second activation. Both tests passed (`/tmp/411-recovery-test3.log`). The real-process
three-activation regression passed on the integrated runtime in 1.36s
(`/tmp/411-native-continuation4.log`). `git diff --check` passed.

## Final integration and durability corrections

Fetched main advanced from `8b59e770225cf8ae39bfdfe2a6e50b0137bfa146` to
`f268175bb8d31010706e7070aae80d2b46b7aced` (#409). The complete issue implementation
was preserved in commit `afd5fb3c`, then merged in `3a166204`. Main's native Turn
ownership and resident conversation presentation remain intact. Both domains now
use App Server v24 / Runtime Client50; merged Web tests increased to 977.

The full boundary lane exposed a real persistence bug: ordinary credential
serialization is deliberately redacted and cannot round-trip literal values.
SQLite schema45 now atomically captures admitted credentials in a private Agent
record alongside the initial ownership event. The Event Journal carries only
opaque references. Replay hydrates the exact admitted private capture; no current
configuration/environment lookup or fallback occurs. Private records follow the
owning Conversation database's deletion, and are excluded from lineage copies.

Resume reservations now carry their own activation ID and cancellation signal;
cleanup can release only that generation. The owner holds counted runtime
admission across preparation/commit/rollback, and shutdown cancels reservations.
The command lane uses the existing Tokio FIFO substrate without bounded-channel
loss: a deterministic gated32message+cancel test verifies ordered delivery and
physical reap. Publication-abandoned wait/interrupt returns a settlement error.

Crash-reconciled Interrupted events cannot establish physical containment proof.
Such an Agent retains identity/history but cannot resume from a clean Git check.
An ordinary explicit interrupt settles Cancelled and remains resumable. Tests use
the real RecoveryPlan fold and both shared and isolated workspaces to prove this.

Session ownership folds repeated activations into one child edge. Agent workspace
ownership survives activation terminality. Session deletion can remove a clean,
unchanged admitted workspace only through the existing exact owner proof and an
unforced Git remove; dirty/diverged/unresolved work stays protected. Deterministic
Git tests cover dirtiness before preview and after the deletion commit.

## Final merged WebUI validation after current-main integration

Merged `origin/main` #409 (`f268175b`) preserves resident ConversationLive/Seat,
TurnProcess ownership, native clocks/control cursors, and local read-error owners.
The #411 Job/Agent controls now attach through ConversationLive rather than the
old shell-owned transcript path. Final protocol is App Server v24 / Runtime
Client v50. Main's provenance additions and upstream pins were retained, with
reviewed merged local import closures/hashes.

Final validation on the merged runtime, including private admitted credential
persistence and recovery containment fixes:
- Web `pnpm install --frozen-lockfile`: passed.
- Web `pnpm typecheck`: passed.
- Web `pnpm test`: **977 passed / 56 files**, `/tmp/411-web-merge-tests.log`.
- Web `pnpm check:provenance`: **135 source records / 131 package notices** passed.
- Web `pnpm build`: passed, existing bundle-size advisory only.
- `CONTAINER_ENGINE=podman pnpm test:e2e`: **91/91 passed, no skips, 5.6 minutes**,
  `/tmp/411-web-final-merged-e2e2.log`, against the final merged v24 native binary.
- Dev `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm test`: passed,
  **37/37 tests**, rerun after the merge.
- `git diff --check -- web-console`: passed.

The first merged full browser run exposed an existing test teardown ordering
assumption: `workspaces.spec.ts` stopped the routed Product Host while a resident
view could still issue reads. The final test retires the Page before its remote
clients and Host fixtures. It passed in isolation and in the complete rerun.
Only the Inspector screenshot differs from current main's baselines; the pinned
container regeneration was visually inspected and all screenshot checks passed.

A further recovery regression proves actual `RecoveryPlan::reconcile` Interrupted
facts do not create physical containment proof, even for clean isolated/shared
workspaces. Explicit interrupt settles Cancelled and remains resumable. All three
recovery revocation tests passed after credential hydration; see
`/tmp/411-recovery-final.log` and the distinction in `docs/subagent-resources.md`.

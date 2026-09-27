# Session startup ownership (#419)

Implementation base: `6ff49deb4c0855e64f0acaed85ebea2b6a277103`, branch
`issue-419-session-startup-ownership`, worktree `../rustX-issue-419`.
PR #416 is unmerged and overlaps native/protocol owners; this branch does not stack on it.

## Audited owners and transition plan (before implementation)

`ConversationComposer` currently owns `firstSubmitMachine` via `useActorRef`.
Binding changes RESET it; component retirement stops invoked promises and discards
results. `firstSubmitPort.current` includes the captured navigation epoch and
transport authority. Opening at create ACK alone would invalidate that fence.
`App.focusSession` invalidates navigation, restores the view, and classifies its
Workspace. It does not need a catalog row. `ConversationSeat` gates execution.

Native `Connection::handle` dispatches `SessionCreate` to
`SessionRuntimeManager::create_session`. `ApplicationState::initial_binding`
validates explicit `SessionPersistentState.model` or captures the published native
default. At the audited base, the manager passes the selected settings to `SessionController::create_session`,
which prepares private storage and publishes the catalog. Committed durability
errors return identity plus a diagnostic. At that base, configuration binding
registration followed catalog visibility and preceded ACK; the PR #422 repair
closes that visibility gap, as described below. Runtime composition is separate.
The existing v23 contract already carries optional `settings.model`.

Planned ownership: the AppServerClient lifetime retains first-submit work.
Creation acknowledgement is captured before further authority checks. Before
navigation the pending text, ordered File references, receipts and operation state
are assigned to the exact Session owner. Only that owner dispatches continuation.
UI mount/render subscribes; it does not dispatch. Expected handoff renews only the
navigation fence, after old-route eligibility has been checked. Unrelated
navigation and transport/attachment retirement prevent subsequent dispatch.
Already acknowledged creation/upload/admission facts survive those fences.

Creation rejection keeps the editable draft; lost creation responses are terminal
uncertainty. After commit, attach/readiness, upload and admission failures belong
to the Session. Pending input is local intent, never fabricated canonical history.
Admission alone consumes it. Explicit discard clears retained Files after physical
client work settles; no retry edge is introduced. Transport retirement cannot
replay mutations. Closing/reloading the whole client ends in-memory support.

Display selection is explicit choice ?? browser preference ?? native projection;
creation intent is explicit choice ?? browser preference ?? omitted. The existing
native creation owner validates and persists it. Remove post-create setModel and
its repair; retain later intentional changes and ordinary attachment snapshots.
Catalog invalidation stays independently owned by AppServerClient.

## Reference

Separate detached reference: openai/codex
`985cf47a4eb6084b2ff6b30ebdb1216acda85bb4`.
Inspected startup.rs (pending startup draft/submission), session_lifecycle.rs
(`handle_startup_thread_started`, widget replacement), input_flow.rs
(`set_queue_submissions_until_session_configured`, queued input dispatch),
thread_processor.rs (`thread_start`, `thread_start_task`, listener setup),
session/session.rs (`Session::new`, joined persistence/state/auth/MCP futures),
startup_prewarm.rs (`SessionStartupPrewarmHandle::resolve`, abort ownership),
and tasks/regular.rs (`RegularTask::run`, one-time prewarm consumption).
Adapt ownership transfer and execution readiness separation only. Do not copy
thread/start's heavier startup/listener ACK, prewarm, queues, flags or provider
protocol. No source is copied and no pinned Harness reference is changed.

## Final owners and transitions

| Boundary | Owning symbol | Fact / next permitted effect |
| --- | --- | --- |
| Draft gesture | `ConversationComposer`, `AgentComposer` | Resolve authorized Workspace; seal text, ordered browser Files and creation intent |
| Identity commit | `SessionRuntimeManager::create_session`, `SessionController::create_session_with_binding`, `SessionCatalog::publish_session` | Full captured binding installed before catalog visibility; validated/persisted initial model and native identity; no loaded runtime required |
| Client handoff | `AppServerClient.request` decoded acknowledgement → `FirstSubmissions.submit` committed callback | Publish exact Session operation before `firstSubmitPort.handoff` calls `App.focusSession` |
| Navigation | `App`, client-owned `NavigationEpoch` | Restore native ID directly; Conversation renders without a sidebar row or attach ACK |
| Runtime readiness | `firstSubmitPort.attach`, `AppServerClient.performAttach`, native `load` | Capture this operation's exact attachment; consume established native model/configuration |
| Upload | `FirstSubmissions.submit`, `AppServerClient.upload` | Dispatch one ordered file at a time after readiness; capture receipt before any subsequent fence |
| Admission | `AppServerClient.send` / `turn/start` acknowledgement | Consume local input only upon `inbound_accepted`; provider output remains a later native fact |
| Catalog | Native `SessionSummaryInvalidations` → existing client catalog invalidation | Refresh independently, never awaited by first-submit or attachment |
| Final retirement | `FirstSubmissions.discard` / `AppServerClient.dispose` | Clear retained Files on admission/discard; final disposal clears maps/listeners and fences outstanding work |

The continuation is a small client-owned observable store, not a component actor
or generic scheduler. Its draft and Session indexes point to the same immutable
operation snapshot. Rendering/subscribing cannot dispatch work. An in-flight RPC
settles through the existing transport deadline/retirement; disposal does not
pretend to cancel a mutation that may already have committed. Once it settles,
no further continuation effect can pass the disposed/authority fence.

Expected draft-to-created-Session navigation replaces only the original route
fence, after checking it is still current. Unrelated navigation stops the next
effect and cannot be hijacked by a late create callback. Endpoint replacement,
connection generation, authority revision, attachment identity and native
Conversation identity remain distinct checks. A same-client App remount shares
the navigation owner and operation; it does not acquire a duplicate startup claim.
Transport reconnect may recover ordinary attachment observations but never retries
create/upload/admission. Retired endpoint input has inspection/discard-only UI,
separate from the replacement endpoint's Session namespace.

Known creation rejection allows a new explicit corrected gesture. Unknown create,
upload or admission outcomes have no retry edge. ACK facts survive subsequent
fence failure, including committed creation durability diagnostics. Failed or
uncertain post-create input stays read-only and recoverable with its receipts;
explicit discard releases it without claiming that an uncertain native mutation
failed. Admission clears local input; native history/status alone indicates Agent
execution and output. This guarantee covers the in-memory client lifetime only:
hard reload, tab close and client disposal do not preserve browser File objects.

## Model authority and removed dependencies

No new model wire field was needed: base v23 already had the correct native
`SessionPersistentState.model: Option<SessionModelConfig>` creation contract.
The Web now uses that contract. Explicit choice and browser preference are sent;
a displayed projected default is not. Native creation validates without fallback,
persists/readbacks the initial model and captures configuration authority before
catalog visibility (and therefore before ACK). Attach reads it, and the first Attempt freezes it. A later intentional
`setModel` still follows native busy/settlement rules and cannot rewrite an
already-admitted Attempt.

A native creation ACK for explicit model intent also seeds the existing browser
preference for future Sessions; omitted intent never seeds the displayed default.

The old composer-owned `firstSubmitMachine`, RESET/completion navigation chain,
post-create initial `selectSessionModel` and redundant `repairAgentModel` call
are deleted. Legitimate snapshot/recovery/model-switch paths remain. Neither
`listSessions` nor display-summary reads gate successful attachment. Native
catalog additions and removals publish membership invalidation even when publication
returns a committed durability diagnostic. v24 adds mandatory `catalog_changed`
to the existing summary invalidation; it introduces no second catalog authority.
The coalescer retains unobserved membership invalidation across preview updates.
Cleanup/recovery without another membership transition emits no duplicate.
ACK observer exceptions are isolated from wire settlement and request pumping.
See the [PR #422 repair contract and regression map](repair-422.md) for native
creation exclusion, rollback/durability semantics and multi-client proofs.

## Deterministic coverage

| Invariants | Tests (all use gates/controlled replies rather than sleep proofs) |
| --- | --- |
| Visibility before attach/catalog; no readiness bypass; original component retires | `startup-ownership.spec.ts`: English/Chinese ACK + remount tests, actual `App`, production client and continuation owner |
| One continuation, ordered Files/receipts, one `turn/start` | Same browser tests gate attach and admission across two App remounts; `first-submit.test.ts` repeated subscriptions/submissions |
| Attach/admission failure remains Session-scoped, create count one | Browser post-create rejection tests in both locales |
| Lost create/upload/admission responses never replay on remount/reconnect | Three browser transport-loss tests and owner tests |
| ACK facts survive subsequent authority failure; diagnostics preserve identity | `first-submit.test.ts`: create, upload, admission ACK/fence tests and durability diagnostic test |
| Expected navigation survives; unrelated navigation/endpoint changes cannot hijack | `new-conversation.test.tsx`, `conversation-residency.test.tsx`, owner authority-retirement tests, existing client attachment fencing suites |
| Partial upload retains receipts, input and file order | `first-submit.test.ts`: second-upload rejection and confirmed-upload fencing |
| Explicit/preference/omitted model and changed native default | Three browser intent cases; native `issue419_initial_model_commit_readback_admission_and_later_switch`, `issue419_omitted_intent_uses_native_default_at_creation_not_draft_projection`, `issue419_rejected_explicit_model_does_not_publish_or_substitute` |
| Later intentional model switch and frozen Attempt | Native creation/readback/admission test; existing model-switch client/native suites |
| Catalog parked/failed then resumed | Browser ACK/remount and catalog-failure tests; native membership/coalescing/durability tests |
| Discard/disposal, stale observer, retired endpoint retained intent | `first-submit.test.ts`; production detached inspection UI |
| Final protocol and TUI initial intent | Generated Rust fixtures/TypeScript contracts; TUI startup explicit/omitted cases, real stdio/WebSocket integration suites |

Final screenshots are retained in [evidence/](evidence/). Screenshots are generated by the controlled browser suite in
`web-console/test-results/startup-{en,zh}-attaching.png` and
`startup-{en,zh}-{session-attach,turn-start}-failed.png`.
See [measurement evidence](measurements.md) and [validation](validation.md).

Exact Codex reference paths inspected (all at the pinned SHA above):

- `codex-rs/tui/src/app/startup.rs`
- `codex-rs/tui/src/app/session_lifecycle.rs`
- `codex-rs/tui/src/chatwidget/input_flow.rs`
- `codex-rs/app-server/src/request_processors/thread_processor.rs`
- `codex-rs/core/src/session/session.rs`
- `codex-rs/core/src/session/startup_prewarm.rs`
- `codex-rs/core/src/tasks/regular.rs`

Codex's external `thread/start` waits for core startup and listener installation;
its internal SessionConfigured event is not an early external ACK. rustX retains
lightweight create and separate attach. Long-lived pending-work ownership and
readiness-gated execution were adapted; joined initialization and owned provider
preparation were evaluated but not copied because no measured unnecessary native
startup dependency justified a refactor. No Codex provider protocol, queue,
feature flag, compatibility behavior or prewarm framework is introduced.

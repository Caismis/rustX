# App Server composition acceptance

This is the product integration map and runbook. Semantic definitions remain in
[the protocol](app-server-protocol.md), [architecture](architecture.md),
[durable Sessions](durable-sessions.md), [residency](runtime-residency.md) and
[configuration lifetimes](effective-settings.md).

## Reference higher-level host

`tests/process/app_server.rs::app_server_reference_host_two_users_and_external_crash_recovery`
is the executable reference. Its `users` mapping stands for already-authenticated
identities. For each identity it supplies distinct User/Workspace configuration and resource sources,
runtime root, authorized tool environment, fake process credential and workspace.
It starts one external WebSocket process, waits for its advertised bound address,
initializes the public protocol and creates/opens a Session with explicit cwd.

The surrounding `Fixture` supplies native `rustx init` authoring and process/pipe
cleanup. It is test support, not another Session/configuration authority. Session
creation in the reference flow goes through `session/create`; configured tool
execution and recovery remain native. Provider credentials are injected into the
process and do not implicitly enter a tool's explicitly authorized environment.

The reference deliberately does not implement authentication, account storage,
workspace allocation/sandboxing, service discovery, a supervisor or restart policy.
Those belong to the higher-level product. `src/app_server/host.rs` is the internal
admission/drain owner and must not acquire those external responsibilities.
Two roots can allocate the same Session ID spelling; IDs are resolved only within
the selected process/root. This test proves product-state isolation, not OS isolation.

## Acceptance evidence

Paths below reuse the existing semantic classes. Timeouts only bound failure;
gates and committed observations establish ordering.

| Scenario | Composition evidence | Native owner / frontier |
| --- | --- | --- |
| A: one process, concurrent Sessions | Process `app_server_concurrent_sessions_finish_across_external_disconnect`; TUI multi-Session integration | Manager + attempts; A provider gate held until B settles, exact two requests and isolated histories/cwd |
| B: ordinary local TUI | TUI `runs Session A while Session B is visible…` and child/focus tests | Typed client/focus; same PID, no cancel/unload call, authoritative resync; explicit child drain on exit |
| C: external disconnect | Same process test as A | Connection detach observed with zero external attachments; provider released, root execution settles while detached; reconnect sees result once |
| D: interactions | Browser `console.spec.ts`; scripted `headless_approval_and_questionnaire_survive_detach_and_settle_once` and `adapter_disconnect_cannot_settle_approval_or_questionnaire` | Native coordinator; same pending identity after reconnect, exact continuation count; provider gate permits headless publication; stdio EOF cannot settle |
| E: canonical sources | Process `app_server_current_sources_and_persisted_selection_survive_process_reconstruction` | Resolver + Session settings; TOML edit after provider admission, live/admitted A frozen, cold B sees edit, cold A combines current defaults and explicit model |
| F: target replacement | Scripted `replacement_waits_for_active_attempt_task_and_changes_only_incarnation_a`, `unload_claim_rejects_late_operations_and_old_incarnations` | Manager writer claim and endpoint fence; semantic reconstruction completes before reattach, only A incarnation changes; native gates prove no second writer |
| G: idle unload | Scripted `idle_grace_detach_eviction_cold_resume_and_session_independence` | Existing manual clock + unload claim; durable list/read/settings/history survive; B remains resident |
| H: two users/processes | Reference-host process test above | Separate native controllers/roots/config/env; real tool cwd and environment, per-process catalog/read, B death leaves gated A alive |
| I: transport parity | `tests/support/app_server_conformance.rs::representative_scenario` through direct/stdio/WebSocket; TUI parity | Same DTO version, model/domain vocabulary, snapshot/event transitions and stale attachment rejection |
| J: same server, both clients | Browser `console.spec.ts` imports actual TUI `AppServerHost` | Browser releases B, remote TUI reads/resyncs B while A runs, releases B back to browser; single-controller refusal first |
| K: restart | Reference-host SIGKILL test; existing process drain/cold-resume tests; TUI unexpected child death | Tool result has reached next gated provider request; no replay, catalog reads load nothing, native cold recovery |
| L: bounded repetition | TUI `bounded product lifecycle`; existing browser 34 detach/reopens and transport capacity reaping | Three owned stdio children and three WS lifetimes, deterministic turns, detach/delete; runtime and pending/child/workflow registries return to baseline; child exit awaited |

The lower-level race/recovery matrices remain at their owners. No RSS threshold,
transport-specific runtime semantics, or test-only semantic RPC is introduced.

## Local terminal flow

Prerequisites: build `cargo build --bins`, install `tui` dependencies with
`pnpm install --frozen-lockfile`, and configure a model through `rustx init`.
Use only a local emulator for automated acceptance; no paid provider is required.
For deterministic manual use, start the existing emulator in a separate terminal:

```sh
cd test-support/fake-provider
uv sync --frozen
uv run --frozen fake-provider --scenario tui_multi_session --port 8765
```

Before starting the TUI, append `[model_timeout_policy]` with
`response_start_timeout_ms = 600000` and `stream_idle_timeout_ms = 600000` to
the test User rustx.toml. These finite ten-minute deadlines leave time for
human gate control; the normal defaults may expire while reading this runbook.


In another terminal, choose private test directories outside the repository:

```sh
dogfood_home=/tmp/rustx-dogfood-home
mkdir -p "$dogfood_home"
export RUSTX_DOGFOOD_KEY=fake-only
mkdir -p /tmp/rustx-dogfood-work
# Use the built binary's absolute path for these commands.
env HOME="$dogfood_home" rustx init --template openai-chat --provider fixture --model-id integration-model \
  --endpoint http://127.0.0.1:8765/v1 --credential-env RUSTX_DOGFOOD_KEY \
  --context-window 128000 --max-output 4096 --tool-calls true --reasoning false \
  --compat 'chat_reasoning_replay = "omit"'
# From the repository root, through the canonical development owner:
env HOME="$dogfood_home" pnpm --dir dev tui -- --workspace /tmp/rustx-dogfood-work \
  --runtime-root /tmp/rustx-dogfood-runtime
```


1. Record `/debug`'s owned-child PID and `/session` identity for A.
2. Send `tui multi-session: session A long task`. A stops at the provider gate.
3. Use `/new` for B, then send `tui multi-session: session B quick task`.
   B finishes while A is held. `/debug` must show the same PID.
4. Use `/resume` to return to A. Release its gate from another terminal:
   `curl -X POST http://127.0.0.1:8765/__control/gates/session-a-holding/release`.
   A shows its completed answer; B's answer remains in B.
5. Exit with `/quit`. The TUI sends explicit owned-child shutdown and waits for
   native drain. Verify the recorded PID no longer exists. This flow does not
   promise execution survives local TUI exit.

Stop the emulator with Ctrl+C. Use fresh test directories for another run.

## Shared external-server flow

`web-console/` includes a ready-to-run canonical-source/emulator fixture:

```sh
# From web-console/, after pnpm install --frozen-lockfile:
pnpm dogfood:server
# A second terminal:
pnpm dev
```

The first terminal prints the endpoint, transport token **file**, workspaces,
canonical settings path and provider-control URL. Follow the detailed deterministic
prompt/gate sequence in [Web Console README](../web-console/README.md).
Connect the browser at `http://127.0.0.1:5173`. Connect the ordinary remote TUI:

```sh
# From tui/, substitute the printed values:
pnpm start --connect ws://127.0.0.1:PORT --token-file /PRINTED/token-file \
  --workspace /PRINTED/workspaceB --resume
```

Keep A attached in the browser. Explicitly detach B there before choosing B in
the TUI; v1 refuses competing writable controllers. Compare Session/Conversation
identities and `/debug`. Remote TUI exit detaches and leaves this process alive.
Reopen B in the browser after terminal exit.

Exercise browser Disconnect/Reconnect and page reload, including pending Approval
and Questionnaire. The same pending identity must reappear. Answer once. Use the
raw protocol log and Runtime facts panel to inspect attachment/incarnation/cursor
and authoritative state; never resubmit a mutation merely because its reply was lost.

Edit the printed canonical TOML source after a runtime is loaded. Resync preserves
that Session's adopted binding. **Rescan configuration files** ingests external
changes; independently valid policy applies automatically and prepared context
requires explicit adoption. Runtime reconstruction does not adopt pending context.
To dogfood automatic idle residency, configure `[app_server] idle_grace_ms` before
server startup, detach every view/controller of an idle Session, and inspect
`server/diagnostics` until it reports unloaded. The catalog remains visible and a
later open cold-resumes history/settings. Automated TTL proof uses the manual
clock, never real waiting. Finish by Ctrl+C in the external server terminal;
that is the host's explicit process shutdown, independent of client disconnect.

## SESSION-01 / v8

Ordinary Session listing is durable-only. Manager diagnostics retain residency.
Preview accepts resident/attached/current Sessions without destructive guards.
Confirmed deletion fences manager admission, joins transitions and proves native
writer retirement before exclusion, revision validation, commit and cleanup.
The manager suite parks real composition, retirement and operation boundaries to
prove both admission orderings, no Loading/replacement publication after fencing,
external-route closure, active-attempt settlement, absent-Session fencing and
unrelated-Session isolation. Unproven retirement keeps the writer slot and fence.

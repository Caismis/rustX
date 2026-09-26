# WEB-16 validation evidence

PR #409 terminal-process follow-up: [current ownership contract](terminal-process-ownership.md).
The earlier terminal-marker representation described below is superseded by
native `TurnProcessView` in App Server v23 / Runtime Client v49. Follow-up
validation is recorded in the PR report.


## Deterministic architecture coverage

`web-console/test/conversation-residency.test.tsx` instruments the actual
AppFrame and AgentComposer functions (not a parent wrapper) while executing
their real hooks. Five authoritative streaming updates change transcript text
without another invocation of either component. It also retains exact textarea,
card, seat and header nodes, focus and selection. A separately synchronized
fake-clock Attempt advances five seconds with no shell/composer invocation.

Held native create, attach and send acknowledgements prove first-submit
residency and the absence of transaction-phase layout rows. Both bindings enter
the same trigger reducer from `+` and `/`; synthetic entry preserves unrelated
prose and its caret. Successful `/model` consumption and launcher preservation
are tested separately, including Tab selection. IME Enter admits nothing.

Model tests prove native successful selection seeds the next draft without
mutating another Session; a held acknowledgement still commits the preference
after navigation. Lost acknowledgement preserves the previous preference and
does not replay on reconnect. An unavailable saved model is displayed literally
and cannot create a Session. Persistence is checked through a new preference
owner reading the same named storage key.

`shell.spec.ts` additionally compares exact browser header/card rectangles,
retains DOM handles through five stream publications, then advances the browser
clock five seconds and verifies geometry, focus, draft and selection again.
`convergence.spec.ts` retains actual DOM handles across real native first submit.

Native `conversation_totals_and_clock_are_native_and_independent_of_loaded_rows`
compares full/latest/older transcript pages against the same exact native totals
and Attempt clock. Tail tests assert native origin identity and placement outside
the Assistant article. Existing cancellation, queue/steer, upload/receipt,
uncertainty, model admission and draft-isolation suites remain in the full runs.

## Manual browser pass

Used agent-browser session `issue406-e7b2a2a190e1` against an isolated real rustX
App Server, Product Host, two Workspaces and `web_harness_convergence` provider
scenario, not the user's configuration or runtime data.

- Selected Workspace A; verified `+` over `draft untouched` retained selection
  `[3,6]` and textarea focus. Typed `/` entered the same command list.
- Selected `fixture/second-model` through `/model`, submitted
  `Converge the conversation`, and compared the retained input/seat DOM objects
  after native creation: both identical.
- Inspected native write/bash/read activity and explicit approval. The turn-local
  clock reached `Deep diving for 1m 1s`; no normal shell Working banner appeared.
  Approval takeover intentionally hid, rather than destroyed, the resident input.
- Completion showed `Took 1m 1s`, direct Copy/Fork/Branch/Retry and the native
  response timing clock. Copy and a native Branch were exercised. Branch retained
  the original tail while the new Conversation correctly showed zero new totals.
- Statistics details showed one Turn, four Steps and four native requests before
  branching. No raw cwd appeared in the ordinary header.
- Existing Session displayed its own native model while New Conversation read the
  product preference. Navigating before a model acknowledgement exposed a real
  preference-commit lifetime bug; `selectSessionModel` and the held-ack regression
  fix it independently of the initiating control's lifetime.
- Known successful Session deletion left zero Session rows, zero dialogs and zero
  alerts. Only the isolated fixture Session/history was deleted.
- Seeded an explicitly missing saved model/profile in the isolated browser's
  named product-preference key, reloaded, reconnected, selected Workspace A and
  entered a task. The literal missing model and native catalog error stayed
  visible; Send stayed disabled and zero Sessions existed.

Manual captures: `/tmp/issue406-manual-running.png`,
`/tmp/issue406-manual-deleted.png`, `/tmp/issue406-manual-unavailable.png`.
They are supplementary evidence, not lifecycle proof or screenshot baselines.
Pinned automated references were reviewed at desktop and narrow widths; updates
do not widen rasterizer tolerances or change the screenshot comparison policy.

The provider's control report showed all four expected requests matched and
settled, `complete: true`, `ok: true`, no failures. During the manual pass the
preview server stopped; a Fork attempt surfaced local `Failed to fetch` and was
not replayed. Restarting the isolated preview restored access. Later Ctrl+C
shutdown reported `fetch failed` after child-process termination, so that teardown
is not claimed as a passing shutdown check. Automated native Fork/shutdown
scenarios provide the reproducible coverage. Manual browser/server sessions were
closed; fixture-only resources were cleaned by the launcher.

## Validation commands

The PR records final results and revision. Commands actually exercised include:

- Web: frozen install, `pnpm typecheck`, `pnpm test`, `pnpm build`,
  `pnpm check:provenance`, `CONTAINER_ENGINE=podman pnpm test:e2e`.
- Explicit reference updates: `CONTAINER_ENGINE=podman pnpm test:e2e:update`
  and targeted pinned-browser agent/shell reference runs, followed by ordinary
  comparison runs with updates disabled.
- Targeted Vitest: residency, first-submit, new-conversation, turn-process,
  response-tail, session-surface and composer-context.
- Native: `cargo fmt --all -- --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`,
  `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features`,
  `cargo build --bins`, targeted native response-fold tests.
- Protocol: `pnpm generate`, `pnpm check`, `pnpm typecheck`.
- TUI and dev launcher: frozen install, `pnpm typecheck`, `pnpm test`.
- Fake provider: `uv sync --frozen`, `uv run --frozen pytest`.
- `git diff --check`; 176 upstream provenance hashes independently verified
  against their exact local Harness Git objects without changing its checkout.

Initial failures were investigated and corrected: missing native binary before
the first E2E run; Docker unavailable (the prescribed runner supports Podman);
stale protocol-number assertions; obsolete cwd/Lineage/Working/delete-banner
assertions; intentional changed references; model-menu tests assuming selection
left the menu open; and a Clippy large-enum warning resolved with a boxed native
default-model payload. Build retains its existing non-fatal large-chunk warning.

## PR #409 architectural review follow-up

Starting PR HEAD: `34c983353c4a18a835421bc3cbba669955b075eb`.
Latest fetched main: `8b59e770225cf8ae39bfdfe2a6e50b0137bfa146`.
Worktree: `/home/caismis/Documents/codes/rustX-issue-406`, existing branch
`issue-406-conversation-surface`; the original `rustX` main worktree and its
pre-existing `.playwright-mcp/` directory were left unchanged. The pushed final
SHA and final PR state are recorded in PR #409's Repository state section.

New deterministic regressions:

- Native terminal-event folds cover cancellation, failure, timeout and limit
  exhaustion without Assistant output. They compare identity/outcome/native
  timestamps after SQLite reopen, later successful and running Attempts,
  repeated reads, one-entry paging, and wire serialization round-trip.
- Real AppFrame, SidebarRoot, WorkspaceNavigation and ConversationHeader function
  spies stay unchanged through idle/admitted/running/streaming/settled/next
  Attempt, while transcript, TurnProcess and composer actions update. All four
  non-success outcomes reconnect with their original identity.
- A settled live Attempt with no durable transcript terminal cannot create a
  Stopped row. Existing streaming/clock and resident input tests still run.
- Transcript and trace request failures produce exactly one owner-local alert
  and no App notice. Connection/durability recovery and active-surface attachment
  failures keep global presentation. A Settings-owner lookup error stays beside
  its configuration action.

Native final validation: formatting and Clippy with warnings denied pass.
`cargo test --all-targets --all-features --no-fail-fast` passes: **3,924 passed,
0 failed, 7 ignored**, across 18 result blocks. `cargo build --bins` passes.
The initial exact `cargo test --all-targets --all-features` run failed on a
managed-Python fixture's transient PyPI network error; the full no-fail-fast
rerun passed that fixture and every remaining target. No test was disabled.
TUI typecheck and all **852 tests** pass; protocol typecheck passes and the
updated schema/TypeScript were generated using `pnpm generate`.

Validation incidents: an intermediate web run under concurrent native build and
browser load hit two existing five-second Settings test timeouts; the next full
run passed without threshold changes. One intermediate browser Settings test
was invalidated by Vite hot updates to App and ConversationHeader while the
suite was running (confirmed in its trace); final browser validation runs with
source files held fixed. No reference screenshot or comparison tolerance was
changed. The web build retains its existing nonfatal large-chunk warning.

Final web validation: `pnpm typecheck`, `pnpm test` (**55 files / 964 tests**),
`pnpm build`, `CONTAINER_ENGINE=podman pnpm test:e2e` (**90 passed in 5.3m**) and
`pnpm check:provenance` (**135 source records / 131 production notices**) all pass.
The intermediate Workspace browser fixture also had a provider-teardown
connection refusal; the final complete browser run passed it unchanged. The
narrow Settings case passed in nine seconds with no source hot reload. Final
`git diff --check` passes. No browser reference changes were needed.

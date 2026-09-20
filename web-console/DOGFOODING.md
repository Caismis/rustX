# Full Web dogfooding (WEB-10)

For normal local development with your own native settings, use the
[canonical dev launcher](../DEVELOPMENT.md). This guide is the strict scripted
acceptance environment; its fake provider, scenario assertions, isolated CFG3 sources and
test Workspaces are intentionally fixture-only.

Normal local startup is `pnpm --dir dev web -- --workspace /absolute/path/to/project`:
the browser opens, authenticates, reaches a clean URL and connects automatically.
Reload reconnects automatically. `--no-open` prints the startup URL without browser handoff.


This guide uses real rustX App Server, native Tools, the local Product Host and
strict local provider scripts. No paid model or external MCP service is needed.
The scripts validate exact prompt order. A fresh fixture for each chapter keeps
configuration experiments from changing a later script's required model.

## Prepare and start

From the repository root, using Node 24+, the pinned pnpm, Rust and uv:

```sh
cargo build --bins
pnpm --dir web-console install --frozen-lockfile
pnpm --dir tui install --frozen-lockfile
(cd test-support/fake-provider && uv sync --frozen)
pnpm --dir web-console build
pnpm --dir web-console dogfood:server web_console_dogfood
```

The launcher prints `endpoint`, `tokenFile`, `hostConfigFile`, `workspaceA/B`,
`settings`, and `providerControl`. In a **second terminal**, substitute the printed
Host path (the launcher itself relinquishes its Host before printing):

```sh
RUSTX_WORKSPACE_HOST_CONFIG=/printed/host-config.json pnpm --dir web-console preview --port 4173 --strictPort
```

This independently managed acceptance fixture uses explicit Remote mode. Open
`http://127.0.0.1:4173`, then **Settings → Connection → Remote App Server**. Read the
fixture's printed token file locally, enter its endpoint and transport token, and
Connect. Close Settings. Workspace A and B must appear. If they do not,
check the Host environment variable; do not bypass authorization by entering cwd.
The fixture uses isolated User/Workspace configuration and runtime roots, removed
on shutdown, with fake credential references only. Both scopes participate in CFG3
resolution. Host navigation authorization does not gate Workspace configuration.
Do not use this local fixture as a remote authentication service.

The scenario is optional (default `web_console_dogfood`). Unknown flags and multiple
scenarios are rejected.

For gate commands below, set `control` to the printed `providerControl` URL:

```sh
control=http://127.0.0.1:PRINTED_PORT/__control
# Example: curl -fsS -X POST "$control/gates/finish-a/release"
```

Stop the Web server **before** Ctrl+C in the fixture terminal. The fixture checks
all scripted provider requests and cleans up. Ending a scenario early reports an
incomplete script; that is not passing execution acceptance. Configuration-only
exploration intentionally leaves the provider script unconsumed.

## 1. Two Workspaces, concurrent Sessions, recovery and frozen configuration

1. Select Workspace A and Create Session; record its native SessionId from the
   inspector. Create B in Workspace B; keep both views open.
2. Send `Long action in A` in A. Observe `A is running.`. The named `finish-a`
   provider gate now holds work open. In B send `Use B while A runs`; observe
   `B stayed responsive.`. Confirm the inspector's SessionId/cwd changes with the
   selected view, while A's work remains running.
3. In A switch Chat → Trajectory. Inspect the running model request: an unavailable
   end/duration is unavailable, not zero or a client timer. Switch to Settings,
   choose Workspace model `fixture/second-model`, Save Workspace. Prospective
   effective changes; current loaded and frozen admitted request stay
   `fixture/console-model`. Reset Workspace then Save Workspace to remove that
   authored selection. Return to Chat.
4. Disconnect/reconnect in Settings → Connection, then reload and explicitly select
   Remote again and re-enter the fixture token. A is still running;
   the browser neither replays the prompt nor cancels it. Close A's view, then:
   `curl -fsS -X POST "$control/gates/finish-a/release"`.
   Open A again; its settled answer occurs once. B's history stays independent.
5. In A send `Approval please`. Reload while approval is pending; reconnect and
   Allow once. Inspect the real bash Tool input/result and `Approval completed.`.
   Send `Questionnaire please`, reload/reconnect, choose Keep native and submit.
6. Send `Publish while detached`. After `Preparing a question.`, close A's view. Release
   `publish-question` using the same gate URL pattern. Open A; answer the native
   question and observe `Detached question completed.`.
7. In Settings save a global default model. Existing Sessions retain their selected
   model. Deliberately change the current Session through its model selector while
   idle. Instruction changes prepare automatically and require **Adopt prepared
   context**. External file edits enter through **Rescan configuration files**.
8. Search Session metadata, try grouped/flat navigation, rename/reorder and
   unregister Workspace B. Its Session still exists and can be opened as an
   authorized unregistered cwd. No unregister operation cancels/deletes it.

## 2. Transcript, Markdown, images, Trace and bounded windows

Restart both servers with `dogfood:server web_chat_history` and the new Host config.
Create A. Send `History 0` through `History 33`, waiting for each `Answer N` before
sending the next. This crosses the native initial history page. Load earlier and
verify the visible anchor stays in place. Return to latest.

Send `Rich reply`; observe the heading/table/open Rust fence. While streaming,
load earlier history and switch to Trajectory. Load older Trace, inspect a request,
and scroll away from the tail. Reconnect while the `settle-chat` gate is held;
release that gate. The final Markdown list/math/code settles once without jumping
an off-tail reader. Trace inspector Input may be redacted; it must not fabricate
hidden payload, usage or timing. Trace has no mutation controls.

Attach a file named `note.txt`, type `Keep this draft`, and Send. Observe the native
upload attachment. Reconnect; it returns from typed canonical history. Send
`Image please`: the local MCP fixture returns a managed Tool image. Load it, open
the lightbox, Escape, reconnect and repeat. The Tool image is deliberately a
managed artifact; it is **not** the lifetime model of user uploads below.

## 3. Session-owned file/image uploads, exact paths and independent Fork

Restart with `dogfood:server web_upload_conformance`. Create two local input files:
`acceptance.txt` containing `UPLOAD_NATIVE_SENTINEL`, and any small valid PNG named
`pixel.png` (each below 256 KiB). Create Session A, attach both, type exactly
`Use my uploaded files`, Send and Allow once. The real bash Tool reads the uploaded
text and prints its absolute execution-world path; the answer is
`Source upload read through native Tool.`.

Inspect `<workspaceA>/.agents/uploads/<SessionId>/<batch>/`. Compare both paths to
the provider request (`curl -fsS "$control/requests"`): `<user_uploaded_files>`
precedes the unchanged user body. The model receives usable absolute paths, not
ArtifactIds. Reload/reconnect; both typed attachments return.

Fork at the `Use my uploaded files` User boundary. This cut is **before** that
message; the destination composer restores native text/upload receipts. Record the
new SessionId and confirm destination copies exist beneath its own upload root.
Keep this draft open. Close the source Session view without switching to it. In a
second browser page connected to the same Host/server, open the source and choose
Delete the currently selected Session directly after confirmation
through Actions → Delete → Confirm delete. The source root
is gone; destination files remain. Return to the first page, Send the restored
draft and Allow once. Observe `Destination upload read through native Tool.`.
Reload/reconnect. Finally delete the destination and confirm only its root
is removed. Do not replace native cleanup with shell deletion. Crash/recovery and
copy/publication race frontiers are covered by the native upload owner tests.

## 4. Todo, Goal, Queue and commands

Restart with `dogfood:server web_composer_context`. Create A. Todo starts composed
but empty; to inspect the distinct absent state, disable the Todo extension in User Settings
and create a separate Session, then re-enable the extension and create the Session
used for the rest of this script. Send `Plan the composer
docks`, expand Todo, and inspect native status/dependencies. Send `Keep working
until the docks are verified`. At the `goal-round` gate, queue exactly
`Queued during the Goal round`. The seats are Todo → Goal → Queue → Composer.
The empty running composer has one Stop button; entering the queued text replaces
it with Queue. Plain Enter and that button use the same native `turn/start` path;
Ctrl/Cmd+Enter uses existing `turn/steer` for a fresh draft (neither a priority nor
interrupt guarantee). There is no Delivery selector. At desktop and 390px widths,
check the 36px resting editor, multiline growth, internal text scrolling at the
configured cap and shrinkage on deletion. `+` opens commands; the quiet paperclip,
paste and drop all retain upload intake. Effective permission is available on the
control tooltip, with native policy/application information still visible.
Edit the pending row, save, remove, and queue the original text again. Open its
editor, release `goal-round`, and wait for `Queued input handled.`. The claimed
row's obsolete draft must not be sendable. Cancel the draft. Pause/resume Goal;
edit its objective/budget while paused. Reload; native current data returns.
Plugin composition in Settings is separate from this current domain data.

Restart with `dogfood:server web_commands`. Create A. Type `/mdl`, choose
`fixture/second-model`; global approval policy is authored in Settings and requires
a separate Reload after Save. `/tools` opens
inventory. Escape returns focus to the composer. `/not-a-command` must refuse,
not become a prompt. Attach `note.txt`; send `Regenerate my uploaded note`.
Retry / Regenerate at that User boundary. Release `retry-request-reached` after
inspecting the new native ConversationId in the same Session. The regenerated
answer has new lineage. Session tree can still open the original unchanged answer.
Fork the original boundary to inspect a distinct Session's native restored upload.
Stale boundaries are refused by Rust; no UI silently substitutes a newer cut.

### Workflow/Agent execution and inventory

Restart with `dogfood:server web_workflow_conformance`, create A and send
`workflow conformance request`. At `workflow-child-admitted`, inspect the native
Workflow and Subagent cards, reload/reconnect and compare their identities in Developer Inspector. In
Trajectory inspect Workflow Timing: end/duration remain unavailable while running.
Release the gate and observe `workflow conformance complete`. Settings →
Integrations contains the existing acceptance Skill, reviewer Agent and review_pr
Workflow. At Workspace scope toggle the root Workflow selection; its YAML and
Agent TOML files must remain byte-identical. This is selection, not content editing.

## 5. CFG3 Settings and source authoring

Use a fresh fixture without prompts. Open **Global Settings** for User authoring or
the exact registered **Workspace Settings** entry; Session adoption remains in the
focused Session header.

- Add/edit/delete Providers and Models independently in both authored scopes.
  Workspace replaces the complete same-name object; omitted credentials never
  come from User. Effective shows native values and provenance read-only.
- Edit Root Native Tools as an exact whitelist; MCP/Python source selections and
  Skill prompt visibility support all/exact/none. Resource existence grants no
  Root authority. Inspect the actual User config binding separately from fixed
  User resource roots.
- Enable a closed Plugin explicitly; all default off. Plugin selection never
  mutates current Todo/Goal domain state.
- Add an inert MCP definition, then a complete same-name Workspace replacement.
  Saving the definition alone must not connect it. Agent/Workflow selection and
  finite admitted demand drive materialization. Use `test/e2e/web09-mcp.py` only
  as the local fixture when deliberately selecting that source.
- Edit a named Agent as a complete profile, including description/instructions,
  model inheritance, independent Tools/Skills/Plugins, timeout and worktree policy.
  Root's delegation allowlist is separate. Skill, Python and Workflow source
  editors are outside this UI; their native inventory remains available.
- Save and confirm automatic policy application or native pending adoption. Adopt and
  verify N → N+1. Invalid source or busy ownership retains N with native diagnostics.
- Create an external-editor CAS conflict. The draft survives; review the current
  revision before another deliberate Save. Lost responses trigger authoritative
  rereads without replay. Reconnect rereads authority without replaying Save or adoption.
- Invalid Workspace content must be diagnosed, never treated as an inactive trust
  scope. Host picker authorization remains separate from native configuration.

The browser owns forms and drafts. Rust owns parsing, validation, overlay,
provenance, serialization, CAS and publication. No browser configuration layer,
secret-value viewer or full raw-source editor is introduced.

## 6. Responsive and keyboard pass

At 1600, 1280, 820 and 390 CSS pixels, use the same product: Workspace/session
navigation, Chat/Trajectory, docks/composer, command popup, Settings, catalog/MCP
forms, integration inventory and inspectors must remain reachable without page
horizontal overflow. Scroll long forms and inspect wrapping of source paths.

Use Tab/Shift+Tab without the mouse. Horizontal tabs use Left/Right/Home/End.
Open `/model`, use its filter/arrows/Enter and Escape; focus must return to the
composer. Open a Tool image lightbox; Tab stays inside the modal and Escape
restores its trigger. Inspect visible focus, labels, disabled untrusted controls,
Goal edit Enter/Escape and long history without a focus trap. Enable reduced motion.
Repeat Session switching/reconnect and image open/close; automation checks native
attachment counts, object URL release and bounded caches/log retention directly.

## 7. Product surface audit

Use [PRODUCT-SURFACE.md](PRODUCT-SURFACE.md) to classify every visible control.
Select Sessions only in Sidebar; verify no horizontal Session selector exists.
Open A and B, leave A working, select B, and close A's browser view from its row
menu: only the explicit close releases its controller; neither action stops A.
Use Sidebar View options → Close all views to release even restored views whose
catalog rows are absent, then reopen a Session without an invisible capacity block.
Verify empty labels say New session, unnamed committed work uses native preview,
and manual names win permanently. For an off-page unnamed view, interrupt the exact
`session/summary` read after canonical user-message commit, then reconnect: the
native preview must converge without rename or replay. A successful file-only read
stays New session without repeated metadata IO. Sidebar `session/list` search/page
must not change to resolve the header; `view.summary` is a replaceable observation.
No LLM naming is implemented. Check deletion
title and impact counts without raw identity/revision. With A healthy and B uncertain,
verify B's visible row warning, A's clean status, and Inspector ownership separation.
Read global uncertainty separately; review/acknowledge non-interaction notices only
after checking affected work, through Connection → Review uncertain operations.
Fresh/idle Sessions have no exact Attempt or attachment line. A running turn says
Working; queued input stays in Queue; a stop request says Stopping until authority
settles it. Disconnect exposes Reconnect. A lost response says Needs verification
and is never replayed; inspect the exact request evidence in Developer Inspector.
Read all Inspector sections and filter/pause/clear the local log: none may emit a
native mutation. Verify Session actions → Session tree, Chat/Trajectory, Settings,
light/dark and 390/820/1280/1600px keyboard access. Confirmed deletion settles
active work through the native runtime owner; Close view only detaches.

# PR #404 review corrections

Starting HEAD: `eb85c45dfd80b4020e01dd11d82446bde3a8a0f3`.
Base: `da43450b77d3c195d95b06818be8142317663c91`.
Worktree: `/home/caismis/Documents/codes/rustX-issue-402`.
Branch: `issue-402-harness-conversation-models`.

## Create rejection and commit

The old create invocation shared a post-effect authority check with later steps,
and every create failure entered a terminal failed state. Known rejection now
returns to `drafting`, retaining the draft and error. Only a new explicit SUBMIT
can retry. `isOutcomeUncertain` and `WorkspaceHostError.uncertain` instead enter
`uncertain_creation`, which has no SUBMIT transition and requires native inspection.
No error-string classification is used.

Create has a pre-effect fence. Its acknowledged `CreatedSession` is assigned to
machine context before the next effect's fence can reject continuation. A changed
authority therefore stops attach/model/upload/send but retains a committed-Session
recovery outcome and its real identity. Obsolete navigation does not open a newer
route; the recovery presentation identifies the committed Session. Ordinary later
failures still open the committed Session under current authority. No native
identity is manufactured and no uncertain operation is replayed.

## Permission admission

`WorkspaceControls` shares one `useUnitEditing` binding to the existing Settings
target/approval-unit actors between the permission seat and first-submit readiness.
`workspaceApprovalBlock` reads those actors directly at the Send boundary, as well
as driving the rendered disabled state. It owns no transaction or configuration.
The composer remains editable while Send is blocked; submission is never queued.

Blocking facts: submitting; committed but not authoritatively observed; uncertain;
conflict/review required; unapplied approval draft; target mutation/observation
barrier, disconnected/read-error/missing observation, or unavailable prospective
approval. Review, reread, apply and discard remain existing actor events.

The required fence is authoritative observation of the committed source revision.
`UserConfigManager::resolve_session` resolves canonical sources when composing the
new Session (`local_runtime/composition.rs`), and runtime Attempt admission captures
`effective_approval_mode` into its immutable execution settings
(`runtime/conversation_runtime.rs`). Application to an unrelated resident Session
is neither required nor sufficient. The real-browser test verifies the admitted
Attempt's `execution_settings.approval_mode == full_access`, not just RPC ordering.
Existing native t01/t02 configuration tests preserve running Attempt policy.

## Process disclosure

Native `completed_process` remains the only Assistant/Tool membership source.
Status ownership is the exact snapshot Conversation ID plus native Attempt ID.
`agentStatusPlacement`/`statusesAt`, the Status identity and its transcript anchor
are unchanged.

A separate finite seat is either before a process entry or before a Status at its
existing anchor. It is selected in presentation order after indexing native owners,
including completed owners represented only by final text. Thus a User/steering
body remains visible, followed by the disclosure and its anchored Status. No
controlled foldable content precedes its control. A Status-only process gets a
“Thought for a while” disclosure; its final answer stays outside. Pagination can
move the seat while preserving its native key and local expansion state.

No native/protocol/version, lineage, Tool renderer, Models, Trajectory, TUI,
dependency or Harness-pin changes were needed. The pin remains
`ddefc45fbc7f8e46dd73185e68295696d1297887`. Local hashes/treatments for the two
changed Harness-derived composer files are updated; the source/import closure
checks remain strict.

## Deterministic proof

| Regression | Contract |
| --- | --- |
| `first-submit.test.ts`: typed known rejection | Preserved draft, no Session, no automatic retry, one new create only after explicit second SUBMIT |
| `first-submit.test.ts`: typed uncertain creation | SUBMIT cannot replay; no attachment starts |
| `first-submit.test.ts`: resolve create then replace authority before continuation | Native Session retained; no attach/model/upload/send; no replay |
| Existing first-submit phase tests | Model observation gate, individual receipts, later failure recovery, exactly-once and authority fencing retained |
| `new-conversation.test.tsx` | Actual composer editable after known resolve/RPC rejection; draft retained; uncertainty wording/disabled Send; real committed ID retained after authority replacement |
| `settings-machines.test.ts` approval cases | Exact revision, held write, acknowledged commit, failed reread, authoritative settlement, conflict/uncertainty preserving intent |
| `turn-process.test.tsx` | Status before first process member; independent User anchor; Status-only process; unchanged anchor; stable expanded key across pagination; separate retry Attempt; final reasoning/text separation; existing 0/1/N Tools |
| Real `convergence.spec.ts` | Hold before native permission write and after native write/reread but before browser observation; Enter cannot admit; then explicit Send creates exactly one Session/turn with native `full_access` |
| Native configuration t01/t02 | Running Attempt retains admitted policy across later source changes and Tool/model steps |

No sleeps establish these races. Gates, typed outcomes, actor observations and
native event facts establish ordering.

## Affected browser evidence

All are real application/native-server captures, dark theme, 1440×1000, manually
inspected. No unrelated expected snapshot was regenerated and no pixel/axe rule
was weakened.

- [Permission pending](browser/05-permission-write-pending.png): requested Full
  access, editable draft, disabled Send, concise applying status.
- [Folded](browser/07-process-folded.png): control follows independent User input;
  Status hidden and final answer visible.
- [Expanded](browser/08-process-expanded.png): control, then anchored Status,
  reasoning and Tools, then final answer. Alignment and disclosure rhythm checked.
- [Terminal](browser/09-terminal.png) and [Diff](browser/10-write-diff.png): same
  corrected disclosure composition; existing specialized bodies remain intact.

## Development iterations

- Initial HTTPS fetch/PR query hit TLS EOF/handshake timeout; SSH fetch and the
  repeated PR query verified the unchanged reviewed head and main.
- First typecheck found the Session permission-seat caller needed the new shared
  binding argument. The caller was updated without changing Session admission.
- The next typecheck rejected `Promise.withResolvers` under the existing ES target
  and an incomplete typed conflict fixture. Tests now use an ordinary controlled
  Promise helper and the full native conflict shape; no compiler setting changed.
- New UI tests initially supplied a User source to a Workspace read. The real
  authority check correctly refused it; the fixture now names the Workspace.
- First broad unit run: 929 passed, one 5-second remount timeout while native gates
  ran concurrently. The unchanged test passed in isolation. Another full run ended
  with SIGTERM (143), without a test diagnostic. A subsequent full run passed 933;
  after adding the committed-ID UI regression, the final full run passed 934.
- First full browser run: 88 passed, one Settings-observation fixture teardown
  failure (`fetch failed`, upstream socket closed, then browser context shutdown
  failure; its trace records a 120-second context teardown timeout). Its assertions
  completed. The unchanged scenario and the permission convergence scenario then
  passed together (3 tests). No assertion, timeout or screenshot threshold changed.
  The final complete browser rerun passed all 89 tests in 5.4 minutes.

## Validation commands

| Command | Final result |
| --- | --- |
| `git diff --check` | Pass |
| `pnpm --dir web-console exec vitest run test/first-submit.test.ts test/turn-process.test.tsx test/settings-machines.test.ts` | Pass: 168 tests |
| `pnpm --dir web-console exec vitest run test/new-conversation.test.tsx` | Pass: 4 tests |
| `pnpm --dir web-console exec vitest run test/workspaces.test.tsx -t 'repeated product remount'` | Pass: selected regression |
| `pnpm --dir web-console typecheck` | Pass |
| `pnpm --dir web-console test` | Pass: 53 files, 934 tests |
| `pnpm --dir web-console check:provenance` | Pass: 135 source records, 131 notices |
| `node web-console/scripts/provenance.ts --reference /tmp/rustx-402-harness` | Pass: exact pinned source |
| `pnpm --dir web-console build` | Pass; existing large-chunk advisory |
| `CONTAINER_ENGINE=podman bash web-console/scripts/browser-tests.sh convergence.spec.ts` | Pass: 1 real-browser scenario |
| `CONTAINER_ENGINE=podman bash web-console/scripts/browser-tests.sh settings-observation.spec.ts convergence.spec.ts` | Pass: 3 scenarios |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Pass: 89 scenarios, 5.4 minutes |
| `cargo fmt --all -- --check` | Pass |
| `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets --all-features` | Pass |
| `CARGO_BUILD_JOBS=2 cargo test --workspace --all-targets --all-features` | Pass: 3,893 tests, seven pre-existing ignores |
| `CARGO_BUILD_JOBS=2 cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass |
| `pnpm --dir protocol/app-server check` | Pass: runs generator and exact generated-file diff |
| `pnpm --dir protocol/app-server typecheck` | Pass |
| `pnpm --dir tui typecheck` | Pass |
| `pnpm --dir tui test` | Pass: 852 tests |

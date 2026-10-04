# Issue 434 validation and ownership audit

Initial repair started from PR HEAD `da93e80afadfb56299ec9725cd9f1a1c4b8321df`.
The commit-boundary/catalog-invariant repair started from
`ea07fa454e8d00c141cb95c9c20a7e45600d5bc4`. The actor/outcome repair starts from
`ce23a59b5586bf3a0e5dc2510125634e43f03cbb`; previous green CI is not evidence for later changes.
Base: `260994fc27ebc1ef1f767b6e6aa21a01c5f676c6` (PR #446, including #447).
Final fetched main: `260994fc27ebc1ef1f767b6e6aa21a01c5f676c6` (unchanged; no rebase required).
Branch: `issue-434-upload-intake`.
Worktree: `/home/caismis/Documents/codes/rustX-issue-434`.
The primary checkout's pre-existing `.playwright-mcp/` was preserved.
App Server advances v33 → v34; Runtime Client remains v57; Session catalog
advances 13 → 14 for native operation correlation. There is no compatibility reader.

## Contract and resource accounting

Browser/TUI → exact control preparation → single-use relative binary WebSocket
capability → native Session upload allocation/materialization → ready commit →
control status read with original receipts. Product Host does not ingest or stage
these bytes. The ordinary 1 MiB JSON control limit is unchanged.

The native policy is 2,097,152 bytes/file; 4,194,304 bytes and eight files per
transfer; independently 4,194,304 admitted bytes and eight uploads per User input;
two prepared-or-active transfers and 65,536-byte binary messages. Native payload
buffering is at most 8 MiB plus a conservative 512 KiB framing allowance; OS TCP
buffers and metadata are separate. Maximum admitted concurrent file bytes/disk
exposure is 8 MiB. This is not a total Session disk quota: committed files and
failed native residue remain native cleanup work. The browser reads one 64 KiB
slice per server demand. TUI reading uses a bounded file buffer and Blob copies.

The control plane validates attachment, metadata and native policy before payload
allocation. The carrier owns capabilities, permits, finite buffers and settlement.
Only `SessionController`/`UploadRegistry` owns durable claims, safe filesystem
writes, synchronization, ready publication, receipts and deletion cleanup.

Operation identity precedes native allocation. Status distinguishes absent,
unresolved, confirmed failed-before-ready and ready. Native ready publication
follows complete writes, file/directory fsync and reopen/identity verification.
A lost reply does not revoke ready. Retry requires authoritative absent/failed
proof and uses a new operation; Check status reads the original operation and
never retransmits bytes. Prepared/active carriers prevent a premature absent read.

Binary admission shares the host drain mutex and acquires an independent host
upload guard. Drain revokes unconsumed preparations and their loopback listeners;
admitted carriers remain counted through terminal settlement. `finish_drain` checks
zero upload guards, an empty transfer registry and zero tracked supervisors after
their task futures have been destroyed. Route/resource drops occur
outside ownership mutexes to avoid host/transfer lock inversion.

Native Session access excludes deletion through carrier/native settlement.
Deletion refuses while that access is held, including immediately before ready;
a deletion winner prevents subsequent upload admission. Removing a browser card
only removes presentation. It cannot roll back a native allocation.

The one selected Composer intake owner retains per-file identities and outcomes
across compatible remounts. Only committed semantic activation retires an incompatible
binding/Session/Conversation owner; speculative render never mutates the committed
ownership. Native authority replacement explicitly retires its File references; disposal clears the collection. First-submit
sealing transfers references to FirstSubmissions before create begins. Native message
ACK, explicit removal/clear, first-submission admission/discard release their owners. Normal mixed selections retain accepted and rejected cards;
over-count selections produce one bounded rejection summary. Every unresolved
card gates submission. Initial per-file dispatch is finite and ordered, stops at
binding replacement, and is never restarted by observation/reconnect. Retained
first submissions remain owned by `FirstSubmissions`, including operation IDs,
receipts, explicit recovery and continuation; no repeated create or turn admission.

## Deterministic evidence

- `native_policy_boundaries_are_independent_of_json_and_images`: below/at/above
  file bytes, batch bytes and count. The same boundaries are tested in Web intake.
- `binary_upload_one_mib_document_repairs_original_receipts_without_model_requests`:
  real native one MiB document, byte equality, exact repeated status receipts,
  one allocation and no model calls. Browser E2E also uploads a one MiB Markdown
  document and checks bytes plus zero provider requests/failures.
- `saturated_binary_admission_does_not_hold_control_and_refuses_excess`: both
  allowed sockets held at server byte demand, excess preparation refused, status
  and detach advance before releasing either stream. Each owned loopback listener
  closes after its own transfer, even while the other remains held.
- `binary_carrier_rejects_bad_lengths_reuse_expiry_and_paths_before_allocation`:
  truncation/disconnect, short/oversized/over-frame payloads, extra trailing
  binary data, reuse, wrong route, controlled expiry and cross-loopback-port
  capability substitution. No allocation occurs. HTTP Content-Length does not
  delimit this dedicated WebSocket carrier; actual framed bytes are enforced.
- `deletion_loses_to_native_upload_exclusion_at_each_materialization_boundary`:
  channels hold allocation claim, directory creation, write, file sync and
  directory sync. Deletion refuses, receipts remain unusable, then ready and
  subsequent deletion succeed. Every injected checkpoint is consumed.
- `lost_upload_waiter_does_not_cancel_or_replay_the_owned_commit`: explicit
  pre-ready gate, deletion refusal, aborted waiter, native completion fence,
  original operation read-repair and exactly one original allocation.
- `binary_ready_commit_precedes_lost_reply_and_exact_read_repair`: a distinct
  post-ready/pre-reply gate proves commit before response loss. The receipt is
  usable at the gate; aborting the socket waiter then returns the same receipts
  on repeated reads without another allocation. Restart also preserves exact
  operation/receipt correlation.
- Existing sync-failure, ready-commit-failure, traversal, ancestor replacement,
  symlink substitution, ownership, branch/fork and deletion-cleanup tests retain
  their assertions. Sync failures remain correlated after controller reopen.
- Web intake tests cover equivalent picker/drop/paste decisions, unsafe names,
  directories, duplicate names/selections, giant bounded rejection, exact pasted
  text/selection, file-only paste, busy-editor preservation, send gating, removal,
  duplicate Retry, uncertain read-only reconciliation, binding replacement, reconciliation while an old carrier promise remains pending,
  exact action settlement and attachment presentation after first admission.
- First-submission tests hold exact replies, preserve receipts across retired
  acknowledgements, recover without another allocation/create, and admit once
  only after explicit continuation. Browser remount/reconnect recovery runs in
  English and Chinese using production owners with gated transport replies.
  Removing an unrelated card preserves an unresolved operation; removing a card
  cannot reopen an uncertain turn admission. Correlated status repair also clears
  only the matching preparation diagnostic, retaining unresolved evidence.
- Shared descriptor tests reject foreign origins, arbitrary paths, credentials,
  query/fragment confusion and remote/loopback mixing.

No new race test uses sleeps, retries, timeout increases or global serialization.
Existing image capability, projection, decoder and token-accounting tests keep
their meaning. Image/provider processing, managed Artifact reads and Session-file
preview policies are unchanged. Uploading an image remains ordinary workspace
file storage until an existing image capability explicitly reads it.

## Validation environment and results

Linux; Rust/Cargo 1.98.1; Node 24.21.0; pnpm 11.13.1; uv 0.11.12.
Provider preparation used `uv sync --frozen --project test-support/fake-provider`
and `uv run --frozen --project test-support/fake-provider pytest test-support/fake-provider`
(51 passed).

Native tests use `TMPDIR=/var/tmp/rustx-434-tests`. The ambient `/tmp/.git` and
`/tmp/rustx.toml` predate this task and redirect discovery fixtures into an
ancestor workspace. The initial default-TMPDIR run failed 32 launch/settings
fixtures; a diagnostic isolated the exact disjoint-runtime-root refusal. The
same fixture passed with the isolated temporary root. No ambient state or native
assertion was changed.

Browser validation uses the repository's supported `CONTAINER_ENGINE=podman`
with its pinned Playwright 1.63.0 image. Docker is not installed. No browser,
font, screenshot baseline, tolerance or timeout was changed. The control-only
wire probe now excludes the binary carrier rather than parsing its frames as JSON.

The eight existing native ignores are two opt-in measurements, one fixture
regeneration writer and five credentialed live-provider tests. Required emulator
coverage was enabled; no ignore was added for this issue.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Passed |
| `cargo build --bins --all-features --locked` | Passed |
| `cargo test --all-targets --all-features --locked -- --list` | Passed, all harnesses discovered |
| `TMPDIR=/var/tmp/rustx-434-tests RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features --locked` | 4,106 passed; eight existing ignores; 19 harnesses |
| `cargo run --example check_test_lanes -- --job rust-contracts` | Linux selectors and coverage passed |
| `cargo run --example check_test_lanes -- --job rust-boundaries` | Linux selectors and coverage passed |
| `pnpm --dir protocol/app-server generate` | Passed |
| `pnpm --dir protocol/app-server check` | Passed; generated outputs have no drift |
| `pnpm --dir protocol/app-server typecheck` | Passed |
| `pnpm --dir tui typecheck` | Passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | 895 passed, zero skipped |
| `pnpm --dir dev typecheck` | Passed |
| `pnpm --dir dev test` | 38 passed |
| `uv sync --frozen --project test-support/fake-provider` / `uv run --frozen --project test-support/fake-provider pytest test-support/fake-provider` | Passed / 51 passed |
| `pnpm --dir web-console typecheck` | Passed |
| `pnpm --dir web-console test` | 1,528 passed |
| `pnpm --dir web-console check:i18n` | Passed |
| `pnpm --dir web-console check:provenance` | Passed; 148 source records and 132 production package notices |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | 165 passed; one Settings focus failure (11.0 minutes); see follow-up evidence below |
| `git diff --check` | Passed |

The full native run includes App Server process, stdio/WebSocket conformance,
Linux filesystem, image capability/projection/accounting and durability tests.
Web tests include affected Product Host admission/file-read boundaries and retained
first submissions. Intermediate browser runs were explicitly interrupted for the original stale
action guard and the repair's known-create-rejection handback; neither is counted
as a passing run. The subsequent
runner initially found the interrupted run's own preview processes still bound to
5173/5174. Those exact owned processes were stopped before the final execution;
no reuse-existing-server setting or retry policy was introduced.

macOS filesystem/platform execution and real desktop GUI checks were not run
locally. Linux success does not claim macOS success. Hosted checks are reported
on the repaired head through required completion for this repair; auto-merge remains disabled.

DSH inspected files, adopted/rejected patterns and exact reference hashes are in
[the architectural decision](issue-434-decision.md). The original reference
checkout was not modified; the audit uses the required pinned independent worktree.

## Repair regressions and macOS investigation

- Owned stdio prepare, then close the control caller and begin drain: revoke
  capability/capacity, reject handshake, prove zero allocation and finish drain.
- Owned stdio admission, gate before native ready, abort caller and close control,
  begin drain: poll proves drain pending, finish refuses and diagnostics count one
  active upload. Release proves ready, empty transfer registry and successful drain.
- Raw start/steer reject nine valid ready receipts, over-budget ready bytes,
  cross-Session and unready receipts with no canonical User/inbound/provider request.
  Exactly eight receipts and exactly four MiB succeed. Files/registry remain owned.
- Nonregular substitution after materialization settles durable Failed, survives
  controller reopen, exposes no receipt, and permits a new explicit operation.
  A consumed pre-rename ready fault settles Failed; a consumed post-rename fault
  preserves visible ready/original receipts, never fabricating failure.
- Web tests prove handoff empties intake, admission/discard release retained files,
  known create rejection returns files/IDs to the live intake for further selection,
  remove/clear and ACK release files, endpoint authority replacement/disposal retire
  owners, disposal is terminal even if a later render requests an owner, and 100
  navigation transitions retain exactly one current owner.

Original macOS failure: run 37116713316, job 111184830556, exact test
`runtime::subagent::registry::tests::parked_recovery_probe_does_not_hold_registry_mutex`.
The first panic was try_lock; hook RecvError followed the test dropping its release
sender. The hook is already taken after releasing the registry mutex. However,
`restore_agents` also starts periodic reconciliation on the Tokio executor. That
independent pass can briefly hold the claim mutex while the explicit test probe is
parked, invalidating try_lock as proof about that probe. It can also consume the
hook first, so joining only the manually spawned thread did not identify the
parked hook owner. The fixture now restores
on a thread with no Tokio executor, then explicitly runs both competing passes.
No production recovery code, timeout, sleep, assertion or suite parallelism changed.
The exact test passed 20/20 Linux executions before and 20/20 after the fixture fix.
Current main's CI run 37111005268 passed; this does not prove the old test race absent.
Hosted macOS evidence must come from the final repaired head, not the old PR run.

## Commit-boundary and durable admission invariant repair

`AttachmentIntakes.lookup` is read-only. A Composer render may create an unattached
candidate, but `useAttachmentIntake` activates it only in the commit layout effect.
Activation registers/binds the selected owner and retires incompatible owners;
transient unmount has no retirement effect. Same-key remount recovers the existing
owner. Explicit authority retirement and terminal client disposal remain intact.

The component regression renders the production intake hook with AgentComposer
and an explicit Suspense gate. A transition attempts B after A owns a visible File.
The render witness proves B ran, and the layout witness proves B did not commit;
A's visible card, exact File reference and owner remain, with zero retirement calls.
Releasing the gate proves B commits, A retires exactly once and releases its File,
and B alone is registered. Unmount/remount then proves B's File owner survives.

Catalog validation checks each durable allocation has one through eight entries,
each admitted size is at most two MiB, and the checked aggregate is at most four
MiB. A real controller-reopen regression writes malformed catalog facts: empty
allocation, oversized file, oversized aggregate of individually valid entries,
and over-count allocation all fail closed. Exact file/aggregate/count boundaries
open successfully. A separate final reopen after workspace content changes proves
the original admitted size is retained; fork/clone still carry that original fact.
No public shape, version, carrier, drain boundary or resource limit changed.

Focused repair validation passed:
- `pnpm --dir web-console exec vitest run test/upload-intake.test.tsx test/first-submit.test.ts test/artifacts.test.tsx`: 57 tests, including the Suspense commit boundary and existing handoff/restoration/authority disposal.
- `TMPDIR=/var/tmp/rustx-434-tests cargo test --lib --all-features --locked catalog_reopen_validates_durable_upload_admission_bounds`: controller reopen corruption/boundaries passed.
- `TMPDIR=/var/tmp/rustx-434-tests RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features --locked upload`: 33 tests, including stdio drain orderings, Failed/Unresolved, mutable fork/clone and one MiB.
- The same native command with filters `native_user_input_receipt_collection` and `binary_ready_commit_precedes`: one test each passed, preserving complete receipt policy and original-receipt response-loss repair.

The complete validation table above records fresh runs for this repair, not the
previous green HEAD. Local logs use `/tmp/rustx-448-r2-*.log`. Hosted required
checks must pass on the new commit before completion is reported.

## Upload actor and truthful recovery repair

One upload-owned TaskTracker supervisor contains expiry and the optional loopback
listener. Publication registers it under Host admission; its transfer permit and
Session exclusion survive until its future is destroyed. Settlement cancels expiry
instead of leaving detached sleeping tasks. Host drain revokes prepared intent,
awaits admitted native work and tracker settlement, and refuses termination with
any remaining actor. Detach/close removes only unconsumed preparations under the
transfer mutex; route destruction remains outside that mutex. Revalidation during
publication prevents a concurrently detached route from publishing new intent.

Deterministic regressions hold the supervisor after listener closure, prove an
empty registry is insufficient for finish_drain, then release it and prove physical
settlement. Six sequential successful uploads each await zero tracked actors.
Explicit detach/re-attach reads original-operation Absent, refuses the old port,
proves no allocation, and successfully uses released capacity. Controlled expiry
now additionally awaits supervisor termination. Existing consumed-work disconnect,
ready response loss and saturated control-progress regressions are preserved.

Both Web upload owners preserve typed Failed/Uncertain evidence. Native Absent
and Failed permit fresh-operation Retry; Unresolved and unrepairable response loss
permit only original-operation Check status. A rejected prepare reads exact status
before classifying, preserving any already-existing ready allocation. Never-dispatched
queued work becomes known retryable when fenced, never uncertain or auto-replayed.
DataTransfer null extraction is rejected metadata; no File content is fabricated.

Focused validation: 36 native upload tests, six binary-filter tests and 63 Web
intake/first-submit/client tests passed. The Web tests exercise native outcomes
through AppServerClient into FirstSubmissions, localized recovery controls, duplicate
gestures, continuation failures, exact operation identities and captured receipts.
The obsolete queued-is-uncertain and rejected-upload-is-uncertain expectations were
replaced with the truthful states. The prior Suspense commit-boundary regression
remains in the executed suite.

The first browser invocation was deliberately interrupted because the new native
binary build had not completed; that run is not counted. Browser and TUI validation
were started again after the final binary build. Current logs use
`/tmp/rustx-448-r3-*.log`.

## Request dispatch certainty follow-up

Started from PR HEAD `6672d055eb1437adc920a1b827b96d82436eacb8`.
The request owner now structurally distinguishes definite non-dispatch from
transmitted mutation response loss. Local capacity/admission/serialization refusal,
validation refusal/timeout, and disconnect of an unsent queued request are known
no-effect failures. Once send begins, even a synchronous transport exception
remains conservatively uncertain. Upload consumes this evidence without inspecting
error text: definite non-dispatch requires neither status read nor carrier.
Explicit server refusals retain exact-operation native read-repair.

Ten new deterministic tests in `upload-dispatch.test.ts` cover a queued unsent
prepare, immediate request-capacity refusal, sent response loss, synchronous send
failure, and six admission-validation refusal cases. The queue regression holds
eight ordinary RPCs, registers prepare, fills the remaining 55 pending slots, and
proves the next registration is refused. Closing that transport yields typed known
failure with zero prepare sends, zero status reads and zero carriers. The real
AttachmentIntake exposes failed recovery, rejects reconciliation, and duplicate
explicit Retry gestures dispatch only one fresh operation identity. Sent-loss
cases retain the original diagnostic identity and reconnect sends nothing again.
Controlled time advances only the validation deadline; no sleeps prove ordering.

Focused Web validation: 93 tests passed across upload-dispatch, workspace-admission,
artifacts, first-submit and upload-intake. Full validation logs for this follow-up
use `/tmp/rustx-448-r4-*.log`; previous-head hosted checks are not evidence for it.


Full follow-up validation: all commands in the table above passed except the
browser aggregate. Native: 4,106 passed / eight existing ignores; TUI: 895;
dev: 38; provider: 51; Web unit: 1,528. Protocol generation/check has no drift;
both Linux CI discovery selectors passed.

The browser aggregate ran all 166 cases: 165 passed, including both upload E2Es;
`settings-presentation.spec.ts:580` failed its remove-trigger focus assertion
after closing/reopening Settings, before any configuration write. Its trace shows
the programmatic focus call followed by an inactive trigger. The test and
DialogSurface implementation are unchanged from main. One isolated diagnostic
execution, `CONTAINER_ENGINE=podman bash web-console/scripts/browser-tests.sh
test/e2e/settings-presentation.spec.ts -g 'confirming a removal settles focus'`,
passed (one test). This does not replace the failed aggregate or establish a
root cause. No unrelated Settings change, timeout, retry, assertion relaxation
or baseline update was made. The full-run failure remains a validation limitation;
macOS and fresh-head hosted CI are not claimed as locally executed.

# PR #416 TUI admission and native portability repair

This follow-up starts at `64fee52393bd1a950705d79ed33ef3f56ffcfc89`, on the
existing `issue-411-jobs-continuable-subagents` branch/worktree. Fetched main and
merge base are `6ff49deb4c0855e64f0acaed85ebea2b6a277103`. The earlier
[publication, answer, and Web-wait repair](pr-416-publication-answer-waits.md)
remains authoritative for those contracts. No wire shape or version changes.

## TUI submission and transport ownership

The editor intentionally retains `/send-message` until classified acknowledgement,
but slash commands did not share ordinary prompt submission exclusion. Repeated
Enter therefore emitted independent guidance RPCs. The dispatcher now owns one
pending Agent-message submission per captured `AppServerSession`, released in
`finally` only after classification. Other commands remain usable. Existing UI
attachment/presentation fencing and exact submitted-draft comparison own clearing:
success clears only unchanged text; not-delivered/unknown retain it; new typing
survives late outcomes. The dispatcher neither retries nor owns Agent lifecycle.

The typed TUI client now admits four waits (`agent/wait`, `job/wait`), two message
admissions (`agent/sendMessage`), two controls (`agent/interrupt`, `job/cancel`,
`turn/cancel`, `interaction/respond`, `interaction/cancel`), and eight ordinary
RPCs. The total is the server's sixteen-request bound. Excess fails locally before
request ID allocation or transport send. There is no queue, deadline, automatic
retry, or implicit domain cancellation. Existing per-method response-loss classes
remain unchanged, including uncertain lost Agent waits that cannot recapture a
later activation. Pending operations release slots only at classified response or
transport termination. A replacement connection never inherits requests.

`one Agent message submission owns its pending draft` covers success,
not-delivered, and unknown, each with unchanged/new editor text, using actual app,
dispatcher, session and client plus a held fake-transport response. Enter is repeated
before release. `pending Agent message leaves explicit interruption admissible`
completes interruption while guidance remains pending. The client tests
`full mixed wait capacity reserves interruption, cancellation and inspection slots`
and `all lanes stay within the server budget and release capacity only on
classified outcomes` exercise all sixteen slots, local rejection, successful
controls/inspection, slot release, connection loss, uncertainty and no replay.
Request-log barriers establish transmission, not wall-clock delays.

Against starting source, the duplicate-submission regression fails with two RPCs
instead of one; the corrected capacity regression fails with five waits instead
of four. These were executed failures, not static predictions.

## Native failures: four distinct causes

* **Socket address:** the process-death harness combined the platform temporary
  root with an unbounded scenario name. Its control listener now owns an exclusive
  short `/tmp/rx-<random>/c` directory, retained through child termination. TempDir's
  exclusive creation provides concurrent uniqueness and cleanup ownership; names
  are bounded independently of Darwin's long TMPDIR. Production validation is
  unchanged. `native_control_paths_are_bounded_and_independently_owned` binds two
  actual listeners, checks both below Darwin's 104-byte sockaddr bound, connects,
  and proves deleting one namespace leaves the other usable.
* **Recovery receipt:** `restore_agents` starts reconciliation. The test's later
  synchronous pass could encounter that pass's `recovery_inflight` claim and return
  before its durable proof commit. Native `prove_after_release` alone is not that
  commit. Production exclusion was correct. The refined
  `recovered_physical_receipt_requires_owner_release_and_durable_proof` parks the
  real startup reconciler after exact claim capture using channels. While parked,
  the registry mutex remains available; releasing the native owner and separately
  acquiring native proof still leaves idle fenced and zero committed proof events.
  Releasing the reconciler and joining its completion yields Inactive, settled,
  idle and an available workspace with exactly one durable proof. Existing retry
  and reopen assertions prove idempotence. No optimistic production idle shortcut.
* **Binary data:** the fixture created a non-UTF-8 filesystem name, which Darwin
  rejected before collection. It now uses a valid Unicode/metacharacter path,
  tests non-UTF-8 bytes as a Git configuration argument payload, and reads 128 KiB
  containing every byte value through supervised Git blob output. The original
  >64 KiB NUL-delimited listing comparison remains. No lossy conversion or change
  to production Git collection.
* **Stopped anchor:** both outer supervisors passed waitpid's `WUNTRACED` to
  `waitid`. Linux aliases it to `WSTOPPED`; Darwin gives them different values.
  The shared native adapter now observes with `WSTOPPED | WNOHANG | WNOWAIT`.
  Bash and interactive supervisors retain their dedicated terminal result owners.
  [Apple's wait header](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/wait.h)
  explicitly distinguishes the flags. `stopped_observation_preserves_stop_and_terminal_status_for_owner`
  establishes SIGSTOP with blocking native observation, observes it twice, then
  establishes SIGKILL termination without reaping; the sole Child owner still
  collects the exact signal. The stopped-anchor integration test now uses a FIFO
  readiness handshake and explicitly stops the known anchor while its command
  blocks on a second FIFO; polling sleeps no longer establish ordering.

Eight equality/inequality assertions in conversation/registry recovery tests now
use `assert_eq!`/`assert_ne!`. No lint suppression. New test assertions were split
into cohesive helpers where strict Clippy's function-length limit required it.

## Ownership re-audit

Read the final owning implementations, not just earlier reports:

* `physical_recovery::initialize` persists private allocation and ancestors before
  authority initialization, under the allocation namespace lock. Complete locked
  lease/Unstarted receipt is atomically renamed and parent-synced before a handle
  escapes. Recovery must acquire that same lock before sealing private names;
  published names always require exact lease/receipt/identity/helper proof even
  when initialization returned an error. Consumed scanners retain all ordinals.
* `agents::capture_agent_activation` captures reserved/current ID under the registry
  lock. Reservation cancellation and subsequent wait use that ID, never a newer
  Agent generation. Resume uses existing durable Agent/conversation identity and
  one reservation. Unproven settlement remains Unavailable.
* `recovery_settlement` owns exact in-flight claims outside the registry mutex and
  retains native proof locks across durable proof and in-memory settlement. Logical
  publication abandonment cannot erase native containment or invent missing proof.
  Goal idle, drain, residency and deletion still include recovery obligations.
* `AttemptTerminal::Completed(AttemptId)` flows from authoritative observation to
  `durable_final_assistant`: final `AssistantMessageCommitted` for that exact attempt
  selects canonical MessageId/content. No historical text or earlier narration can
  replace a refusal-only final answer. Guidance-driven reopen updates the concluding
  attempt. Ordinary cancellation remains absorbing; Workflow committed output plus
  authoritative Completed keeps its separate late-cancellation ordering.
* Accepted parent guidance remains durable; delivery classification distinguishes
  not-delivered, unknown, refusal and accepted. Neither client automatically replays.
  The pre-Delegate turn gate and finite Job/durable Agent separation are unchanged.

Existing allocation, shutdown/deletion, persistent child answer, Workflow/guidance,
generation-capture, Web lifetime and response-loss regressions remain in the full
validation matrix. This audit makes no new power-loss execution claim; durability
is established by the explicit fsync/rename order above.

## Validation

Final command results are recorded below after execution. Linux only: no macOS
execution is claimed. The macOS adapter/integration tests remain enabled for CI.

Development failures (not counted as passes): the initial capacity fixture checked
before FakeTransport's asynchronous write barrier (0 observed requests); after
adding that barrier, the starting client demonstrably transmitted 5 instead of 4.
The first new TUI typecheck found invalid fixture fields/result tags/close reason;
a subsequent client run rejected malformed scripted Job/Agent-list DTOs. Those
fixtures were corrected to the generated schema; the complete client/composer run
then passed 78 tests. One initial assertion-conversion script used the TUI directory
instead of repository root and failed before changing Rust files. The first new
native observation test expected only StillAlive after terminal transition; Linux
correctly returned ECHILD for the stopped-only selector. The test now accepts the
platform's no-eligible-child result and additionally re-observes the exact terminal
status before the sole owner reaps it. Initial strict Clippy found two newly
expanded tests over its function-length limit; helpers removed the violations.
No deadlines were lengthened, assertions weakened, skips added, or platform
coverage removed. These development runs do not replace the full final suites.

### Final executed matrix

All commands below exited 0 on Linux. Rust-bearing commands used
`RUSTUP_TOOLCHAIN=1.98.1` (including protocol generation); the machine's default
1.95 toolchain was not used to claim the newer CI Clippy lint passed. The Web/dev/
provider-support matrix was refreshed after the last Rust test-helper edits.

| Directory | Exact command | Result |
| --- | --- | --- |
| `.` | `cargo fmt --all -- --check` | Passed |
| `.` | `cargo clippy --all-targets --all-features -- -D warnings` | Passed, strict Rust 1.98.1 Clippy |
| `.` | `cargo build --bins` | Passed |
| `.` | `cargo test --lib --all-features subagent` | 337 passed |
| `.` | `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,181 passed; 2 existing ignores; 194 boundaries run separately |
| `.` | `cargo test --test contracts --test provider --all-features` | Contracts 28 passed; provider 166 passed, 5 existing live-credential ignores |
| `.` | `RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features -- boundary_suites::` | 194 passed; no ignores |
| `.` | `RUST_TEST_THREADS=1 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | 419 passed: catalog 26, managed output 5, conformance 22, durable 129, process 62, subagent 45, tools 130 |
| `protocol/app-server` | `pnpm check` | Passed; generated artifacts unchanged |
| `protocol/app-server` | `pnpm typecheck` | Passed |
| `tui` | `pnpm typecheck` | Passed |
| `tui` | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 888 passed; no skips |
| `web-console` | `pnpm typecheck` | Passed |
| `web-console` | `pnpm test` | 1,093 passed in 60 files |
| `web-console` | `pnpm check:i18n` | Passed |
| `web-console` | `pnpm check:provenance` | Passed: 143 provenance records, 131 package notices |
| `web-console` | `pnpm build` | Passed |
| `dev` | `pnpm typecheck` | Passed |
| `dev` | `pnpm test` | 37 passed |
| `test-support/fake-provider` | `uv run --frozen pytest` | 51 passed |
| `.` | `cargo test --lib --all-features recovered_physical_receipt_requires_owner_release_and_durable_proof` | 1 passed |
| `.` | `cargo test --lib --all-features supervised_settlement_git_preserves_large_binary_output_and_argument_bytes` | 1 passed |
| `.` | `cargo test --lib --all-features stopped_anchor_supervisor_is_contained_by_the_outer` | 1 passed |
| `.` | `cargo test --lib --all-features native_control_paths_are_bounded_and_independently_owned` | 1 passed |
| `.` | `cargo test --lib --all-features clean_native_settlement_survives_abandoned_parent_terminal_publication` | 1 passed |
| `.` | `cargo test --lib --all-features stopped_observation_preserves_stop_and_terminal_status_for_owner` | 1 passed |
| `.` | `cargo build --bins` | Passed; final binaries for browser tests |
| `web-console` | `CONTAINER_ENGINE=podman pnpm test:e2e` | 110 passed; pinned Podman Chromium, final binaries |

Also passed `node --test tui/test/app-server-client.test.ts tui/test/composer-app.test.ts`
(78 tests), `git diff origin/main --check`, `git diff --check`, and
`git diff --cached --check`. The final diff was reviewed against both starting HEAD
and fetched `origin/main`; only the intended repair files were committed.

The two existing library ignores are the instrumented stage-profile test and
fixture-corpus writer. Five provider ignores require live OpenAI/Anthropic
credentials. The prescribed library filter is covered by the separate complete
serialized boundary command. No new ignores or platform exclusions. Web build
retains Vite's bundle-size advisory; browser launch prints Node's NO_COLOR /
FORCE_COLOR warning. Neither prevented a check. The browser environment is
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.

There were no failed or interrupted final-matrix runs. Development failures are
listed above. macOS runtime execution, macOS cross-compilation, live-credential
provider tests, and actual power-loss durability were not performed. The portable
fixes and closest Linux regressions passed; the retained macOS CI remains the
platform execution gate. Hosted CI was not monitored after delivery.

### Concurrent base advancement

During browser validation, main advanced through PR #422 to
`ad863a24cf48fbb0d5182746e1046d4b64e8c167`; a concurrent fetch updated the shared
remote-tracking ref at 16:15 +08:00. The initial fetched main and this repair's
merge base remain `6ff49deb4c0855e64f0acaed85ebea2b6a277103`. The delivered branch
is 13 commits ahead and one behind current main. Its complete validation above
covers this repair branch, not an integration with the newly landed Conversation
navigation change. That integration remains outstanding; no newer main work was
reset or modified, and the primary worktree remains untouched.

# EXEC-01 handoff: PR #580

**Resume from implementation checkpoint `61c6d509ab2c0a0b38f7ab7a9107cef18f9089a6`. Issue #576 is not complete.** This is a working branch for another implementer, not a release or a request for final review. Continue `feat/576-execution-boundary` on [PR #580](https://github.com/Caismis/rustX/pull/580); do not merge it or move unfinished EXEC-01 work to #577. The PR base at handoff was `50d0e5728851143b42dfa70f53458eb4f3ebf0b7`.

## What to keep

| Checkpoint | Implemented | Limit of the evidence |
| --- | --- | --- |
| Through `37a459ea9` | Process-owned Session binding; read, metadata, directory pages, conditional write; native Agent read/image/glob/grep and ticketed write/edit, including Workflow mutation. Server makes the permission decision; Client retains the target. | Linux separate-mount evidence is for file operations. The historical line/search measurements use a measuring peer, not a complete Agent Server. |
| `f015a614d` | Completed foreground MCP results keep their business verdict and bounded preview when no save binding is eligible. No Server foreground result spill or invented locator. | Not the entire save protocol, background capture, or Host receiver. |
| `2bd05296a`, `78bef0819` | UDS and authenticated binary WebSocket execution; WSS via verified TLS ingress; paired data/control connections, bidirectional health, mutation cancellation, Client-local arbitration at file publication. | No recovery or replacement claim. A pairing or a health response does not prove workspace config, MCP or Python readiness. |
| `e39279267`, `a6ac82f10` | Independent physical supervisor package; corrected strict docs and real scripted Agent Write above the 64 KiB chunk boundary. | The scripted test is about provider progress, no retry and exactly one tool result, not a complete benchmark of write progress. |
| `61c6d509a` (WIP) | Shared process driver and cancellation primitive in `rustx-process-supervisor`; operation identity, registration, START, input, output pages and settlement DTOs; Execution Client argv/PTY actor; `command/exec` now takes `sessionId` and routes through the admitted Client. UI owns its process handles; Client owns physical subprocesses. Includes generated protocol changes. | This is App Server command execution, **not** native Agent Bash or completed T02/T05/T29. See blockers below before trusting a remote peer or proposing merge. |

## Fix first

1. **Validate process replies at the Server boundary.** `crates/execution-server/src/lib.rs`, `LocalConnection::process`, currently checks the operation key and encoded length but not all request/reply variants or content. Reject and retire on a wrong variant, input sequence, invalid Base64, empty/non-advancing data page, cursor overflow or mismatched end/release. Preserve unknown outcomes after a START crossing. Test with a scripted peer before accepting another page.
2. **Bound and reconcile Server output.** In `src/app_server/command.rs::run`, check stream offsets, global cursor, decoded sizes, per-stream caps, queued bytes/records, final sequence, totals and truncation before reporting terminal success. PTY counts as stdout. Reinstate behavioral coverage for many tiny deltas, a slow or absent consumer, exact cap/cap+1, and terminal-last. A bound enforced only by an honest Client is not a Server bound.
3. **Settle cancellation and release.** Several `request_process_cancel` results are discarded; a full control queue is not proof that cancellation arrived. Make delivery/fencing explicit. Release retained output after UI detach, pre-START cancellation and event delivery failure. Retry only proven pre-wire busy within the original deadline; never retry an ambiguous business START. Check that these cases cannot exhaust the four live/unreleased slots on a healthy Session.
4. **Split logical root authorization from local storage.** `Environment::retirement` in `src/local_runtime/environment.rs` still looks up roots that the Server physically opened. An authorized workspace absent on the Server can therefore fail before Client dispatch. Maintain logical root retirement without opening that workspace. Extend the real Linux mount-namespace process proof to execute a command that reads and writes only the Client view, and exercise commands over WS as well as UDS. Existing file proofs are not a Bash proof.
5. **Make tests fail closed.** `crates/execution-client/src/process.rs` reads ambient `TRACE_ENV` and some real-spawn tests return early when `supervisor_staged()` is false. Remove the production ambient diagnostic path; make a missing staged supervisor a test failure, not a pass. Verify actual FAST/FULL native preparation and package-aware discovery. The test-only pre-START hold in `src/app_server/command.rs` is keyed by process ID so unrelated parallel tests cannot consume it.

## Remaining issue scope

- Move native Agent Bash, including background acceptance, live output, promotion, physical capture and Read/Grep continuation, onto the same Client process kernel. Do **not** fix live-output reads by reopening a Server-local spill. `tools::background::tests::background_live_output_is_readable_while_running_and_reused_at_settlement` is the regression to preserve.
- Finish the remaining environment operations/watch, WorkspaceChanges capture/comparison, minimal config/readiness, parent-controlled child routing, order/terminal proofs and any remaining local workspace/process bypasses. Do not change tests to call an unavailable feature complete.
- Freeze and exercise save/publication and Host Begin/Chunk/End/Abort schemas, candidate identity, digest, first-complete claim and independent budgets. The full later receiver/storage lifecycle is separate; EXEC-01 still owes its own executable contracts and no-Client behavior.
- Complete the 22 required phase-one evidence entries in [the evidence matrix](exec-01-evidence.md), independent builds, resource measurements, platform gates and final HEAD FULL/Merge verification. The [protocol ADR](execution-protocol.md) distinguishes later issue ownership. Do not add `Closes #576` yet.

## Verification and reproduction

The latest WIP tree passed the Linux `boundary_suites::app_server_environment::` selector (24 tests, three consecutive runs), root library Clippy with all features, rustfmt, and `git diff --check`. Earlier in development the Client binary suite passed 40 tests, and strict workspace rustdoc and the scripted multi-chunk write passed on the earlier checkpoint. **These are not a full workspace run or FULL verification of `61c6d509a`.** Do not count an opt-in mount test, macOS, missing helper, or a previous head as verified.

The last inspected CI for the previous pushed head `a6ac82f10142c61389bf64538a015e897245c981` [failed FAST and Merge verification](https://github.com/Caismis/rustX/actions/runs/44612651397). An earlier [failed run](https://github.com/Caismis/rustX/actions/runs/44601924279) exposed the large-write fixture, live-output Read, and strict rustdoc issues. Only the write fixture and rustdoc failures have a demonstrated fix; do not infer a newer run's failing assertion from an older one.

Use the existing worktree `/home/capi/Documents/codes/rustX-576`. On **this machine**, use the large alternate Cargo target and a POSIX temporary directory. `/mnt/Data` is suitable for build artifacts but not permission/symlink semantics of test fixtures; `/dev/shm` may be `noexec`, so use an executable POSIX temp directory for fixtures that directly exec temporary scripts.

```sh
export CARGO_TARGET_DIR=/mnt/Data/rustx-576-target.ZGOkIU
export TMPDIR=/dev/shm
git status -sb
git log -5 --oneline
python3 test-support/prepare_native_tests.py -- cargo test -p rustx --lib --all-features --locked --no-run
cargo test -p rustx --lib --all-features --locked boundary_suites::app_server_environment::
python3 test-support/prepare_native_tests.py -- cargo test -p rustx-execution-client --bins --all-features --locked --no-run
cargo test -p rustx-execution-client --bins --all-features --locked
```

Start with these narrow checks, then add hostile-peer and isolation tests for the blockers above. Use `pnpm --dir protocol/app-server check` and `typecheck` after protocol changes. Before final review run the complete gates specified in #576 on the actual pushed SHA, separately recording PR head, Actions checkout/merge SHA, FULL and Merge verification URLs.

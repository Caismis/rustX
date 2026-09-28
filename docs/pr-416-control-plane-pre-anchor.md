# PR #416: complete supervisor control-frame ownership

Repository: `Caismis/rustX`; worktree:
`/home/caismis/Documents/codes/rustX-issue-411`.
Branch: `issue-411-jobs-continuable-subagents`.
Starting HEAD: `36b2568e44de610d80c8049223a5faa4a7b087bc`.
Main: `a64e8ae79b2fa03da87d9995038670f179434845`, already the merge base
and ancestor (26 ahead / 0 behind at start). No integration or history rewrite.

## Confirmed defect versus historical hypothesis

Live run `36389802926` failed only its macOS job. Its sole failing test was
`unprovable_pre_anchor_reap_never_settles_physically`:

```
pre-anchor owner did not settle;
driver: ["owner_attached"];
stderr:
```

The same run passed normal setsid-failure cleanup, exact recovery waiting and
Bash stopped-anchor. Previous fixture hypotheses did not establish the cause
of the hosted stall, and this report does not reinterpret them as proven.

Before changing either production writer, a real Unix socket-pair regression
requested a 4 KiB send buffer, made the sender nonblocking, and attempted
a 1 MiB payload while the receiver did not drain. The existing writer returned
`Ok(())` after committing **8,064 of 1,048,581 encoded bytes**. The real
`FrameReader` could not reconstruct a frame and the regression failed.

This confirms the protocol defect, not the historical Darwin root cause. The
control-frame writer used a single raw write on an O_NONBLOCK Unix stream and
treated syscall success as whole-frame commit. The repair gives encoded-frame
bytes explicit writer ownership until complete delivery or conclusive channel
failure. The hosted FAIL_SETSID messages are much smaller than the deliberately
oversized regression; the historical log contains no native write result.

The hosted stall was later traced to a fixture control mis-dispatch, not a
lost frame; see `docs/pr-416-macos-pre-anchor-root-cause.md`. A future timeout now
passively reports PID-file existence, outer PID, non-consuming `waitid` status,
driver events and stderr. That inspection cannot reap or establish settlement.
No new diagnostic IPC was introduced.

## Writer contract and liveness

`FrameWriter` owns one encoded `[u32 length][kind][payload]` buffer and its byte
offset. Success requires offset == encoded length. It is local to one synchronous
control owner; there is no background queue or second writer.

- Positive short sends advance only by the returned count.
- EINTR preserves the offset and resumes the same frame.
- EAGAIN / EWOULDBLOCK waits for POLLOUT with the existing nix polling primitive.
  Readiness and interrupted polls are hints; the next send resumes outstanding
  bytes. EWOULDBLOCK aliases EAGAIN on the supported Linux/Darwin targets.
- Sends use MSG_DONTWAIT, including startup calls before O_NONBLOCK is installed.
  The Rust supervisor binaries retain their existing ignored SIGPIPE behavior,
  so a broken peer is reported as an error.
- Zero sends, fatal send/poll errors, peer hangup, or budget expiry fail delivery.
  The socket is shut down in both directions before returning failure, even if
  no caller acts on the error. A partial prefix cannot be followed by a later
  frame on a still-live channel.

A dedicated **one-second per-frame delivery budget** bounds the synchronous
owner when a peer never drains. It is an absolute deadline, not refreshed after
partial progress or EINTR. Bounded channel failure is necessary because writable
readiness alone may never arrive. This is not a physical-proof deadline, a test
deadline, TERM grace, or a retry of a lifecycle operation. Expiry closes the
control channel and existing failure/containment ownership remains responsible
for settlement. No sleep was added.

The nix socket feature supplies safe nonblocking send, shutdown and fixture
socket-buffer configuration; its lockfile adds the transitive memoffset crate.
No protocol version, wire format, shared public type or generated artifact changed.

## Shared Bash boundary

Bash had the identical single-write defect on its fd-0 Unix control socket.
Its small wrapper now calls the same framing primitive, then records existing
bounded trace events only after complete commit. No Bash signal ordering,
TERM/grace/KILL, group ownership, containment or terminal-proof code changed.

## Pre-anchor semantics preserved

Both outer and inner remain the real `interactive-supervisor` executable.
`attach_inner_control` and `await_anchor_commit` retain ordinary `child.try_wait()`.
No test-executable supervisor, polling override or observation side channel exists.

Only the existing private semantic controls from `InteractiveTestControl` arm
the proof-theft scenario. Ordinary `ToolEnvironment` is still opaque command
data and cannot configure the supervisors. Typed physical continuation is untouched.

The FAIL_SETSID branch now requires both its injected process-control failure
and its setup-ending candidate to commit successfully. On failure it emits a
bounded stderr diagnostic and exits via the existing failure exit code; it
cannot enter its permanent park. No transport error manufactures ownership proof.

After both commits, only the proof-theft inner parks until the real outer kills
it. Normal FAIL_SETSID still exits normally. The real outer's fault seam consumes
only the exact inner PID's terminal status, requires SIGKILL for that PID, calls
ordinary `Child::wait()`, and requires actual ECHILD before annotating its error.
No synthetic wait error, process-wide reaper, orphan adoption or test-side cleanup.

The real control-plane observer still requires:

1. `owner_attached`
2. `injected_setsid_failure_received`
3. `injected_reap_failure_received`
4. `control_eof`
5. `direct_child_wait`
6. `direct_child_reaped`
7. `terminality_unproven_publication`
8. `settlement_publication`

The regression requires stored `TerminalityUnproven` and a returned error naming
the pre-anchor state. It rejects `NoOwnership`, `AllChildrenReaped`, `AnchorReady`
and server launch. Its existing 20-second deadlock guard is unchanged. Ordinary
successful direct-child reap remains the prerequisite for proof-carrying
`NoOwnership(pid)`. Recovery, Jobs/Agents, Runtime Client, App Server v27,
TUI/Web and their public protocols are unchanged.

## Deterministic framing regressions

- A non-draining real socket cannot produce successful partial commit. Delivery
  now fails explicitly and the socket cannot accept another frame.
- A real socket is filled until EAGAIN. The receiver waits on a channel until
  the frame writer itself observes EAGAIN, then drains. The real FrameReader
  reconstructs exactly ProcessControlFailure plus its payload, then NoOwnership,
  with no duplicate or trailing bytes.
- Scripted EINTR, short sends and EAGAIN retain the exact byte offset.
- A closed peer produces a channel failure.
- An expired absolute budget cannot attempt another write.

Tests use kernel backpressure and channels for ordering, not sleeps or elapsed
threshold assertions. New timeouts serve only as deadlock guards or the explicit
production control-channel failure budget.

## Validation and delivery

Local validation passed on Linux with normal suite concurrency. Linux does not establish
Darwin correctness. The exact final SHA still requires hosted macOS success.
One post-push CI snapshot is reported in the delivery message. PR #416 is not
merged and auto-merge is not enabled.


| Command | Result |
| --- | --- |
| `cargo test --lib --all-features runtime::supervised_unit::frame_tests -- --nocapture` | 5 passed |
| `cargo test --lib --all-features <interactive/closed filter> -- --nocapture` | All 9 requested filters passed |
| `cargo test --lib --all-features runtime::interactive_process::` | 20 passed |
| `cargo test --lib --all-features runtime::interactive_supervisor::` | 2 passed |
| `cargo test --lib --all-features tools::native::bash::tests` | 37 passed |
| `cargo test --all-features --test tools bash::<filter> -- --exact --nocapture` | All 3 cancellation filters below passed |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,216 passed; 3 existing opt-in tests ignored |
| `cargo test --lib --all-features -- boundary_suites::` | 194 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | durable 129, process 63, subagent 45, tools 133, conformance 22, CFG 26 + 5; all passed |
| `cargo test --test contracts --test provider --all-features` | contracts 28; provider 166 passed, 5 live opt-in tests ignored |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed after the doc-comment formatting correction |
| `cargo build --bins` | Passed, including final build |
| `git diff --check` and `git diff --cached --check` | Passed |

Interactive and closed filters:

- `unprovable_pre_anchor_reap_never_settles_physically`
- `setsid_failure_before_the_anchor_settles_by_direct_pid_reap`
- `inner_exit_before_connecting_reaches_proven_pre_ownership_settlement`
- `post_spawn_handshake_failure_settles_without_stranding`
- `command_environment_cannot_configure_interactive_supervisor`
- `inner_supervisor_loss_is_contained_by_the_outer`
- `direct_supervisor_child_is_reaped_before_settlement`
- `pre_reserved_wait_settles_independently_of_another_agents_recovery`
- `stopped_anchor_supervisor_is_contained_by_the_outer`

Bash cancellation filters:

- `background_cancel_records_term_before_trap_and_physical_terminal`
- `bash_background_cancellation_uses_the_same_process_group_path`
- `bash_kill_escalates_when_term_is_ignored`

Failed diagnostics and corrections:

- The original-writer regression intentionally failed: success with only 8,064
  of 1,048,581 bytes committed. This was observed before production writer changes.
- The first new-writer compile rejected `PollFlags::default`; replaced with the
  actual `PollFlags::empty` API.
- A now-unused stream alias produced a warning and was removed.
- Initial Clippy rejected an unquoted O_NONBLOCK identifier in a doc comment;
  formatting was corrected and the full Clippy command passed afterward.
- Two initial `git fetch origin main` attempts failed DNS resolution. A later
  fetch succeeded and confirmed unchanged main; authenticated CI reads succeeded.
- No test failed after the writer repair, and no diagnostic run was interrupted.
  No retry-until-green test procedure, sleep, existing deadline increase, or
  broad-suite serialization was used.

Client/protocol sources and generated inputs were unchanged; no local client
regeneration or protocol version change was needed. Final pushed SHA, clean
worktree/topology and the single post-push hosted snapshot are in the delivery
message. Final-SHA hosted macOS success remains mandatory.

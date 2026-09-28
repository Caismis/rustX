# PR #416: real pre-anchor supervisor proof-theft fixture

Repository: `Caismis/rustX`. Worktree:
`/home/caismis/Documents/codes/rustX-issue-411`.
Branch: `issue-411-jobs-continuable-subagents`.
Starting HEAD: `9132d94c039883e9f0c35228ac7ab50ff6272c05`.
Fetched main: `a64e8ae79b2fa03da87d9995038670f179434845`, already the merge
base and ancestor (25 ahead / 0 behind at start). No history rewrite or main
integration was performed.

## Hosted evidence

Run `36383023274` on the starting HEAD passed every required job except macOS.
Its only failing test was
`unprovable_pre_anchor_reap_never_settles_physically`:

```
pre-anchor owner did not settle;
driver: ["owner_attached"];
stderr:
```

Normal setsid-failure cleanup, exact recovery waiting and Bash stopped-anchor
passed on that same hosted Darwin run. Earlier runs `36371768937`,
`36374808352` also failed this regression. Earlier Linux success did not
establish Darwin correctness. The latest log does not identify the native
stalled transition; no more precise historical native trace is claimed.

The previous test-harness outer and fixture polling override changed the
scenario: an inner that did not send its expected frame could exit without the
outer observing it. Both have been deleted, including the ignored subprocess
entry and serialized launch arguments. No diagnostic transport or compatibility
path replaces them.

## Real process topology and fault ownership

Both roles now use the ordinary `interactive-supervisor` binary:

```
rustX driver -> interactive-supervisor outer -> interactive-supervisor inner
```

Role arguments, stdio, environment isolation, direct parentage and signal
behavior are the normal execution path. Both pre-anchor state machines call
`child.try_wait()` without a fixture override.

Only `InteractiveTestControl` configures the existing private
`RUSTX_TEST_INTERACTIVE_FAIL_PREANCHOR_REAP` semantic switch after `env_clear`.
It is inherited by the real inner. Its fixture support must be compiled into
the real binary; the runtime's configuration seam remains `cfg(test)`.
Ordinary `ToolEnvironment` entries remain serialized command data, never
supervisor configuration. No new key, public setting or reserved-name rule
was added. The collision regression retains this key and checks that user
values are visible to the command without activating supervisor capabilities.

## Deterministic proof theft

Only when both semantic injections are configured, the real inner:

1. Connects its ordinary supervisor control socket and enters FAIL_SETSID.
2. Writes the known-PID fixture best-effort, sends the fixed injected setsid
   failure and its setup-ending `MSG_NO_OWNERSHIP` candidate.
3. Enters a `thread::park()` loop until killed. Spurious returns cannot cause
   natural exit. There is no elapsed-time synchronization or observer release.

Thus ordinary pre-anchor polling naturally sees a live child and cannot cache
its terminal status. The outer consumes the setup-ending candidate through its
normal protocol path; that candidate is not forwarded as physical proof.

The real outer then performs:

```
child.kill()
-> exact waitpid(inner_pid, None) by the semantic fault seam
-> require Signaled(exact_inner_pid, SIGKILL)
-> ordinary child.wait()
-> require actual OS ECHILD
-> ordinary process-control failure, annotated with the injected-failure marker
```

No synthetic wait error is returned. The exact foreign wait consumes the child
before the designated owner attempts its proof. There is no process-wide wait,
orphan, Linux subreaper dependency, or subsequent test-side cleanup. The PID
file is not read as physical proof. The old fixture had confused an unreaped
physical child with a designated owner lacking proof; these are distinct facts.

Without the proof-theft switch, FAIL_SETSID still sends its setup-ending frames
and exits normally. Ordinary cleanup remains `child.kill(); child.wait()`, and
only a successful designated reap permits `MSG_NO_OWNERSHIP(reaped_pid)`.
Anchor commit, group ownership, emergency containment and driver reap are
unchanged.

## Required regression evidence

The existing real control channel and driver observer require this order:

1. `owner_attached`
2. `injected_setsid_failure_received`
3. `injected_reap_failure_received`
4. `control_eof`
5. `direct_child_wait`
6. `direct_child_reaped`
7. `terminality_unproven_publication`
8. `settlement_publication`

The fault marker is emitted only after the exact SIGKILL status and real ECHILD
checks. The test requires stored `UnitSettlement::TerminalityUnproven` and a
returned error naming the pre-anchor unproven state. It forbids `NoOwnership`,
`AllChildrenReaped`, `AnchorReady`, and server launch. No assertion or deadline
was weakened; the 20-second timeout remains only a deadlock guard.

Recovery, Bash, MCP, incremental Runtime Client, App Server v27, protocol,
TUI and Web sources are untouched. No sleeps, retries, longer deadlines,
broad-suite serialization, diagnostic IPC or test-executable supervisor remain.

## Validation and delivery

Local validation ran on Linux with normal suite concurrency. Final hosted macOS
on the pushed SHA is still required; Linux results do not establish Darwin
correctness. The final SHA, topology and one post-push CI snapshot are reported
in the delivery message. PR #416 is not merged and auto-merge is not enabled.


| Command | Result |
| --- | --- |
| `cargo build --bins` | Passed |
| `cargo test --lib --all-features <filter> -- --nocapture` (nine filters below) | All passed |
| `cargo test --lib --all-features runtime::interactive_process::` | 20 passed |
| `cargo test --lib --all-features runtime::interactive_supervisor::` | 2 passed, no ignored fixture entry |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,211 passed; 3 existing opt-in tests ignored |
| `cargo test --lib --all-features -- boundary_suites::` | 194 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | durable 129, process 63, subagent 45, tools 133, conformance 22, CFG 26 + 5; all passed |
| `cargo test --test contracts --test provider --all-features` | contracts 28; provider 166 passed, 5 live opt-in tests ignored |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `git diff --check` and `git diff --cached --check` | Passed |

Focused filters:

- `unprovable_pre_anchor_reap_never_settles_physically`
- `setsid_failure_before_the_anchor_settles_by_direct_pid_reap`
- `inner_exit_before_connecting_reaches_proven_pre_ownership_settlement`
- `post_spawn_handshake_failure_settles_without_stranding`
- `command_environment_cannot_configure_interactive_supervisor`
- `inner_supervisor_loss_is_contained_by_the_outer`
- `direct_supervisor_child_is_reaped_before_settlement`
- `pre_reserved_wait_settles_independently_of_another_agents_recovery`
- `stopped_anchor_supervisor_is_contained_by_the_outer`

No local build, test, quality check or diagnostic run failed or was interrupted
in this repair. The passing deterministic suite retained its existing
archive-cancelled cleanup diagnostic. The hosted failure above is historical
Darwin evidence, not a local Darwin reproduction. No protocol/client sources
or generated artifacts changed, so no local client regeneration or rerun was
needed. The removed fixture launch and polling symbols have no remaining
source or documentation references.

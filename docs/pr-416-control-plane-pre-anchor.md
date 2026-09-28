# PR #416: one pre-anchor supervisor control plane

Repository/worktree/branch: `Caismis/rustX`,
`/home/caismis/Documents/codes/rustX-issue-411`,
`issue-411-jobs-continuable-subagents`.
Starting HEAD: `4602f807a5408ab4f70e1a7c0702e37a0ba4a10a`.
Fetched main: `a64e8ae79b2fa03da87d9995038670f179434845`, already an ancestor
(23 ahead / 0 behind). No integration or history rewrite was performed.

## Hosted evidence and removal

Live run `36371768937` matched the review. All required jobs except macOS passed.
The only macOS failure was the unprovable pre-anchor regression:

```
pre-anchor owner did not settle;
driver: ["owner_attached"];
native: ["inner_control_connected"];
stderr:
```

The same run passed the semantic fault scenario without a successful live
observer, normal setsid-failure cleanup, recovery, and Bash stopped-anchor. The
superfluous observation plane was removed rather than replaced with another
transport or used to justify a production ownership change. This evidence came
from hosted Darwin logs; Linux is not substituted for Darwin validation.

Removed completely: the private observation field and environment key, native
observation enum and recorder, driver-side datagram drain, socket constructors
and paths, observer-only regression, and observation-key collision fixture. The
passive-observation design report was deleted; the earlier investigation and
environment-ownership documents now describe the actual control-plane test.
No compatibility path, alternate recorder or replacement diagnostic key remains.

## Evidence from the actual protocol

The inner's existing semantic FAIL_SETSID branch writes its known PID best-effort
and sends the fixed process-control message:

```
injected setsid failure after the inner control connection
```

The outer relays that informational frame through its existing
`await_anchor_commit` path. In the driver, a cfg(test) **exact equality** check on
that received message records `injected_setsid_failure_received`. It does not
change lifecycle state or protocol shape and does not classify arbitrary error
text as ownership evidence. Existing injected-reap receipt observation is retained.

The final regression configures only the two semantic faults, spawns, and waits
for its final owner result under the unchanged 20-second deadlock guard. It
requires this ordered driver evidence:

1. `owner_attached`
2. `injected_setsid_failure_received`
3. `injected_reap_failure_received`
4. `control_eof`
5. `direct_child_wait`
6. `direct_child_reaped`
7. `terminality_unproven_publication`
8. `settlement_publication`

The first injected receipt proves the connected inner executed the setsid fault
and sent its real control frame. The second proves the outer entered pre-anchor
cleanup, selected unproven reap and reported it. EOF and direct-child reap prove
outer termination. The stored typed `UnitSettlement::TerminalityUnproven` value,
returned unproven reason and publication observations prove explicit termination
of the logical owner without fabricating physical proof.

No `NoOwnership`, no `AllChildrenReaped`, and no server launch remain mandatory
assertions. Only after the final unproven result and causal evidence does test
cleanup read the known inner PID and terminate/reap any unresolved/adopted inner.
Cleanup is not runtime proof, and PID absence is never treated as settlement.

## Preserved ownership

`attach_inner_control`, `await_anchor_commit`, direct-inner PID ownership,
NoOwnership proof requirements, anchor commit, post-anchor ownership, and
emergency containment are unchanged. The production supervisor diff only removes
the observer and its unused write-result binding. All real semantic fixture
controls remain in `InteractiveTestControl`, isolated from command environment.
The ordinary environment regression still checks child visibility and successful
execution with all retained private-control names. No reserved-name contract or
command filtering was broadened. Typed physical continuation is unchanged.

Recovery, Bash stopped-anchor, MCP, incremental Runtime Client, App Server v27,
protocol generation, TUI and Web source are untouched. No sleeps, retries,
timeout increases or broad-suite serialization were introduced.

## Validation and delivery

Validation ran on Linux with normal suite concurrency.

| Command | Result |
| --- | --- |
| `cargo build --bins` | Passed |
| `cargo test --lib --all-features <filter> -- --nocapture` for the eight filters below | All eight passed |
| `cargo test --lib --all-features runtime::interactive_process::` | 20 passed |
| `cargo test --lib --all-features runtime::interactive_supervisor::` | 2 passed |
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
- `pre_reserved_wait_settles_independently_of_another_agents_recovery`
- `stopped_anchor_supervisor_is_contained_by_the_outer`

The recovery and Bash regressions also passed in the broad deterministic suite.
A source/test/docs search confirms no obsolete observation symbols, private key,
socket path prefix or observer-only test references remain. No protocol/client
reruns or generated changes were needed locally because those files are untouched.

No local build, test or validation run failed or was interrupted. The passing
deterministic suite emitted the existing `ZipWriter::drop ... archive cancelled`
cleanup diagnostic. No retry-until-green runs were used.

The final pushed SHA, topology and one post-push hosted CI snapshot are reported
in the delivery message. Hosted macOS on that exact SHA remains mandatory;
no merge-readiness claim is made here. PR #416 is not merged and auto-merge
is not enabled.

# PR #416: one pre-anchor supervisor control plane

Repository/worktree/branch: `Caismis/rustX`,
`/home/caismis/Documents/codes/rustX-issue-411`,
`issue-411-jobs-continuable-subagents`.
Starting HEAD: `d1e42bffb276fe40b70631c7a255d4343a771064`.
Fetched main: `a64e8ae79b2fa03da87d9995038670f179434845`, already an ancestor
(24 ahead / 0 behind). No integration or history rewrite was performed.

## Historical hosted evidence and removal

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
assertions. The fixture now consumes the exact inner terminal status before the designated
owner attempts its wait. No orphan cleanup is needed, and PID absence is never
treated as settlement. `AnchorReady` is also explicitly forbidden.

## Preserved ownership

`attach_inner_control`, `await_anchor_commit`, direct-inner PID ownership,
NoOwnership proof requirements, anchor commit, post-anchor ownership, and
emergency containment are unchanged. The production cleanup still kills and waits for its exact direct child;
only a successful wait permits `NoOwnership`. All real semantic fixture
controls remain in `InteractiveTestControl`, isolated from command environment.
The ordinary environment regression still checks child visibility and successful
execution with all retained private-control names. No reserved-name contract or
command filtering was broadened. Typed physical continuation is unchanged.

Recovery, Bash stopped-anchor, MCP, incremental Runtime Client, App Server v27,
protocol generation, TUI and Web source are untouched. No sleeps, retries,
timeout increases or broad-suite serialization were introduced.

## Exact wait-proof theft repair

The subsequent final-SHA run `36374808352` at `d1e42bff` passed every required
job except macOS. Its sole failure remained:

```
pre-anchor owner did not settle;
driver: ["owner_attached"];
stderr:
```

The old fixture confused “physical child remains unreaped” with “designated
owner lacks reap proof.” It sent SIGKILL and manufactured an error without
waiting, leaving descriptor closure, exit and reparenting to scheduling. Its
later cleanup incorrectly assumed rustX could adopt that orphan on Darwin;
macOS has no Linux child-subreaper equivalent. The hosted log does not identify
which native transition stalled, and no stronger historical diagnosis is claimed.

The new regression deterministically consumes the exact child's wait status
outside the designated owner, then verifies the owner fails closed without
leaking a child or fabricating `NoOwnership`:

1. The real inner sends its existing injected setsid-failure frame and setup-end
   candidate through the supervisor control channel.
2. The outer enters its existing pre-anchor cleanup and calls `child.kill()`.
3. A `cfg(test)` seam calls `waitpid(inner_pid, None)` for that exact child and
   requires a terminal status. There is no process-wide reaper.
4. The unchanged designated `child.wait()` executes and must return real
   `ECHILD`. Only after checking that OS result does the fixture annotate its
   ordinary process-control failure with the existing injected-failure marker.
5. The outer emits no proof, exits, and the driver observes EOF, reaps its direct
   outer child and publishes typed `TerminalityUnproven` and final settlement.

Rust's Unix `Child` caches a successful `try_wait` or `wait`, as confirmed
by the installed std source and the [standard-library contract](https://doc.rust-lang.org/stable/std/process/struct.Child.html#method.try_wait). The test-only
polling seam therefore lets the existing setup-ending control frame drive this
fixture's cleanup, without an earlier poll consuming/caching the proof. Ordinary
polling remains `child.try_wait()` and ordinary cleanup remains kill/wait.

The fault outer runs as a narrowly selected subprocess entry in the test
executable, so the actual proof-theft code is absent from production builds.
That entry is marked ignored because it is an executable fixture, and is
explicitly invoked by the non-ignored regression with `--exact --ignored`.
The inner is still the normal supervisor binary. The existing private
`RUSTX_TEST_INTERACTIVE_FAIL_PREANCHOR_REAP` key carries the fixture launch
arguments only from `InteractiveTestControl`, after `env_clear`; production
binaries no longer interpret this key at all. No new key, observation IPC,
protocol field, public configuration or compatibility alias was added.
Same-named command data remains isolated in the serialized command environment.

The fixed driver event sequence above, absence of all three forbidden proofs,
server marker absence and stored `TerminalityUnproven` remain required. The
foreign reaper has consumed the child before the real failed wait, so there is
no adopted-inner assumption, test cleanup or leaked inner obligation. The PID
file written by the setsid injection is not read as proof or cleanup authority.

## Validation and delivery

Validation ran on Linux with normal suite concurrency. Hosted
macOS on the final pushed SHA remains mandatory; Linux success is not Darwin
validation. No deadline, assertion, suite concurrency or production ownership
rule was weakened. Recovery, Bash, MCP, protocol and client sources are untouched.


| Command | Result |
| --- | --- |
| `cargo build --bins` | Passed |
| `cargo test --lib --all-features <filter> -- --nocapture` (nine filters below) | All passed |
| `cargo test --lib --all-features runtime::interactive_process::` | 20 passed |
| `cargo test --lib --all-features runtime::interactive_supervisor::` | 2 passed; subprocess entry excluded from standalone execution |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,211 passed; 3 existing opt-in tests plus the explicitly invoked subprocess entry ignored |
| `cargo test --lib --all-features -- boundary_suites::` | 194 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | durable 129, process 63, subagent 45, tools 133, conformance 22, CFG 26 + 5; all passed |
| `cargo test --test contracts --test provider --all-features` | contracts 28; provider 166 passed, 5 live opt-in tests ignored |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed after the test-control helper extraction |
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

Failed diagnostics: the first focused build failed to compile because fixture
argument collection chained `&PathBuf` with `&String`; corrected before execution.
An exploratory source read used a hyphenated binary filename instead of the
actual underscored filename; corrected on the next read. The first Clippy run rejected `spawn_with_control` at 107 lines; the test
executable launch was extracted into a small test-control helper. No test run
failed or was interrupted. The deterministic suite retained its existing archive-cancelled
cleanup diagnostic while passing. The complete validation sequence was rerun after that source extraction; all
final-source commands passed. No retry-until-green procedure was used.
The historical macOS failure above remains honest hosted evidence, not a locally
reproduced Darwin trace.

Final pushed SHA, topology and one post-push CI snapshot are in the delivery
message. No client/protocol artifacts changed; no local client regeneration or
rerun was needed. PR #416 is not merged and auto-merge is not enabled.

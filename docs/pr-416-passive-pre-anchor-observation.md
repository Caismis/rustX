# PR #416: passive interactive pre-anchor evidence

Repository: `Caismis/rustX`. Existing worktree:
`/home/caismis/Documents/codes/rustX-issue-411`, branch
`issue-411-jobs-continuable-subagents`.
Starting HEAD: `7311e956f3faae5d52123265da558cea89ff1107`.
Fetched main: `a64e8ae79b2fa03da87d9995038670f179434845`, already an ancestor
(22 ahead / 0 behind). No merge integration or history rewrite was performed.

## Hosted evidence and defect

Live run `36367939559` matched the review: all jobs except macOS passed.
Its only failing test was
`unprovable_pre_anchor_reap_never_settles_physically`, reporting:

```
missing pre-anchor boundary inner_connected;
events: ["owner_attached"];
stderr:
```

Recovery and Bash stopped-anchor passed on that same hosted Darwin run and are
untouched by this repair. The reported timeout was inspected in the hosted log;
it was not reproduced as a Darwin failure locally on this Linux machine.

The previous observer connected a Unix stream, wrote a boundary, then blocked
reading a test acknowledgement before the supervisor could continue. Worse,
connect/write/read errors became process-control failures and selected an early
owner exit. That made both successful and failed observability part of lifecycle
control. The new design removes both couplings; it does not infer a production
pre-anchor state-machine defect from the old gated fixture.

## Passive ownership model

`InteractiveTestControl` alone supplies a cfg(test)
`pre_anchor_observation_socket: Option<PathBuf>`. It configures
`RUSTX_TEST_INTERACTIVE_PREANCHOR_OBSERVATION` in the private supervisor
environment. The obsolete rendezvous key and implementation are removed with no
compatibility path. `fail_pre_anchor_reap` remains an independent semantic switch.

The dedicated supervisor records fixed enum labels through an unbound Unix
datagram socket put into nonblocking mode **before** sending. There is no
connection, acknowledgement, receive, retry, sleep, observer task join, or test
release. Missing destinations, full queues and other send/setup errors simply
drop evidence. They cannot write a process-control frame, choose an exit branch
or supply physical proof. PID-file recording on the semantic setsid-failure
fixture is likewise best-effort rather than a panic path.

This is a narrow recorder using the existing bounded Bash diagnostic pattern,
not a generic diagnostics framework. Attempts are capped at 32 per dedicated
supervisor; labels contain no command, output, environment values or arbitrary
error text. The fixture socket uses `/tmp/rx-pre-<random>/observations`, independent
of Darwin TMPDIR length. The test reads its queue only after owner settlement.

The native labels identify:

1. InnerControlConnected
2. FailSetsidPathEntered
3. ConcludePreAnchorEntered
4. InjectedPreAnchorReapFailure
5. ProcessControlFailureWritten
6. OuterExiting

The existing driver observer identifies OwnerAttached, receipt of the injected
reap failure, control EOF, direct outer wait, direct outer reap and settlement
publication. A cfg(test) event now explicitly identifies publication of the typed
`UnitSettlement::TerminalityUnproven` outcome. The test checks native and driver
causal sequences, without inventing cross-process logging order. EOF and the
direct-child reap establish the actual outer exit after its exit-path label.

## Semantic and physical assertions

Both FAIL_SETSID and FAIL_PRE_ANCHOR_REAP still run through the existing owner.
The rewritten primary regression first awaits final settlement under the original
20-second deadlock guard. It checks the stored typed TerminalityUnproven value,
the returned unproven reason, both observation sequences, no NoOwnership, no
AllChildrenReaped, no server launch and direct outer reap. Only afterward does
test cleanup use the known inner PID to terminate/reap any unresolved/adopted
inner. Neither test cleanup nor diagnostics contribute a successful unit proof.

`attach_inner_control`, `await_anchor_commit`, direct-PID ownership, the group
commit point, emergency containment and normal production cleanup are unchanged.

`pre_anchor_fixture_transport_failure_is_explicit` is replaced by
`broken_pre_anchor_observer_does_not_change_settlement`. Its four cases combine
absent/missing observation with enabled/disabled semantic reap failure. Only the
semantic switch selects proven NoOwnership versus explicit unproven settlement.
Every case requires direct outer reap and no server launch; diagnostic errors
cannot become process-control evidence.

`command_environment_cannot_configure_interactive_supervisor` now supplies a live
passive socket path through ordinary authorized ToolEnvironment under the new
private-looking name. The executed child sees that exact string, but the socket
receives no supervisor observations. No new command-environment reservation or
filter was introduced. Typed physical continuation is untouched.

## Scope and validation

Only interactive supervision/fixtures and their documentation changed. Recovery,
Bash stopped-anchor, MCP, Runtime Client, App Server v27, protocol generation,
TUI and Web are untouched. No deadlines or existing proof assertions were
weakened. No sleeps, retries or broad-suite serialization were added.

All validation ran on Linux with the repository's normal suite concurrency.

| Command | Result |
| --- | --- |
| `cargo build --bins` | Passed |
| `cargo test --lib --all-features <filter> -- --nocapture` for the nine filters below | All nine passed |
| `cargo test --lib --all-features runtime::interactive_process::` | 21 passed |
| `cargo test --lib --all-features runtime::interactive_supervisor::` | 2 passed |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,212 passed, 3 existing opt-in tests ignored |
| `cargo test --lib --all-features -- boundary_suites::` | 194 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | durable 129, process 63, subagent 45, tools 133, conformance 22, CFG 26 + 5; all passed |
| `cargo test --test contracts --test provider --all-features` | contracts 28; provider 166 passed, 5 live opt-in tests ignored |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `git diff --check` and `git diff --cached --check` | Passed |

Focused filters:

- `unprovable_pre_anchor_reap_never_settles_physically`
- `broken_pre_anchor_observer_does_not_change_settlement`
- `setsid_failure_before_the_anchor_settles_by_direct_pid_reap`
- `inner_exit_before_connecting_reaches_proven_pre_ownership_settlement`
- `post_spawn_handshake_failure_settles_without_stranding`
- `command_environment_cannot_configure_interactive_supervisor`
- `inner_supervisor_loss_is_contained_by_the_outer`
- `pre_reserved_wait_settles_independently_of_another_agents_recovery`
- `stopped_anchor_supervisor_is_contained_by_the_outer`

The closed recovery and stopped-anchor regressions passed both focused and
broad local runs without source changes. Shared protocol/client code is
untouched, so no generated artifacts were churned and separate client commands
were not rerun locally. Their required hosted jobs remain part of final-SHA CI.

## Failed/interrupted diagnostics and delivery

One validation attempt stopped at Rust test compilation with E0433: the added
explicit stored-outcome assertion used `UnitSettlement` without qualifying it in
the nested test module. Qualifying it as `super::UnitSettlement` fixed the source;
all final focused and broad tests passed. There were no runtime assertion failures
or interrupted runs. The first exploratory passive-regression run also passed.
The deterministic suite emitted the existing archive-cancellation cleanup message
(`ZipWriter::drop ... archive cancelled`) while all selected tests passed.

Hosted Darwin has not been rerun locally or inferred from Linux. The delivery
message records the final pushed SHA and one post-push CI snapshot. PR #416 is
not merged and auto-merge is not enabled. Final hosted macOS success remains a
merge gate; this report does not claim readiness.

# PR #416: exact recovery ownership and native fixture evidence

Repository: `Caismis/rustX`; existing worktree
`/home/caismis/Documents/codes/rustX-issue-411`; branch
`issue-411-jobs-continuable-subagents`.
Starting HEAD: `9e51f95bce10e1994ff46c100086650216660d1b`.
Fetched main: `a64e8ae79b2fa03da87d9995038670f179434845`.
The live PR and run `36363800065` matched the review. Main was already an
ancestor (21 ahead / 0 behind); no integration, rebase or history rewrite was
performed.

## Recovery: observe the existing exact owner

The old regression armed Y's positive-proof seam after releasing admission,
then started another reconciliation pass. An existing owner could already have
claimed Y. The second pass correctly skipped the inflight obligation, but the
test incorrectly interpreted that empty pass's return as Y's settlement.

The revised test arms Y before releasing admission. The existing cfg(test)
proof-selection seam publishes `state_version` after taking the exact seam.
The test observes pending + inflight + consumed seam while its physical lock is
still held, confirms B remains waiting, then explicitly unlocks Y. It watches
that same owner's pending/inflight removal before awaiting B's captured result.
It does not call `reconcile_recovered_settlements()` for Y. X stays pending,
blocks Goal idle, and is released only after B completes. The no-fake-admission
assertion remains. The five-second timeout is a deadlock guard.

Only test synchronization changed. Production claim exclusion, proof acquisition,
pre-Reserved ownership, `wait_recovery_settlement_for`, and registry-wide
reconciliation execution are unchanged.

## Interactive pre-anchor evidence

The temporary diagnostic transports from this investigation have been removed.
The regression now uses only semantic fault controls, received supervisor failure
frames, control EOF, direct-child reap and explicit unproven settlement. See
[the control-plane repair](pr-416-control-plane-pre-anchor.md) for the final design.

## Stopped anchor: native observation before cancellation

The basic regression now orders command readiness -> successful SIGSTOP ->
outer AnchorStopObserved -> cancellation -> release of a private fixture gate.
The gate is owned by cfg(test) `RunnerTestControl`, never ToolEnvironment, and
uses `/tmp/rx-stop-<random>/gate`. No second diagnostic framework was introduced.

The existing bounded trace gains AnchorStopObserved, AnchorUnwedgeKillAttempt
(zero or errno), AnchorTerminalObserved, GroupChildrenReaped and Darwin
GroupAbsenceProven. An unexpected nonterminal WEXITED result is recorded once.
The existing fallback, signal, publication, observation and reap evidence stays
intact. The trace remains limited to 32 entries per owner, contains no command,
output or environment values, and does not establish lifecycle truth.

The test asserts outer transition order and runner observation-before-reap.
Publication logging occurs after the actual frame write, so it deliberately does
not impose a false cross-process append order between publication and receipt.
Failure includes the native trace. The native stopped-observation test now runs
the outer's WEXITED|WNOHANG|WNOWAIT observation before asserting that WSTOPPED
still observes the same retained stop and that the terminal status remains
available to its reaping owner.

Linux observation: stop observed -> PID SIGKILL returns zero -> retained anchor
terminal -> fallback group SIGKILL -> group child reap -> terminal publication /
receipt -> direct-child reap. WEXITED on the proven stopped child returns
StillAlive on Linux. No Darwin sequence is inferred from this evidence.
The prior Darwin timeout's root cause remains unclassified until native results
are available; no speculative change was made to waitid interpretation,
containment, TERM/grace/KILL, group proof or reaping policy.

## Preserved architecture

No deadlines or existing proof assertions were weakened. No sleeps, retries,
broad-suite serialization or protocol changes were added. App Server v27,
Runtime Client incremental projection, Jobs/Agents, exact activation identity,
physical continuation's typed authority and destructive-deletion proof remain
unchanged. Production command environment is still isolated from supervisor
controls. TUI, Web and generated protocol inputs are untouched.

## Validation and delivery

All local validation below ran on Linux with normal suite concurrency.

| Command | Result |
| --- | --- |
| `cargo build --bins` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `git diff --check` and `git diff --cached --check` | Passed |
| `cargo test --lib --all-features <filter> -- --nocapture` for each library filter below | All then-current focused tests passed, one selected test each |
| `cargo test --all-features --test tools bash::<filter> -- --exact --nocapture` for each external filter below | All 5 passed |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | 3,212 passed; 3 existing opt-in tests ignored |
| `cargo test --lib --all-features -- boundary_suites::` | 194 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | durable 129, process 63, subagent 45, tools 133, conformance 22, CFG 26 + 5; all passed |
| `cargo test --test contracts --test provider --all-features` | contracts 28; provider 166 passed, 5 live opt-in tests ignored |

Library filters:

- `pre_reserved_wait_settles_independently_of_another_agents_recovery`
- `recovered_verification_before_reserved_retains_physical_exclusion_after_parent_drop`
- `pre_reserved_allocation_blocks_destructive_deletion_until_exact_proof`
- `parked_recovery_probe_does_not_hold_registry_mutex`
- `reconciliation_owner_cancellation_releases_completion_without_physical_proof`
- `unprovable_pre_anchor_reap_never_settles_physically`
- `setsid_failure_before_the_anchor_settles_by_direct_pid_reap`
- `command_environment_cannot_configure_interactive_supervisor`
- `post_spawn_handshake_failure_settles_without_stranding`
- `stopped_anchor_supervisor_is_contained_by_the_outer`
- `stopped_observation_preserves_stop_and_terminal_status_for_owner`

External filters:

- `bash_background_cancellation_uses_the_same_process_group_path`
- `background_cancel_records_term_before_trap_and_physical_terminal`
- `bash_kill_escalates_when_term_is_ignored`
- `ordinary_environment_cannot_arm_supervisor_term_gate`
- `ordinary_environment_cannot_enable_supervisor_controls`

The deterministic and boundary commands include the full relevant in-crate
subagent, interactive, process and Bash groups. The full tools target also passed
the MCP routing-header regression; no MCP code was changed.

Client/protocol inputs and shared types are unchanged, so TUI/Web/protocol
commands were not rerun locally for this narrow repair. Their required hosted
jobs will run on the final pushed SHA. Darwin-only `macos_waitid_tests` and the
Darwin side of the common native regression cannot execute on this Linux host.

No local validation command failed or was interrupted. One intermediate build
emitted two unused-variable warnings after an editing mistake temporarily removed
unrelated fixture PID writes; the writes were restored before focused validation.
The deterministic suite printed an existing archive-cancellation cleanup diagnostic
(`ZipWriter::drop ... archive cancelled`) while all selected tests passed. No
retry-until-green runs were used. Initial exploratory focused recovery, pre-anchor
and stopped-anchor runs also passed; source changes then triggered final validation.

At the start, hosted run `36363800065` had exactly the three reviewed failures
(recovery on Linux and Darwin; pre-anchor and stopped-anchor on Darwin). This
report does not reinterpret those failed runs as successes. The final pushed SHA
and the single post-push CI snapshot are reported in the delivery message. No
merge-readiness claim is made without final hosted Linux and macOS success.

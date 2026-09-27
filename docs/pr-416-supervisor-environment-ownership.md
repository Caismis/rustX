# PR #416: supervisor environment ownership

Starting head: `d17be2bf6797a59c1c9e1885b70a6f5ca8fda5b9`.
Base inspected: `ad863a24cf48fbb0d5182746e1046d4b64e8c167`.

## Invariant and repair

ToolEnvironment configures the executed tool process. It must never configure
rustX supervisor ownership, cancellation, settlement, tracing, fault injection,
or control-plane behavior.

Previously `SupervisedCommandSpec.environment` was installed directly in the
outer supervisor, inherited by the inner, and also searched by the runner for
its trace destination. Consequently ordinary authorized entries could activate
the blocking before-TERM gate, tracing, and older failure seams. Interactive
supervision had the same direct forwarding pattern.

`SupervisedCommandSpec.command_environment` now carries only child data. Both
runner spawn paths clear the supervisor environment and encode child entries
in `RUSTX_COMMAND_ENVIRONMENT`. Only the final Bash/server launch decodes that
payload and applies it after `env_clear()`. Payload keys never become supervisor
environment keys. No name filter, fallback, global reservation, or ambient
configuration lookup is involved. Malformed/missing transport refuses launch.

Supervisor controls come exclusively from explicit runtime setup or the existing
test control objects. `RunnerTestControl` owns `FixtureControl` for bounded trace
and before-TERM diagnostics. The external tests select `fixture_executor`
explicitly; normal native registration always constructs `BashTool::new()` with
no control. Pure runner fault fields remain `cfg(test)`. Interactive fixture
controls likewise live in `InteractiveTestControl` under `cfg(test)`.

Option A is intentional: every same-named ordinary entry remains visible in Bash
or the interactive child, including the transport key itself. It cannot shadow
the supervisor's private value. The six existing ToolEnvironment reserved
baseline/overlay keys are unchanged.

## Audited namespace

| Keys | Owner / authority |
| --- | --- |
| `RUSTX_SUPERVISOR_ROLE`, `RUSTX_SUPERVISOR_COMMAND` | Runtime-private Bash supervisor configuration |
| `RUSTX_COMMAND_ENVIRONMENT` | Runtime-private opaque command-data transport; decoded only for child spawn |
| `RUSTX_SUPERVISOR_ANCHOR_PID_FILE` | Test-only Bash observation |
| `RUSTX_TEST_FAIL_SIGNAL`, `RUSTX_TEST_FAIL_WAIT`, `RUSTX_TEST_FAIL_BASH_SPAWN`, `RUSTX_TEST_FAIL_SIGTERM_HANDLER`, `RUSTX_TEST_FORCE_ANCHOR_LOSS`, `RUSTX_TEST_OUTER_BARRIER_DIR`, `RUSTX_TEST_FAIL_CONTAINMENT` | Test-only Bash supervisor fault/gate controls |
| `RUSTX_TEST_SUPERVISION_TRACE`, `RUSTX_TEST_BEFORE_TERM_SOCKET` | Explicit trusted diagnostic fixture controls; compiled for external integration tests |
| `RUSTX_PHYSICAL_CONTINUATION` | Physical authority transport: runtime-authored metadata plus the inherited typed continuation descriptor; only the outer receives the authority |
| `RUSTX_INTERACTIVE_CONTROL`, `RUSTX_INTERACTIVE_INNER_CONTROL` | Runtime-private interactive supervisor sockets |
| `RUSTX_INTERACTIVE_ANCHOR_PID_FILE` | Test-only interactive observation |
| `RUSTX_TEST_INTERACTIVE_OUTER_FAIL`, `RUSTX_TEST_INTERACTIVE_FAIL_SERVER_SPAWN`, `RUSTX_TEST_INTERACTIVE_FAIL_SIGNAL`, `RUSTX_TEST_INTERACTIVE_FAIL_SIGTERM`, `RUSTX_TEST_INTERACTIVE_INNER_EXIT_BEFORE_CONNECT`, `RUSTX_TEST_INTERACTIVE_FAIL_SETSID`, `RUSTX_TEST_INTERACTIVE_INNER_STALL_BEFORE_ANCHOR`, `RUSTX_TEST_INTERACTIVE_FAIL_PREANCHOR_REAP` | Test-only interactive supervisor fault/gate controls |

All user-supplied occurrences of the above names are ordinary command data,
never any of these private capabilities. Shared `supervised_unit` consumes only
the new data transport. Nested containment uses its typed process-global
`NestedAnchorAuthority`/`AnchorGate`, not environment keys. Physical continuation
setup still comes from `ParentPhysicalContinuation`, including its inherited
file; no command data can supply that descriptor. The outer removes private
continuation metadata before launching the inner and does not pass its authority
file to the command. Child data containing that spelling confers no authority.

## Deterministic coverage

- `ordinary_environment_cannot_arm_supervisor_term_gate`: supplies a live Unix
  listener path through authorized ToolEnvironment. Bash checks the exact value,
  installs its TERM trap, and emits readiness through a FIFO. Cancellation must
  enter the trap and physically settle; nonblocking accept then proves no gate
  connection was ever queued. The timeout is only a deadlock guard.
- `ordinary_environment_cannot_enable_supervisor_controls`: supplies all Bash
  control/fault/authority/transport names; successful child checks prove ordinary
  visibility, and absent trace/anchor files prove no diagnostic activation.
- `background_cancel_records_term_before_trap_and_physical_terminal`: retains
  the trusted pre-TERM gate and native signal/terminal/reap assertions, now with
  conflicting ordinary trace/gate values to prove private controls cannot be
  shadowed. The untrusted trace path stays absent.
- `command_environment_cannot_configure_interactive_supervisor`: supplies every
  interactive control name plus transport and physical-continuation collisions;
  the real child sees them and the unit settles normally.
- Existing TERM-ignore escalation and background cancellation trace regressions
  now select the trusted fixture executor explicitly.

All existing bounded diagnostic events remain available, with 32 entries per
owner, no command/output/environment dumps, no diagnostic lifecycle truth, and
the grace deadline still anchored to the signal boundary. The cancellation
ladder, signal ordering, TERM grace, physical proof, direct-child reap, Linux
membership restriction, and macOS fallback are unchanged. Exact-activation
recovery and pre-Reserved allocation ownership are untouched. No protocol,
generated contract, Jobs/Agents behavior, or UI changes are required.

## Changed code paths

- `src/runtime/process_runner.rs`: command-only spec, private spawn environment,
  runner-owned fixture diagnostics, typed physical continuation unchanged.
- `src/runtime/supervised_unit.rs`: shared command-environment payload decoder.
- `src/tools/native/bash/supervisor.rs`: apply explicit child environment only at
  Bash spawn; update direct supervisor fixtures for the required payload.
- `src/tools/native/bash/supervisor/diagnostics.rs`: explicit fixture control and
  external fixture executor factory; existing evidence/gate implementation retained.
- `src/tools/native/bash/executor.rs`: thread the trusted runner control through
  the existing Bash seam; ordinary constructor leaves it absent.
- `src/runtime/interactive_process.rs`, `src/runtime/interactive_supervisor.rs`:
  isolate server environment identically; retain explicit private test controls.
- `src/tools/environment.rs`: document the invariant; acceptance rules unchanged.
- `src/runtime/workspace.rs`, `src/runtime/workspace/git_output.rs`,
  `src/runtime/nested_containment.rs`, `src/skills/environments.rs`,
  `src/tools/python.rs`: mechanical command-environment field rename only.
- `tests/tools/bash.rs`: adapt three diagnostic fixtures and add two isolation
  regressions, including child visibility and signal-fault collisions.

## Local validation

All listed final checks passed on Linux, with normal test concurrency:

| Command | Result |
| --- | --- |
| `cargo test --all-features --test tools bash::background_cancel_records_term_before_trap_and_physical_terminal -- --exact --nocapture` | 1 passed |
| `cargo test --all-features --test tools bash::bash_background_cancellation_uses_the_same_process_group_path -- --exact --nocapture` | 1 passed |
| `cargo test --all-features --test tools bash::bash_kill_escalates_when_term_is_ignored -- --exact --nocapture` | 1 passed |
| `cargo test --all-features --test tools bash::ordinary_environment_ -- --nocapture` | 2 passed; final strengthened cases also passed in the full tools suite |
| `cargo test --lib --all-features tools::native::bash::tests` | 37 passed |
| `cargo test --all-features --test tools` | 133 passed |
| `cargo test --all-features --test process` | 62 passed |
| `cargo test --lib --all-features pre_reserved_wait_settles_independently_of_another_agents_recovery -- --nocapture` | 1 passed |
| `cargo test --lib --all-features command_environment_cannot_configure_interactive_supervisor -- --nocapture` | 1 passed |
| `cargo test --lib --all-features runtime::interactive_process::` | 20 passed |
| `cargo test --lib --all-features runtime::process_runner::` | 12 passed, including typed physical continuation |
| `cargo test --lib --all-features tools::native::bash::supervisor::` | 5 passed |
| `cargo test --lib --all-features runtime::interactive_supervisor::` | 2 passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo build --bins` | Passed |
| `git diff --check` and `git diff --cached --check` | Passed |

No shared protocol types or generated inputs changed; protocol/client generation
checks and a version bump are not needed for this repair.

Development diagnostics: two intermediate Clippy invocations failed. The first
reported function length, explicit default construction, documentation markup,
semicolon placement, import placement, and test string construction; the second
reported a fixture path passed by value. All findings were repaired without lint
suppression. An additional supervisor test filter using the public re-export
matched zero tests; the corrected module-path run passed five tests. No behavioral
test failed and no test was interrupted.

Starting-SHA hosted CI was fully green, including macOS. Local Linux validation
does not replace required hosted macOS validation of the pushed repair SHA. This
report does not claim merge readiness; no merge or auto-merge is authorized.

# PR #416: macOS pre-anchor regression root cause

Failing test: `runtime::interactive_process::interactive_tests::unprovable_pre_anchor_reap_never_settles_physically`
(macOS only, every run from `a9bbbc5f` through `8fe8a0f7`).

## Evidence

Passive trace run `36413079811` (commit `0b829c0f`) showed:

```
[trace inner] entered run_inner
[trace inner] control socket connected
[trace inner] control socket nonblocking
[trace outer] inner control accepted
```

There was never a `FAIL_SETSID enabled` line. `ps` showed both supervisors
sleeping (`S`). `sample` showed the outer polling in `await_anchor_commit` and
the inner inside `run_inner -> std::thread::sleep`. The only sleep loop in
`run_inner` before the FAIL_SETSID branch is the `INNER_STALL_BEFORE_ANCHOR`
fixture. The macOS build log also contains rustc's `unreachable pattern`
warning ("you might have meant to pattern match against the value of constant
`INNER_STALL_BEFORE_ANCHOR_ENV`").

## Root cause

The test fixture dispatched controls by environment-variable name:

```rust
match key.as_str() {
    FAIL_SETSID_ENV => control.fail_setsid = Some(value),
    INNER_STALL_BEFORE_ANCHOR_ENV => control.inner_stall_before_anchor = Some(value),
    FAIL_PRE_ANCHOR_REAP_ENV => control.fail_pre_anchor_reap = true,
    _ => panic!(..),
}
```

The `INNER_STALL_BEFORE_ANCHOR_ENV` import was `#[cfg(target_os = "linux")]`.
On macOS that name was not in scope, so the arm was a catch-all variable
binding, not a constant pattern. `(FAIL_PRE_ANCHOR_REAP_ENV, "1")` therefore
armed the inner stall (with pid file `1`), and the reap theft was never armed.
The inner connected and then slept forever. No frame was ever written, so the
driver observed only `owner_attached`.

Reproduced on Linux by removing the import: the test failed with the identical
signature (driver `["owner_attached"]`, no pid file, outer alive).

This is not a supervisor lifecycle or wire defect. The complete-frame writer
fix (`8fe8a0f7`) repaired a real partial-write bug, but it could not affect
this failure, because the inner never reached any frame write.

## Repair

- The fixture no longer translates env-var names back into controls. Tests
  set the typed `InteractiveTestControl` fields directly, and
  `configure_supervisor` is the single field-to-environment mapping. A
  platform-gated constant can no longer turn into a catch-all arm.
- `fixture_controls_configure_exactly_the_selected_supervisor_seams` pins
  the proof-theft controls to exactly `FAIL_SETSID` + `FAIL_PREANCHOR_REAP`
  on every platform.
- Fixture pid-file writes report failure on stderr instead of discarding
  it, so "did not execute" and "executed but failed" are distinguishable.
- macOS `proc_state` queries `ps` instead of the absent `/proc`. Previously
  every `wait_for_reaped` assertion passed vacuously on macOS.
- The passive trace (`RUSTX_TEST_INTERACTIVE_TRACE`, test-control only) and
  the timeout `ps`/`sample` snapshot remain for future stalls.

Pre-anchor semantics are unchanged. The inner pid is only a direct-child
identity, no group-scoped proof exists before `AnchorReady`, and
`NoOwnership(pid)` follows only a proven direct reap. An unprovable reap
publishes `TerminalityUnproven`.

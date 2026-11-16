# PR #485 review follow-up (2026-11-16)

The reviewed baseline is `3c7825d7f4d47b742ca2238b9c0d399bcbf7a3e9`, based on
main `d82d7f9b8d9dd4f5064dba8d377103c8c6049d28`. This follow-up addresses the
three P2 findings. It does not rerun comparative performance measurements or
change the accepted final-settlement, request-sharing or delta contracts.

## Imported Git index authority

A real index can hold a clean-filtered blob whose cached stat exactly matches
the worktree. Our private Git deliberately cannot execute the filter. Trusting
that stat therefore gave the two comparison endpoints different semantics.
The new regression stages the filtered blob, fixes the worktree mtime before
staging, and proves a second real `git add` does not execute the filter. The
pre-fix test harness returns `FILTERED before` instead of `original before`.

Before producing the first baseline, normalize the private Repository's ordinary existing tracked
entries under the private filter-free configuration. Git `add --renormalize`
is the explicit reprocessing boundary. Do not apply it blindly to the entire
index: it clears assume-unchanged. `ls-files -v -z` selects only ordinary `H`
entries; sparse/skip-worktree and assume-unchanged remain outside that set.
An ordinary private add first reconciles missing paths and directory/symlink
replacements using Git itself. The private
scratch namespace stays excluded. NUL-delimited literal pathspecs preserve
filename boundaries; inventory streaming avoids a new whole-repository memory
limit. The user's index, objects, refs and files remain untouched. Normalization holds
the existing physical snapshot lease across all subprocesses; only success
marks the private index ready. Cancellation leaves it retryable after child
reaping and lock cleanup, tested at both add and renormalize boundaries.

This pays a one-time private add, inventory and content-normalization cost when importing
an index. It deliberately re-reads ordinary tracked content even if its stat
is clean: stat does not authenticate the original filter authority. Subsequent
text/read turns retain the incremental index path, with explicit command-count
and unchanged-object regressions. Git built-in EOL/encoding normalization and
the already documented dependency on alternate objects remain unchanged.

## Recovery oracle compilation

Module, test export, frontier clone and full-Journal comparison now share
`cfg(test)`. Both debug and release library tests execute the same oracle,
including the two existing `caught_by_the_debug_oracle` panic regressions.
Normal debug and release binaries perform bounded frontier recovery without
this test-only full-history fold. Current-row authentication and corruption
checks in the actual recovery path remain intact. Work-count tests suspend
only the independent test oracle while measuring the normal runtime path.

## Turn-growth fixture contract

Read and Bash use the current required `arguments` envelope; Bash keeps
`execution_mode` at the runtime envelope level. Only the isolated Git mutation
fixture uses top-level `approval_mode = "full_access"`. No obsolete native-tool
approval overlay remains. Fixture Git commits disable detached maintenance so
that later whole-directory identity checks observe only intentional writes.

The provider oracle validates every conversational message in order, including
historical Assistant text, exact tool call IDs/names/envelopes and result IDs.
The final canonical ledger must contain successful native tool results. The
mutation case verifies physical modify/create/delete effects as well as the
change card and before/after hunks. Negative tests reject dropped/changed old
Assistant messages, changed call/result IDs, reordered history, flat or wrong
arguments, preflight failures and extra system messages. Native system/tool
inputs are not claimed equivalent to another product's inputs.

## Validation

The first complete workspace run caught missing physical-lease ownership in
initialization. The retained implementation moves normalization into the first
snapshot under the existing gate; both cancellation phases now pass, with no
weakened assertions or deadlines.

| Check | Result |
| --- | --- |
| Pre-fix filtered-index regression | Expected failure: `FILTERED before` instead of `original before` |
| Workspace Changes, all features | 80 passed, one intentional ignore |
| Recovery frontier | 11 passed |
| Bounded cold attachment across history sizes | 1 passed |
| Benchmark Python contracts | 59 passed, including 10 new oracle cases |
| Request comparator / Pi observer / Pi oracle Node contracts | 36 passed; no reference product execution |
| Real turn-growth text / Read | Two turns each; provider, canonical success, close and reopen checks pass |
| Real Git mutation fixture | Two turns; Read then Bash; physical effects, exact change hunks, unchanged real Git and reopen checks pass |
| Additional real Write + Bash filtered-index smoke | Both tool results successful; both before/after endpoints exact; real Git unchanged; no snapshot filter execution |
| Default workspace/all-target Clippy, warnings denied | Passed |

The production changes are independently reviewable:

- `3b11be54133fb6d9738824f2ebeb113124cf917d`: private-index initialization and regressions.
- `dd96ceb5409ca2777f2193dcbddeea12ae0d7f9e`: consistent test-only recovery oracle.
- `46b27514011df1f60c13d7ade3a0020b18a2b0ba`: tool fixture and ordered-history oracle.

The [PR validation section](https://github.com/Caismis/rustX/pull/485) records
completion of the longer release build, final local suite and exact-head FULL
CI. The table above reports targeted checks completed before submission; it
is not an assertion that a subsequently running CI job has passed.
The release regression commands are:

```sh
cargo test --release --locked --lib --no-run
cargo test --release --locked --lib caught_by_the_debug_oracle
```

The two turn-growth smoke invocations use Python 3.12 through the existing fake
provider environment with `websockets==10.4`, `--skip-build`, the current debug
App Server, `--turns 2 --window 1 --reopen 2`, and separate output directories.
The ordinary invocation selects `--scenarios text tool`; the mutation invocation
selects `--scenarios tool --git-workspace --git-workspace-mutations`.
The supplemental filtered-index smoke invokes actual native Write and Bash,
then reads Changes after the thread is idle, when the optional observation has
settled. It supplements the permanent in-crate regression for native
before-image capture and shell-equivalent mutations.

Smoke resource counters are diagnostic only: debug binaries and concurrent
builds make these runs unsuitable for performance conclusions. Pi and DeepSeek
are not run.

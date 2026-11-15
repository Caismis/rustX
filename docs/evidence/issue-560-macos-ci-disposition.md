# Issue #560 macOS CI correction and diagnostics

## Failed candidate

The first published candidate was
`263e395849398a9c3f7aa733ccdfbcdfbdbf2d7a`. Its
[FULL run](https://github.com/Caismis/rustX/actions/runs/43719839514) tested merge
`1b68d41839e2f1b1f2d4281d63cfb6bd02c18a02` against base
`33234e5b19f9074924a5fba1315c23dcf7210c32`.
The [macOS full job](https://github.com/Caismis/rustX/actions/runs/43719839514/job/132829450572)
reported **4389 passed, 2 failed, 20 ignored**. Both failures were the new
Commands lock-order fixtures waiting for READY. Compilation had completed;
the linker warning was not the failing assertion. Merge verification correctly
rejected the run. The other 14 jobs succeeded, including complete browser
coverage (775 discovered, 773 passed, 2 intentional skips). Those results remain
historical evidence for that candidate.

The local failed-job log is `ci-2004-macos-full-failed.log`, 800908 bytes,
SHA-256 `92a81b761e6713f9ee30ca6c8799336d9a97662df61b83c29a1aaa04f032436f`.
It is a local receipt, not a hosted artifact download.

## Fixture correction and proof preservation

Native `Environment` admits roots through `PackageDirectory` descriptor opens
with `NOFOLLOW` on every component. A macOS temporary path can traverse the
`/var` to `/private/var` alias. The original fixtures granted that raw spelling
and reused it as cwd; an early directory refusal could be hidden by the READY
watchdog. This is a source-backed diagnosis of the old timeout: its original
early RPC error was not logged.

The revised fixture deliberately creates a directory alias on every platform,
canonicalizes it once for both grant and cwd, and checks the real native grant
before starting work. Removing canonicalization reproduces the grant refusal
immediately on Linux. The test retains actual `Commands::start`, the held watch
borrow, exact active identity and nonblocking State-lock witness.

READY observation now races the actual command result. Errors cancel the
existing Events scope, and command ownership settles before assertions. The
READY bytes, termination result, terminal status and cleanup preconditions are
asserted before the State/watch or admission-observer property, so the intended
negative-control assertion cannot mask a different native fixture failure. The
first test removes its redundant unbounded admission wait; the second still
proves the admission observer is Pending before start and wakes after publication.
A new ordinary test leaves a real root ungranted and requires the exact
`EnvironmentUnauthorized` result, a consumed native settlement and an empty
active map. Observer/READY watchdogs remain 5 seconds and command deadlines
remain 10 seconds; no output-batching or production admission policy changes.

The serial negative controls select exactly one test each:

- Removing canonicalization fails at the native grant precondition (exit 101,
  0 passed / 1 failed, test body 0.00s).
- Omitting the actual production `drop(state)` fails the intended State/watch
  assertion after real command settlement (exit 101, 0 passed / 1 failed,
  test body 0.03s), rather than failing on READY or hanging.

Both controls restore exact original bytes in `finally`; the production
`command.rs` restores to SHA-256
`739072fb8258db1332dbe817c3826e561d718b0c32ddfe5ab439cfb608a69d15`.
Commands tests execute in Linux `rust-contracts` (and the applicable FAST
App Server selection) and macOS `rust-platform-boundaries` through `--lib`.
The new ordinary compiled namespace is
`app_server::command::tests::readiness_observes_ungranted_root_failure_and_settles_before_assertions`.

## First-party warnings repaired

| Diagnostic | Actual owner and correction | Preserved boundary |
| --- | --- | --- |
| Unused `super` import in `resource_directory.rs` | Remove it from the macOS-only case-insensitive test module; its references already have explicit crate paths. | The native case-insensitive volume test remains. |
| `seed_benchmark_sessions` unused on macOS | Gate the fixture helper on `test` + Linux, matching its sole caller in the existing Linux-only scheduler benchmark. | Normal catalog commit/durability and benchmark isolation are unchanged. |
| `RunnerLifecycleHook::proceed_tx`, `await_shell_exit` and `release` unused on macOS | Gate the complete shell-exit pause sub-boundary on Linux tests: fields, channels, initialization, methods, control flag and producer call match the existing Linux-only consumers. | Ownership, terminal and direct-child-reap hooks remain cross-platform; native macOS group absence is still required. |

No blanket warning suppression, production fallback or public API expansion is
introduced. Native macOS compilation must confirm the repaired warning scope.

## Native and upstream warnings retained

Apple `ld`'s `__eh_frame` warning concerns the compact-unwind DWARF-offset
encoding limit: [Apple defines a 24-bit offset](https://github.com/apple-oss-distributions/libunwind/blob/main/libunwind/include/mach-o/compact_unwind_encoding.h),
approximately 16 MiB. Rust forwards the diagnostic through
[`linker_messages`](https://doc.rust-lang.org/rustc/lints/listing/warn-by-default.html#linker-messages).
The same warning appears in the Rust 1.98.1
[baseline macOS job](https://github.com/Caismis/rustX/actions/runs/43659074269/job/132644155483).
Repeated printouts include Cargo diagnostic replay and do not count distinct
fresh links or oversized sections. Actual section size, unwind-path performance
and exhaustive unwind correctness were not measured. The warning remains
visible; unwind generation and panic policy are preserved.

Actions' forced Node 20-to-24 notices and `punycode` / `url.parse` deprecations
come from the action dependencies. Repository Node inputs remain pinned to
24.22.0. Expected negative CLI-argument diagnostics and archive-cancellation
stderr belong to passing behavioral tests and are not additional build failures.

## Evidence scope and acceptance

The [portable measurements](issue-560-measurements.json) retain the failed-run
identity and local positive/negative commands, statuses and log hashes. The
correction changes Git's `src` tree through test bodies and test-only `cfg`.
Earlier timings retain their actual frozen `6cb5a60d` / `053df860` source and
artifact identities. No old binary or measured HEAD is retargeted, and no new
performance saving is claimed for this correction. Ordinary non-test execution
logic is unchanged; revised test/build inputs receive fresh functional checks.

After exact restoration, local checks pass all 7 Commands tests, all 51 native
Bash tests, strict default and all-feature Clippy, and 31 documentation/contract
tests. Linux lane discovery, strict rustdoc, formatting and the full PR diff
checks also pass. These Linux checks do not establish native macOS compilation
or execution success.

Final acceptance still requires independent review and the configured FULL
workflow, every selected job and Merge verification at the PR's current HEAD.
Their exact run links and final source identity are recorded in PR metadata.

# PR #485: current-round closeout and main integration

## Scope and identities

This closes the current optimization round and integrates main into the PR branch. It does not merge PR #485, start another optimization project, or rerun any Pi, pi-rust or DeepSeek product. Keep the PR Draft. Remote CI is not awaited.

The merge parents are:

- PR: `4c68834dc9311dc0515310f2a0f9c07a2078675a` (the frozen optimization report and evidence).
- main: `d80b34fb9d5586a12cdd7f51ac50ee2c17a073b0`.
- Common ancestor: `8bf2f5cc1c68d6d05e26f752f2a8743f89853230`.

The merged Rust source tree tested locally is `7e4b4834d0a9a9b35657d27e8485c6affbce55e7`. The evidence records source/build identities independently of the merge commit's documentation. Tests use fresh binaries built from this merged tree, not the pre-integration release binaries. Rust 1.98.1, Node 24.22.0 and pnpm 11.13.1 were used on Linux. Other developers' worktrees and all reference repositories were left unchanged.

## Optimization result being closed

The [frozen round report](2026-11-10-replay-layout.md) measures production commit `86bf61a3050dbf85818386d7242d141c59785439`, before this merge. Its figures are not measurements of the merged source.

Six large optional RuntimeClientEvent payload fields move behind Box. The common event slot falls from 552 to 400 bytes; payload ownership, replay limits, old cuts, SQL, commit boundaries and JSON remain unchanged. The diagnostic reachable-owner census includes the new Boxes and falls by approximately 4.6/9.1/18.2 MiB at 128 sessions with 8/16/32 turns. This partial census is not a partition of process PSS.

Four balanced clean ordinary capacity pairs show:

| Sampled anonymous PSS | Before median, MiB | After median, MiB | Median paired change |
|---|---:|---:|---:|
| 128 subscribed empty sessions | 338.148 | 341.891 | +0.550% |
| 128 sessions, 8 turns each | 656.945 | 654.576 | -0.501% |
| 128 sessions, 32 turns each | 1123.346 | 1106.398 | -1.599% |
| Same-process growth from 8 to 32 turns | 465.178 | 454.006 | -2.482% |

The growth decreases in all four pairs, but populated endpoints decrease in only three. Empty-session PSS increases in all four. Seed CPU and process writes are approximately unchanged. The original short-history performance matrix remains adverse (paired P50 +42.72%, CPU +25.48%); shared-machine follow-ups do not prove the cause or establish a fix. The fixed-four supplement also overlaps external compilation and cannot isolate the layout effect. All negative and excluded samples remain archived.

The round decision remains **Accept with notes for the local representation change; Revise before merge for the PR**. There is no claimed general speedup, solved CPU regression or reduced write amplification. The user subsequently ended the broader optimization goal; the next action in this task is integration, not another performance campaign.

## Conflict resolution

Main was merged into the PR branch without rebasing, force-pushing or replacing its history. There were 514 unmerged paths; most were protocol-version conflicts (462 files, 662 hunks). Resolution did not select one side wholesale.

The single current contract is App Server v78, Runtime Client v85, SQLite schema 106, Session Catalog 19 and manifest 5. Rust regenerates the v78 schema and fixtures, and the Node generator regenerates the TypeScript types. Earlier generated protocol files are removed. All active clients and new main fixtures use v78; there is no legacy decoder, migration or compatibility path. Historical release notes remain historical. The documentation explicitly rejects v77 and earlier.

Main's model/provider settings, resource editing, skill discovery and configuration changes are retained. Where main deleted/reorganized old settings files, the PR-only change in those obsolete files was an import-version change; that change is applied to the replacement files rather than restoring the deleted implementation. Trace refresh and read-cut behavior from the PR remains present. Session-latency fixtures retain both main's thread-status recovery predicate and the PR's superseded-Trace predicate.

The provider translation conflicts required semantic integration:

- Preserve the PR's typed borrowing and shared immutable payloads rather than restoring a whole-request JSON tree.
- Apply main's exact native-priority, bounded and sanitized correlation-label plan directly to typed wire fields. Stored continuation reserves native boundary identities while omitting their already-owned calls from transmission.
- Retain origin-aware native replay. Readable foreign reasoning/refusal becomes ordinary text where specified; opaque foreign provider state is not replayed as native state.
- Stateless Responses continuation replays full earlier history with its accepted native output. Stored continuation retains its narrower boundary and previous_response_id.
- Synthesized Responses assistant messages have completed status, collision-safe deterministic message IDs and output_text annotations; user input retains its distinct representation. Accepted native bundles remain exact.

New main fixtures use the PR's shared result/container types and new replay-origin fields. Tests were adapted to the merged representation; new main coverage was not deleted to obtain a passing build. Storage production code and the compact event layout remain the PR implementation.

## Local correctness validation

The final merged Rust source, protocol and clients pass the following local checks. The successful all-target suite is collected in two segments: Cargo stopped at a documentation contract failure in its first execution; after the prose-only correction, that target and every remaining target were run. Already successful library/conformance targets were not counted twice.

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| Fresh all-feature native bins and protocol generator | Pass; binary hashes retained |
| Rust all-target/all-feature tests | 6,564 passed; 33 ignored |
| Benchmark/controller Python unit tests | 48 passed |
| CI-planning Python unit tests | 64 passed |
| Protocol TypeScript typecheck | Pass |
| Protocol default-feature regeneration (`pnpm check`) | Pass; no generated diff |
| Rust documentation tests | 9 compile-fail examples passed |
| Web TypeScript typecheck | Pass |
| TUI TypeScript typecheck | Pass |
| Web Vitest | 4,299 passed; 4 skipped |
| TUI tests | 1,352 passed; 1 skipped |
| Web production build, including native host addons | Pass; existing large-chunk warnings retained |
| Web source provenance/notices/provider icons | Pass; 349 source records |
| Web i18n | Pass |

The Rust suite includes 5,836 library tests, 30 catalog, 2 managed-output, 46 conformance, 31 documentation/contracts, 151 durable, 68 process, 194 provider, 45 subagent, 134 tools and 27 example tests. The ignored tests are reported rather than counted as passes. Initial compilation failures, the missing Trace refresh import caught by TypeScript, and the initial documentation failure are retained with the successful results. The documentation correction does not change the tested Rust source tree.

The main command sequence used the isolated target directory `/home/capi/.cache/rustx-pr485-main-integration-target`, `CARGO_BUILD_JOBS=8` and fresh sibling CLI/supervisor executables. Runtime-backed Node tests set RUSTX_BINARY to that tree's debug/rustx. The Rust execution commands were:

```sh
cargo test --locked --all-targets --all-features -- --test-threads=8
cargo test --locked --all-features \
  --test contracts --test durable --test process --test provider \
  --test subagent --test tools --examples -- --test-threads=8
```

Protocol regeneration and Rust documentation-test results are recorded in the [validation summary](evidence/main-integration-20261110/validation.json). The [evidence archive](evidence/main-integration-20261110/main-integration.tar.xz) retains merge decisions, failed and successful check logs, source/build identity and test counts. Its [index](evidence/main-integration-20261110/archive-index.json) lists members and hashes. Executables, dependency trees and live runtime roots are excluded.

## Limits and handoff

No new performance, capacity, WAL or reference-product comparison is inferred from these correctness runs. Existing pre-integration performance data retain their exact original identities. No browser screenshot/E2E, macOS or completed remote-CI result is claimed. Required remote checks must be assessed on the actual pushed merge HEAD before a later merge decision.

The requested main integration is complete once the merge is committed and pushed. PR #485 stays Draft and is not merged. Subsequent CPU/write work, if requested, should use a separately controlled measurement; it is outside this closeout.

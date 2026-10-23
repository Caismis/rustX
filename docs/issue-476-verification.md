# Issue #476 verification

## Baseline and delivery

- Starting `origin/main`: `5c8be3af0fe08c0fb35c5553901a32d6cf17b9d1`.
- Integrated prerequisites: #473/#486 `0f3d978e`, #474/#490 `df06c307`,
  #475/#493 `5c8be3af`.
- Branch: `codex/issue-476-auxiliary-delivery`; dedicated managed worktree
  `/home/capi/.codex/worktrees/issue-476/rustX`. The original checkout is untouched.
- Final commit and exact-head CI are recorded in the PR. No merge or auto-merge.

The native consumer, submission permit, result/copy owner and observer queue are
specified in [auxiliary-consumers.md](auxiliary-consumers.md), including lock and
commit points, every payload component, concurrent copies and maintenance limits.
App Server publication reuses DeliveryAccess and registration/Writer acceptance.
The Web client owns one visible observer and one read, never execution authority.

Breaking versions: App Server/WebSocket 52→53, Runtime Client 72→73, SQLite
75→76, Session catalog 17→18, archive 8→9, archive Journal 6→7, auxiliary audit 3→4 and inherited audit
3→4. Event and independent child-control versions are unchanged. Obsolete v52
artifacts and imports are removed. There is no migration or compatibility reader.

## Contract-to-test map

The integrated prerequisite suites remain authoritative for unchanged executor,
Store, Source and physical-transport contracts. The added tests exercise the new
consumers through those owners; unit tests do not substitute for real transports.

| Contracts | Evidence and synchronization |
| --- | --- |
| D01–D04, D08–D09; V01–V03, V06, V10–V11 | Existing `durable::sqlite::auxiliary`, `runtime::auxiliary` and scripted native suites; new strict classification tests use real SQLite and assert domain failure separately from Completed execution, with no canonical messages. |
| D05–D07; V07–V09 | Existing auxiliary execution retry/deadline/integrity suites retain distinct RequestIds and frozen input. New observer tests verify Reset/frontier ordering and retirement on Full. Web tests inject missing/late attempts and repair only from the exact retained final result. |
| D10–D11; V04–V05, V12–V14 | `tests/scripted/runtime_client/auxiliary.rs`: admitted work survives detach, successor cancellation, 16 unread results refuse before provider entry, failed preparation releases capacity. `tests/scripted/app_server/protocol/auxiliary.rs`: original manager handoff parked then closed cannot register after negative confirmation; receipt precedes gated provider completion; a real SQLite read holds the connection while exact cancel succeeds on one async worker. Lock-order regression parks transfer with channels and requires cancellation to finish before release. |
| D12; V15–V18 | Complete inherited Summary/Compaction and automatic Title suites run with the new consumers. No purpose switch or separate executor, queue, writer or cancellation owner is introduced. |
| V19 | `auxiliary_classification_real_stdio_and_websocket_preserve_primary_and_correlation`: real standalone binaries, both physical carriers, Catalog and external provider emulator; primary stream is gated while valid/invalid classification completes. Primary state remains equal; restart returns the original correlation without another provider request. |
| V20 | `web-console/test/e2e/auxiliary.spec.ts`: real WS/Catalog/HTTP provider/parser, concurrent Agent, close without cancellation, reopen/final repair, exact cancel, keyboard/focus, English light/Chinese dark and narrow viewport. `web-console/test/auxiliary.test.ts` covers late receipt, Pending identity, terminal fences, Source failure, coalesced reads and unknown receipt reconciliation. |
| V21 | Existing typed media/preflight tests and three-protocol provider capability fixtures; adapter and native BTW maximum-media experiments exercise actual PNG validation, sealed Artifact capture, frozen retry bodies, base64 translation, HTTP buffering, three held Calls and cancellation join. |
| V22 | SQLite/archive schema checks and generated protocol fixture/type contracts run at the new versions. Existing clone/fork/archive tests retain inherited audit without target-local correlation authority. |
| V23 | `response_budget` exact/+1 and cumulative-chunk tests; `auxiliary_raw_response_budget_precedes_success_and_error_parsers` compares the same valid response without/with the cap for all three real protocols. Classification parser rejects arrays, duplicate properties, prose/fences and extra members. Result/transport tests count full escaped records. |
| T01, T15 | Existing request-ID/full-record tests plus new 64 KiB auxiliary envelope, count-limited retention/copies/observers, small-record and strict UTF-8 input tests. |
| T05, T13, T18 | Native cursor remains the existing durable owner; new segments have no cursor. Existing checkpoint/replay/receipt/recovery suites plus Web source-failure tests prove no mutation replay or invented settlement. |
| T07, T12, T16–T17 | Existing real stdio/WS and deterministic registration/Writer tests rerun with the updated route; auxiliary reply tests revoke DeliveryAccess before acceptance. All new carriers explicitly retain publication authority. |
| T10, T19–T20 | Detach/reconnect tests, physical Store-read gate and native cancellation, existing source lifetime/process-death suites, counted result copies held through discard/expiry, and panel read cancellation tests. |

The transfer lock test has a timeout only as an outer liveness failure guard; the
channels establish the disputed order. The raw-response test has a valid negative
control: removing the budget makes successful data complete and exposes the error
code beyond the over-budget prefix. Provider gates, actual authoritative reads and
physical joins establish the other races; sleeps do not establish their winners.

## Initial implementation review

An independent subagent reviewed the full implementation and subsequent repairs.
The repairs include attachment/map lock order, moving Store operations off async
workers, successful receipt confirmation at CallStarted, parser-level raw response
limits, late receipt/segment terminal fences, and keeping Pending Call identity
separate from confirmed admission. The final independent review found no remaining
code issues, including the complete Delivery Access credential path and final
capacity preflight. Its two final documentation version findings were corrected.
The reviewer performed read-only review; execution results below were verified
separately.

## PR #494 review repairs

The review of `df2cfa1` identified four lifecycle/input defects. These repairs
keep the existing native execution, publication, Source and Context owners:

| Finding | Repair and deterministic regression |
| --- | --- |
| Admitted receipt revoked before Writer acceptance | Capture execution uncertainty before response encoding; publication or encoding refusal preserves `outcome_unknown`. The real Call is admitted and its provider held before a zero-byte Writer acceptance barrier. Detach, grant revocation and delivery cancellation all preserve reconciliation, including uncertain failures and encoding substitution. Direct return and Writer paths both return the same original Call through successor lookup; Web retains correlation and cannot submit a second question. |
| SourceUnavailable prevents precise cancellation | `auxiliary/cancel` uses retained observation identity in the Web pipeline; native controller authority remains mandatory. Web tests deliver the real `session/observationUnavailable` wire notification, with and without a held checkpoint. Native tests create a real Source fault or physically parked projection read and cancel the exact auxiliary Call without waiting on preparation or stopping the primary Attempt. |
| Incomplete Tool batch makes BTW unavailable | Store selects the committed prefix before the earliest unresolved Tail Assistant, using the canonical occurrence index in the same read transaction. Tests cover cold/hot caches, absent/present Projection Head and a partially completed two-Tool batch; the exact selected frontier is asserted and primary compilation remains strict. A real primary Tool is held after canonical admission while BTW completes, then primary execution resumes with its original Tool continuation. |
| Classification replays orphan Tool messages | Historical input becomes bounded request-only data with source role/MessageId and historical Call evidence. Native and actual Chat/Responses/Anthropic request builders cover User, Assistant and ToolResult data; ToolCall-bearing Assistant input is rejected before dispatch. Media, reasoning and empty text are rejected; the complete UTF-8 source header plus data is tested at exactly 16 KiB and one byte over. |

An independent read-only subagent reviewed the repair diff, then reviewed the
additional uncertainty, input-boundary and Projection Head regressions. It found
no remaining actionable issues. The final repair validation is recorded below.

## Initial implementation commands and results (`df2cfa1`)

Local environment: Linux x86_64, Rust 1.98.1, Node 24.19.0, pnpm 11.13.1,
Python 3.12.3; no live model credentials. Provider tests use the frozen local
emulator. Browser screenshots use the repository's pinned Linux Playwright Docker
image; no host-browser reference substitutions.

| Check | Local result |
| --- | --- |
| Rust formatting / diff whitespace | Passed |
| Clippy, all targets and all features | Passed; repeated after final test additions |
| Binary builds | Passed |
| Native contracts, provider contracts, boundary/death/process targets | 5305 passed, 14 opt-in tests ignored; all four complete partitions passed |
| Native test-lane discovery | Both rust-contracts and rust-boundaries passed on Linux; exhaustive target/filter coverage confirmed |
| Protocol generation/check and typecheck | Passed |
| Configuration schema generation | Passed; checked by native schema contract |
| Dev launcher tests / typecheck | 41 passed; typecheck passed |
| TUI tests / typecheck | 1190 passed, 1 skipped; typecheck passed |
| Web tests / typecheck / production build | 3207 passed, 4 skipped; typecheck and build passed |
| Web i18n / provenance / package notices | Passed; 311 source records and 287 production packages |
| Fake provider Python tests | 55 passed |
| CI selection/policy tests | 64 passed |
| Real Classification stdio + WebSocket flow | Passed, including primary coexistence and restart correlation |
| Real browser BTW/authentication/managed-launcher flows | 4 passed, English light/Chinese dark, 1440/390px; screenshots inspected |
| Complete browser regression | 400 cases covered: 397 passed in the complete run; all 7 cases in the two corrected specs passed on the focused rerun |
| Raw response budget valid negative controls | Passed for all three protocols, both successful SSE and error JSON |
| Provider JSON exact / one-byte-short limit | Passed for all three protocols, nested JSON and adversarial escaping |
| Maximum-media experiments | Both passed; counts and measured RSS/HWM in auxiliary-consumers.md |

The first complete browser run found three stale test expectations: two missing
new follow-Session model defaults and one old archive component-version map.
The expectations were updated without weakening exact equality; the two complete
specs then passed (7 cases), including all three previously failing cases. No
screenshot references or liveness deadlines were changed. The focused command
adds `live-model.spec.ts session-export.spec.ts` to the browser command below.

The complete Rust coverage was executed as the repository's exhaustive lane
partitions, with the emulator required for every partition:

```sh
export RUST_TEST_THREADS=2 RUSTX_REQUIRE_PROVIDER_EMULATOR=1
cargo build --bins
cargo test --lib --bins --examples --all-features -- --skip boundary_suites::
cargo test --test contracts --test provider --all-features
cargo test --lib --all-features -- boundary_suites::
cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output
cargo run --all-features --example check_test_lanes -- --job rust-contracts
cargo run --all-features --example check_test_lanes -- --job rust-boundaries
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
git diff --check
```

App checks use `pnpm --dir <package>`: `check` and `typecheck` in
`protocol/app-server`; `test` and `typecheck` in `dev` and `tui`; `test
--maxWorkers=2`, `typecheck`, `check:i18n`, `check:provenance` and `build` in
`web-console`. The complete browser command is
`RUSTX_E2E_PREVIEW_PORT=5273 RUSTX_E2E_FIXTURE_PORT=5274 bash web-console/scripts/browser-tests.sh`.
Distinct local ports isolate this worktree from another running checkout.
The provider's Python suite uses `uv run --frozen pytest` in
`test-support/fake-provider`; CI policy coverage uses
`python3 -m unittest discover -s .github/scripts -p 'test_*.py'`.

The complete Rust rerun uses `RUST_TEST_THREADS=2` and serial lane execution,
with `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`. An earlier unrestricted parallel run
exhausted test liveness deadlines under concurrent builds/suites; guards and
assertions remain unchanged. The final run, not that failed attempt, is the
reported verification. No live inference is used.

Browser plugin not available: browser verification uses the repository's regular
Playwright runner and pinned Linux container. Linux local checks cannot certify
macOS process/store behavior; that lane belongs to exact-head FULL CI. The PR
receives `full-verification`; its final SHA and actual CI state are reported in
the PR, with merge and auto-merge left disabled. Initial FULL CI run
[1485](https://github.com/Caismis/rustX/actions/runs/39895057960) subsequently
completed successfully, including Linux, macOS, both browser shards and merge
verification. That run validates `df2cfa1`, not the review repairs.

## Review repair validation

The repair runs used the same Linux toolchain, local provider emulator and
pinned browser image. Native test execution used two workers and serial lanes.

| Check | Result |
| --- | --- |
| Rust formatting, all-target/all-feature Clippy and diff whitespace | Passed |
| Native test-lane coverage discovery | Both rust-contracts and rust-boundaries passed on Linux |
| Rust library, binaries and examples, excluding the independent boundary lane | 4,388 passed, 4 opt-in tests ignored |
| Native contract and provider targets | 212 passed, 6 opt-in tests ignored |
| Web lifecycle regressions (five relevant test files) | 70 passed, including all 14 auxiliary cases |
| Complete Web suite | 3,209 passed, 4 skipped; one unrelated PTY cleanup test exceeded its existing 5-second guard |
| Isolated complete PTY test-file rerun after other execution finished | 22 passed; no assertion, deadline or product code was changed for this rerun |
| Web typecheck, production build, i18n, provenance and notices | Passed |
| Real browser temporary-question flows | 2 passed: English desktop/light and Chinese narrow/dark; both screenshots inspected |

The one Web timeout occurred in `workbench-host.test.ts` during final Host
physical cleanup while the broader build/test workload was active. The full
test file passed in isolation with its original guard. Both results are recorded
instead of presenting the first complete run as clean. The original exhaustive
boundary evidence above is retained; the repair's final-head FULL CI state is
recorded separately in the PR.

```sh
RUST_TEST_THREADS=2 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --bins --examples --all-features -- --skip boundary_suites::
RUST_TEST_THREADS=2 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --test contracts --test provider --all-features
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo run --all-features --example check_test_lanes -- --job rust-contracts
cargo run --all-features --example check_test_lanes -- --job rust-boundaries
git diff --check
pnpm --dir web-console exec vitest run --maxWorkers=2
pnpm --dir web-console exec vitest run test/workbench-host.test.ts --maxWorkers=1
pnpm --dir web-console typecheck
pnpm --dir web-console build
pnpm --dir web-console check:i18n
pnpm --dir web-console check:provenance
RUSTX_E2E_PREVIEW_PORT=5273 RUSTX_E2E_FIXTURE_PORT=5274 bash web-console/scripts/browser-tests.sh auxiliary.spec.ts
```

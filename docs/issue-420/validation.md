# Issue 420 repair validation

The follow-up repair of `9fbfda118b2875038974cc18a2ecb6f0533a1e47` is recorded in
[process-lifetime-validation.md](process-lifetime-validation.md). The record below
is historical validation of the first repair, not the current lifetime/snapshot
contract. See [read-domains.md](read-domains.md) for the final architecture.

Repair of existing PR #423, starting from authoritative clean branch
`issue-420-incremental-projection-chat-stability` at
`d9cf99886d09dbf6c846a8b5876fa0b6d4825531` in
`/home/caismis/Documents/codes/rustX-issue-420`. Initial fetched `origin/main`:
`ad863a24cf48fbb0d5182746e1046d4b64e8c167`. Primary development worktree was not
modified. No issue/PR was created; no merge or post-push CI watch is part of this task.

The original PR's shutdown liveness failure and standard Web measurement timeout
were PR regressions, not pre-existing failures. The process test is unchanged by
this repair. The earlier implementation's bounded-worker Web result was not proof
that standard `pnpm test` passed. This document supersedes those validation claims.

## Focused regression contracts

- `journal_batch_invalidates_trace_even_when_session_events_publish`: real
  `PendingObservations` committed receipts and Published acknowledgements release
  one JournalBatch containing Attempt start, Turn start and Tool start. All Session
  publications remain, and one independent Trace invalidation is coalesced.
- `failed_journal_batch_wakes_subscribers_without_inventing_a_trace_cut`: a failed
  Journal frontier wakes subscribers and returns exhaustion without allocating a
  fake Trace cursor.
- `durable_read_cut_retires_stale_results_without_overwriting_semantics`: shutdown
  does not dirty derived domains; a later durable revision rejects an older result.
- `blocked_read_materialization_does_not_own_shutdown_or_projection_drain`:
  single-worker Tokio test, oneshot/mpsc-gated materialization, shutdown observation,
  actual native shutdown and delivery drain all progress before the read is released.
- The unchanged
  `app_server_websocket_drain_supervises_active_root_and_cold_resume` passes locally,
  including its real SQLite `BEGIN IMMEDIATE` boundary.
- `incremental-client.test.ts`: a resync **without an overlapping ordinary event**
  during held explicit snapshot acquisition forces subscribe after N; attachment
  remains resynchronizing until ACK; the old continuation cannot fold; exact RPC,
  cursor/target/state and no-mutation-replay assertions. Replay-window refusal tests
  cover recovery on the second acquisition and stale failure at the third refusal.
  A separate registered-replay-before-ACK test matches native transport scheduling:
  contiguous replay advances once while controls stay resynchronizing; old cursors
  and duplicates do not fold.
- `trace-cache.test.ts`: read-domain update alone issues no Trace read; 101 native
  Trace notifications use one in-flight plus one coalesced trailing Trace RPC and
  zero Session snapshots.
- Native request-payload tests now leave Trace invalidation to the real batch owner.
  Conformance/endpoint tests consume the exact contiguous read-only suffix after
  settlement instead of equating execution settlement with the last presentation
  cursor. The admission-fault test still proves the exact fault script, closed
  admission, no admitted attempt/model request and no duplicate semantic failure;
  only the already accepted input's derived publication may follow.
- The independent native snapshot/event capture was regenerated from the real native
  host and compared by the production DTO fold. Canonical expected state is not
  synthesized by that TypeScript reducer.

## Commands and results

Commands below ran in the issue worktree; repetitions with identical arguments are
grouped. Logs are local `/tmp/issue420-*.log`. `CARGO_BUILD_JOBS=2` was used for the
final native build to avoid competing compiler/linker memory use on the shared
machine; it does not change test-thread or Tokio-worker settings. Web uses its
unchanged standard concurrency. No test timeout or screenshot threshold changed.

| Actual command | Result |
| --- | --- |
| `git fetch origin`; `gh pr view 423 --json url,state,isDraft,baseRefName,headRefName,headRefOid,autoMergeRequest,body`; `gh issue view 420 --json title,body` | Verified clean authoritative worktree/branch and matching PR/remote head before edits. |
| `cargo check --all-features` | Passed. |
| `cargo test --lib --all-features runtime_client::projection::tests` | Final: **44 passed**, including failed-frontier wakeup. Initial direct-Trace cursor assertions were corrected to the real batch-owner contract. |
| `cargo test --lib --all-features runtime_client::host::tests` | 61 passed; also included in the final lane below. |
| `cargo test --lib --all-features runtime_client::` | **337 passed** before the final failed-frontier test was added, including projection, real JournalBatch, blocked materialization, native host, both transport conformance drivers and managed-Python coverage. |
| `cargo test --lib --all-features alternating_select_adopt_failures_exhaust_one_admission_cycle -- --nocapture` | Compilation was terminated during shared-memory pressure; the corrected test passes in final all-targets. |
| `RUSTX_PROJECTION_CAPTURE=web-console/test/fixtures/incremental-native.json cargo test --lib --all-features incremental_projection_independent_snapshot_capture -- --nocapture` | Passed; final independent native capture regenerated. |
| `cargo test --all-features --test process app_server_websocket_drain_supervises_active_root_and_cold_resume -- --nocapture` | Passed repeatedly, unchanged; final focused run 1 passed in 4.26s. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Final passed. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features` | **Latest run not green:** 3,342 library tests passed, one managed-Python preparation failure, three existing ignored; Cargo stops after the library failure. The failure is `uv lock --no-config` fetching `https://pypi.org/simple/pyyaml/`: `Network is unreachable (os error 101)` after its normal three retries. The preceding full run passed 3,953 tests/18 targets before the final failed-Journal wakeup test was added. Remaining targets were rerun separately below; no gate was skipped or failure relabeled. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | **417 passed**, all seven external CI boundary targets. These targets also passed in the final separate integration run. |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test '*'` | **611 passed**, five existing ignored, nine targets; explicitly covers remaining targets after the environment failure. |
| `cargo test --bins --examples --all-features` | Passed. |
| `cargo build --bins` | Final rebuild passed. |
| `(cd protocol/app-server && pnpm generate && pnpm check && pnpm typecheck)` | Passed; wire vocabulary unchanged, no unnecessary version bump or generated drift. |
| `(cd tui && pnpm typecheck && pnpm test)` | Passed: **852 tests**. |
| `(cd web-console && pnpm exec vitest run test/incremental-client.test.ts test/trace-cache.test.ts)` | 28 passed at that development cut. |
| `(cd web-console && pnpm exec vitest run test/incremental-client.test.ts)` | 11 passed after the bounded-replay and pre-ACK replay cases were added. |
| `(cd web-console && pnpm exec vitest run test/incremental-client.test.ts test/trace-cache.test.ts test/incremental-equivalence.test.ts test/incremental-vocabulary.test.ts test/incremental-presentation.test.tsx test/scroll.test.tsx)` | Final: **38 passed**, six files. |
| `(cd web-console && RUSTX_PERFORMANCE_OUTPUT=/tmp/issue420-performance-final.json pnpm test:issue-420-performance)` | Passed; unchanged fixed fixture and counters. |
| `(cd web-console && pnpm typecheck && pnpm build && pnpm test && pnpm check:i18n && pnpm check:provenance)` | Final: all passed, **1,080 tests / 62 files** with standard `pnpm test` concurrency; 143 provenance records and 131 package notices verified. Existing bundle-size advisory only. |
| `(cd web-console && CONTAINER_ENGINE=podman RUSTX_E2E_PREVIEW_PORT=15473 RUSTX_E2E_FIXTURE_PORT=15474 pnpm test:e2e)` | Final handoff revalidation: **125/125 passed (6.3m)**, including the fixed browser measurement with unchanged counters. The preceding full run also passed 125/125; the initial run had 123 passed/two strict Settings edge failures described below. Screenshot references and tolerances remain unchanged. |
| `git diff --check` | Passed; repeated before commit. |

No macOS execution was performed locally. The Linux process regression and the exact
external targets corresponding to both relevant CI boundary jobs were executed.
No hosted CI result is claimed and no CI watch/poll is performed after push.

## Measurement reproduction

The exact fixed `200-long-reasoning-tool-v3` fixture was renamed without content or
assertion changes to `test/incremental-performance.measurement.tsx`. The normal
`test/**/*.test.*` correctness discovery excludes it naturally. The six-line
`vite.performance.config.ts` reuses the production test setup and selects only that
fixture with one worker. No CI lane or ordinary-suite concurrency change is needed.

```sh
cd /home/caismis/Documents/codes/rustX-issue-420/web-console
RUSTX_PERFORMANCE_OUTPUT=/tmp/issue420-performance.json pnpm test:issue-420-performance
```

Repair measurement equals the previous jsdom evidence: 0 Session snapshot RPCs,
210 React commits, 100 automatic writes after setup, 101 bottom writes including
setup, 0 anchor writes under mock geometry, and 0 message DOM seat replacements.
This is not a browser-layout measurement. Original Chromium before/after evidence,
source SHAs and recordings remain attributed to their original runs in
[README.md](README.md); no new comparison data is invented.

```sh
cd /home/caismis/Documents/codes/rustX-issue-420/web-console
CONTAINER_ENGINE=podman RUSTX_E2E_PREVIEW_PORT=15473 RUSTX_E2E_FIXTURE_PORT=15474 \
  bash scripts/browser-tests.sh incremental.spec.ts --output=/tmp/issue420-browser
```

The browser image/digest, baseline source, baseline instrumentation and exact
fixture geometry are documented in [README.md](README.md). To reproduce the old
baseline's jsdom measurement, use its existing `.test.tsx` fixture with
`pnpm exec vitest run test/incremental-performance.test.tsx --maxWorkers=1` in the
separate baseline worktree. No baseline run was fabricated during this repair.

## Development findings and environment observations

- Earlier repairs tried optimistic live-cut acquisition (first repeated reads,
  then a finite retry limit). Hosted process regressions proved that model wrong.
  The current [candidate repair](candidate-validation.md) completes a captured
  historical cut independently of live installation. Web replay-window recovery
  retains its separate bounded policy.
- The first clippy run found documentation markup and test-hook type complexity;
  both were repaired. A Web typecheck caught a missing native subscribe ACK cursor
  in the new test fixture; it was supplied.
- An early focused native run had two managed-Python source-preparation failures.
  A subsequent full run and external boundary runs successfully exercised them.
  The latest full rerun independently records a concrete PyPI `pyyaml` network
  failure above. This local network limitation is separate from the repaired
  PR-specific shutdown and ordinary Web test regressions.
- A standard Web run passed 1,077 tests before the final two recovery tests were
  added. A later overlapping run hit the unchanged 5s limit in the existing product
  remount test (not the removed performance fixture). Native/compiler/browser work
  was then serialized for final validation; no assertion or deadline was weakened.
- Shared RAM and swap were exhausted by simultaneous Rust compilation in this and
  another worktree. Only this task's two overlapping compilation commands were
  terminated; the other worktree and its processes were not modified.
- The initial browser run reported strict Settings corner-edge screenshot differences
  (15 pixels, max channel delta 5; 34 pixels, max delta 7). Captures were inspected;
  screenshot references and comparator policies were not changed. Final status is
  reported in the command table, without labeling these failures pre-existing.

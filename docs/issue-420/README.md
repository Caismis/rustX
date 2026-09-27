# Issue 420 delivery evidence

## Ownership and implementation

The [pre-edit audit](ownership.md) records the actual base and inspected Harness
sources. App Server **25**, Runtime Client **50** are mandatory. No decoder,
legacy streaming path, new state-store dependency or provider-specific projection
was retained. Current contracts: [Chat](../../web-console/CHAT.md),
[App Server](../app-server-protocol.md), [Trace](../trace.md),
[TUI](../tui-app-server.md).

`protocol/app-server/projection.ts` is the pure Web client DTO fold. Existing
AppServerClient attachment/correlation/generation ownership validates the complete
target before folding exact cursor C+1. Duplicates do nothing; gaps stop incremental
continuation. Snapshot acquisition retains no browser event queue: the bounded native
replay ring supplies events after the acquired cursor. A resync received during acquisition marks the old continuation untrusted and
retains the subscription handoff requirement. The snapshot installs its exact cursor;
attachment becomes `attached` only after the required subscribe succeeds.
If subscription returns typed `resync_required` because replay was evicted, Web
performs at most three snapshot/subscribe handoffs; exhaustion leaves stale controls
and requires a later recovery. Native snapshot completion itself never retries
because the live head moved. No write
is retried. Same-Session replacement retires outstanding work.

Native durable read cuts publish `read_domains_updated` only when the decorated
transcript/statistics/occupancy changes, carrying current native Todo as well.
`AttemptSettled` ends semantic execution; it does **not** await durable enrichment.
The later read-domain event and Trace invalidation are presentation publications,
not execution ownership. See [the repaired cut and invalidation contract](read-domains.md).

The projection worker folds queued semantics under the host mutex, captures the
read revision and represented Journal frontier, then releases the mutex. At most
one blocking read is in flight. Its completion is installed only after queued
semantics are folded again, the cut still matches and no native publication is
outstanding. Installation and the event cursor allocation share the projection
mutex. Stale reads are retired and the current dirty cut is retried. Requests that
need authoritative snapshots capture one finite semantic candidate and materialize its exact durable dependencies outside
the mutex. Later live progress does not invalidate C; bounded replay owns later
cursors. Closing delivery cancels
presentation connection waits and SQLite queries, then joins the worker and read task.
The Store-only read scope cannot install after close or keep Tokio destruction waiting
on an uncancelled operation. See the process ownership audit in read-domains.md.

`PendingObservations` owns the closed Trace-anchor vocabulary and represented
JournalBatch frontier. Every released successful JournalBatch emits one
`TraceChanged`, independently of Session events in that same batch. Web refreshes
Trace only on that signal; `read_domains_updated` is not a Trace surrogate.
Exact Tool assembly still uses `arguments_json`, including native number spelling.

Committed native content wins over transient concatenation. A single keyed
MessageSeat → Message → AssistantMessage path preserves the message DOM and reasoning
disclosure state. No event writes canonical storage. Chat's only automatic scroll
assignment is its RAF commit; observers and React commits only dirty it. The 24px
tail threshold, synchronous user intent, semantic anchor fallback, short-history
prepend and browser shrink-clamp rules are covered below. Trace virtualization is
unchanged. Subscriptions separate transcript/action guards, activity, docks, totals
and Trace; unrelated metadata preserves transcript references.

The TUI already had its own normalized presentation reducer. It now consumes native
read-domain events instead of settlement snapshots, preserves history through that
adapter, uses exact assembled arguments and coalesces cursor recovery. Its renderer
and normalized state are retained: adopting the Web raw-snapshot representation
would introduce a redundant model and a speculative rewrite. The obsolete
`refreshFromSnapshot` and settlement refresh loop were removed. No new user-facing
copy was added; existing English/Chinese locale keys remain the presentation owner.
FirstSubmissions, creation ACK navigation, upload/admission evidence and uncertainty
ownership are unchanged.

## Deterministic acceptance mapping

| Gate / issue requirement | Concrete regression |
| --- | --- |
| A, independent native equivalence | `src/runtime_client/host.rs::incremental_projection_independent_snapshot_capture`; `web-console/test/incremental-equivalence.test.ts` compares every Session field against the separately acquired native snapshot. Only the separately owned Trace page/repairs are excluded. |
| B, complete event vocabulary | `web-console/test/incremental-vocabulary.test.ts`: exhaustive typed table of all 39 variants; captured native text/reasoning/refusal/Tool/Attempt transitions plus explicit non-text transitions and audit domain no-ops. Exact `1.0` Tool assembly assertion. |
| C, duplicate delivery | `incremental-client.test.ts`: `production client consumes 100 contiguous deltas once with zero snapshot RPCs`; every delta delivered twice. |
| D, gap/resync/failure | Same file: `gap retires continuation and repeated resync coalesces; snapshot cursor joins replay`; `failed snapshot remains stale until explicit recovery and never replays a mutation`. TUI `live-refresh.test.ts`: gated/coalesced gap and failed-repair retirement. |
| E, snapshot/event overlap | Same file: `an explicit snapshot overlapping events installs its cut then replays only later cursors`; `notifications preceding the attach response join the returned cursor through native replay`. Native `snapshot_cursor_race_snapshot_wins` and its event-first companion. |
| F, stale connection/attachment/Conversation | `incremental-client.test.ts`: same-Session replacement; existing `client.test.ts` generation/attachment/branch fencing; TUI `app-server-client.test.ts` stale incarnation/attachment and pending-read closure tests. |
| G, real production client RPC count | `incremental-client.test.ts` drives actual AppServerClient; `incremental-performance.measurement.tsx` and browser `e2e/incremental.spec.ts` drive AppServerClient + ConversationLive, count actual request records. |
| H, single frame writer | `scroll.test.tsx`: `one frame owns 10 observer deliveries and 5 React updates; newer user intent wins`; one correction, and no write when position is already correct. |
| I, history reading | Same test: 100 content updates cause no further writes; browser fixture independently checks zero bottom-follow during its last 100 text deltas. |
| J, return to tail | `scroll.test.tsx`: natural return restores following; explicit latest delegates to the same frame owner. |
| K, queued frame versus user | Same controlled frame test scrolls manually after scheduling and before flushing. |
| Reading anchors / resize / prepend | `scroll.test.tsx`: stable prepend/growth; short transcript prepend transfers ownership; next-row fallback after disappearance, 75px expansion, shrink clamp and final bounded absolute fallback. Browser checks 123px asynchronous growth correction. |
| L, stable identity and canonical correction | `incremental-presentation.test.tsx`: same row, same Assistant element, same expanded Reasoning across 20 deltas and differing canonical final content; one row/no transient residue. Native capture includes multiple Assistant IDs and a real Tool. |
| M, narrow subscriptions | Same file: Trace/summary/Goal changes preserve message/transcript references and actual React Profiler commit count; shutdown controls still rerender. |
| N, preserved native/product contracts | Existing `first-submit.test.ts`, `e2e/startup-ownership.spec.ts`, `client.test.ts`, native runtime-client/settlement/provider suites, `tests/conformance/{lifecycle,transcript_history}.rs`, real App Server process/transport suites, `tool-correlation.test.ts`, `e2e/{composer,convergence,trajectory}.spec.ts`. |

All race proofs use controlled notifications, held RPCs, native channels/probes or
explicit frame/observer drivers. No timeout was increased and no test was skipped
to make this implementation pass.

## Performance and visual evidence

The normal `pnpm test` lane contains deterministic correctness contracts. The exact
long fixture runs separately with `pnpm test:issue-420-performance`, using one worker
and the existing timeout. This does not alter normal-suite concurrency, fixture
content or assertions. Export its counters with
`RUSTX_PERFORMANCE_OUTPUT=/tmp/issue420-performance.json pnpm test:issue-420-performance`.
The existing Chromium fixture remains in browser acceptance. The repair rerun is
recorded in [validation.md](validation.md); prior before/after evidence retains its
original source and environment attribution.

Fixture: `200-long-reasoning-tool-v3`, checked in as
`web-console/test/incremental-performance.measurement.tsx` (jsdom geometry) and
`web-console/test/fixtures/incremental.{html,tsx}` (Chromium layout), exercised by
`web-console/test/e2e/incremental.spec.ts`. Both run the production client and React
components. It emits one Attempt, reasoning, 200 long text deltas, Tool start/argument
fragments/exact assembly, and a differing canonical commit. It switches to history
reading after delta 100 and injects asynchronous content growth. The browser uses a
600px viewport, disables browser CSS scroll anchoring, and paints through explicit
native frames every 25 deltas. Observer notifications and automatic scroll writes
are counted separately from the manual user movement.

Baseline is detached commit `ad863a24cf48fbb0d5182746e1046d4b64e8c167` in
`/home/caismis/Documents/codes/rustX-issue-420-baseline`. The primary development
worktree was never rewound or edited. The baseline receives only the measurement
fixture and isolated port configuration, adapting the mandatory protocol import
and omitting the new `arguments_json` wire field; fixture content/order/geometry is
identical. Each lane creates a fresh client, DOM and browser page. Counts cover the
complete fixture, not an arbitrary timed sample. `automaticWrites` starts after
message-start setup; bottom/anchor classifications include setup. DOM row replacement
counts are identity observations, not invented React mount telemetry. Profiler
counts describe this subtree and fixture, not total application performance.

Reproduction (Node 24.21.0, pnpm 11.13.1, Linux x86_64):

```sh
RUSTX_PERFORMANCE_OUTPUT=/tmp/measurement.json pnpm --dir web-console test:issue-420-performance
cd web-console
RUSTX_E2E_PREVIEW_PORT=15473 RUSTX_E2E_FIXTURE_PORT=15474 CONTAINER_ENGINE=podman \
  bash scripts/browser-tests.sh incremental.spec.ts --output=/tmp/issue420-browser
```

The browser script pins Playwright 1.63.0 / Chromium and its fonts/libraries to
`mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7`.
Correctness assertions, counter measurements and recordings are separate lanes.
No native attach/readiness/provider latency improvement is claimed.

### Recorded results

Exact implementation source commit: `8acb89249361a90dba4e4b2fc998983fd691aaae`.
The later evidence commit changes documentation/artifacts only.

| Chromium fixture observation | Before | After |
| --- | ---: | ---: |
| Actual Session snapshot RPCs | 209 | 0 |
| Conversation subtree Profiler commits | 419 | 210 |
| Automatic scroll assignments after setup | 2,413 | 94 |
| Bottom-position assignments including setup | 1,208 | 93 |
| Reading-anchor assignments including setup | 1,212 | 1 |
| Message DOM seat replacements | 1 | 0 |
| Bottom-follow assignments during history reading | 0 | 0 |
| Position before/after asynchronous 123px growth | 210 → 333 | 210 → 333 |

The previous viewport also preserved history in this fixture; the improvement is
bounded frame ownership and fewer redundant writes, not a claim that every old
history case failed. The independent deterministic tests cover race and prepend
cases. Totals and classifications use the documented different setup boundaries.
The jsdom lane reports snapshots 209→0, commits 419→210, automatic writes
2,413→100, bottom writes 1,207→101, anchor writes 1,213→0 and seat replacements
1→0; its mocked geometry must not be compared numerically with Chromium layout.

[Before browser metrics](evidence/before/metrics.json),
[after browser metrics](evidence/after/metrics.json),
[before recording](evidence/before/video.webm),
[after recording](evidence/after/video.webm),
[before trace](evidence/before/trace.zip),
[after trace](evidence/after/trace.zip) and final screenshots are retained with
[SHA-256 digests](evidence/sha256.json). Recordings show the same fixed response;
counter assertions, rather than appearance alone, establish correctness.

## Validation record

The delivery record lists exact validation commands, failures, repairs and final
results in [validation.md](validation.md). Browser/source metrics and recordings are
kept under `evidence/`; the exact implementation commit used for the final measurement
is recorded with them. Existing ignored tests are instrumented startup measurements
and fixture regeneration, not disabled correctness gates.

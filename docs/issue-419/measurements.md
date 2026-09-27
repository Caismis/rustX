# Startup measurements (#419)

Base `6ff49deb4c0855e64f0acaed85ebea2b6a277103` versus this implementation.
Three samples each, isolated temporary runtime/configuration/Workspace per sample,
repository `web_composer_context` fake-provider scenario, Rust debug build and
Vite production build, identical lockfiles/configuration, Linux x86_64 and the
repository digest-pinned Playwright 1.63.0 container. Each sample starts a fresh
native process and creates a new Session; compiler/OS caches are warm. These are
local observations on a shared development host, not performance thresholds or
production-provider benchmarks. Concurrent builds caused memory pressure during parts of full validation; the
reported correlated browser samples were collected after those builds finished.
The two isolated before/after browser runs overlapped on this shared host; results
have no statistical significance at n=3.

A baseline reference worktree was created before replacing the path. The same
`startup-measurement.spec.ts` runs on both revisions. All browser times use one
`performance.now()` clock. A MutationObserver measures the actual active
Conversation surface, not the navigation callback. Native provider request records
carry their own dispatch/first-output offsets and request/Conversation identities;
no browser timestamp is subtracted from a native monotonic timestamp.

| Measurement, milliseconds | Before median (range) | After median (range) |
| --- | --- | --- |
| Create ACK → actual Conversation visibility | 318.6 (309.2–329.7) | 8.5 (8.2–8.5) |
| Gesture → native admission | 453.2 (441.9–459.0) | 499.0 (498.8–504.2) |
| Attach request → ready attach ACK | 260.8 (258.1–262.3) | 260.8 (257.7–261.4) |
| Gesture → first model output event | 508.6 (494.1–512.1) | 550.6 (547.8–552.3) |

The observed visibility interval changed because navigation no longer waits for
attach, model mutation/repair or admission. **These samples do not establish
faster native startup, admission or provider output.** No native startup
parallelization or provider prewarming was introduced.

## Boundaries and raw evidence

[Browser samples](browser-measurements.json) retain every observed timestamp,
startup RPC list/IDs, created Session and exact attachment identity, and native
provider request timing. Each provider request ID contains the same native
Conversation ID as its sample's creation/attachment; this correlation is verified. T0 is the Send click, T1
Workspace resolution response, T2 create dispatch, T3 create response, T4 attach
dispatch, T6 attach response, T7 established model observed in that response,
T8 turn/start dispatch, T9 inbound_accepted response and T10 the first native
model-output event. `visible` is the active Conversation DOM; `outputVisible` is
the final fixture answer DOM. T10 can precede final answer rendering. Browser
T7 is an observation bound, not the exact instant native preparation finished.

T5 (native compose/load complete) is measured separately by the opt-in native
`issue419_measure_native_cold_load` benchmark. It times `SessionRuntimeManager::load`
and verifies ready model state after return; create is separately timed and
asserted to leave no resident runtime. The isolated native fixture uses the
same `Fixture::with_session_count(None, 0)` on both revisions. These native samples
are not the browser Sessions and are not spliced into its timeline. No exact
browser-clock T5 or real external-provider latency is claimed.

## RPC counts

Each baseline sample dispatches 9 RPCs from gesture through turn/start:
`create`, `settings`, `attach`, `settings`, `summary`, `list`, `setModel`,
`snapshot`, `turn/start` (all prefixed `session/` except `turn/start`).
Each after sample dispatches 10: `create`, `list`, `settings`, `configuration`,
`settings`, `attach`, `configuration`, `settings`, `summary`, `turn/start`.
Early Conversation rendering starts legitimate configuration observations sooner;
therefore the total observed count did **not** decrease. The removed startup
mutation and its repair are exactly zero after (versus one each before).
The native membership-driven list runs independently; gate tests prove it is not
a startup dependency. Attachment/recovery and normal streaming snapshots remain.
Controlled two-file browser tests dispatch create=1, attach=1, upload=2 in order,
turn/start=1 across remounts, setModel=0 and initial-model repair snapshots=0.

The native `generation.ttft_ms` values in raw evidence measure provider dispatch
to first output in the fake-provider requests. Admission-to-output includes local
preparation and is deliberately not labelled provider TTFT.

## Reproduction

Use an isolated base worktree and this branch, install their pinned lockfiles,
`cargo build --bins`, and `pnpm --dir web-console build`. Copy only the measurement
spec into the base worktree. Run from each `web-console` directory:

```sh
CONTAINER_ENGINE=podman RUSTX_BINARY=/absolute/path/to/that/revision/target/debug/rustx bash scripts/browser-tests.sh startup-measurement.spec.ts
```

For native measurements, copy the benchmark module and its module declaration to
the base, use **separate Cargo target directories**, then run in each worktree:

```sh
CARGO_BUILD_JOBS=1 cargo test --lib --all-features issue419_measure_native_cold_load -- --ignored --nocapture
```

The local runs temporarily used ports 25173/25174 for baseline and 15173/15174 for
implementation because unrelated development servers occupied 5173/5174. These
port-only substitutions are not shipped. No real user Session was used.

## Native cold composition/load samples

[Native samples](native-measurements.json), n=3 per revision. Every sample loads a
new nonresident Session; the first also pays process-local initialization costs.
The first sample's load+readiness was 228.284 ms before / 229.325 ms after; the
next two were 49.059 / 44.875 ms before and 44.406 / 45.076 ms after. Native
creation was 65.965–70.760 ms before and 65.715–72.543 ms after. These small
local samples show no established native startup improvement. The implementation
does not change composition ordering. An initial attempted shared-Cargo-target
baseline was excluded because Cargo reused the implementation artifact; the
reported baseline was rebuilt in a separate target directory (3,331 filtered
tests versus 3,336 after), at the exact base revision plus this benchmark only.

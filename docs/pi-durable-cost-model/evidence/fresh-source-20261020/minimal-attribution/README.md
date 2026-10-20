# Minimal Native attribution, 2026-10-20

Four serial diagnostic products passed the unchanged full oracle (96 fixed fake-provider requests each, 64 warm-up plus 32 measured turns). This does **not** establish a causal explanation or supersede the earlier paired regression result. No production source was edited, no product rebuilt, no real model API used, and no browser product run in this step. Preserved baseline and geometry O3/debug0 binaries were verified before and after execution.

The single unprofiled control pair did not reproduce the earlier steady geometry regression: baseline 128.141 ms/turn versus geometry 100.084 ms/turn. Whole-process ordering reversed: geometry control spent much more CPU during warm-up. Separately profiled steady runs were 83.375 ms baseline and 91.246 ms geometry. These single, order-sensitive observations cannot estimate profiler overhead, acceptance ratios or confidence intervals. The earlier three-pair evidence and original browser failure remain intact.

| Run, actual order | Steady wall ms/turn | Steady sampled CPU ms/turn | Native full-exit CPU s | Provider command CPU s | Controller self CPU s | Controller full lifecycle s |
|---|---:|---:|---:|---:|---:|---:|
| baseline control |128.141|160.000|19.526356|0.688173|1.386755|18.799936|
| geometry control |100.084|120.313|34.193587|0.810506|4.116234|29.895055|
| geometry CPU profile |91.246|113.750|16.938612|0.537211|1.002286|16.149437|
| baseline CPU profile |83.375|107.500|22.112530|0.445538|2.914752|21.505189|

Native and provider-command CPU are actual `wait4` user+system values through OS exit, including final JSON emission. Provider-command rusage can include its waited descendants; do not add separately sampled descendant CPU again. Controller self CPU excludes children. Direct PID, full argv, user/system split, exit status, faults and switches are in `orchestration.json`; raw per-process snapshots are in `*-processes.json`. The latter sample nominally every 250 ms, can miss exits, and are not exact final descendant CPU. Full lifecycle is controller-owned, including provider startup/teardown and verification, distinct from per-turn settled wall time.

Warm-up and verification explain why lifecycle and steady ordering differ. Geometry control all-96-turn CPU was 27.03 s, of which only 3.85 s was the measured 32-turn window; baseline control was 11.94 s total and 5.12 s measured. Request verification consumed 3.39/3.19 s CPU baseline/geometry control; reopen verification consumed 3.32/3.01 s. In the profile pair, baseline reopen verification rose to 7.42 s CPU versus geometry 2.94 s. These stages are separate in `phase-summary.csv`; do not charge all lifecycle cost to routine turns or infer stable overhead from this sample.

## Sampling capability and integrity

`perf` CLI was absent. Ordinary unprivileged `perf_event_open` self probes succeeded for software CPU clock counting and a disabled user-space sampling descriptor; hardware CPU cycles returned ENOENT (no available PMU), **not** a permission denial. `perf_event_paranoid` was 2. No capability, sysctl, permission or security configuration changed. Probe JSON is retained. No perf sample stream was collected, so no perf loss counter exists.

The existing published `benchmarks/runtime/profiling/run.py` and unchanged `bridge.c` implement phase cuts with gperftools 2.16 at 250 Hz, system allocator. Only the diagnostic bridge was compiled before products; Debian libraries and Perl google-pprof were unpacked locally, with package URL/hash provenance retained. Bridge/library hashes and exact product `env` argv are in the manifests. No tcmalloc preload or heap profile was used. All products ran sequentially; compilation, compression and other profiling did not overlap them.

Steady profiles are `before_measured_turns.cpu`, ending at `after_turns`. Baseline has 733 weighted samples, geometry 710; both headers record a 4,000 us interval. Pprof flat sums, raw binary profile counts and corresponding stderr interrupt counts agree. All phase profiles remain in the raw archive, including validation and reopen. `profile-integrity.json` records every raw profile size, SHA, denominator, stack depth and mapped leaf module.

Actual sampling loss rate is **unavailable**, not zero. The sampler does not expose an attempted-versus-delivered signal loss denominator. Baseline nominal sample time/nearby process CPU is 2.932/3.44 = 0.8523; geometry is 2.840/3.64 = 0.7802. These are diagnostics of coverage, **not loss-rate estimates**: process-wide signal coalescing and multithread timer behavior, phase handshake, proc tick quantization and slightly different recording boundaries prevent that interpretation. Steady stderr evictions are 8 baseline and 3 geometry; they are flushed hash-table entries, not lost samples. See [gperftools 2.16 profile data implementation](https://github.com/gperftools/gperftools/blob/gperftools-2.16/src/profiledata.cc), especially Evict, Stop and Add.

## Observed hotspots, with limits

The following are inclusive sample counts and shares; parent/child paths overlap and must not be summed. No statistical significance or exclusive elapsed-time attribution is implied.

| Path | Baseline /733 | Geometry /710 |
|---|---:|---:|
| RuntimeInner::publish_attempt |384 (52.4%)|376 (53.0%)|
| PublicationDraft::encode |166 (22.6%)|164 (23.1%)|
| sqlite3_prepare_v3 |140 (19.1%)|150 (21.1%)|
| ReadDomainCut::read_with_trace |105 (14.3%)|100 (14.1%)|
| index_request_read_layouts |72 (9.8%)|77 (10.8%)|
| composed::fragments::scalar |41 (5.6%)|39 (5.5%)|

Both runs spend material CPU on publication/checkpoint encoding, SQL preparation/parsing, and projection/index construction. Flat resolved leaves include JSON value lookup, cloning, SQLite parser/VDBE, serialization and hashing. These identify useful boundaries for future targeted attribution. The geometry scalar path does not show an obvious sampled amplification here; this does not prove absence of one. The binaries deliberately retain opt-level=3/debuginfo=0, so an absent/inlined `original_root_field_geometry` symbol cannot establish source-level ownership or no cost.

Unresolved weighted **leaf** counts are baseline 299/733 (40.79%) and geometry 263/710 (37.04%). Raw maps place 296/299 and 261/263 of those samples in libc; the remaining 3/2 have no file map. Conformance leaves are 434/733 and 447/710. We do not assign libc private addresses the nearest exported function name. Unknown root/trampoline frames occur in nearly every stack (largest unknown inclusive frame 733/733 and 709/710); that is separate from unresolved leaves, and does not invalidate resolved Rust callers. Full text plus every flat/inclusive row are retained. No Source fix is justified solely by this sample.

## Reproduction and identities

Actual launch was `python3 /workspace/pr485-study/run-minimal-attribution.py` from the measured checkout. That wrapper invokes the unchanged profiling runner through Python `runpy`, once per control/profile, with argv recorded in `orchestration.json`. It adds external wait4 accounting and 250 ms process sampling, restores preserved helpers outside timing, verifies source/binary hashes and enforces strictly more than 5 GiB free plus idle products/builds/compression before each run. Controls have neither preload nor profile socket. Common runner environment sets fixed `short-large`, `live-subscriber`, output path, UV cache and UTC; source fixes model-facing time to `2026-10-19 00:00:00`, while real runtime deadlines remain active. Runtime environment was inherited after sensitive-name filtering; a complete arbitrary environment snapshot was not recorded and is not fabricated retrospectively.

| Source | Exact source SHA | Preserved conformance SHA-256 |
|---|---|---|
|baseline|623b41b869a95c888eb04f907cbf607232699a2f|711b5af1ac3500e36f0f1bd827d108fb975a088b5e24a8d3a99218d7812ed388|
|geometry|89d6b618eeba8afaf4b95af4406057330baa60a6|d25cae1955161392e1ecfde60b1fcdf87ee17577ee4531da1dac133391dbd4a1|

Original Cargo stdout/stderr, rustc arguments, build environment overrides and trees remain in the earlier build evidence. This run reused those bytes. Effective compile flags were `CARGO_PROFILE_RELEASE_OPT_LEVEL=3`, `CARGO_PROFILE_RELEASE_DEBUG=0`, `RUSTFLAGS=-Copt-level=3 -Cdebuginfo=0`; no diagnostic Cargo rebuild substituted different code. Rebuild summaries offline with `python3 scripts/summarize-minimal-attribution.py --root /path/to/extracted/minimal-attribution-20261020`. The exact wrapper, summarizer, bridge source, profiling runner/controller/oracle and tool-package provenance accompany this evidence. Symbolization commands are recorded separately; a slow optional warm-profile offline symbolizer was intentionally stopped to prioritize both steady reports, with its PIDs and reason retained. No timed product or profile collection was interrupted.

At the final ready check the four runs had 9,891,012,608; 9,860,173,824; 9,828,433,920; and 9,794,957,312 bytes usable, all strictly above 5 GiB. RAM/cgroup is 16 GiB with no swap, CPU quota 4 cores. Docker vfs shares the workspace backing filesystem. No archive was generated during the products; all binaries and earlier failures are retained.

## Independent browser diagnostic proposal

Fixture-only commit [cef4d78ad234b8cf60acb563a5e42bc0263ea0bf](https://github.com/Caismis/rustX/commit/cef4d78ad234b8cf60acb563a5e42bc0263ea0bf), branch `codex/pr485-diagnostic-fixture-observability-20261020`, is based on original published fixture bddf435705f545f9b05ccd9b637d0fe85d4baa47. It adds bounded send/receive/close/state observations, proxy callback times, fixture Node event-loop delay, child exits and Node process CPU counters. Workload, production code, provider, browser image, original timeout values and both original completion measurements are unchanged. Three unit checks plus syntax/diff checks passed. **No real browser execution has occurred; coordinator independent review of this exact SHA is pending before any dependent run.** Diagnostic capture adds overhead and is explicitly ineligible for formal timing. See its `DIAGNOSTICS.txt` for observation limits.

Main and PR #485 remain unchanged and unmerged. The earlier interrupted geometry browser cohort is retained. Pi remains deferred. Next useful work is independent review of the fixture proposal, followed only when authorized by a fresh diagnostic replay; no large matrix or Source optimization is proposed from these four runs.

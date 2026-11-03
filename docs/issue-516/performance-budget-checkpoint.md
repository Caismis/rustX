# Geometry performance checkpoint — paused 2026-11-03

Work is paused at the user's request. This is a development checkpoint, not
acceptance of the requested latency targets. Resume from the existing
`MiNisterPaX/rustX:codex/issue-516-cad-previews` branch and PR #517.

## Target and measurement contract

The requested targets are at most 200 ms for rotation and pan, and at most
500 ms for first display. The working interpretation retains the existing
whole eight-step drag P95 and click-to-model-ready-plus-two-rAF first-display
measurement. Loading text or an empty canvas does not satisfy first display.

The unchanged performance fixture runs the 735,000-vertex STL and 96-part STEP
assembly. Chromium 153.0.8010.12 comes from the pinned Playwright 1.63.0 OCI
image, with its fonts, via Bubblewrap. The browser is bound to logical CPUs
0 and 1 on an i9-14900K. These are **SMT siblings of one physical core**, not
two physical cores. There is no additional memory quota. Rendering uses
ANGLE/SwiftShader; this machine exposes no hardware GPU device. Viewport is
1440 × 1000 at DPR 1. Five samples use nearest-rank P95 (the maximum).

The tests retain Playwright's `<loopback>` forwarding and the development
fixture, including click/actionability and readiness-poll overhead. Keep these
figures separate from production-engine-only or hardware-GPU measurements.
The [previous follow-up](performance-followup.md) and its raw measurements
remain historical evidence for commit `306967199`.

## Retained changes

- Replace repeated iterator/getter scans with indexed typed-array validation;
  compute actual position bounds in the same pass. The position-derived AABB
  diagonal supplies a conservative culling/picking sphere. Normalized and
  interleaved attributes retain their accessor-based path. Worker reception and
  GPU-scene admission remain independent; the intervening duplicate model scan
  is removed.
- Optimize large generated meshes inside their disposable Worker using the
  pinned `meshoptimizer@1.1.1` simplifier. Exact-position welding distinguishes
  normal/color discontinuities. Simplification locks boundaries, accounts for
  normal/color attributes and permits a relative surface error of `1e-5`.
  Meshes under 50,000 vertices and meshes with material ranges are unchanged.
  Source arrays, occurrence IDs, hierarchy and retained source bytes are not
  replaced. No parsed-model, Worker or WASM lifetime cache is added.
- Use Lambert lighting for generated STL/CAD meshes. Authored GLB materials
  remain unchanged. Disable renderer MSAA and request high-performance graphics.
  These are visible quality/power tradeoffs: generated surfaces lose PBR
  highlights, and edges may show aliasing. Review image quality before release.
- Release the mounted graphics owner if initial sizing/state restoration fails.
- Pin the simplifier distribution, including its embedded WASM, in the runtime
  asset record and update production dependency notices.

Unit coverage verifies exact area and bounds for the dense planar fixture,
retained hierarchy/source arrays, a small hole and a hard color seam. This does
not establish fidelity for every curved, thin, closed or non-manifold model.

## Measurements so far

Milliseconds; orbit/pan columns are P50 / P95. These are exploratory captures,
not cross-machine guarantees or final-commit acceptance. Source and fixture
details are in the [environment record](performance-budget-checkpoint-environment.json);
all samples, frame distributions, long tasks and retirement counts are in the
[raw record](performance-budget-checkpoint-samples.json).

| Configuration | Workload | First display | Orbit P50 / P95 | Pan P50 / P95 |
| --- | --- | ---: | ---: | ---: |
| Published `306967199` | STL | 2285.8 | 3000.1 / 3095.6 | 1816.6 / 2715.8 |
| Published `306967199` | CAD | 2968.6 | 599.9 / 745.1 | 566.7 / 600.0 |
| Typed scans + simplification; Standard + MSAA | STL | 2277.4 | 383.3 / 415.7 | 217.2 / 244.5 |
| Typed scans + simplification; Standard + MSAA | CAD | 3052.6 | 616.3 / 716.3 | 566.7 / 615.7 |
| Retained configuration: Lambert, no MSAA | STL | 2217.9 | 249.4 / 259.9 | 186.5 / 200.0 |
| Retained configuration: Lambert, no MSAA | CAD | 2755.6 | 332.3 / 366.4 | 300.0 / 316.6 |
| Rejected batching experiment | STL | 2363.5 | 251.0 / 267.4 | 194.3 / 200.5 |
| Rejected batching experiment | CAD | 3005.4 | 336.5 / 366.1 | 316.8 / 333.7 |
| Rejected dynamic-resolution experiment | STL | 2260.3 | 233.4 / 247.4 | 200.0 / 233.4 |
| Rejected dynamic-resolution experiment | CAD | 3007.0 | 333.0 / 445.3 | 295.3 / 350.2 |

The retained configuration substantially improves the pressure-test drags,
especially the coplanar STL. **The overall 200/500 ms goal is not met.** Only
the STL pan P95 reaches 200 ms in this five-sample capture; rotation and both
CAD actions still exceed it, and first display remains 2.2–2.8 seconds.

## Attempts and current bottlenecks

- The STL fixture is a densely triangulated plane. SwiftShader rasterization of
  many tiny triangles and MSAA dominated its earlier stalls. Topology reduction
  and disabling MSAA produced the largest observed gains. A zero-error
  simplification attempt took seconds while reducing little; the retained
  `1e-5` relative error was much faster. Do not generalize this planar speedup
  to arbitrary detailed STL files.
- Three.js `BatchedMesh` was tried for the 96-part generated assembly, retaining
  the original hierarchy for picking/visibility and separate batch disposal.
  Transform/color/visibility/disposal tests passed after accounting for Float32
  color rounding, but whole-drag performance did not improve. Its implementation
  and tests were removed; the samples remain above.
- An interaction-only drawing-buffer cap of 262,144 pixels, with full resolution
  restored at interaction end, did not improve P95 consistently. It is also
  removed. Buffer resizing and the final full-resolution draw may offset any
  rasterization savings; that explanation is an inference, not an isolated
  measurement.
- Diagnostic timing of the pinned STEP importer recorded approximately **683 ms
  inside the cold native `ReadStepFile` call alone** under the pressure setup,
  before conversion, main-thread admission and painting. Native output for this
  assembly is only 18,480 vertices / 17,824 triangles across 96 meshes. Worker
  module startup, WASM initialization, validation and render scheduling add more
  latency. Current cold import therefore cannot satisfy 500 ms in this setup.
  This is a measured current bottleneck, not proof that the target is impossible
  on other hardware or with a different importer build.
- Separate Node diagnostics of the same pinned importer observed roughly
  724 ms cold / 298 ms repeated native reads on CPUs 0,1, versus 454 / 208 ms on
  distinct cores 0,2, and about 293 ms cold without affinity. These are diagnostic
  runs, not substitutes for browser first-display measurements. Cold compilation
  and CPU contention merit further investigation.
- A direct native-loopback diagnostic reduced first-open overhead relative to
  Playwright forwarding, but still missed 500 ms. Keep it a separate lane.

Temporary profiling fixtures/log statements have been removed. No batch cache,
pre-open source read, server-side conversion, or persistent parsed/WASM cache
has been introduced. The decoded/source resource ceilings remain unchanged.
The new simplifier adds temporary Worker allocations; peak RSS has not been
measured for this checkpoint.

## Validation and remaining work

Local checkpoint validation: Web suite **4007 passed / 4 platform skips**;
61 focused geometry/Worker/component/retirement/optimization cases and all
16 pinned-browser functional cases passed. All four exploratory performance
captures passed both workload/retirement cases, with zero owned Workers,
contexts, buffers, textures and URLs after retirement. Type checking,
production build, source provenance, runtime assets, localization, dependency
notices, Rust formatting and whitespace checks pass. The first full-suite invocation was stopped because
its sandbox denied local HTTP/PTY operations; the authorized rerun above passed.
The previous published commit `306967199` has a successful
[FULL CI run](https://github.com/Caismis/rustX/actions/runs/41601036697).
That result does not certify this new checkpoint. Current commit/CI status is
recorded in the PR handoff comment.

When the user resumes:

1. Review the generated-model shading/MSAA quality tradeoffs and simplification
   fidelity on curved, closed, thin, multi-solid and normal/color-seam models.
   Measure optimizer peak allocations near resource ceilings.
2. Reproduce the retained configuration on the same unchanged harness before
   setting another budget claim. Add a separate browser event-to-painted
   breakdown if useful, while retaining the original whole-action timing.
3. Investigate cold STEP native parsing/WASM compilation and Worker/module
   startup. Check upstream build/compiler optimization options before changing
   architecture. Preloading UI code concurrently with an already authorized
   read is an unimplemented idea; exclude speculative source reads and do not
   conceal prework by moving the timer.
4. Isolate CAD render submissions versus rasterization and final-frame costs
   before trying another camera optimization. Neither rejected experiment is
   evidence for reintroducing it without a better measurement.
5. If measuring distinct physical cores or hardware graphics, publish an
   explicitly separate lane and compare its baseline under identical settings.
6. Require the new commit's CI and final functional/quality checks before
   acceptance. PR #517 remains draft; interaction feature suggestions outside
   performance are still deferred. Do not merge or mark ready automatically.

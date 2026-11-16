# Geometry budget follow-up

Reviewed on 2026-11-04, Asia/Shanghai, from
`d7edb628ae1861b19f4e84c303c1b7021881c603`. The review-thread list is empty;
[comment 6625398717](https://github.com/Caismis/rustX/pull/517#issuecomment-6625398717)
is the latest timeline entry. This increment fixes that finding and reduces
CPU work in certified planar preparation. **The 200-ms cold-display / 50-ms
interaction-P95 acceptance budget still fails. The PR remains draft.**

## Camera reconstruction

The real OrbitControls constructor immediately updates around its default
origin target. For a tiny focused occurrence far from that origin, this can
move a replacement camera before the intended target is restored. The viewer
now preserves position, quaternion and zoom across construction, then applies
the retained target/limits and the supported final update. Listener ownership,
single-renderer lifetime and metadata publication remain unchanged.

Real-controls Front and Top tests fail without the fix and pass with it. The
browser regression reaches the reported path through Front, selection, Focus,
Orthographic and Perspective on a tiny occurrence at y=500 between parts at
y=±1000. It verifies pose/scale, resource stability and no idle rendering.

## Retained preparation optimization

Constant normal/color checks now compare scalar triplets against their first
triplet rather than recomputing component modulo for every value. Triangle
indices are loaded once and reused by the unchanged robust orientation and
edge insertion loops. Attribute deadline observations occur every 4,095 scalar
positions, within the prior observation interval. There are no new allocations,
approximations, budgets, caches, parser changes or runtime assets. Robust
predicates, arithmetic/insertion order, the 32-probe limit, topology/bounds
certification and original output construction remain.

The alternating Node microbenchmark uses four warmup pairs and twelve measured
pairs per fixture/phase. It compares complete output buffers, metadata and hashes
outside the measured interval. Plane, constant-color plane, closed box, curved
color and color-seam cases all match. Three independent plane runs give:

| Plane measurement | Baseline median, ms | Candidate median, ms |
| --- | ---: | ---: |
| Warm planar phase, initial | 29.77 | 24.85 |
| Warm planar phase, fresh-process repeat | 27.95 | 24.41 |
| Warm planar phase, shared unchanged indexing | 28.84 | 24.94 |
| Complete preparation, repeat | 57.23 | 52.34 |
| Fresh Node first-call preparation, four alternating pairs | 86.19 | 83.46 |

The planar-phase reductions are 12.7–16.5%; complete warm preparation reductions
are 3.8–8.5%. Closed-box full timings fluctuate with unchanged indexing/JIT and
allocation work; its planar rejection path and the curved control are effectively
unchanged. Fresh Node first-call preparation improves about 3.2%, and is not a
browser first-display result. A distinct-RGB/mixed-signed-zero regression also
checks successful planar certification and retained authored hull attributes.

## Browser observations and budget result

All captures use the existing pinned Playwright 1.63.0 Chromium 153 / SwiftShader
browser, DPR 1, MSAA and default outlines on the same i9-14900K host. Each
workload has three fresh browser contexts and at least 100 orbit/pan events per
context. No task builds/tests run alongside a performance capture. Source
modules are cold per context; driver caches and host scheduling are not reset.
The production fixture retains its documented explicit `esnext` target.

The clock is unchanged: trusted browser input to submitted model microtask,
GPU fence completion and next animation-frame opportunity. It does not observe
physical presentation. The whole-gesture harness and outline-completion timing
are separate. Cold compilation starts only after visible-body demand.

| Final workload | Cold frame opportunity, ms | Orbit P95, ms | Pan P95, ms |
| --- | ---: | ---: | ---: |
| 735,000-vertex STL | 322.1–371.4 | 36.2–40.1 | 37.3–39.7 |
| 96-part STEP | 719.3–787.6 | 53.1–55.8 | 56.9–59.6 |

STL Worker preparation is 83.1–86.7 ms, versus 89.4–118.3 ms in the initial
baseline and 95.9–106.6 ms in the later unchanged control. The initial baseline
cold observations are 339.4–518.9 ms; later control observations are
350.3–374.2 ms. These ranges overlap the candidate, so the evidence supports
reduced preparation cost, not a universal first-display speedup.

CAD does not enter the changed planar loop for this fixture. Its input/GPU
timings vary across unchanged controls, and no CAD speedup is claimed. The
initial CAD baseline spends 210.1–240.2 ms waiting for immutable code compilation,
then 305.1–322.1 ms in the synchronous OCCT read, with about 7–10 ms conversion
and 23–27 ms main display preparation. Worker startup overlaps the code wait;
these phases must not be summed as independent costs. The cold budget cannot be
certified by the smaller JavaScript-loop improvement. Reader/compiler costs
remain the next measured targets; coarsening tessellation, moving work before
demand or retaining parsed models/instances is not part of this change.

The explicit final budget-gate run records STL 354.5 ms and CAD 773.8 ms cold
frame opportunity and **fails both cases** against the original 200-ms limit.
Its CAD orbit/pan P95 is 54.9/56.2 ms, also above 50 ms. The functional and
observation runs do not relabel that gate as passing.

## Rejected experiment

A public renderer scissor restricted the axis-overlay depth clear to its
112-pixel rectangle. Initial CAD pan P95 appeared lower, but A/B/A captures
showed the unchanged control returning to 40.3–42.3 ms, versus candidate
41.1–46.8 ms. The first baseline's 45.1–117.2 ms was not a repeatable gain.
Orbit and STL results likewise did not establish an end-to-end benefit.

Default/front CAD pixels are identical in all RGBA channels at DPR 1 and 2,
and closing releases all resources, but the extra GL calls are unjustified.
The implementation and its dedicated tests were removed. Raw rejected results
remain in the evidence so the earlier apparent improvement is not presented as
an optimization win.

## Validation and reproduction

The complete Web/Host suite passes **4,144 tests with four platform skips**.
Type checking, full application/fixture builds, localization, 336 source records,
345 notices, exact runtime assets/2-GiB WASM maximum, Rust formatting and
whitespace pass. The default-fixture selection initially passes 43 of 44 tests,
including the new off-origin roundtrip. One idle probe records three draws:
its trace shows outlines still preparing before the probe and becoming enabled
during the alleged idle second. This is the ordinary outline-completion frame,
not an idle loop. The test now waits for enabled/completed outlines and zero
owned Workers before its existing two-frame settlement and one-second zero-draw
assertion. The original display clock, lazy-loading and retirement assertions
are unchanged. Both initial failure and repaired-run results are retained in the
environment artifact. The changed idle case passes in three fresh contexts each
through development and production fixtures. The production setup first found
that the application build had cleared the nested fixture output; no test body
started in that attempt. The exact frozen fixture was restored and its bundle
hashes verified before the passing run. The pushed head requires its own full CI.

[Every browser sample and microbenchmark pair/source](budget-follow-up-samples.json)
and [source/runtime/bundle hashes](budget-follow-up-environment.json) retain the
evidence, including unsuccessful budget assertions and rejected work. The raw
microbenchmark embeds its scripts and fixture definitions; its path references
identify the original local run and must be adjusted for another checkout.

Use [the committed production-fixture commands](production-fixture-benchmark.md)
for the browser observations and `RUSTX_GEOMETRY_BUDGET=1` for the unchanged gate.
Run without concurrent builds/tests. From `web-console`, functional validation is:

```sh
pnpm typecheck
pnpm test
pnpm build
pnpm check:i18n
pnpm check:provenance
pnpm test:e2e -- geometry-code.spec.ts geometry.spec.ts geometry-review.spec.ts geometry-roadmap.spec.ts geometry-parts-search.spec.ts geometry-sidebar.spec.ts
```

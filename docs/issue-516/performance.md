# CAD sidebar performance report

Measured on 2026-11-03. The [validation report](README.md) records functional checks;
[geometry contracts](../geometry-previews.md) define the unchanged admission limits.
[Environment and digests](performance-environment.json) identify the source,
fixture bytes, browser and compiled artifacts used for these measurements.

## Environment and method

- Windows host, AMD Ryzen 7 9800X3D, Ubuntu 24.04/WSL 2, Node 24.21.0.
- Exact Playwright 1.63.0 image pinned by the repository; Chromium 153.0.8010.12.
  Browser cgroup quota: **2 CPU equivalents / 2 GiB memory**. DPR is 1; viewports
  are 1440 × 1000 and 390 × 844. AA, the pixel cap and CAD tolerances are unchanged.
- WebGL reports **ANGLE/SwiftShader software rendering**. Results characterize
  this constrained software backend; hardware-GPU performance was not measured.
- Display clocks use monotonic `performance.now()`. Native production-sidebar
  timings end after two animation frames following the loaded canvas. The
  format matrix retains its DOM-ready display fence for comparison with earlier
  samples. It includes input, transport and display overhead.
- Import probes run the actual GLB/STL/CAD adapters after their modules and source
  bytes are available. They exclude source transport and rendering. Worker
  startup, WASM initialization, import and typed-result transfer remain included.
  The importer exposes one synchronous CAD call, so parsing/tessellation and
  bridge conversion are not individually attributed.
- Format rows are sequential first-delivery opens followed by Workbench opens in
  the same browser context. These are not independently cache-flushed cold runs.
  The native STEP first opens use separate product-page contexts; OS/process
  caches can remain warm. Retained reopen reauthorizes and reconstructs display.
- Repeated action rows use **five samples** each. P50/P95 use nearest rank; with
  five elapsed samples P95 is the observed maximum. Orbit/pan use consecutive
  eight-step drags, so poses vary during the series. Picking uses a verified
  visible hit. Remount includes hide, teardown, reveal, reauthorization and paint.
- Frame intervals use consecutive rAF timestamps; the first callback establishes
  the origin. Browser long tasks are entries ≥50 ms during the phase. Frame
  distributions cover the action series and automation gaps. Elapsed actions
  include Playwright forwarding and paint fences, rather than measuring isolated
  input-handler CPU. React counters cover the development fixture subtree;
  native first-display timings use the production application.

## Production sidebar: named twelve-part assembly

The original STEP is 162,365 bytes; GLB is 771,980 bytes. Both retain named parts,
materials and bounds of 120 × 72 × 80 mm, with GLB coordinates in meters.

| Entry | Source | Phase | First painted display, ms |
| --- | --- | --- | ---: |
| delivery | mounting-bracket.step | first-open | 1470.7 |
| delivery | mounting-bracket.step | retained-reopen | 955.3 |
| delivery | mounting-bracket.glb | first-open | 487.6 |
| workbench | mounting-bracket.step | first-open | 2138.8 |

Raw samples: [native delivery](native-display-timings.json),
[native Workbench](native-workbench-display.json). Both use the real App Server,
Product Host, source authorization and existing document sidebar.

## Format and import matrix

All values are milliseconds. Import columns show delivery / later Workbench.

| Fixture | First delivery display | Later Workbench display | Import-only stage |
| --- | ---: | ---: | ---: |
| cube.glb | 1176.0 | 159.3 | 0.7 / 0.7 |
| cube.stl | 856.8 | 892.8 | 472.4 / 462.6 |
| cube-ascii.stl | 857.8 | 879.6 | 471.6 / 461.2 |
| cube.step | 1365.2 | 1376.8 | 936.2 / 916.7 |
| cube-inch.step | 1889.2 | 1537.5 | 887.8 / 918.0 |
| cube.iges | 1462.0 | 1461.7 | 687.1 / 661.6 |
| cube.brep | 902.6 | 900.9 | 584.1 / 578.3 |
| cube-textured.glb | 186.1 | 192.4 | 58.5 / 2.6 |

Raw samples: [format-display-timings.json](format-display-timings.json). The
analytic cube corpus also covers ASCII/binary STL, embedded GLB texture, STEP
inch conversion and unknown-unit BREP/STL. No geometry runtime loads during
ordinary production startup; GLB/STL do not request the CAD Worker/WASM.

## Repeated heavy-model interaction

The STL contains 245,000 triangles / 735,000 vertices / 12,250,084 source bytes.
The CAD array contains 96 parts with 570 × 72 × 190 mm bounds. Both retain the
existing importer tolerances. Controls cause no additional complete source reads.

| Workload | Action | Elapsed P50 / P95, ms | Frame P50 / P95, ms | Long tasks | Workspace publications / React commits |
| --- | --- | ---: | ---: | ---: | ---: |
| STL, 735k vertices | remount | 2133.8 / 3083.3 | 16.7 / 50.1 | 15 | 30 / 35 |
| STL, 735k vertices | orbit | 4182.6 / 4600.0 | 16.7 / 583.2 | 40 | 40 / 60 |
| STL, 735k vertices | pan | 583.6 / 3457.0 | 33.4 / 483.3 | 28 | 40 / 60 |
| STL, 735k vertices | picking | 583.3 / 600.1 | 16.7 / 550.0 | 5 | 5 / 8 |
| CAD, 96 parts | remount | 2129.0 / 2649.1 | 16.7 / 50.1 | 15 | 30 / 35 |
| CAD, 96 parts | orbit | 700.0 / 700.4 | 16.7 / 83.4 | 27 | 40 / 60 |
| CAD, 96 parts | pan | 683.7 / 733.0 | 33.3 / 100.1 | 30 | 40 / 60 |
| CAD, 96 parts | picking | 63.3 / 165.1 | 16.7 / 99.9 | 2 | 5 / 8 |

Raw samples and full long-task lists: [STL](stl-735000-vertices.json),
[96-part CAD](cad-96-parts.json). Each test additionally opens a GLB cube and
splits into two live models. Time from opening that second document through
both painted panes was 2378.1 ms for STL + GLB and
2197.2 ms for CAD + GLB. Split panes share the same viewport;
this does not model two full-screen worst-case inputs.

The software-rendered near-limit STL has substantial interaction stalls: orbit
frame P95 is 583.2 ms and picking elapsed P95 is
600.1 ms. This report makes no smooth-frame-rate claim for that
workload. Camera-only workspace/React publication remains an optimization
opportunity, with its measured counts shown above.

## Initialization and allocation findings

The same 2-CPU/2-GiB STL display probe produced these single-run observations:

| Rendering path | Display-ready, ms | Largest sampled frame gap, ms |
| --- | ---: | ---: |
| Per-body environment prefiltering experiment | 5096.7 | 3299.9 |
| Fixed studio lights before buffer/scan changes | 2427.4 | 783.3 |
| Shipped studio lights and buffer/scan changes | 2746.2 | 1000.0 |

Sources: [environment experiment](room-lighting-envelope.json),
[studio comparison](studio-before-copy-envelope.json),
[shipped envelope](stl-envelope.json). The viewport uses a hemisphere light and
an off-axis key/fill pair, with neutral tone mapping. Environment prefiltering
caused a multi-second startup stall and is absent from the shipped path. These
single samples do **not** establish a wall-time speedup from the later copy/scan
changes; that narrower claim would need a controlled repeated comparison.

For this single-group STL, each Float32 position/normal array is 8,820,000 bytes.
The Worker now adopts those parser-owned arrays and reads its whole input buffer
in place. This avoids two group slices plus two second constructor copies
(35,280,000 bytes), and one adapter-local input copy (12,250,084 bytes):
**47,530,084 bytes, about 45.3 MiB, of cumulative avoidable allocation per import**.
Multi-group output still gets one exact owned copy per group; offset input views
retain an exact-range copy. The owner's independent 12,250,084-byte Worker input
copy remains, preserving the fixed source. This is allocation arithmetic and
identity-tested ownership, not measured simultaneous-live memory or RSS savings.

Scene factories return their admitted bounds for immediate reuse. Worker receipt
and GPU admission validation remain; position-derived boxes/spheres, finite values,
indices and resource limits are still checked. The caller avoids immediately
repeating the full scene scan on the unchanged object.

## Resource and bundle accounting

Final dedicated browser-container `memory.peak` was **1,215,262,720 bytes
(1.13 GiB)** across this implementation's acceptance/performance runs.
Swap usage at collection was 0 bytes; cgroup OOM events were zero.
The peak includes the browser, Workers, software renderer, launcher and page cache;
fixture runs also retain their emulated source bytes. It excludes the WSL
Node/native test host. No per-Worker JS/WASM/GPU RSS or GC-time attribution is claimed.

Two-model samples observed exactly two contexts. After closing both models,
owned Worker/context/buffer/texture/URL counters were **0 / 0 / 0 / 0 / 0**.
The idle probe observed **0 WebGL draw calls in one second**, confirming demand
rendering. Browser/driver memory can remain cached after logical resource disposal.
The 2-GiB compiled WASM maximum and source/output limits are admission ceilings,
with the accounting and limitations documented in the geometry contract.

| Lazy artifact | Raw bytes | gzip bytes |
| --- | ---: | ---: |
| GeometryPreview-5WZG113V.js | 665,533 | 165,454 |
| GeometryPreview-DNFY_fYT.css | 4,748 | 1,480 |
| cad-worker-BAnTL3Pd.js | 63,614 | 22,279 |
| stl-worker-A-LFrhC_.js | 118,119 | 33,653 |

The CAD WASM remains 7,604,031 bytes. Gzip sizes are offline artifact estimates,
not a claim that every deployment serves Content-Encoding. Assets and licenses
remain pinned and local; no new production package was added.

## Reproduction

Use the repository's pinned browser path and prepared native App Server and
supervisors. Run from `web-console`:

```sh
pnpm test:e2e -- geometry.spec.ts geometry-sidebar.spec.ts geometry-performance.spec.ts
```

For Docker Desktop with a separate WSL runner, expose the pinned run-server and
set `RUSTX_BROWSER_WS_ENDPOINT`; these geometry tests enable native loopback
forwarding. The committed raw JSON files retain the actual sampled values.
Five-sample estimates on one software-rendered host do not characterize other
hardware, native-GPU frame rates, all admitted models or total application memory.

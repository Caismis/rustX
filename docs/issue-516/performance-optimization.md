# Quality-preserving geometry preview optimization

Implementation: `7044adafc40b6c75e421c6e486b50a7f703083d1`.
Measured on 2026-11-04, Asia/Shanghai. The requested interaction P95 of 50 ms
and first complete display of 200 ms are **not achieved across the measured
workloads and entry points**. This report preserves that acceptance gap.

[Raw samples and compiler experiments](performance-optimization-samples.json)
and [environment, source, fixture and artifact hashes](performance-optimization-environment.json)
identify the evidence. The [geometry contract](../geometry-previews.md) owns
admission, source authority and resource limits. Earlier reports in this folder
remain historical measurements of their named implementations.

## Current behavior and quality

- Generated meshes use the original Standard shading, metalness 0.05,
  roughness 0.65 and renderer MSAA. CAD tolerances, pixel budget, authored GLB
  materials, output units, hierarchy and occurrence identity retain their contracts.
- Exact float-bit tuple indexing preserves triangle tuples, order, normals and
  color discontinuities. Planar reduction requires an axis-aligned convex disk
  with constant attributes, exact orientation signs, paired edges, one convex
  boundary and unchanged position-derived bounds. Unproved geometry keeps its
  admitted triangles. Original source arrays/bytes are preserved.
- The planar pressure fixture becomes four vertices/two triangles representing
  the same surface. This result is not generalized to curved or detailed meshes.
  Regressions cover holes, colors, reverse winding, overlaps, thin closed and
  curved surfaces, tiny disconnected solids, unused indexed coordinates and
  cancellation in floating-point determinants near widely separated coordinates.
- Optional preparation uses a cooperative 200-ms budget, checks long scans and
  copies, and starts no work in the final second of the original 15-second
  import deadline. Allocation/GC cannot be hard-preempted by JavaScript. The
  real Worker regression imports a 992,016-vertex closed STL without the former
  multi-second simplifier timeout.
- Code loading overlaps the already-authorized read. Resolved viewer code mounts
  directly: production profiling found about 300 ms between completed module
  loading and Worker construction under the former Suspense fallback. GLTFLoader
  is requested only for GLB, in parallel with its common viewer code.
- One owned GPU fence prevents accumulation of obsolete submitted frames.
  Pointer gestures retain their camera locally and flush the final pose on end
  or retirement. Metadata publication does not reconcile body admission inside
  a retiring lease. The real full-capacity coordinator regression verifies no
  admission exception, synchronous retirement and saved final metadata.

## Measurement definitions

Two latency endpoints are reported independently:

1. **GPU complete:** trusted pointer-event timestamp through a subsequent actual
   model-render callback and completion of a public WebGL 2 fence.
2. **Frame-opportunity proxy:** GPU completion followed by the next rAF callback.
   This includes a rendering opportunity; it does not independently observe the
   compositor or physical display presenting that frame.

The draw hook is scoped to the selected geometry canvas and runs after the full
model-render callback, including its parts and axes. A skipped/coalesced rAF
cannot stand in for a newer model draw. A bounded set of MessageChannel turns
avoids adding a Windows timer quantum to a fast GPU; subsequent polling yields
to timers. Cancelled samples are rejected by the benchmark.

The first-frame clock begins on the preview button's pointer-down. Each native
production workload has five fresh contexts and 100 trusted moves per action per
context (500 samples per action). Nearest-rank percentiles are used; first-open
P95 with five samples is their maximum. Process/OS/driver caches were not flushed.
The software production lane has one context per workload, so its single first
open is an observation, not a cold-open distribution.

The production fixture bundles the real viewer and logical source owners, but
uses deterministic browser-host source doubles. It does not include native file
transport. Real App Server/Host entry-point results are reported separately.
The unchanged whole-action harness remains a development fixture with five
complete eight-step drags, Playwright forwarding and two-rAF display fences.
These measurements are not interchangeable.

## Production fixture results

Chromium 153.0.8010.12 / Playwright 1.63.0; viewport 1440 x 1000, DPR 1.
Values are milliseconds. Native is RTX 5080/D3D11 with unrestricted CPU and native
loopback. Software is the pinned Linux image, SwiftShader, 2 CPU equivalents /
2 GiB and Playwright loopback forwarding.

| Lane / workload | First frame-opportunity P50 / P95 | Orbit GPU / proxy P95 | Pan GPU / proxy P95 |
| --- | ---: | ---: | ---: |
| Native, 735k-vertex STL | 177.8 / 178.9 | 35.1 / 53.1 | 36.3 / 54.2 |
| Native, 96-part STEP | 415.0 / 416.8 | 35.2 / 53.0 | 36.0 / 54.1 |
| Software, 735k-vertex STL | 334.5 / 334.5 | 27.9 / 44.0 | 19.7 / 33.2 |
| Software, 96-part STEP | 1324.9 / 1324.9 | 97.7 / 107.7 | 201.4 / 201.7 |

The native GPU-completion interaction figures are below 50 ms, while the
frame-opportunity proxy remains above it. The native planar STL first-frame
proxy is below 200 ms; cold STEP and the software first opens exceed that budget.
Neither GPU completion alone nor this fixture proves a universal display SLA.

## Real production sidebar and source transport

The named twelve-part assembly is opened through the actual App Server,
Product Host, authorization, existing delivery/Workbench owners and sidebar.
The legacy elapsed endpoint includes Playwright click/actionability/readiness
polls and two rAFs. Its clock and endpoint were retained. The separate first-frame
proxy starts at the browser pointer event and waits for the model/GPU fence.
These are single observations per row.

| Lane / entry | File / phase | Legacy display elapsed | First-frame proxy |
| --- | --- | ---: | ---: |
| Native / delivery | STEP first open | 458.8 | 360.8 |
| Native / delivery | STEP retained reopen | 383.6 | - |
| Native / delivery | GLB first open | 168.3 | - |
| Native / Workbench | STEP first open | 1210.8 | 640.4 |
| Software / delivery | STEP first open | 1152.0 | 1078.9 |
| Software / delivery | STEP retained reopen | 881.8 | - |
| Software / delivery | GLB first open | 355.8 | - |
| Software / Workbench | STEP first open | 1616.7 | 1254.5 |

Native source-entry results do not support the 200-ms STEP claim. Worker timing
records separate runtime initialization, the synchronous ReadStepFile call and
conversion. Parsing/tessellation/bridge work inside that native call is not
individually attributed. Source authorization and physical settlement are still
required and are not removed to improve the figures.

## Original whole-action and resource checks

The unchanged development harness produces the following P95 elapsed values.
They include the complete eight-step gestures, not one event's response:

| Workload | First display | Orbit | Pan | Picking | Orbit frame interval |
| --- | ---: | ---: | ---: | ---: | ---: |
| 735k-vertex STL | 1602.0 | 216.4 | 183.8 | 34.0 | 16.8 |
| 96-part CAD | 2172.0 | 625.4 | 650.5 | 71.2 | 83.3 |

All source-read counts remain one through the interaction phases. The two-model
check owns exactly two contexts; after closing, owned Workers, contexts, buffers,
textures and URLs are all zero. Idle scenes issue zero draws in the defined
one-second window. Dedicated browser-container memory.peak was 1,316,315,136 bytes
(about 1.23 GiB), with zero OOM events. This cumulative browser-container peak
includes its launcher/cache and excludes the Node/native test host and compilation;
it is not a per-Worker, GPU allocation or browser-RSS reduction claim.

The initial `ea51060ab` checkpoint used lossy simplification, Lambert materials
and disabled MSAA. Its timings are not an equal-quality speedup baseline for this
implementation. The raw retained whole-action harness and older full-quality
reports remain available without relabeling their measurements.

## Compiler and bridge experiments

The shipping runtime remains the exact reviewed occt-import-js 0.0.23 release:
WASM SHA-256 `33391fc9d94ea5c869a6718488bf0a9a464222bac9bdc764dfe1690cef281952`,
7,604,031 bytes. No compiled-runtime fork or warm parsed-model/Worker/WASM-instance
cache is shipped.

Experiments used the same importer and OCCT commits and Emscripten 3.1.69.
The [pinned upstream CMake](https://github.com/kovacsv/occt-import-js/blob/c2148e54b456b571238d35cac037d304053d64b2/CMakeLists.txt)
uses -Oz for Release. Trials covered -O3, safe constructor evaluation, LTO,
the SDK's bulk numeric input copy, and bounded 4,096-double output chunks.
No fast-math, tolerance, geometry or source-authority changes were used.

All six fixture cases matched full JSON and Float32/index/hierarchy/color/face
range digests in fresh-process comparisons. Those are single-run diagnostic
measurements, not statistical speedup acceptance. Original assets were verified
against the repository manifest before those comparisons. Only the manifest-verified final comparisons are included in the raw report.

-O3 grew WASM to 12,149,391 bytes, and LTO to 14,835,245 bytes. The browser -O3
comparison increased the 96-part STEP first-frame proxy from roughly 506-524 ms
to 542-599 ms under the same then-current frontend. Safe ctor evaluation stopped
at an environment import. Size-oriented bulk copying preserved bytewise output
but did not establish enough end-to-end improvement to justify replacing the
reviewed distribution. These experiments remain outside the shipped runtime.

## Validation and reproduction

- Complete Web suite: **4023 passed, 4 platform skips**.
- Fixed Linux browser geometry, real sidebar and document cases: **19 passed**.
- Separate installed Edge 156 functional/resource lane: **14 passed**.
- Actual-source native-GPU sidebar: **2 passed**. Native fixture measurement:
  10 captures; software fixture and unchanged whole-action captures: 2 each.
- Type checking, full production build, localization, source provenance,
  third-party notices, exact runtime hashes/memory maximum, Rust formatting and
  whitespace checks pass. The latest PR head still requires its CI result.

The full suite was executed on a byte-for-byte native Linux filesystem copy with
the same pinned dependencies/binary. The first cross-drive run had import-time
false timeouts and an unprepared provider environment; those failures were not
hidden by extending test timeouts. The prepared native-filesystem run passed.

From web-console, use the normal project build and pinned browser path:

```sh
pnpm typecheck
pnpm test
pnpm build
pnpm check:i18n
pnpm check:provenance
pnpm test:e2e -- geometry.spec.ts geometry-sidebar.spec.ts geometry-performance.spec.ts geometry-latency.spec.ts workbench-documents.spec.ts
```

For the separate bundled performance fixture, build
`test/geometry.vite.config.ts`, copy the licensed geometry fixtures to its
`test/fixtures/geometry/` output, serve that output and set
`RUSTX_E2E_FIXTURE_PORT` to its port. Preserve the pinned image for screenshots.
`RUSTX_GEOMETRY_DIRECT=1` is only the separately reported native-loopback lane.
`RUSTX_GEOMETRY_BUDGET=1` enables the explicit 50/200-ms assertions; current budget
acceptance is red, and must not be represented as a passed correctness run.

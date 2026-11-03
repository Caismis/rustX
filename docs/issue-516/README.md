# CAD sidebar validation

Current implementation: `7044adafc40b6c75e421c6e486b50a7f703083d1`.
[Geometry contracts](../geometry-previews.md) define the formats, source authority,
fixed-copy lifetime, lossless preparation and resource limits.
[Current performance evidence](performance-optimization.md) contains measured
results, raw samples, environment/source/artifact digests and the open budget gap.
The earlier performance reports in this directory are historical baselines.

## Outcome and verification

GLB, binary/ASCII STL, STEP/STP, IGES/IGS and Open CASCADE BREP open through the
shared delivery/Workbench document body. The original PBR shading and MSAA are
preserved. Exact indexing and certified planar preparation retain represented
surfaces, bounds, scale, normals/color semantics, hierarchy and original bytes.
No lossy simplifier or warm parsed-model/Worker/WASM-instance cache is shipped.

The 50-ms interaction P95 / 200-ms first-display target remains **unmet across
all measured workloads and entry points**. Native GPU completion is reported
separately from the next-rendering-opportunity proxy and the legacy display
clock; none is silently substituted for independently observed presentation.
The explicit budget-gate run fails, while correctness and lifecycle checks pass.

| Check | Result |
| --- | --- |
| Complete Web/Host suite | 4023 passed, 4 platform skips |
| Type checking and full production application build | Passed |
| Source provenance, dependency notices and exact runtime assets/memory maximum | Passed: 336 source records, 345 notices, original pinned CAD assets |
| Product localization ownership | Passed |
| Fixed Linux browser geometry, real production sidebar and document acceptance | 19 passed |
| Separate installed Edge 156 functional/resource smoke | 14 passed |
| Native GPU real App Server/Host sidebar | 2 passed |
| Production-fixture latency capture | 10 native captures and 2 constrained software captures |
| Unchanged whole-action/two-model/retirement capture | 2 passed |
| Explicit 50/200-ms budget gate | Failed for both workloads; acceptance gap retained |
| Rust formatting and whitespace | Passed |

The latest pushed PR head requires its own selected CI checks. A prior head's CI
success is not certification of the current revision. The PR remains draft.

## Functional and fidelity coverage

The named twelve-part STEP/GLB assembly opens through the real production App
Server and Product Host with existing source authorization, sidebar tabs/path
header, Refresh, original Download and file-location/application actions.
Coverage includes orbit, pan, fit/standard views, selection/visibility, a
collapsed in-viewport part tree, Escape/focus return, Markdown/model tab changes,
short/narrow windows, Chinese/English and dark/light layouts.

Fixtures and independent analytic/OCCT references cover unit conversion,
unitless BREP/STL, embedded GLB textures, authored hierarchy/transforms and
materials. GLB admission independently checks the selected scene and actual
position-derived bounds/spheres, with complete cleanup on rejection. The existing
external-resource, malformed/oversized, decode-failure, context-loss, source
retirement, copy/Refresh and current-original Download cases remain enforced.

New regressions cover a 992,016-vertex closed STL through the actual Worker and
its original deadline; a tiny disconnected zero-normal cube; curved/thin closed
surfaces; holes/colors/reverse winding/overlaps; unused indexed coordinates;
and floating-point cancellation that otherwise erases a tiny attached surface.
The real coordinator/lease regression exercises final-pose retirement at full
two-pane capacity, rather than relying on a capacity-free mock.

The pinned browser verifies zero idle draws and zero owned Workers, contexts,
buffers, textures and URLs after closing. Resource ceilings and CAD tolerances
remain as documented. Source-only inactive retention and authorization remain
with the existing owners.

## Current visual evidence

The images below are diagnostic production-sidebar captures from the pinned
Linux browser; no screenshot-comparator threshold or reference was changed.

![Production document sidebar](cad-preview.png)

[Expanded parts](cad-parts.png) · [GLB in the sidebar](cad-sidebar-glb.png) ·
[Chinese dark Workbench](cad-workbench-zh-dark.png)

![Narrow Chinese dark view](geometry-narrow-zh-dark.png)

[Expanded narrow parts](cad-parts-narrow-zh-dark.png)

## Reproduction

From web-console:

```sh
pnpm typecheck
pnpm test
pnpm build
pnpm check:i18n
pnpm check:provenance
pnpm test:e2e -- geometry.spec.ts geometry-sidebar.spec.ts geometry-performance.spec.ts geometry-latency.spec.ts workbench-documents.spec.ts
```

The native-filesystem validation copy uses the same source bytes, locked
libraries and native binary. The [performance report](performance-optimization.md)
describes separate production-fixture/native-source lanes and their limitations.
Fixture origins and generators are recorded in the
[fixture manifest](../../web-console/test/fixtures/geometry/README.md).

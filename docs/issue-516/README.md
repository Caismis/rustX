# CAD sidebar validation

Current implementation: `fbf0a875c27f2b39529793fb032919eb56940797`.
[Geometry contracts](../geometry-previews.md) define the formats, source authority,
fixed-copy lifetime, lossless preparation and resource limits.
[Interaction controls and current validation](interaction-roadmap.md) describe
isolation, focus, properties, direct axis navigation and bounded default-on
outlines. The [comment inventory](comment-status.md) lists remaining requests.
[Performance evidence at implementation 7044adafc](performance-optimization.md)
preserves the original methods and open 50/200-ms budget gap. The interaction
increment adds separate diagnostic outline costs; it does not recertify that SLA.
The earlier performance reports retain their named implementations.

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
| Complete Web/Host suite | 4044 passed, 4 platform skips |
| Type checking and full production application build | Passed |
| Source provenance, dependency notices and exact runtime assets/memory maximum | Passed: 336 source records, 345 notices, original pinned CAD assets |
| Product localization ownership | Passed |
| Fixed Linux browser geometry, reviews and real production sidebar | 27 passed |
| Shared document regression with the complete fixture | 5 passed |
| Separate installed Edge 156 functional/resource smoke | 21 passed |
| Two-pane outline off/on captures | 2 workloads in each browser lane |
| Explicit 50/200-ms budget gate | Prior failed acceptance retained; not relabelled by feature checks |
| Whitespace | Passed |

The latest pushed PR head requires its own selected CI checks. A prior head's CI
success is not certification of the current revision. The PR remains draft.

## Functional and fidelity coverage

The named twelve-part STEP/GLB assembly opens through the real production App
Server and Product Host with existing source authorization, sidebar tabs/path
header, Refresh, original Download and file-location/application actions.
Coverage includes orbit, pan, fit/standard views, selection/visibility, a
collapsed in-viewport part tree, Escape/focus return, Markdown/model tab changes,
short/narrow windows, Chinese/English and dark/light layouts.
Isolation/restore/show-all, small-part focus, transformed occurrence properties,
Fit/Reset, signed-axis/menu equivalence and DPR 2 navigation have dedicated
coverage. Optional outline checks include smooth surfaces, coplanar tessellation,
tiny boundaries, two panes, oversized/alpha inputs, cancellation, independent
Worker-result admission and full retirement. Camera-only metadata still causes
no React part-tree update.

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

![Production document sidebar and occurrence properties](interaction-structure.png)

[Curved mesh silhouettes](interaction-silhouette.png) ·
[Chinese dark Workbench](interaction-structure-zh-dark.png)

Earlier captures remain with their historical measurement reports.

## Reproduction

From web-console:

```sh
pnpm typecheck
pnpm test
pnpm build
pnpm check:i18n
pnpm check:provenance
pnpm test:e2e -- geometry.spec.ts geometry-review.spec.ts geometry-roadmap.spec.ts geometry-sidebar.spec.ts workbench-documents.spec.ts
```

The native-filesystem validation copy uses the same source bytes, locked
libraries and native binary. The [performance report](performance-optimization.md)
describes the prior production-fixture/native-source lanes and their limitations.
The [interaction report](interaction-roadmap.md) defines the separate outline
cost captures and their production/complete-fixture requirements.
Fixture origins and generators are recorded in the
[fixture manifest](../../web-console/test/fixtures/geometry/README.md).

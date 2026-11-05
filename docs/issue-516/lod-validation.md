# Whole-model CAD LOD validation

Validated on 2026-11-05, Asia/Shanghai. This replaces the completed-piece display
prefix introduced at `235559e78`. Every optional level now covers the whole
model; it replaces the previous graph rather than adding finished pieces.

The original three-sphere assembly has three named, colored occurrences at
every level:

| Level | Bounding-box deflection ratio | Angular deflection, radians | Triangles |
| --- | --- | --- | --- |
| Coarse | 0.02 | 1.0 | 338 |
| Medium | 0.005 | 0.75 | 1,294 |
| Final | 0.001 | 0.5 | 6,136 |

The source is parsed once in one native document. Each optional full-scene
export has independent 65,536-triangle / 8-MiB admission, with complete
hierarchy and material bounds. An oversized level is skipped entirely.
Embedded BREP triangulations keep their original final path and skip optional
retessellation. The rough STEP coordinate envelope remains a labeled loading
aid; it is not counted as a whole-model LOD.

## Native equivalence

The rebuilt `occt-import-js@0.0.23-rustx.3` uses the pinned Emscripten 3.1.69,
importer and OCCT revisions in the distributed source offer. Its five modified
source files and exact runtime assets are audited. The WASM maximum remains
2 GiB. Final CAD tolerances are unchanged.

[Ten standalone native comparisons](lod-native-samples.json) match full JSON
SHA256 across the preserved `.2` runtime, `.3` without optional levels, and the
final `.3` result after optional levels. They include STEP units, IGES, BREP,
12/96-part assemblies, curved assemblies and embedded BREP triangulations.
The 257-part SDK-only case verifies that an oversized optional scene is never
published; its final result exceeds client part admission and is not a client
acceptance case. These native API clocks are not browser first-display timings.

## Browser and lifecycle

The production fixture was exercised in installed Google Chrome
157.0.8103.38, separately from the pinned browser-container acceptance lane.
Controlled delivery of actual native coarse, medium and final messages proves
matching complete hierarchy/names, increasing submitted triangle counts,
single-context replacement, active gestures, focused keyboard navigation,
external-focus preservation, cancellation and zero owned resources after close.
It does not establish cold latency.

A separate [ordinary unheld 96-part import](lod-natural-browser-samples.json)
uses no message withholding or artificial delay. Its complete coarse level
contains all 96 parts and 9,088 triangles. The GPU-fence checkpoint was 604.0 ms
after trusted input, followed by a frame-opportunity proxy at 622.8 ms; the final
17,824-triangle model's corresponding opportunity was 654.4 ms. The medium
level was produced but skipped for display when final geometry was ready.
Only one context was created; close returned every tracked resource to zero.
This is one functional observation on a small assembly, not physical-presentation
measurement, a cold benchmark or the 200-MB latency certificate.

[Coarse](cad-lod-coarse.png), [medium](cad-lod-medium.png) and
[final](cad-lod-full.png) captures show the same complete three-part model.
Camera input between captures is intentional.

Real Three.js/OrbitControls regressions also cover distant replacement bounds,
both projections, stable camera/target during input, source-space pan across
changed tessellation bounds, disposal and late-result fencing.

## Checks

- Full Web/Host suite: 4,383 passed, four platform skips; 302 test files passed.
- Geometry suite: 286 passed, including actual STEP/BREP and real OrbitControls.
- Installed-Chrome production-fixture functional lane: four passed.
- TypeScript, application and production-fixture builds, source/notices/runtime
  audits, localization, Rust formatting and whitespace checks passed.

The historical approximately-200-MB STEP and large-STL timing reports retain
their named revisions. They do not certify this LOD pipeline. General
first-display/maximum-input acceptance and current-head pinned CI remain open;
PR #517 remains draft.

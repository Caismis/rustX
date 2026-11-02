# Browser geometry previews

The Web Console's shared, read-only geometry viewer opens committed Session-file
deliveries and Workbench files. Managed Artifacts retain their existing smaller
transfer policy. Source authorization, historical Conversation/root mapping,
physical file-read settlement and original Download remain owned by the existing
[file delivery](file-delivery.md) facilities.

## Formats and interaction

| Suffixes (case-insensitive) | Import | Coordinates and units |
| --- | --- | --- |
| GLB | Three.js GLTFLoader, static glTF 2.0 triangle scenes | Meters, original hierarchy/transforms, admitted materials and PNG/JPEG textures |
| STL | Three.js STLLoader, binary or ASCII | Original coordinates, unknown units; ASCII syntax is normalized before loading while solid names/groups are preserved, supported binary vertex colors are retained |
| STEP, STP | occt-import-js ReadStepFile | Explicit millimeter output; this labels the imported output, not an inferred source unit |
| IGES, IGS | occt-import-js ReadIgesFile | Explicit millimeter output |
| Open CASCADE BREP | occt-import-js ReadBrepFile | Native coordinates, unknown units; linearUnit does not affect BREP |

CAD triangulation uses `bounding_box_ratio`, linear deflection `0.001`, angular
deflection `0.5`, and `linearUnit: millimeter`. Available names, mesh colors,
face-color ranges and hierarchy survive import. Unnamed parts receive localized
ordinal labels. The independently exported cube fixtures cover STEP inch-to-mm
normalization and unitless BREP/STL behavior.

One scene implementation supplies orbit/pan/zoom, fit, isometric and six standard
views, an orientation axis indicator, part selection/highlighting and visibility.
Camera fitting centers a presentation-only parent; original geometry and its
physical scale remain intact. Bounds are recomputed from admitted coordinates,
rather than trusting GLB accessor bounds. Culling/raycast spheres are recomputed
from the same positions. Camera, target, up vector, selected part
and hidden IDs are bounded inert occurrence metadata tied to a content copy.
Hiding a selected part also hides its selection outline.
Changing or restoring the up axis recreates OrbitControls through its public
lifecycle so orbit gestures use that axis and old listeners are released.
The controls reuse the DSH floating zoom toolbar stylesheet, shared Button/Menu
primitives and existing theme tokens. Standard views live in one view-angle menu.
Product copy follows the existing concise preview vocabulary, and fixed-copy
previews show a refresh hint rather than claiming file notifications are missing.

GLB supports core static triangle meshes and `KHR_materials_unlit`,
`KHR_texture_transform`, `KHR_materials_ior`, and
`KHR_materials_emissive_strength`. Other extensions, sparse accessors, animations,
skinning, morph targets and non-triangle primitives produce an unsupported
result. Buffers/images must reside in the GLB BIN chunk or admitted base64 data
URIs. Relative, filesystem, model-supplied Blob and network URIs are rejected
before loader use and by the loader resolver. No sibling Workspace resolution,
decoder download, CAD editing, exact measurement or format export is provided.

## Copy and authority lifetime

Geometry uses an explicit `geometry` mode in `PreviewContentOwner`. It does not
change a mutable reader into an immutable source or introduce a second source
cache. A complete authorized read establishes the displayed copy. Source-version
hints still validate authority but cannot replace that copy. Refresh starts a
new content generation and reauthorizes the current source; loading/errors follow
the existing preview flow. Other viewers keep their automatic-refresh behavior.

Hidden bodies terminate their import Worker and release every renderer resource.
Only admitted source bytes remain in the existing 8-entry/64-MiB inactive cache.
Reactivation probes authority before showing those bytes, including when the
reader has no observation facility. A changed version alone preserves the copy.
interrupted probes and hidden version hints preserve the same completed saved
snapshot while the newest validation retains publication authority.
Source-unavailable, revoked authority, observation loss, explicit close and
eviction retain their existing retirement rules. Eviction ends the copy lifetime.

The renderer captures both the content-generation predicate and body signal.
Publication checks those owners even before React unmount runs. Old import
successes/failures cannot publish a scene, overwrite view metadata or clear the
replacement's controls. Camera/part metadata is fenced by copy ID, including a
Refresh that returns the same source version.

Delivery Download independently reads the currently authorized original. It may
therefore differ from an older displayed copy. Workbench retains its existing
source actions; a Workspace path is never converted into a delivery coordinate.
Viewer rejection preserves applicable source actions under their original byte
limits. It does not grant a download above the 32-MiB complete-read limit.

## Runtime and resource limits

`GeometryDocument` is the small shared lazy mount. Ordinary startup loads no
Three.js geometry viewer. GLB uses the main-thread GLTFLoader after preflight.
STL and CAD use separate Worker entry bundles; STL cannot load CAD glue/WASM.
Workers reuse one protocol, admission owner and cancellation/deadline handler.
All assets are shipped locally, with no runtime CDN use.

The following are per visible body unless stated otherwise:

| Resource | Bound |
| --- | --- |
| Complete Session source / applicable original Download | 32 MiB; managed Artifact policy remains 256 KiB |
| GLB JSON / declared accessors / buffer views | 2 MiB / 4,096 / 4,096 |
| Vertices / rendered triangles | 1,000,000 / 1,000,000; repeated mesh instances also consume triangle admission |
| Scene nodes / parts / source hierarchy depth | 512 / 256 / 32 |
| Materials / draw groups, including CAD face ranges | 256 / 512 |
| Decoded vertex/index/attribute buffers | 64 MiB |
| Textures / dimension / aggregate source-image pixels | 8 / 2,048 per side / 8,388,608 pixels |
| Coordinate / matrix magnitude | Finite, absolute value at most 1e12; resulting world bounds are checked too |
| Canvas backing pixels / CSS dimension / pixel ratio | 4,194,304 / 4,096 per side / at most 2, reduced to fit the pixel budget |
| STL/CAD Worker deadline | 15 seconds, parent-owned, includes initialization |
| CAD WASM linear-memory maximum | 2 GiB, verified in both the shipped binary declaration and glue |
| Aggregate renderer contexts / import Workers | 2 / 2, additionally restricted by existing visible-body ownership |
| Physical GLB imports, including retiring image decodes | 2; capacity remains occupied until all declared texture work settles |

Original bytes stay with the content owner. Worker input is a separately charged
source-sized copy, transferred without detaching the retained source. Only
validated typed result buffers leave the Worker. Three.js adopts those arrays
without another full attribute copy. Visible bodies do not allocate unused
original-file Blob URLs. GLB loader-created image URLs/bitmaps are separately
disposed with the scene.
The GLTFLoader texture hook owns partial import results too: decode completion
after cancellation or parse rejection disposes its texture/bitmap even when no
scene can be returned.
LoadingManager also owns generated image URLs through settlement: successful
loads use the loader's revocation path, decode failures revoke through the
manager's error callback, and retired work cannot start another image load.

For two panes, admitted attribute buffers are at most 128 MiB and admitted image
pixels at most 16,777,216. RGBA image storage is approximately 32 MiB per body
before mipmaps/driver overhead. Each parent/Worker input copy is independently
bounded by the 32-MiB source limit. These are allocation/retention admission
bounds, not a hard cap on browser RSS or graphics-driver memory.
Retiring GLB imports have a separate two-slot physical obligation. In the worst
case, two visible completed scenes coexist with two retiring decode batches:
32 textures / 33,554,432 pixels aggregate, approximately 128 MiB of RGBA image
storage before mipmaps/driver overhead. Repeated cancellation cannot admit
additional decode batches while those slots remain occupied.

The stock importer constructs OCCT shapes and JavaScript result arrays before
returning them for output admission. Its WASM maximum excludes JavaScript heap,
temporary JSON/array objects, decoded images and the browser process. Browser
Workers have no general JavaScript heap/RSS limit. Their deadline can terminate
Worker execution; a main-thread GLB parse or GPU submission is cooperative and
cannot be preempted by a timer. Measurements must describe these distinctions.

Idle scenes have no continuous animation loop. Close, hide, supersession and
authority retirement dispose controls/listeners, geometry, materials, textures,
image bitmaps, axes/highlight resources and the renderer, then lose the context.
Context loss releases Three.js's internal fallback textures as well. A stale
completion also disposes its unmounted graph. Native read admission continues to
follow physical settlement independently of browser cancellation.

## Runtime provenance and validation

Three.js is pinned to 0.187.0 (MIT). occt-import-js is pinned to 0.0.23
(LGPL-2.1), importer commit `c2148e54b456b571238d35cac037d304053d64b2`, with
OCCT submodule commit `d2abb6d844231cb8f29be6894440874a4700e4a5` from
`https://git.dev.opencascade.org/repos/occt.git`.
The exact distributed glue/WASM and both distributed license texts are recorded
in `web-console/geometry-runtime.json`. `scripts/geometry-runtime.ts` verifies
their hashes and the actual WASM memory maximum, and checks that the reviewed
WASM enters production assets. The existing notices pipeline includes both
importer and OCCT license texts in the shipped third-party notice file.

Corresponding source and upstream build instructions are available at
[the pinned importer revision](https://github.com/kovacsv/occt-import-js/tree/c2148e54b456b571238d35cac037d304053d64b2),
including the pinned OCCT submodule and Emscripten build scripts. rustX ships the
unchanged release runtime rather than maintaining a fork or changing compiled
memory settings through an ineffective JavaScript option.

Deterministic adapter tests, content-owner regressions, controlled Worker races,
and view-generation tests accompany `test/e2e/geometry.spec.ts`. Browser tests
use the repository's pinned Chromium container and the actual delivery and
Workbench components. They verify format/units, interactions, copy refresh,
current-source Download, lazy startup, external-reference rejection, context
loss and resource counts. The STL envelope case uses 245,000 triangles /
735,000 vertices in a 12,250,084-byte source; record first display and observed
frame gaps together with the browser/container memory scope.

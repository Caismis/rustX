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

### STEP first picture

The first model picture must use independently admitted complete geometry.
STEP/STP reads use ordinary contiguous, authorized ranges of at most 2 MiB.
There are no point-sampling probes, projected convex-hull placeholders or sparse
triangle bridges. Source reading, viewer loading and CAD code compilation can
overlap after explicit demand. Before a real mesh is ready, the viewer displays
its actual loading stage rather than inventing a model picture.
CAD refinement operates on the entire model at each level of detail (LOD).
The Worker parses the source once, then tessellates the same native document
at coarse, medium and final precision. Coarse and medium levels use bounding-box
deflection ratios of 0.02 and 0.005, and angular deflections of 1.0 and 0.75
radians; the final level retains 0.001 and 0.5. Each published level contains
every imported mesh and the complete occurrence hierarchy. Levels replace one
another atomically; completed pieces never accumulate into an incomplete model.
The coarsest available complete tier supplies the first model picture. A ready
finer or final result can skip an unpainted lower tier without artificial delay.

Each optional level is independently bounded to 65,536 rendered triangles and
8 MiB of attribute/index streams, including generated normals, with the existing
node, material and draw bounds. Native preflight rejects an entire oversized
level before exporting its arrays. Producer and receiver independently admit
the complete result, and level numbers must strictly increase. Skipped levels
do not affect final-model admission or extend the original import deadline.
BREP sources with embedded triangulations skip optional retessellation to
preserve their original final geometry. There is no minimum display duration:
a ready final model can skip an unpainted intermediate level.

The localized Refining model label identifies provisional precision; picking,
final occurrence metadata and outlines wait for final admission. Replacing a
level preserves the active camera and gesture, refreshes clipping and navigation
bounds, and retires the old graph after adopting the new graph's shaders. Final
adoption waits for the end of an active gesture and its 120-ms quiet interval.
It reuses the renderer and the attached canvas, preserving keyboard focus
without taking it from another control. LOD gestures seed normalized
direction, pan, zoom, projection and up axis for the final model, with viewport
aspect included in framing. Old callbacks, fences, controls and temporary
resources retire. No provisional data enters the final geometry streams.

A fresh geometry range read authorizes itself; watch registration follows the
complete copy instead of blocking first display. The existing reauthorization
barrier still precedes reuse of a hidden retained copy. Version changes during
reads, errors, hide, cancellation and authority retirement fence provisional
publication. Incomplete source copies are evicted on hide. Source reading yields
between bounded tasks; cancellation does not release a physical read permit
until its transport has settled.

Generated mesh preparation is lossless. Exact float-bit tuples can share a
vertex only when positions, authored normals and any colors all match; triangle
order and winding remain intact. Missing normals and material ranges prevent
this indexing pass. A separate planar pass can replace redundant triangulation
only for an axis-aligned convex disk with constant attributes. Adaptive exact
orientation predicates, opposite edge pairings, one convex boundary, area checks
and unchanged position-derived bounds certify that disk. Unused indexed
coordinates still contribute to the bounds. Holes, seams, disconnected regions,
degenerate faces and unproved surfaces keep their admitted triangles.

Preparation is restricted to meshes of at most one million vertices and one
million triangles, has a
cooperative 200-ms budget and starts no optional work in the
final second of the original import deadline. Long scans and copies check that
budget; individual allocations, garbage collection and sorting cannot be
preempted by JavaScript. The parent still terminates the Worker at its original
deadline. The pass preserves original arrays and returns the admitted original
when optional work cannot finish within its budget. Position/normal/color data,
hierarchy, occurrence IDs and original source bytes retain their owners.

One scene implementation supplies orbit/pan/zoom, fit, isometric and six standard
views, an orientation axis indicator, part selection/highlighting and visibility.
Fit recenters the model without changing the chosen viewing direction; the
view-angle menu explicitly chooses a standard direction.
Reset view restores the initial isometric framing without changing visibility.
Three enlarged SVG arrows supply orientation and navigation, with no visible
signed coordinate labels. Activate an arrow to align its positive axis; activate
it again to reverse the view. Manual orbit updates the next action from the
actual camera direction. Axis actions use orthographic projection; menu actions
retain the chosen projection. Each arrow has a 24-pixel hit target, keyboard
activation and a localized action name; projected pole axes remain addressable.
The View control uses rotation arrows, while Outlines uses a dashed boundary.
Background outline preparation has no visible loading message; busy state and
explicit unavailable/failure feedback remain accessible.
Camera fitting centers a presentation-only parent. The admitted mesh surface,
position-derived bounds and physical scale remain intact; preparation can remove
redundant tessellation. Bounds are recomputed from admitted coordinates,
rather than trusting GLB accessor bounds. Culling/raycast spheres are recomputed
from the same positions. All loaded GLB scenes share resource admission; the
selected scene must independently have nonempty, finite, nondegenerate bounds.
Its already-computed bounds are returned without repeating the attribute scan.
Rejection disposes every loaded scene and preserves the original-file action.
Camera, target, up vector, selected part
and hidden IDs are bounded inert occurrence metadata tied to a content copy.
Isolation, its one previous visibility snapshot and the outline preference use
the same copy fence. Isolation shows the selected subtree and its ancestors,
including a previously hidden selection. Choosing a different isolated part
retains the original snapshot. Exit isolation restores that snapshot exactly;
Show all clears both visibility overrides and isolation. Manual visibility edits
while isolated do not overwrite the snapshot. Each action publishes and draws
once, without cloning scene resources or rereading/tessellating the source.
Focus selected frames its currently visible descendants in centered display
coordinates while preserving direction. A selection behind a hidden ancestor
cannot be focused. Small occurrences use their own framing distance, including
after compatible restoration. Fit frames visible model content and preserves
direction; Reset uses the full model bounds and the initial orientation.
New previews default to orthographic projection. The View menu switches
perspective and orthographic projection on the same
renderer/context. Switching preserves the target, up axis, direction and apparent
vertical size at the target plane. Orthographic zoom saves its effective vertical
span with the copy-bound pose; compatible remounts restore it. Fit, Focus and
Reset retain the chosen projection. Resize preserves effective vertical span,
and picking uses the active camera's projection.
Hiding a selected part also hides its selection outline.
Changing or restoring the up axis recreates OrbitControls through its public
lifecycle so orbit gestures use that axis and old listeners are released.
Pointer gestures retain the latest pose locally and flush it at interaction end;
public input completion also observes camera/target changes below OrbitControls'
absolute event threshold, so tiny focused pans draw and survive retirement.
camera changes outside a gesture publish at most once per 100 ms. Normal
body retirement also saves a pending pose through a separate inert-metadata path
that checks the retained occurrence, source authority and exact content copy.
Revocation, Refresh and occurrence replacement cannot save obsolete view state.
Initial hidden-part restoration publishes once, and camera-only publications do
not rebuild the part rows or their derived metadata. Repeating an unchanged
selection or visibility action does not publish or schedule a draw.
Picking prunes hidden subtrees before nonrecursive raycasts against admitted
original geometry, preserving foreground occlusion even while a display
approximation is pending. Inspection and framing use the same originals. Size, DPR
and quality changes are coalesced into one public drawing-buffer update at the
next draw; an unchanged effective buffer causes no allocation.
Both entry points mount inside the existing document sidebar, retaining its tabs,
path header, source actions, split/fullscreen behavior and content ownership.
The geometry body fills the remaining height without a fixed minimum or a second
outer scroll area. The compact left operator bar shares its presentation module
with the Workflow canvas. Its fit, view-angle and parts controls use DSH tokens,
tooltips and the existing menu facilities.

The part tree is an overlay inside the viewport, closed by default, with initially
collapsed branches. Whole segments of occurrence path IDs establish ancestry;
unnamed intermediate nodes and increasing depths cannot attach unrelated parts.
It supports selection and visibility through the same scene
owner. Its bounded list scrolls independently; Escape closes it and returns focus
to its trigger. Format, dimensions and units occupy a compact footer. There is no
permanent Refresh explanation; the existing document-header action remains.
Name search filters the admitted hierarchy and shows whole-path ancestors of
matches, including collapsed branches. It matches localized fallback names for
unnamed parts. Search does not change selection, visibility, camera state or the
manual expansion set; clearing it restores the prior expansion. Its query is
limited to 256 characters, stays local to the open panel and resets on close or
copy replacement. The search field stays above the independently scrolling list.
The structure panel also reports the selected occurrence (or whole model), its
admitted and visible triangle counts, format/output units and a world-axis-aligned
bounding box. Bounds include occurrence transforms and undo display centering.
Hidden/occluded geometry is not an exact CAD solid classification or measurement.
These properties update on selection/structure changes, not camera gestures.
All product text is localized in the project's concise English/Chinese style.

Model outlines are enabled initially, with an explicit on/off preference saved
for the current copy. Exact-position adjacency is prepared in a disposable Worker
after model import settles. Boundary/non-manifold edges and 30-degree creases
are distinct from paired edges that can become view-dependent silhouettes.
Coplanar tessellation edges are removed; smooth-surface candidates are gated by
adjacent face orientation in the line shader, rather than drawn as wireframe.
These are mesh-derived inspection lines, not exact CAD topology. The shaded
surface, authored normals/materials, MSAA and source geometry remain unchanged.
Lines inherit occurrence transforms and ancestor visibility; they do not
participate in picking. There are no outline render targets or per-camera
adjacency rebuilds. A failed/over-budget request leaves the shaded viewer and
original source actions usable. Translucent/alpha-tested models currently keep
their original shaded view because their silhouettes require texture-aware work.
Generated identity-transform occurrences pack into at most two line draws,
grouped by sidedness. Occurrence identity shares the existing edge-kind slot;
64 packed visibility vectors preserve ancestor visibility and isolation without
rebuilding adjacency on camera motion. The original 5-MiB output cap includes
repeated occurrences. Original mesh data and the inspection/picking graph remain
unchanged. Authored or transformed graphs retain per-occurrence line objects
and local normal transforms.
Anti-fighting converts a depth pull of one part per million of eye distance
through the active perspective or orthographic projection. Orthographic
silhouettes use parallel eye rays. Thin overlapping opaque boxes at small,
ordinary and large scales retain hidden-edge occlusion in both projections.

Loading labels report observed reading, CAD-runtime initialization, import and
display preparation. The synchronous CAD call is one import stage; parsing and
tessellation are not fabricated as separate milestones. Worker observations do
not validate a model, extend its deadline or survive body retirement. Main-thread
preparation timing retains only the latest measurement per supported format.

PBR materials use bounded, fixed studio lighting: a hemisphere light and two
off-axis directional lights, with neutral tone mapping. No environment-map asset
or synchronous environment prefiltering is needed. Source material semantics,
represented mesh surfaces, bounds and scale are preserved; lighting belongs to
the visible scene. Generated
materials use the original Standard shading, metalness 0.05 and roughness 0.65;
renderer MSAA remains enabled.

Small generated assemblies can combine identity-transform occurrences that share
one opaque material into an exact draw batch using Three.js `mergeGeometries`.
This preserves every position, normal, index and triangle without transforming
source floats. Combined input is capped at 16 MiB and groups smaller than four
occurrences stay separate. Hidden ancestors, changed geometry, transparency and
transformed occurrences retain their original draws. Batches attach only during
synchronous submission and are removed in `finally`; original selection,
hierarchy, visibility and inspection remain authoritative. Their copied buffers
are disposed with the body. This reduces small-assembly draw overhead without
lowering pixel quality or shading.

Initial and moving pictures retain the original PBR materials, authored normals,
colors and textures. Their pixel ratio is at most 0.5 and one megapixel. Small
meshes, GLB, colored meshes and material-seamed meshes retain their original
geometry. For eligible generated meshes the import Worker prepares an optional
surface representation using the locked meshoptimizer 1.1.1 simplifier. Locked
borders, attribute-aware joining and a relative error limit of 0.002 protect
boundaries and shading; the renderer additionally requires at most one CSS pixel
of projected error. Near views that exceed this limit draw the original geometry.
This is a display approximation, not a replacement for the admitted model.

Large binary STL first attempts exact retriangulation of consecutive axis-planar,
constant-normal runs in the already owned source buffer. Every source triangle
must have the same nonzero robust orientation within its run. A fixed-capacity
directed-edge table cancels opposite edges; the complete remaining boundary must
be one simple convex cycle. Its winding number proves complete coverage without
a floating-point area tolerance. Positions and normals retain their original
source bits. All runs must certify within the existing output and time budgets;
an unproved run abandons the entire candidate. No sampled prefix is published.
The exact result has zero surface error and does not replace final geometry or
picking. Holes, curved faces and other uncertified inputs use the standard
simplifier, loaded only when needed.

That standard path processes fixed 786,432-vertex chunks from the same owned
source buffer. Shared chunk borders remain locked until an attribute-aware
join removes their redundant triangulation. Other jobs copy at most 16 MiB of
attributes in total. Output is bounded to 200,000 unique triangles and 20 MiB,
including original position/normal vertex IDs. The receiver independently checks
types, backing buffers, counts, exact source float bits, finite error and whole
mesh extents before adopting it. Original geometry, materials, transforms,
hierarchy, bounds and inspection counts remain authoritative.

While that bounded background job is pending or unavailable, the intact original
mesh remains visible. Sparse source-triangle sampling is not a display option.
This can increase transient GPU work for a large model without a ready accepted
surface; it does not justify displaying missing geometry. Every settled draw
uses the original. Selection boxes and axis
navigation remain visible. Mesh outline shaders pause during motion and return
with the settled picture. Geometry references are restored synchronously after
every submission, including on exceptions. Picking always uses originals.

After background work settles, 120 ms without a changed view and the end of the
gesture allow one frame to restore
every admitted triangle, authored material/normal/color/texture, mesh outline
and the original full-resolution pixel policy. New actions cancel/coalesce that
refinement. A gesture with no pose/selection change schedules no extra work.
An idle view has no quality loop. All optional surface buffers are disposed
with the visible body. `data-geometry-lod` identifies `coarse`, `medium` or `full`
geometry. `data-geometry-quality` distinguishes provisional geometry from
`interactive` and settled `complete` final-model draws.
`data-geometry-completeness` separately identifies `preview` versus `complete`
admission before pixel refinement. These are
diagnostic states, not GPU-completion or presentation timestamps.

### Adaptive LOD assessment

Native CAD coarse/medium/final tessellation parameters remain fixed. The
prepared meshoptimizer surface already has camera-dependent one-CSS-pixel
admission relative to the admitted mesh; this is not an analytic CAD error
guarantee or adaptive native retessellation.

A screen-space-error policy is feasible. In orthographic projection, projected
error is approximately `worldError * viewportHeight / effectiveVerticalSpan`;
zoom and viewport size therefore matter even when camera distance is unchanged.
A future native-tier selector should choose the coarsest complete tier meeting
the pixel budget, with hysteresis and the existing memory/Worker limits. It
needs independently admitted tessellation-error metadata and bounded ownership
of reusable whole-model tiers. It must never substitute point or triangle
samples when the desired tier is unavailable.

[Cesium's screen-space-error refinement](https://cesium.com/learn/cesiumjs/ref-doc/Cesium3DTileset.html#maximumScreenSpaceError)
is an established pattern for the pixel criterion;
[Three.js LOD](https://threejs.org/docs/pages/LOD.html) documents distance-based
selection and hysteresis. Distance alone is insufficient for this viewer's
orthographic zoom. This assessment is a design direction, not a claim that
native adaptive tier selection has shipped.

The acceptance workload is approximately 200 MiB of source geometry on the
project's fixed browser/test machine. First demand to a usable model picture
must be at most 200 ms; every observed interaction must be below 50 ms. Initial
and moving pictures may be lossy, while a settled picture must use all admitted
geometry. The [surface-preview report](issue-516/surface-preview-optimization.md) records the
actual failing gate and remaining source/import costs. Loading text and an
empty canvas do not satisfy first display; P95 does not replace the maximum
latency requirement. The current raw-file pipeline does not meet this contract.

The renderer owns at most one submitted GPU frame/fence. While it is busy, new
camera changes replace the pending pose instead of submitting obsolete frames.
WebGL 2 fences are checked without blocking; initialization/wait failure produces
the existing WebGL error. Idle views schedule no polling or drawing. Retirement
deletes the fence with the remaining graphics resources.

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
cache. A complete authorized read establishes the fixed full-source copy.
Geometry Session
files and Workbench sources use the existing at-most-2-MiB byte ranges to assemble
at most 200 MiB in one exact-size buffer. Every range reauthorizes its source and
checks offset, size, version, EOF and response length. A changed version/size
rejects the assembly; cancellation waits for the current physical read to settle
and starts no next range. No partial assembly is published or retained as a copy.
Ordinary complete reads and original Download retain their 32-MiB bound. Source-version
hints still validate authority but cannot replace that copy. Refresh starts a
new content generation and reauthorizes the current source; loading/errors follow
the existing preview flow. Other viewers keep their automatic-refresh behavior.

Hidden bodies terminate their import Worker and release every renderer resource.
Small admitted source copies can remain in the existing 8-entry/64-MiB inactive
cache. A larger copy cannot fit that cache: its content owner relinquishes the
buffer directly to the ready STL/CAD Worker instead of making another source-sized
copy. GLB releases its large raw copy after successful admission. The detached
data object remains only as the current copy token; hiding evicts it, so reopening
performs a newly authorized read. Retirement, Refresh and failed imports cannot
reuse detached bytes. Download remains an independent original-source action.
Separately, the page may retain one compiled immutable CAD code module, just as
it retains loaded JavaScript code. It contains no source, parsed model, WASM
instance or linear memory. Every admitted CAD import instantiates fresh memory
inside its own disposable Worker; retirement never retains that Worker.
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
therefore differ from an older displayed copy. Workbench's existing split action
defaults to opening the file location through the Host's admitted `files` action.
The adjacent application chooser remains available for an explicit alternative;
the chosen application's name labels subsequent opens. A missing file manager
does not silently select an editor. A Workspace path is never converted into a
delivery coordinate.
Viewer rejection preserves applicable source actions under their original byte
limits. It does not grant a download above the 32-MiB complete-read limit.

## Runtime and resource limits

`GeometryDocument` is the small shared lazy mount. Ordinary startup loads no
Three.js geometry viewer. Explicit geometry
demand starts code loading alongside the already-authorized source read.
Resolved eligible code mounts through explicit component state, without a
Suspense fallback throttle. GLB loads its
GLTFLoader only for GLB demand and uses it on the main thread after preflight.
STL and CAD use separate Worker entry bundles; STL cannot load CAD glue/WASM.
Workers reuse one protocol, admission owner and cancellation/deadline handler.
All assets are shipped locally, with no runtime CDN use.
STEP/IGES/BREP code compilation starts on explicit visible demand alongside
source and viewer code loading; STL demand does not compile CAD. The original parent
deadline and two-owner reservation include any remaining compilation wait. Cancelled waiters
clear their source and callbacks and cannot create a late Worker. A failed code
compilation clears the shared promise for an explicit retry; static asset fetch
has its own bounded timeout. Exact binary/glue hashes and the modified interface
source are pinned and checked by the runtime provenance script.

The following are per visible body unless stated otherwise:

| Resource | Bound |
| --- | --- |
| Geometry Session/Workbench source / applicable original Download | 200 MiB through bounded ranges / 32 MiB complete read; managed Artifact policy remains 256 KiB |
| GLB JSON / declared accessors / buffer views | 2 MiB / 4,096 / 4,096 |
| Vertices / rendered triangles | 13,000,000 / 5,000,000; repeated mesh instances also consume triangle admission |
| Admitted scene nodes / parts / source hierarchy depth | 512 / 256 / 32; optional line objects are charged separately |
| Materials / draw groups, including CAD face ranges | 256 / 512 |
| CAD BREP-face metadata | 1,000,000 faces total; separate from material draw ranges. Adjacent equal-color ranges merge, and base-color faces require no override |
| Decoded vertex/index/attribute buffers | 512 MiB |
| Optional whole-model CAD LOD | At most two levels; each has 65,536 triangles / 8 MiB including generated normals; existing aggregate node/material/draw bounds apply |
| Textures / dimension / aggregate source-image pixels | 8 / 2,048 per side / 8,388,608 pixels |
| Coordinate / matrix magnitude | Finite, absolute value at most 1e12; resulting world bounds are checked too |
| Canvas backing pixels / CSS dimension / pixel ratio | 4,194,304 / 4,096 per side / at most 2, reduced to fit the pixel budget |
| Viewer lighting | One hemisphere and two directional lights per body; no environment render targets or shadow maps |
| STL / full CAD Worker deadline | 15 / 60 seconds, parent-owned, includes initialization and remaining code wait; separate from the 200-ms first-model requirement |
| CAD WASM linear-memory maximum | 2 GiB, verified in both the shipped binary declaration and glue |
| Aggregate renderer contexts / import Workers | 2 / 2, additionally restricted by existing visible-body ownership |
| Submitted GPU frames / owned fences | 1 per visible body, 2 aggregate |
| Optional surface input / decoded STL chunk | 16 MiB total copied input, or one 786,432-vertex position chunk from the owned raw STL; no full decoded-model copy |
| Exact STL boundary work | 16,384 fixed hash slots, at most 8,192 live edges, 32-probe limit; output source IDs share the existing surface-output bound |
| Optional surface output / error / Worker budget | 200,000 unique triangles / 20 MiB including provenance; relative error at most 0.002, projected error at most 1 CSS pixel; 2 seconds within the original deadline |
| Exact small-assembly batch input | At most 16 MiB per body, including repeated occurrences; original attributes/materials and all triangles retained |
| Optional outline unique input triangles / copied attributes and indices | 100,000 / 16 MiB per body |
| Optional outline segments, including repeated occurrences / extra draws | 65,536 / 256 per body; 131,072 / 512 for two bodies |
| Optional outline output vertex buffers | 5 MiB per body, 10 MiB aggregate (80 bytes per segment); at most two shared line materials per body |
| Optional outline Worker deadline | 2 seconds, followed by termination; packing and receiver validation are cooperative |
| Physical GLB imports, including retiring image decodes | 2; capacity remains occupied until all declared texture work settles |

Copies that fit the inactive cache stay with the content owner and use a separate
Worker input copy. Larger copies transfer ownership directly at Worker dispatch.
The receiver admits every generated numeric stream, including unreferenced meshes,
in a single scan that also derives bounds. No repeated receiver value scan or
full attribute copy is needed. Optional surface work starts only after receiver
admission and the first GPU fence complete; the same import Worker and its owner
remain responsible for it. The first-frame fence poll stops after that milestone.
Visible bodies do not allocate unused
original-file Blob URLs. GLB loader-created image URLs/bitmaps are separately
disposed with the scene.
The demand-owned renderer context and trusted three-vertex PBR shader preparation
overlap import. This preparation submits no model draw. Early cancellation,
context loss or import failure disposes it under the same two-context cap.
Trusted camera inputs submit the latest coalesced pose at the microtask checkpoint,
avoiding an extra animation-frame wait while retaining the single GPU-fence limit.

An outline Worker starts only after import and optional surface work settle.
Manual requests wait behind that same barrier, and only the latest preference
can start work. Off cancels a queued request immediately; hide/disposal resolves
waiting requests without creating a late Worker. A visible body owns at
most one import or outline-preparation Worker at a time. Input buffers are
separate copies; output backing buffers, values and segment/draw totals pass
independent receiver checks. The adjacency job holds at most 300,000 edge records
and 1,000,000 position-key records, plus typed input, normals, remapping and output
arrays. Output construction can transiently hold two 5-MiB arrays. These are
numeric admission bounds, not a JavaScript heap/RSS limit. Disable, hide,
replacement, close or context loss terminate pending work and dispose all line
buffers/materials. Default shaded-model limits remain unchanged.

Optional indexing uses at most about 16 MiB of hash/remap/index workspace for
one million source vertices, plus output arrays whose decoded size must decrease.
Larger meshes skip optional indexing/planar preparation and retain original
arrays; increased source admission does not enlarge this temporary workspace.
Planar certification can use up to 64 MiB of edge tables, about 4 MiB of vertex
links and bounded 8,192-vertex boundary/hull lists. Meshes are processed
sequentially; earlier temporary arrays may remain until collection or Worker
termination. These transient allocations are additional to source, decoded-model
and WASM storage, and are not a browser RSS cap.
Canceled HTTP reads still consume and validate the complete bounded multipart
terminal response before releasing physical admission. Once canceled, a valid
attachment remains in the parser's Blob without an unused final ArrayBuffer
copy. Malformed attachments and unknown physical settlement keep their existing
error precedence; the carrier is not aborted to save that allocation.
The GLTFLoader texture hook owns partial import results too: decode completion
after cancellation or parse rejection disposes its texture/bitmap even when no
scene can be returned.
LoadingManager also owns generated image URLs through settlement: successful
loads use the loader's revocation path, decode failures revoke through the
manager's error callback, and retired work cannot start another image load.

For two panes, full-model admitted attribute buffers are at most 1 GiB and admitted image
pixels at most 16,777,216. RGBA image storage is approximately 32 MiB per body
before mipmaps/driver overhead. An active and incoming whole-model LOD can
temporarily coexist, adding at most 16 MiB per body / 32 MiB for two bodies.
They retire during final-model adoption before optional surface refinement.
Each raw input buffer is independently bounded
by the 200-MiB geometry source limit. Large STL/CAD transfer ownership instead
of retaining a second copy; two such sources total at most 400 MiB. GLB may
retain source and loader copies until admission, reaching 800 MiB for two bodies,
additional to decoded buffers and WASM. Source
size alone does not guarantee a model fits the separate decoded, node, material
or runtime bounds. These are allocation/retention admission
bounds, not a hard cap on browser RSS or graphics-driver memory.
Retiring GLB imports have a separate two-slot physical obligation. In the worst
case, two visible completed scenes coexist with two retiring decode batches:
32 textures / 33,554,432 pixels aggregate, approximately 128 MiB of RGBA image
storage before mipmaps/driver overhead. Repeated cancellation cannot admit
additional decode batches while those slots remain occupied.

The importer constructs OCCT shapes and JavaScript result arrays before
returning them for output admission. Its WASM maximum excludes JavaScript heap,
temporary JSON/array objects, decoded images and the browser process. Browser
Workers have no general JavaScript heap/RSS limit. Their deadline can terminate
Worker execution; a main-thread GLB parse or GPU submission is cooperative and
cannot be preempted by a timer. Measurements must describe these distinctions.

Idle scenes have no continuous animation loop. Close, hide, supersession and
authority retirement dispose controls/listeners, geometry, materials, textures,
image bitmaps, axes/highlight resources and the renderer,
then lose the context.
Context loss releases Three.js's internal fallback textures as well. A stale
completion also disposes its unmounted graph. Native read admission continues to
follow physical settlement independently of browser cancellation.

## Runtime provenance and validation

Three.js is pinned to 0.187.0 (MIT). The exact orientation predicate is supplied
by robust-predicates 3.0.3 (Unlicense). The local occt-import-js build is pinned
to 0.0.23-rustx.3 (LGPL-2.1), derived from importer commit
`c2148e54b456b571238d35cac037d304053d64b2`, with
OCCT submodule commit `d2abb6d844231cb8f29be6894440874a4700e4a5` from
`https://git.dev.opencascade.org/repos/occt.git`.
The exact distributed glue/WASM and both distributed license texts are recorded
in `web-console/geometry-runtime.json`. `scripts/geometry-runtime.ts` verifies
their hashes and the actual WASM memory maximum, and checks that the reviewed
WASM enters production assets. The existing notices pipeline includes both
importer and OCCT license texts in the shipped third-party notice file.

Corresponding source and upstream build instructions are available at
[the pinned importer revision](https://github.com/kovacsv/occt-import-js/tree/c2148e54b456b571238d35cac037d304053d64b2),
including the pinned OCCT submodule and Emscripten build scripts. rustX ships
a bounded-transfer and whole-model LOD build. Typed input copies
in bulk, numeric output uses fixed 4096-double chunks, exclusively owned Worker
input can be released after native copy, and native input is freed before mesh
extraction. Optional complete-model callbacks export coarse and medium
tessellations of the same parsed document, with independent stream ownership.
Later passes clear temporary triangulations before remeshing; embedded BREP
triangulations retain the original path. Final CAD tolerances, geometry
algorithms, allocator and the 2-GiB memory maximum remain intact.
The runtime audit hashes all five modified C++ files. Their complete source, pinned revisions,
licenses and rebuild script ship at `cad-runtime-source/` in the production
artifact; the repository copy is `web-console/vendor/occt-import-js/`.

Deterministic adapter tests, content-owner regressions, controlled Worker races,
and view-generation tests accompany `test/e2e/geometry.spec.ts`. Browser tests
use the repository's pinned Chromium container and the actual delivery and
Workbench components. They verify format/units, interactions, copy refresh,
current-source Download, lazy startup, external-reference rejection, context
loss and resource counts. The STL envelope case uses 245,000 triangles /
735,000 vertices in a 12,250,084-byte source; record first display and observed
frame gaps together with the browser/container memory scope.

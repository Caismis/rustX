# Approximately 200 MB STEP: first picture, complete geometry and resources

## Requirement and outcome

On the existing fixed browser/test machine, opening approximately 200 MB of
geometry must show a usable source-derived picture within 200 ms, keep every
observed interaction below 50 ms, and restore complete admitted geometry when
quiet. Temporary quality loss is allowed. Original source bytes and configured
CAD import precision remain unchanged; resource ownership stays bounded.

Computed partial results from the full CAD calculation are displayed as they
become available, then the complete model takes over without changing its
precision. The 200,761,968-byte STEP reference passes three independent native Host runs
and three production-fixture runs. Native first picture is
**62.2–62.8 ms**; observed coarse/fine maxima are
**42.0 / 33.7 ms**.
The complete native-path mesh arrives in 34.0–36.1 s.
These are separate clocks. The initial picture is a labeled coordinate
envelope, not the admitted CAD mesh. General STEP assemblies and the broader
format/model matrix remain uncertified; PR #517 stays draft.

## Source and measurement

Recorded on 2026-11-05 (Asia/Shanghai). The original MIT generator creates one degree-three curved B-spline face with
2,102,500 referenced poles (1450 per axis), in millimeters. Every pole belongs
to the face; no padding, unused entities or repeated comments inflate the file.
The source is 200.762 decimal MB / 191.462 MiB, within the 200-MiB admission cap.
It exercises large source parsing with a small final tessellation, rather than
a multi-part assembly or many-faceted CAD workload. The generated file stays
outside Git. SHA-256:

`0bcf7e5313f6b770cd7028360ddb5a88312c3ee8a22c1892421d366bdcf6f81b`

The machine is AMD Ryzen 7 9800X3D / Windows with WSL2, using the repository's
pinned Playwright 1.63.0 Chromium 153 / SwiftShader image, 1440×1000, DPR 1,
without an explicit CPU or memory quota. Runs are sequential with no concurrent
builds/tests. Browser contexts are fresh; OS filesystem/code caches are not
explicitly flushed. Test-machine/image details and 86 matching tested source hashes
are in [the environment record](step-200mb-environment.json).

The native lane opens an actual committed delivery in the production application
through App Server, Product Host authorization and native filesystem/range
transport. It does not preload source bytes into a browser fixture. The clock
starts at trusted Preview pointerdown in an already loaded application; it
excludes browser/application/server startup. SVG first picture is a nonempty
source-derived path followed by two animation-frame opportunities. The full
mesh clock retains actual model submission, GPU-fence completion and next-frame
opportunity. Neither endpoint observes compositor/physical display presentation.

| Lane, three fresh contexts | First rough picture | First fine mesh | Maximum rough input | Maximum fine input |
| --- | --- | --- | --- | --- |
| Native Host / production application | 62.2–62.8 ms | 34.0–36.1 s | 42.0 ms | 33.7 ms |
| Production component fixture | 32.0–36.8 ms | 23.7–23.9 s | 33.4 ms | 41.9 ms |

Each run contains 100 trusted coarse and 100 trusted fine pointer-move samples.
The native gestures alternate direction to retain a visible model; the fixture
retains its original cumulative gestures. Both check the maximum, not P95.
Every settled run draws all 216 imported triangles and returns Worker, context,
buffer, texture and URL counts to zero on close. The fixture's source exists in
memory before demand, so it cannot substitute for the native transport result.
Raw arrays: [native](step-200mb-native-samples.json),
[fixture](step-200mb-samples.json).

## Implementation and display quality

The first authorized STEP range is 32 KiB, followed by at most four 32-KiB
windows. At most 128 finite CARTESIAN_POINT coordinates per window form a
projected convex envelope. The first noncollinear projection draws immediately;
later samples improve its extent. This path retains at most 640 points /
7,680 bytes and needs no Three.js, Worker or WebGL context. Offset, source
version, total size, length and EOF must agree across every physical read.
No degenerate sample becomes a placeholder model.

The envelope can omit holes, concavities and disconnected parts, include control
or construction coordinates, and does not apply assembly placements, topology
or materials. It is explicitly labeled Rough preview and supplies no picking,
engineering measurements or authoritative model metadata. This reference's
thin surface is visually close to its envelope; that is not a universal quality
bound. A complex assembly still needs its own acceptance source and visual proof.

Complete source reading continues in bounded ranges with cooperative task
yields. A fresh range already authorizes access, so native watch registration
follows the complete copy. Hidden retained copies still reauthorize before reuse.
Errors, version mismatch, retirement and cancellation fence provisional data;
an incomplete hidden copy is evicted. Physical read permits remain owned until
transport settlement.

The full-view bootstrap starts after source completion and 120 ms without rough
input. This avoids CAD download/compilation competing with first probes and
initial engine setup interrupting an active rough gesture. Once admitted, CAD
continues in its disposable Worker. Rough camera gestures seed normalized
direction, target and zoom. The envelope yields to the first GPU-ready computed
fragment when one is available; the complete model later replaces that
provisional graph. Full CAD has a 60-second parent deadline; STL and
immutable code fetch retain 15 seconds. These import deadlines do not relax the
200-ms first-picture or below-50-ms input gate.

The local occt-import-js 0.0.23-rustx.2 build changes only its Embind interface:
bulk numeric input, fixed 4096-double output chunks, release of exclusively owned
JS input after native copy, release of the native input vector before mesh
extraction, and optional completed-mesh notifications during export. It keeps upstream -Oz, allocator, geometry algorithms, numeric order,
CAD tolerances, exceptions and 2-GiB WASM maximum. Complete modified C++, pinned
revisions, licenses and rebuild instructions ship alongside the binary in
`cad-runtime-source/`. Asset/source hashes are checked during builds.

The adapter validates values while copying into typed arrays, independently
of the receiver's numeric admission. CAD face metadata has its own one-million
face cap. Equal base colors need no draw group, adjacent equal face colors
coalesce, and gaps/alternating colors preserve the separate 512-draw limit.
Manual outlines now share the surface/import retirement barrier and retain the
latest Off preference. If optional surface preparation times out, moving views
keep their bounded shaded bridge; settled draws and picking use original geometry.

### Computed partial results

Completed CAD meshes are forwarded during native hierarchy export while remaining
export and final typed conversion are pending, with final positions, normals,
indices and colors.
The producer and receiver independently bound the whole prefix to 16 messages,
65,536 triangles, 8 MiB including generated normals, and the existing aggregate
material/draw limits. Sequence and mesh-index ordering are checked. Partial
streams and coalesced color tuples have separate ownership; a native regression
mutates the displayed fragment and proves the final result remains identical.

Fragments accumulate in a labeled provisional graph, with picking, occurrence
metadata and outlines disabled until the complete model is admitted. Quiet
handoff preserves normalized pan, direction, zoom, projection and up axis,
including narrow viewport framing. One renderer/context is reused; old controls,
callbacks and GPU fences retire, and old materials are released after the full
graph's shaders are adopted. An already-ready final result skips an unpainted
prefix. Cancellation fences late fragments and a final result held in transit.

The 96-part assembly lifecycle demonstration grows from 444 triangles to
17,824 complete triangles, with original shading/holes rather than a coordinate
envelope. Its message delivery/final result is deliberately held to inspect
each stage; it is not a cold-performance result. Raw native availability and
trusted partial input samples are in
[the progressive capture](step-progressive-samples.json): 69 trusted partial
input samples have a 33.5-ms maximum and one created context. The
[ordinary CAD capture](step-progressive-natural.json) separately records an
unheld 113.4-ms window from first mesh availability to terminal import and one
actual partial draw submission in that window. Later partial submissions are
also retained; these submission observations are not physical presentation.

Document transfer and root triangulation still precede mesh export. The large
single-face reference has no separately observed calculated-prefix picture in
these runs; its rapid source envelope remains until the full mesh. This mechanism
does not invent progress during atomic parser/tessellator work or replace the
need to certify a complex approximately 200-MB assembly.

![First completed part](cad-partial-first.png)

![More computed geometry](cad-partial-more.png)

## Fidelity and resource evidence

The full reference output retains 118 vertices and 216 triangles with identical
position/normal/index stream digest before and after the transfer build:

`8101630ecc9f59c3a50c4a4f38c1113af605857f7c329ff00fadc3d0355ced72`

Configured CAD triangulation remains bounding_box_ratio 0.001 / angular 0.5,
with millimeter output. Complete display adds no further mesh loss relative to
that original import result; this is not an analytic-NURBS exact-display claim.
Six small independent STEP/inch/IGES/BREP/assembly fixtures also retain complete
JSON and rendered-stream digests in the native differential harness.

One isolated sequential full-completion Node Worker comparison, with the same
source, no display-prefix callback and a diagnostic 60-second deadline, records:

| Native diagnostic | Upstream SDK | Bounded-transfer SDK |
| --- | --- | --- |
| Read/import call | 40.88 s | 21.12 s |
| Process peak RSS | 1,412,984 KiB | 1,263,040 KiB |

That pair reduces import time by 48.3% and process RSS by
10.6%.
It is one diagnostic pair, not a statistical confidence interval or a UI clock.
Process RSS includes Worker and input allocations, not per-Worker attribution.
WASM exported memory remains 1,053,491,200 bytes; freeing buffers does not shrink its
grown linear memory. Worker retirement releases the instance.

The dedicated completed three-context fixture browser container peaks at
2,325,893,120 bytes, with no OOM/kill. This includes fixture storage,
network forwarding, browser/renderer/cache, and excludes Node/native Host and
external driver storage. It includes test tracing/forwarding, is not a
before/after browser-memory reduction claim or a hard RSS budget, and cannot be
compared to an old 15-second timeout as a completed-model memory reduction.
The progressive feature trades bounded extra allocation (up to 8 MiB of prefix
streams per body during full adoption) for earlier calculated detail.

## Regression and remaining limits

- Full Web/Host suite: 4,220 passed, four platform skips. Type checking, both
  production builds, localization, 336 provenance records, 346 production
  notices, pinned runtime assets and production source offer pass.
- Native and fixture strict STEP gates each pass three contexts. All 45 production
  functional browser cases and seven development/native-source regressions pass,
  including both progressive lifecycle cases. Current-head CI is separate.
- Source/version races, physical cancellation, provisional retirement, deferred
  watch setup, quiet bootstrap through sampling/drag/keyboard input, normalized
  handoff, CAD buffer ownership, face-color batching and pending-outline
  serialization have deterministic regressions.

[Before-runtime samples](step-200mb-before-samples.json) retain three
15-second failures with the stock SDK and the new typed adapter; that variant
is not an immutable 3be97a1f4 baseline. [Before-quiet samples](step-200mb-before-quiet-samples.json)
retain the intermediate strict input failures (59.2–67.1 ms): first picture was
fast, but engine setup overlapped rough input. They are not accepted results.
Earlier [large-STL/surface measurements](surface-preview-optimization.md) keep
their named revision and unresolved first-display gap; smaller CAD fixtures or
this single-surface STEP cannot certify a different model class. No threshold,
screenshot reference or CAD precision was weakened to mark this reference green.

Reproduction, including native Host setup and the source-path opt-in, is in
[the benchmark instructions](production-fixture-benchmark.md).

![Native rough source envelope](step-200mb-native-rough.png)

![Complete imported geometry after reset](step-200mb-native-complete.png)

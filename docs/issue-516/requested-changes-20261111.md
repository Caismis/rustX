# Requested-changes corrections for PR #517

The [formal review](https://github.com/Caismis/rustX/pull/517#pullrequestreview-5854777822)
identified four correctness defects in `16871aec4dfdd10bea531bc245581537c0ce26f0`.
These corrections retain source authorization, fixed-copy identity, physical
read/decode settlement, one renderer/canvas, original-geometry picking and
outline-worker sequencing. Resource limits and the one-CSS-pixel contract are
unchanged. Review approval and performance acceptance remain separate.

## GLB backing allocations

Preflight follows the pinned loader's cache identity: each loaded bufferView
allocates its entire declared range once. Distinct view indices are charged
separately even when their BIN ranges overlap. Mesh attributes and indices
reachable from any declared scene contribute their views; orphan accessors and
meshes remain validated but do not cause allocation charges. The texture owner
loads every declared texture before scene construction, so their embedded image
views also contribute. Shared loaded views are charged once. The existing
logical-accessor expansion checks remain independent of this backing budget.

The receiver independently counts the complete `ArrayBuffer.byteLength` behind
every admitted geometry attribute and index, including interleaved data. Buffer
identity is deduplicated across attributes, indices, geometries and scenes.
Unsupported attribute storage is refused before scanning or rendering. The
existing logical geometry, topology, texture, draw, bounds and provenance checks
remain in force.

Regressions use an actual loader import retaining eight distinct 1-MiB buffers
behind 288 logical position bytes. A 132-view, 4-MiB-per-view allocation plan is
refused before `parseAsync`, without materializing 528 MiB. Shared and unreachable
views have positive controls. Receiver tests use a test-local 1-KiB cap and real
small backing allocations to verify padded attributes, aggregate ownership,
interleaved data, indices and shared-buffer deduplication. The product cap remains
512 MiB; these checks do not claim to bound total browser RSS.

## First task model/profile selection

The dialog passes the displayed effective `selection` into the existing model
selection helper. A first same-model profile change retains request parameters,
output limit and all four auxiliary policies. A different primary model clears
only the primary-owned profile/request/output fields while retaining those
auxiliary policies. An untouched picker still omits `settings.model`.

Three real-dialog/server-fixture request regressions cover those outcomes using
a rich default configuration. They assert the complete `thread/start` request
and preserve the no-resume/no-turn side-effect checks. Native model ownership and
the shared helper are unchanged.

## ASCII STL structural records

Admission consumes the complete `solid` and `endsolid` name records before
counting vertex/facet tokens. Free-form names therefore cannot introduce false
structural counts. Case and leading-whitespace normalization still preserve
names and feed the representation actually checked to the pinned STLLoader.

Actual-loader cube comparisons cover `Cube`, `solid plate`, `Vertex Block` and
`facet normal housing`, retaining names and identical position/normal arrays.
Multiple mixed-case/indented solids and the unchanged 256-part boundary have
regressions. Binary classification and geometry budgets remain unchanged.

## Perspective motion LOD

Selection uses both CSS viewport dimensions, the current camera pose and the
complete original bounding box transformed to camera space. Let `E` bound
camera-space simplification displacement, `z` be minimum positive source depth,
`R` the maximum source distance from the view axis, and `F` the maximum CSS focal
scale. Perspective displacement is conservatively bounded by

```
F * E / (z - E) * sqrt(1 + (R / z)^2)
```

For source point `p` and displacement `d`, the projected difference has numerator
`d.xy - p.xy * d.z / p.z` and denominator `p.z + d.z`. The expression bounds both
the off-axis depth contribution and the perturbed depth denominator over the
complete box. Affine amplification includes nonuniform scale and shear via an
upper bound on the largest eigenvalue of `A^T A`; transform roundoff and finite
arithmetic receive conservative padding. Orthographic selection uses its linear
CSS scale. Nonfinite/unknown projections, unsafe camera-plane proximity and
unprovable bounds retain original geometry. Motion DPR does not relax the CSS
error limit. Settled drawing and picking continue to use original geometry.

The real preview-job/Meshoptimizer regression produces 610 triangles from the
45,602-triangle shallow surface and reproduces the review's visible displacement
greater than 1.4 CSS pixels at a panned 4096-by-480 perspective view. Selection
keeps the original there, including an additional affine-shear case, while a
safe centered view still admits the prepared LOD. Updated camera poses, unsafe
depth, unknown projection skew and geometry-reference restoration are checked.

## Validation and acceptance

The new counterexamples reproduced the old defects before implementation.
Focused geometry, task creation, model-selection and scheduler coverage passes;
the real simplifier/perspective regression also passes under the normal Linux
Vite configuration. Full local Web/Host, type, production and audit results are
recorded with this delivery. FULL CI must verify the pushed HEAD independently;
the reviewed revision's [successful run](https://github.com/Caismis/rustX/actions/runs/42958477060)
is baseline evidence only.

The approximately-200-MiB STEP first usable model picture within 200 ms and every
observed interaction below 50 ms remain an unresolved acceptance gate. The two
large-STEP browser cases intentionally require their separate fixture/budget
setup. Default CI success does not close that gate or grant merge readiness.

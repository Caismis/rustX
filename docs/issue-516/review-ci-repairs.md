# PR 517 review and CI repairs

Reviewed on 2026-11-06, Asia/Shanghai, from `2b46317c5`, against
[review 6654200365](https://github.com/Caismis/rustX/pull/517#issuecomment-6654200365)
and [failed CI 41998611497](https://github.com/Caismis/rustX/actions/runs/41998611497).
The PR remains draft. This increment repairs failures; it does not certify the
200-ms first-model / below-50-ms maximum-input requirements.

## Refresh and contiguous reads

`GeometryDocument` retries a failed viewer import when its existing copy ID
changes. A new attempt clears the old error. Cleanup fences late completion
from the previous copy; cancellation, ordinary rerenders and an already loaded
viewer retain their existing behavior. No pane remount or new module cache is
introduced.

The regression uses the real `ArtifactPreview`, `PreviewContentOwner` and Refresh
button. It proves recovery in the same mounted body with the same abort signal,
then proves a later Refresh does not reload successful viewer code. Late success
and rejection from an old copy are independently fenced. All three cases fail
against the previous implementation and pass with the repair.

The CAD performance test derives its expected content-read count from the actual
fixture size and the existing page size. The 168,057-byte STEP fits one 2-MiB
read. Hide/show and picking must still preserve that count; no timeout or latency
threshold changes.

## Large STL preparation

The original standard surface preparation is sensitive to its unchanged 2-second
budget: on this machine the 4,191,372-triangle binary STL takes approximately
1.5–1.7 seconds, while 750/1,000-ms diagnostic budgets produce no complete result.
An unavailable surface leaves every original triangle visible. The old CI trace
confirms extremely slow input, but does not distinguish a producer timeout from
receiver retirement or pixel-error rejection; no more specific CI cause is claimed.

The new path proves complete planar runs directly from all source triangles.
Robust consistent orientation and a single simple convex boundary certify exact
coverage through the oriented-chain winding number. Opposite edges cancel in a
fixed-size hash table; positions and authored normals retain source-bit provenance.
This accepts no coordinate envelope, sparse triangles, bounding-box substitution
or partially certified prefix. Curved faces, holes and unproved inputs retain the
standard meshoptimizer path. Its module and WASM are loaded only when needed.

The dense closed reference reduces to 12 triangles with zero surface error,
closed edges, unit volume and independently checked position/normal provenance.
Local algorithm diagnostics take 432–499 ms with budgets from 500 to 2,000 ms.
These are warm local algorithm observations, not a cold browser latency guarantee.
The Worker deadline, optional 2-second cap, output/memory bounds, first-GPU
admission barrier, original settled geometry, picking, PBR materials and MSAA
remain unchanged.

Tests reject holes, disconnected planar boundaries, overlap, inconsistent winding,
degenerate facets, nonconvex boundaries, branches and a single self-crossing
pentagram cycle. They also cover signed zero, authored-normal seams, small solids,
short runs, shuffled edge cancellation, capacity and deadline exhaustion. Large
curved/hole/seam integration cases independently pass the receiver's normal
source-bit/error/bounds/budget checks through the standard path.

## Workflow fixture shutdown

The failed trace records a new Workflow `check` request after all assertions pass
and before Playwright's later page teardown. A controlled real-fixture experiment
reproduces a route continuing after Workspace Host shutdown in three of three
runs; its refused port belongs to that Host. Twenty ordinary provider shutdown
runs pass. The trace itself does not expose the Node proxy's destination port, so
the controlled reproduction supports, rather than directly proves, that mapping
for the historical failure.

Each Workflow visual test now closes its page before stopping the fixture and
still stops the fixture if page closure fails. This stops the browser request
producer before its dependencies. Provider errors remain errors; intentional
server-disconnection tests elsewhere retain their existing fixture semantics.

## Verification and limits

Installed Google Chrome 157 with SwiftShader is a separate diagnostic lane from
the repository's pinned Chromium 153 CI. No screenshot reference was updated.

- CAD performance/read/retirement browser case: one passed.
- Original Workflow failure: three independent repetitions passed.
- Complete Workflow visual file: nine passed.
- Original approximately-200-MiB STL latency/retirement case: passed in 14.8 s
  within its unchanged 120-second test timeout.
- Full Web/Host suite: 4,395 passed, four platform skips; 306 files passed.
- TypeScript, production build, 336 provenance records, 355 production notices,
  runtime/source audits, localization and whitespace checks passed.

That STL run retains 12,574,116 admitted vertices and all 4,191,372 final triangles;
motion uses the certified 12-triangle surface. Idle draws and all owned resource
counts return to zero. Its first-frame opportunity is **2,530.5 ms**, orbit
P50/P95/maximum **31.6/41.8/115.1 ms**, and pan **33.1/33.4/1,166.0 ms**. Original
full-geometry submissions still occur when settled; these results do not meet the
strict first-frame or maximum-interaction gate. P95 is not substituted for maximum.

[Raw observations and tested source hashes](review-ci-repairs-samples.json) retain
the complete browser sample arrays, algorithm diagnostics and source identity.
These captures cannot be directly compared to the old CI's different machine
and browser as an end-to-end speedup claim. The pushed revision requires its own
pinned-browser and full CI results.

Browser reproduction uses the unchanged `geometry-performance.spec.ts`,
`geometry-latency.spec.ts` and `workflow-visual.spec.ts` through the normal
`pnpm test:e2e -- ...` command. `RUSTX_GEOMETRY_BUDGET=1` continues to enforce the
original strict performance gate, which remains open.

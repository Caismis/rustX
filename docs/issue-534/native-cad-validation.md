# Shared native CAD validation

Repository log copies normalize trailing whitespace, including extra blank lines at end of file; original diagnostics and measurement data are retained unchanged.

The native changes apply to STEP, IGES and BREP through the same importer
boundary. `ReadStepFile`, `ReadIgesFile` and `ReadBrepFile` all call
`ImportFile` in `js-interface.cpp`; typed export and admission use its shared
`WriteResult`. All three formats use `TriangulateShape` in
`importer-utils.cpp`. Reader-specific instrumentation separates STEP/IGES
parsing from document transfer including healing; BREP read combines parsing
and topology reconstruction and reports that combined boundary honestly.

## Immutable build inputs and scope

- Importer: `c2148e54b456b571238d35cac037d304053d64b2`.
- OCCT: `d2abb6d844231cb8f29be6894440874a4700e4a5`.
- Emscripten: 3.1.69, commit `c55ff404dbe1eb9f263c48b584d5ba627224763a`.
- All 4,775 translation units (4,768 OCCT and seven importer units) receive
  the selected optimization/LTO flags at compile and link. CMake's implicit
  Release optimization and the upstream compile-only `-Oz` are replaced.
- Exceptions, 10 MiB stack, the 2 GiB WASM maximum and final tessellation
  parameters remain unchanged.

`rebuild.sh`, `configure-build.py` and `audit-build.py` reproduce and audit the
complete target. Each candidate's `verified-build.json` records actual compile
and link flags, command hashes and the exact nine modified source hashes.
Every candidate's source fingerprint matches the source offered with the
runtime. The release's `UPSTREAM.json` and `geometry-runtime.json` tie the
selected build to its exact shipped glue and WASM hashes.

## Workloads and format coverage

The paired compiler comparison uses final-only imports across these workloads;
the selected compiler is then compared with final-only, coarse/final and
coarse/medium/final policies. Both stages use three paired repetitions and
rotate runtime order. The preserved `.3` runtime is the boxed-output baseline.

| Workload | Format | Source |
| --- | --- | --- |
| Named, colored three-sphere assembly | STEP | `lod-assembly.step` |
| 200 named, colored independent spheres | STEP | `generate-mesh-step.py --parts 200` |
| Curved degree-3 tensor B-spline with 2,102,500 referenced poles | STEP | `generate-large-step.py --poles 1450` |
| Three complete analytic spheres | IGES | `generate-curved-iges.py` from the original BREP fixture |
| Three complete analytic spheres without embedded mesh | BREP | `lod-assembly.brep` |

The public generators use `cadquery-ocp-novtk==7.9.3.1.1`. The large STEP is
200,761,966 bytes in this run; it contains real referenced poles, without
padding entities or comments. The 200-part model exercises substantial actual
mesh output within the unchanged 256-part client limit. Source bytes and
SHA256 are recorded in each raw sample. The IGES generator sets generic author
metadata and normalizes both timestamp fields. Embedded BREP triangulation has
a separate preservation/skip check.

Additional equivalence fixtures cover inch units, placed 12/96-part assemblies,
colors, hierarchy, IGES, BREP and embedded BREP meshes. Comparison checks include both canonical JSON and a separate raw numeric-bits
SHA256 over every position, normal and index stream in mesh order. Boxed
baseline numbers are normalized to Float64 for this second hash, preserving
negative zero that JSON loses. Labels and lengths delimit streams; a separate
metadata hash covers hierarchy, names, colors and face ranges. All hashing
occurs after timing and the pre-digest RSS snapshot.

## Timing and memory semantics

The primary paired runs disable profiling on every runtime. Each sample uses a
fresh process and a fresh WASM instance. The source is read directly into one
exact owned `Uint8Array`, outside the native clock, then detached immediately
after the native copy. There is no same-instance warmed-document import.
Compilation, fresh-instance initialization and the import are reported
separately. Cold-code and cached-module totals are explicitly derived from the
same observation; they do not claim separately measured warmed parser heaps.
These native CPU clocks are not browser first-image measurements.

Each process reports OS peak RSS covering fixture load, Node/V8, code
compilation/initialization, one fresh WASM instance and JS-owned raw output.
The peak is captured before final digest serialization. It is a process
high-water measurement, separate from WASM linear-memory capacity. Numeric
payload size excludes boxed-array capacity/object overhead; exact typed-buffer
bytes are reported only for typed output.

Separate opt-in diagnostic runs collect at most 27 phase/tier aggregates.
`lod-preflight` and `export` can include lazy tessellation/bounding-box work;
first-coarse/final spans include earlier phases and synchronous LOD callbacks.
They must not be summed as exclusive times. `wasmCommittedBytes` measures
linear-memory capacity. `allocatorLiveSampledMaximumBytes` is the maximum
`mallinfo().uordblks` boundary sample, not an instrumented live-memory peak.
Only whole-model boundaries sample the allocator, at most eleven walks per
read. Per-shape bounding box/tessellation aggregates report zero samples and
null live bytes. Inclusive ancestor clocks include nested probe overhead.

## Compiler matrix result

[All sixty raw samples](native-cad-compiler-samples.json) and the
[original failed-run log](native-cad-compiler-run.log) are retained. The
four-candidate equivalence assertion correctly failed for LTO on curved IGES.
Oz, O2 and O3 match canonical JSON, raw numeric bits and metadata on all five
workloads, consistently across the three repetitions. LTO deterministically
changes the IGES numeric/index streams in all three repetitions and is rejected;
no equality check, format or fixture was removed.

The [focused LTO diff](native-cad-iges-lto-diff.json) found 649 changed position
components, 1,195 changed normal components and 9,879 changed indices. Metadata,
7,600 vertices, 15,050 triangles and bounds agree; every value is finite and
indices remain integral/in-range. Differences are not limited to negative zero.
Pointwise maximum component differences are not surface-distance errors,
because vertex ordering/connectivity also changes. No tolerance was relaxed.

Median ordinary import times, milliseconds (profiling disabled):

| Workload | Oz | O2 | O3 | O2 + LTO (rejected) |
| --- | ---: | ---: | ---: | ---: |
| Small curved STEP | 135.8 | 148.9 | 140.1 | 175.6 |
| 1,001,600-triangle STEP | 5,021.8 | 4,297.3 | 4,198.9 | 3,942.6 |
| 200 MB B-spline STEP | 30,025.0 | 26,667.4 | 26,902.1 | 25,490.0 |
| Curved IGES | 192.0 | 173.4 | 176.4 | 158.7 |
| Unmeshed BREP | 55.3 | 48.5 | 53.3 | 49.9 |

| Candidate | WASM bytes | Gzip bytes | Large-input peak RSS, MiB | High-output peak RSS, MiB |
| --- | ---: | ---: | ---: | ---: |
| Oz | 7,096,648 | 3,060,117 | 1,131.0 | 312.7 |
| O2 | 11,589,688 | 3,812,506 | 1,249.6 | 406.2 |
| O3 | 12,134,802 | 3,985,390 | 1,255.4 | 338.5 |
| O2 + LTO | 14,912,087 | 4,283,437 | 1,386.7 | 490.5 |

These are three-run medians, not significance or tail claims. O2 has a larger
payload/RSS than Oz; O3's small high-output median advantage overlaps its run
range and does not improve the large-input median over O2. Raw samples include
compilation, initialization, derived cold/cached-module totals and individual
RSS observations, rather than hiding those costs behind parser-only times.

The selected compiler is **O3 without LTO**. Compared with O2, its large-input
median is 0.9% slower and gzip payload is 172,884 bytes larger. Its high-output
median is 2.3% faster, with measured peak RSS 330–345 MiB versus O2's 364–407 MiB;
this memory difference is the deciding tradeoff. O2 remains the smaller option
and is slightly faster on IGES/BREP. Compared with Oz, O3 improves large-input
and high-output medians by 10.4% and 16.4%, while adding 925,273 gzip bytes and
about 124 MiB large-input peak RSS. Median cold small-STEP total increases from
169.4 to 183.3 ms. This is a latency/memory balance for the measured workloads,
not a universal claim or a measurement on an unavailable device.

## LOD policy and baseline comparison

[All 75 ordinary samples](native-cad-lod-samples.json) and their
[run log](native-cad-lod-run.log) compare the selected O3 build against the
preserved `.3` boxed-output runtime. Every final result matches canonical JSON,
raw numeric bits and metadata across versions and supported policies on all
five workloads. The historical runtime supports final-only or two optional
tiers; the new runtime also supports one optional tier.

Three-run medians, milliseconds. A pair is first complete coarse output / final
output at the native boundary; it excludes browser conversion and rendering.

| Workload | New final only | New coarse/final | New coarse/medium/final | `.3` final only | `.3` coarse/medium/final |
| --- | ---: | ---: | ---: | ---: | ---: |
| Small curved STEP | 154.3 | 111.1 / 146.7 | 108.3 / 146.9 | 139.0 | 102.3 / 152.3 |
| 1,001,600-triangle STEP | 4,028.9 | 504.5 / 4,316.6 | 473.2 / 4,845.2 | 4,879.1 | 531.0 / 5,671.9 |
| 200 MB B-spline STEP | 25,628.0 | 25,837.9 / 27,408.0 | 26,042.5 / 29,156.5 | 27,086.5 | 27,085.6 / 31,875.9 |
| Curved IGES | 167.3 | 84.4 / 180.9 | 71.0 / 189.7 | 177.5 | 58.3 / 204.4 |
| Unmeshed BREP | 51.0 | 21.7 / 49.3 | 21.1 / 54.0 | 57.2 | 18.2 / 62.1 |

The product adapter selects **coarse/final for STEP, IGES and BREP**, through
one shared read-options expression. It requests `lodTiers: 1` when a consumer
exists and no optional work otherwise. Explicit SDK mode 2 remains available
for measurement, rather than becoming a product setting. Embedded BREP meshes
continue to skip optional remeshing. Final tolerances and whole-model admission
remain unchanged; there is no format, extension, filename, file-size or
predictive topology classifier.

The high-output model delivers all 200 parts at about 0.5 seconds rather than
waiting about 4 seconds for final-only output. One coarse pass adds about
288 ms to final completion; adding the medium pass costs another 529 ms while
that medium result exceeds optional admission and is never published. Removing
the medium pass also saves about 1,749 ms on the dense B-spline input. This
supports the shared policy without an unvalidated model-selection threshold.
Small-model differences are within variable short-run timings; an optional pass
is not claimed to make the final tessellation faster.

The dense input remains a limitation: coarse output arrives about 210 ms after
the final-only model would have completed and adds about 1.78 seconds to final
completion. There is no meaningful first-output lead on that input. These
measurements do not establish an overall first-image budget on a weak CPU,
integrated GPU or mobile browser.

The comparison includes both typed output and the new native/cache path; it
does not isolate those changes causally. High-output median process peak RSS
is 387.5 MiB for new coarse/final versus 530.2 MiB for historical all tiers;
final-only is 342.4 versus 484.6 MiB. Dense-input process peak RSS is 1,246.7 MiB
for new coarse/final versus 1,210.2 MiB for historical all tiers, and 1,253.2
versus 1,256.0 MiB final-only. The memory improvement is workload dependent.

## Inclusive phase diagnostics

The [fifteen separate profiling-enabled samples](native-cad-profile-samples.json)
and [diagnostic log](native-cad-profile-run.log) are not primary timing samples.
One diagnostic read per workload/policy illustrates where time is spent;
profiling includes its own boundary-sampling overhead.

Selected coarse/final diagnostic spans, milliseconds. These spans overlap;
preflight/export contain nested lazy work, while completion spans include
parsing, callbacks and prior phases.

| Workload | Input staging | Parse | Transfer + healing | Bbox | Coarse tessellation | Final tessellation | First coarse | Final complete |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Small curved STEP | 0.1 | 41.6 | 36.1 | 0.1 | 10.9 | 26.8 | 105.6 | 133.7 |
| 1,001,600-triangle STEP | 0.2 | 117.1 | 140.1 | 0.9 | 202.1 | 3,681.7 | 496.6 | 4,257.0 |
| 200 MB B-spline STEP | 85.3 | 22,714.9 | 1,056.5 | 41.6 | 1,557.5 | 1,501.6 | 25,743.9 | 27,246.4 |
| Curved IGES | 0.9 | 7.2 | 29.5 | 1.0 | 16.1 | 91.8 | 66.6 | 160.3 |
| Unmeshed BREP | 0.1 | 3.3 combined topology read | included in read | 1.3 | 13.2 | 27.3 | 19.7 | 48.1 |

The dense input's long wait is mostly parsing, followed by transfer/healing;
its coarse tessellation costs about as much as the final tessellation. The
high-output analytic assembly instead has a cheap coarse tessellation relative
to its fine final mesh. This evidence explains the tradeoff without proving a
cheap, robust predictor for arbitrary native CAD.

STEP/IGES show two tessellation calls per free root in each published optional
tier (400 calls for 200 roots); final shows one per root. Upstream
`XcafRootNode::GetChildren()` lazily invokes `TriangulateShape` once during
preflight and again during export. Export sets `resetTriangulation=false`, so
the second `BRepMesh_IncrementalMesh` call sees and validates that same tier's
existing triangulation. It is not a second clean/remesh. Its cost is bounded by
the inclusive optional export span: 38.9 ms for the dense input and 30.7 ms for
the high-output assembly, including all other export work. Aggregated spans do
not isolate the exact repeated-call cost. BREP triangulates once in
`GetRootNode()` and does not repeat it in child enumeration. This existing
behavior remains; skipping it safely would require preserving successful and
void-shape outcomes per pass. The invariant bbox cache still performs only one
bbox calculation per shape across the entire import, and every new tier/final
clears triangulation before remeshing.

## Correctness and lifetime

Raw positions, normals and indices are JS-owned `Float64Array` buffers filled
from fixed 4096-double native chunks. O(1) per-face counts admit buffers incrementally before multiplication,
addition and allocation, preventing 32-bit count wrap from shared/repeated
topology. Raw face indices are promoted to double before adding vertex offsets,
avoiding signed-int overflow before independent JS index checks. Independent raw-double/index validation precedes Float32/Uint32
narrowing; the Worker receiver admits the resulting streams again. No output
borrows the WASM heap, no boxed numeric output remains and no mesh-sized native
output copy is allocated. Later imports cannot mutate earlier raw output.

The shape/location-keyed characteristic-size cache preserves the original
`BRepBndLib::Add(shape, box, false)` computation, extent-average deflection
formula, units, location, void-box rejection and degenerate 1 mm fallback. Each
later tier clears native triangulation before remeshing, including final;
coarse meshes never replace final geometry. Embedded BREP meshes keep their
original final behavior.

The staged runtime passed 68 native-adapter/LOD tests, including typed ownership,
phase bounds, cache reuse, unit/placement/hierarchy/color preservation,
embedded BREP, complete-model refinement and adversarial raw doubles/indices.
The [five-workload product-adapter acceptance harness](native-cad-production-default-smoke.test.ts.txt)
uses the actual `importCad` call rather than a native-only substitute. Its
[successful result](native-cad-production-default-smoke.json) and
[log](native-cad-production-default-smoke.log) confirm exactly one complete
coarse callback, unchanged final triangle counts/hierarchy and owned-input
release for all five measured workloads. The
[initial acceptance log](native-cad-production-default-smoke-initial.log) is
also preserved: two assertions had an incorrect harness expectation of 12,946
triangles instead of the established 6,136 for the small STEP/BREP fixtures.
No runtime or quality setting changed to resolve that harness mistake; the
corrected complete five-case run passed. These acceptance runs and their clocks
are not additional performance comparisons.

[Fourteen additional baseline/new imports](native-cad-additional-equivalence.json)
and their [log](native-cad-additional-equivalence.log) match all three strict
digests on cube STEP, inch STEP, IGES, BREP, the placed bracket fixture, the
96-part assembly and embedded-mesh BREP. These are correctness-only runs; their
clocks are not used as uncontended performance evidence. Final focused tests
pass 68/68, TypeScript passes, and the checked-in runtime/source/recipe audit
passes. Selecting product `lodTiers: 1` does not change the measured O3 native
artifact or source fingerprints.

An independently generated 257-part model returns
`{success:false,error:"limit"}` without crashing; the Worker maps this to a
limit error. A separate compact shared-face BREP negative fixture references
one embedded face 5,115 times (13,002,330 logical vertices in 318,321 source
bytes) and also returns an explicit limit before allocating a huge JS stream.
This is a deliberate negative topology fixture, not a performance workload.
Historical `.3` SDK-only export of 257 parts is now deliberately
rejected by native producer admission, matching existing client admission.

No cross-close geometry cache was added. Existing residency admits at most two
mounted visible/parked owners, preserves the same canvas/scene/source across
tab reselection, and retires them on real scope/lifetime changes.

## Review follow-up: local source checkout guard

The local-checkout rebuild path now checks both index and worktree against
`HEAD` for the pinned importer and OCCT repositories. The former worktree/index
comparison accepted staged changes. Five network-free integration cases use
real Git repositories and real staged/unstaged changes, including an importer
source that the offered overrides do not replace. Only the production commit
identity probes and compiler/build entry are stubbed; the clean case reaches
actual source copying/configuration, while dirty cases cannot create a target.

The identical test against the original recipe fails exactly the staged
importer and staged OCCT cases; the corrected recipe passes all five. The
[proof manifest](native-cad-rebuild-proof.json),
[original-recipe failure](native-cad-rebuild-guard-red.log) and
[corrected-recipe result](native-cad-rebuild-guard-green.log) are retained.
The combined native/regression selection passes 73 cases, TypeScript passes,
and the runtime/source-offer audit passes. The recipe hash is updated together;
native C++ and the distributed JS/WASM hashes are unchanged.

An [independent current-input audit](native-cad-upstream-input-audit.json) checks
all 125 tracked importer files and 35,508 tracked OCCT files and both indexes
against their pins. The 136 raw checkout differences are exclusively files with
declared `eol=crlf`; applying that declared normalization exactly reproduces
their pinned blobs. No other file mismatch is accepted. Each of the four actual
build trees has the same eight non-overridden importer files, the nine exact
offered overrides, the expected flags-only CMake transformation, and 4,775 known
compile inputs; every saved build fingerprint agrees with its source hashes.
Their shared OCCT source resolves to the audited pinned checkout. This is a
current filesystem/index audit, not a retroactive execution trace or evidence
that the review finding changed the committed binary.

## STEP parser experiments

The archived STEP parser experiments are diagnostic only and did not enter the production runtime. The bulk lexer path measured a 51.8% reduction; the tail-list change measured 4.8% with an RSS cost. The combined-over-lexer 15.85% result comes from one generated valid B-spline benchmark and does not generalize. All profiles-off ordinary timing results are reported separately from diagnostic runs. See the [full report](step-parser-experiments/report.md), [compact summary](step-parser-experiments/summary.compact.json), and [reproduction notes](step-parser-experiments/REPRODUCE.md); the full original conclusions are in the native report.

# Bounded OCCT transfer and whole-model refinement

This package derives from occt-import-js 0.0.23 at importer commit
`c2148e54b456b571238d35cac037d304053d64b2` and its OCCT submodule
`d2abb6d844231cb8f29be6894440874a4700e4a5`. Complete changed sources are
in `src/`; `step-bulk-lexer.patch` applies the STEP lexer changes to the pinned
OCCT submodule. `UPSTREAM.json` lists the sources and selected build configuration.

Input uses Emscripten's public bulk numeric-vector conversion. Each mesh's
position, normal and index streams are JS-owned `Float64Array` buffers. Native
export counts/admission precede allocation, then fixed 4096-double stack chunks
copy through public `typed_memory_view` and `TypedArray.set`. No returned stream
borrows WASM memory; no boxed numeric output or mesh-sized native output copy is
retained. Raw doubles and indices remain available for independent validation
before the Worker narrows to `Float32Array` / `Uint32Array`. The receiver admits
those transferred buffers again before graphics upload. Raw output dies with
its synchronous conversion; native shapes die when the read returns and the
Worker/instance is terminated after its final transfer.

Final output is bounded at 13,000,000 vertices, 5,000,000 triangles, 256 meshes,
512 hierarchy nodes, 1,000,000 faces and 1 GiB of raw double streams (512 MiB
when narrowed). Each optional complete tier has independent limits of 65,536
triangles and a conservative 8 MiB including metadata. An oversized optional
tier is skipped entirely; final output uses the full final budgets. Client
admission remains independent.

An optional `onLod(level, result)` callback receives whole-scene results before
final return. `lodTiers: 0 | 1 | 2` allows final only, coarse/final, or
coarse/medium/final. The rustX product adapter explicitly requests one optional
tier (coarse/final) for STEP, IGES and BREP. Two optional tiers remain an SDK
measurement option, not a user setting. Without a consumer there is no optional work. Coarse uses
bounding-box-relative deflection 0.02 and angular deflection 1.0 radians;
medium uses 0.005 and 0.75. The original parameters determine the final result
(rustX: 0.001 and 0.5 radians). Every tier preserves the complete hierarchy,
mesh membership, placement, names and colors. Each later tier discards earlier
native triangulation, including before final; coarse triangles are never
substituted for the final mesh. Embedded BREP triangulation skips optional
refinement and preserves the source's final behavior.

The source is parsed once in one document. The shape's OCCT bounding box uses
`BRepBndLib::Add(shape, box, false)` once per import. A shape/location keyed
cache retains its characteristic size across tiers. The existing formula is
unchanged: the mean of the three extents times the deflection ratio, with the
existing 1 mm in current units fallback below `Precision::Confusion()`. Void
boxes retain their original rejection. This cache never retains meshes.

`onInputCopied` runs after native input ownership is established. The disposable
Worker releases its exclusively owned JS backing buffer then; direct callers
keep input by default. Native input is released after source transfer, before
mesh extraction. Retained owner copies are never detached by this callback.

The STEP scanner uses Flex's existing noninteractive bulk stream refill. The
same preprocessor directive is in `step.lex` and the generated `lex.step.cxx`.
To verify the generated change, run Flex 2.6.4 with `-Pstep -L` on the pinned
and patched `step.lex`. Compare the two generated outputs after removing their
input-path `#line` directive. The delta is applied to the scanner distributed by
OCCT, retaining its whitespace and inline `yywrap` adjustment.

## Native profiling

`onNativeProfile(spans)` is opt-in. It receives at most 27 fixed phase/tier
aggregates, each with duration, call count, tier and `inclusive` flag. Tier -1
marks shared input/parsing/transfer; 0/1/2 mark coarse/medium/final. Ordinary
imports do not read phase clocks or allocator samples. Instrumented boundaries
cover input copy, STEP/IGES parsing, document transfer including OCCT healing,
shape bounding boxes, lazy tessellation, LOD preflight, native export, first
complete coarse output and complete final output. BREP read combines parsing
and topology reconstruction and is labeled `file-parse-topology`.

`lod-preflight` and `export` can include lazy bounding box/tessellation spans;
first-coarse/final clocks include earlier phases and synchronous LOD callbacks.
They must not be summed as exclusive phase times. `wasmCommittedBytes` is WASM
linear-memory capacity. `allocatorLiveSampledMaximumBytes` is the largest
`mallinfo().uordblks` sample at whole-model boundaries, not an
instrumented live-memory peak or browser process memory. Per-shape bounding box
and tessellation aggregates report a null live sample; `allocatorLiveSamples`
counts actual samples. At most eleven allocator walks occur per read, regardless
of part count. Parent inclusive clocks include nested probe overhead. The benchmark records
OS process peak RSS separately, including Node/V8, WASM and JS-owned output.

## Rebuild and source offer

Activate Emscripten 3.1.69 and install CMake, Ninja, Python 3 and Git, then run:

```sh
bash web-console/vendor/occt-import-js/rebuild.sh /absolute/new/build-directory O3 off
```

The selected build and defaults use `O3` without LTO. The optimization argument
is `Oz`, `O2` or `O3`; LTO is `on` or `off`. The
script fetches immutable upstream/submodule revisions from official mirrors,
replaces the complete modified sources and applies the same explicit flags to
both compile and link across all importer and OCCT files. It removes CMake's
implicit Release optimization and the upstream hard-coded compile-only `-Oz`.
`audit-build.py` checks every generated compile command and the actual link
command, emitting `verified-build.json`. An optional fourth argument reuses an
existing pinned upstream checkout for a paired build. The recipe exports OCCT
into the build directory before applying the scanner patch. Exceptions, 10 MiB stack,
growing memory and the 2 GiB WASM maximum are preserved.

The native comparison script is `web-console/scripts/benchmark-cad-native.mjs`.
It runs each case in a fresh process with one fresh WASM instance, reports
compilation, initialization, cached-module import and scoped memory, compares
whole-model final doubles/indices/metadata,
and rotates variant order. Run timed comparisons without concurrent compilers.
Build paths and toolchain packaging can affect binary bytes; review hashes and
behavior before replacing shipped assets. `geometry-runtime.json` pins every
modified source and exact distributed asset hash.

Original full source:
<https://github.com/kovacsv/occt-import-js/tree/c2148e54b456b571238d35cac037d304053d64b2>.
Complete modifications and build scripts accompany the distribution.
`LICENSE.md` and both `dist/license.*.txt` retain upstream licenses and the OCCT
exception. Public notices include those texts. License line endings are LF;
terms are unchanged.

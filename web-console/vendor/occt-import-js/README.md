# Bounded OCCT transfer and whole-model refinement

This package derives from occt-import-js 0.0.23 at importer commit
`c2148e54b456b571238d35cac037d304053d64b2` and its OCCT submodule
`d2abb6d844231cb8f29be6894440874a4700e4a5`.

The five complete modified source files are in `src/` and listed in
`UPSTREAM.json`. Input uses Emscripten's public bulk numeric-vector conversion.
Position, normal and index output uses fixed 4096-double stack chunks and public
typed-memory views / Array push. This avoids per-number Embind property calls
and another mesh-sized native output allocation. Array order, numeric precision,
names, colors and hierarchy stay intact.

An optional `onLod(level, result)` callback receives up to two complete-scene
results before the final result returns. Level 0 uses bounding-box-relative
linear deflection 0.02 and angular deflection 1.0 radians; level 1 uses 0.005 and
0.75 radians. The original import parameters determine the final tessellation
(defaults: 0.001 and 0.5 radians). Every emitted tier includes the whole imported
hierarchy and every mesh at its tessellation quality. These tiers do not sample
triangles or accumulate completed parts.

The source is parsed once, using one native document and one WASM instance.
Native preview triangulations are discarded before each later pass, including
the final pass. A BREP source containing embedded triangulations skips optional
refinement, preserving its original final result. Callers without `onLod` keep
the normal one-pass import behavior.

Each optional tier is admitted before JavaScript mesh arrays are allocated.
Admission caps the complete tier at 65,536 triangles, 256 meshes, 512 hierarchy
nodes and a conservative 8 MiB estimate of numeric arrays and metadata, including
normals. A tier that exceeds any limit is skipped entirely. The final result is
independent of these optional preview limits. Consumers may skip equal or
non-refining tiers and must independently admit the buffers they retain.

`onInputCopied`, an optional import-parameter callback, runs after native input
ownership is established. The disposable CAD Worker uses it to release its
exclusively owned JS backing buffer; direct callers keep their input by default.
The native input vector is released after source transfer, before mesh extraction.
No retained content-owner copy is detached by this callback.

The Release build retains upstream `-Oz`, exceptions, 10-MiB stack and growing
memory with a 2-GiB maximum. `geometry-runtime.json` pins every modified source
and the exact distributed asset hashes.

## Rebuild and source offer

Activate Emscripten 3.1.69, install CMake and Git, then run:

```sh
bash web-console/vendor/occt-import-js/rebuild.sh /absolute/new/build-directory
```

The script fetches immutable upstream/submodule revisions (OCCT from its official
GitHub mirror), replaces the five modified sources and builds into the requested
new directory. Build-directory paths and toolchain packaging can affect binary
bytes; compare behavior and review asset hashes before replacing the distribution.

The original full source is available at
<https://github.com/kovacsv/occt-import-js/tree/c2148e54b456b571238d35cac037d304053d64b2>.
The complete modified source needed to rebuild this distribution is included here.
`LICENSE.md` and both `dist/license.*.txt` files retain the distributed upstream
licenses and OCCT exception. Public notices also include those texts.
License text line endings are normalized to LF; their terms are unchanged.

# Bounded OCCT transfer build

This package derives from occt-import-js 0.0.23 at importer commit
`c2148e54b456b571238d35cac037d304053d64b2` and its OCCT submodule
`d2abb6d844231cb8f29be6894440874a4700e4a5`.

Only `occt-import-js/src/js-interface.cpp` changes. The complete modified source
is in `src/`. Input uses Emscripten's public bulk numeric-vector conversion.
Position, normal and index output uses fixed 4096-double stack chunks and public
typed-memory views / Array push. This removes per-number Embind property calls
without allocating another mesh-sized native output buffer. Array order,
numeric precision, names, colors and hierarchy stay intact. No private glue
symbols, new geometry algorithm, allocator or tessellation setting is used.

`onInputCopied`, an optional import-parameter callback, runs after native input
ownership is established. The disposable CAD Worker uses it to release its
exclusively owned JS backing buffer; direct callers keep their input by default.
The native input vector is released after source transfer, before mesh extraction.
No retained content-owner copy is detached by this callback.

The Release build retains upstream `-Oz`, exceptions, 10-MiB stack and growing
memory with a 2-GiB maximum. O3 and allocator experiments are not included.
`geometry-runtime.json` pins the exact distributed asset hashes; the native and
browser checks cover all supported CAD formats and unit handling.

## Rebuild and source offer

Activate Emscripten 3.1.69, install CMake and Git, then run:

```sh
bash web-console/vendor/occt-import-js/rebuild.sh /absolute/new/build-directory
```

The script fetches immutable upstream/submodule revisions, replaces only the
modified source and builds into the requested new directory. Build-directory
paths and toolchain packaging can affect binary bytes; compare behavior and
review new asset hashes before replacing the pinned distribution.

The original full source is available at
<https://github.com/kovacsv/occt-import-js/tree/c2148e54b456b571238d35cac037d304053d64b2>.
The modified source needed to rebuild this distribution is included here.
`LICENSE.md` and both `dist/license.*.txt` files retain the distributed upstream
licenses and OCCT exception. Public notices also include those texts.
License text line endings are normalized to LF; their terms are unchanged.

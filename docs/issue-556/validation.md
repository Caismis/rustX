# Geometry pipeline validation

Repository log copies normalize trailing whitespace, including extra blank lines at end of file; original diagnostics and measurement data are retained unchanged.

This record describes local acceptance of the completed implementation. Final
remote-head and pinned-browser CI results belong to PR #556; the earlier green
run on `bbf96a76` does not cover these changes.

## Cross-format scope

The common Session/Workbench source operation, bound identity/version/size,
physical read admission, exact destinations, final validation and bounded copy
reuse cover GLB, binary/ASCII STL, STEP, IGES and BREP. Two independent static
audits found no extension gate in that shared layer. GLB reuses exact backing
buffers read-only. STEP, IGES and BREP share native typed output, bounding-box
reuse, profiling, the O3 build and the product coarse/final policy.

Incremental decoding is intentionally limited to binary/ASCII STL: the installed
GLB and native CAD backends consume complete input. Immutable managed Artifacts
retain their separate complete-byte API for every format. The
[scope table](../geometry-previews.md#optimization-scope),
[browser measurements](README.md) and
[native CAD measurements](../issue-534/native-cad-validation.md) distinguish these
boundaries and do not generalize STL throughput to other parsers.

## Final local checks

| Check | Result |
| --- | --- |
| Complete Web/Host suite | 349 files passed, one skipped; 5,057 tests passed, four skipped; exit 0 |
| Focused source operations | 156 tests passed, including late successful-open cancellation and immediate owner reuse |
| Geometry/source/cache regressions | 147 tests passed before final full-suite acceptance |
| Native adapters/LOD and GLB backing | 68 focused tests passed; final full suite includes them |
| Actual native product adapter | Five workloads passed, including genuine 200 MB input and 1,001,600-triangle output |
| Additional old/new native equivalence | 14 imports passed strict JSON, numeric-bit and metadata comparisons |
| Final progressive CAD browser cases | Four passed, including unchanged natural 96-part import and gesture-held final adoption |
| Rust affected paths | Four direct-buffer units and one real authenticated-WS boundary passed |
| Rust library checks | Library Clippy with warnings denied and all-features library check passed on the frozen Rust implementation |
| Formatting | Final `cargo fmt --all -- --check` passed |
| TypeScript | `tsc --noEmit` passed |
| Production bundle | `vite build` passed in 3.11 seconds |
| Source/artifact provenance | 350 source records and presentation boundary passed |
| Native runtime/source offer | Exact glue/WASM/source/recipe hashes and 2 GiB maximum passed |
| Notices, localization, brand icons | 356 production packages, product-copy ownership and 357 bundled icons passed |

The complete Web/Host command was
`node node_modules/vitest/vitest.mjs run --config /tmp/pr556-vitest.config.mts --maxWorkers=1`.
The temporary config imports the repository config, permits the isolated clone's
existing dependency paths, and uses an owned cache keyed to the exact native
glue/WASM hashes. It changes no test assertions, deadlines or iteration counts.
The run took 658.62 seconds; its [raw result](web-host-final.log) is retained.
Required Rust all-target/default/all-feature and platform coverage is delegated
to final-head FULL CI; local library checks are not described as that full lane.

The final production build was followed by `scripts/provenance.ts --artifact`,
`scripts/geometry-runtime.ts --artifact`, `scripts/notices.ts`,
`scripts/check-i18n.ts` and `scripts/provider-icons.ts`; all passed. Final native
assets are the same O3/no-LTO artifacts used in the compiler/LOD measurements.
Selecting one optional tier changes the shared CAD adapter's options only.

## Supplementary installed-browser acceptance

All 16 selected functional cases passed across the final run and one isolated
unchanged-case rerun. They cover actual Session and Workbench operations for all
five formats, final-close mutation, two pending-page cancellation/reopen, the
original Worker deadlines, fixed-copy/camera restoration and renderer cleanup.
The [final 15-pass/one-failure run](browser-final-run.log) and
[passing isolated format-matrix rerun](browser-format-rerun.log) are retained.

This uses installed Chrome 157/Playwright 1.63 on the available Linux machine,
with SwiftShader. It is supplementary functional evidence, not pinned-container
screenshot authority or a broad device/performance certification. Screenshots
were not updated.

The final shared coarse/final selection also passed all four
`geometry-progressive.spec.ts` cases on the final revision in 19.7 seconds;
the [log](browser-progressive-final.log) is retained. Three controlled-delivery
cases now require exactly one complete coarse tier and compare its complete
hierarchy/names directly with the held real final model. They retain the original
orbit/pan iterations, input-sample bound, final quality, focus, canvas/context and
retirement assertions. Final delivery during a held mouse gesture explicitly
keeps the coarse model until release. The natural unheld 96-part case is
unchanged. This aligns policy expectations with intentional medium-tier removal
rather than forcing an obsolete tier into product imports.

## Retained failures and their disposition

- An initial full Web run was stopped after detecting missing native executable
  prerequisites while compilers were active. The required binaries were built
  before subsequent complete acceptance.
- The [next full run](web-host-earlier-failure.log) passed 5,035 cases and failed
  three: two unchanged five-second UI deadlines and one assertion that observed
  a lookup spy before React committed selection. Only the last test changed,
  waiting for its original selection assertion inside `waitFor`. The two
  deadline cases were neither relaxed nor split; both are included in the final
  complete passing run. Isolated baseline success does not prove baseline
  flakiness, so no such claim is made.
- The first installed-browser run exposed a real streaming-cache regression:
  collapse lost a retained small source/camera. Restoring the existing bounded
  raw-copy cache fixed it; the original copy/camera assertions passed unchanged.
  Two new cancellation tests also incorrectly assumed zero private prepared GPU
  contexts before source close; they now check the intended no-publication
  boundary and still require strict old-generation cleanup.
- The remaining final-browser format failure used stale temporary Vite glue
  against the refreshed WASM (`_malloc` export `ea` versus `fa`). Clearing only
  that owned cache and rerunning the unchanged case passed. Cache directories
  now include both asset hashes, as the product config already does.
- LTO's deterministic IGES numeric/index differences were rejected. The native
  report retains the failed matrix and focused diff without loosening equality.
  It also retains an initial adapter-smoke harness count error and the complete
  corrected passing run.

The larger issue's 200 ms first-complete-model and 50 ms maximum-interaction
budgets remain unverified. Dense B-spline coarse work still has no measured
first-complete benefit; O3 payload/memory costs and short-run variability remain
visible in the native report.

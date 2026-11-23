# Geometry pipeline validation

Repository log copies normalize trailing whitespace, including extra blank lines at end of file; original diagnostics and measurement data are retained unchanged.

This record retains local acceptance for the original performance work and the
later review fixes. The PR description identifies the tested head and its
matching remote verification run.

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
| Rebuild checkout review regressions | Five real-Git cases passed; identical tests reject the original recipe's staged-change behavior |
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
The run took 658.62 seconds; its [raw result](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/web-host-final.log) is retained.
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
The [final 15-pass/one-failure run](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/browser-final-run.log) and
[passing isolated format-matrix rerun](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/browser-format-rerun.log) are retained.

This uses installed Chrome 157/Playwright 1.63 on the available Linux machine,
with SwiftShader. It is supplementary functional evidence, not pinned-container
screenshot authority or a broad device/performance certification. Screenshots
were not updated.

The final shared coarse/final selection also passed all four
`geometry-progressive.spec.ts` cases on the final revision in 19.7 seconds;
the [log](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/browser-progressive-final.log) is retained. Three controlled-delivery
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
- The [next full run](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/web-host-earlier-failure.log) passed 5,035 cases and failed
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

The later [staged-checkout review finding](https://github.com/Caismis/rustX/pull/556#issuecomment-6940915651)
is fixed in the rebuild recipe and covered by five additional real-Git cases.
The runtime recipe hash/source offer is refreshed; compiled code and geometry
are unchanged. The [native follow-up](../issue-534/native-cad-validation.md#review-follow-up-local-source-checkout-guard)
retains baseline failure, corrected success and an independent audit of all four
build input trees. The full local 5,057-case run above precedes this recipe-only
follow-up; final-head FULL CI repeats the complete suite including its new tests.


## Native parser investigation and natural LOD follow-up

The completed [FULL run on ce828](https://github.com/Caismis/rustX/actions/runs/44037588332)
passed the Linux contracts/boundaries, default/all-feature lint and rustdoc,
protocol/TUI, both macOS lanes, Web/Host (5,062 passed, four skipped), and browser
shards 1/2. Shard 3 failed the natural 96-part coarse-before-final assertion;
browser coverage and merge verification consequently failed closed. This is not
a green final-head run.

Its [raw observation](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/progressive-follow-up/ci-ce828-natural.json) contains a
complete native coarse model, followed about 146 ms later by final output, but
no coarse triangle submission. Product refinement may skip an unpainted tier
when the final model supersedes it. The test now checks the complete native
96-part tier and exact original model/frames without requiring that superseded
tier to paint. It independently checks GPU fences for every tier actually
submitted. Renderer scheduling and product policy are unchanged.

A new unheld analytic 100-sphere case retains the strict coarse-before-final
presentation requirement, complete names/hierarchy, 23,400 coarse and 500,800
original final triangles, one context and zero resources after retirement.
All coarse frames and every complete-quality full frame require their entire
corresponding triangle counts. The first full frame also requires all original
triangles. Existing interactive-quality complete-surface representations are
recorded separately and do not prove original final tessellation; their
correctness remains covered by the dedicated display-quality tests.

The first new 200-sphere workload exceeded the unchanged five-second readiness
assertion; [that failure](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/progressive-follow-up/local-200-timeout.json) is retained.
The 100-sphere attempt then exposed the overly broad all-full-frame count check:
an existing interactive-quality frame had fewer triangles while first/settled
original frames were exact. [That failure](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/progressive-follow-up/local-100-quality-assertion.json)
is retained too. Passive frame telemetry now includes quality so the assertion
matches the existing rendering contract. No readiness/test deadline, renderer,
held-message behavior or controlled-test assertion was relaxed.

The [final five-case local run](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/progressive-follow-up/local-five-passed.json)
passed all five cases in 26.19 seconds, with no skip/retry; TypeScript and the
four test/fixture files' whitespace check passed. The three controlled-delivery
test bodies are byte-identical. The [natural high-output observation](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/progressive-follow-up/local-100-natural.json)
records coarse GPU/frame opportunity at 1312.6/1340.2 ms, before final at
3486.5/3492.6 ms; the original complete-quality settled frame is at 4020.3 ms.
[Retirement](https://github.com/MiNisterPaX/rustX/blob/efef9f0441de1a31752187ed9287217ec53816fb/docs/issue-556/progressive-follow-up/local-100-retirement.json) is all zero. These
installed-Chrome/SwiftShader measurements are supplementary functional evidence;
new-head pinned CI remains required.

The separate [STEP parser experiments](../issue-534/step-parser-experiments/report.md)
record 90 profiles-off ordinary timing rows and 136 targeted semantic reads.
The generated valid 200 MB B-spline benchmark benefits from bulk lexer input;
per-record tails address an independently measured quadratic wide-list cost.
CPU/allocator diagnostics, memory costs and scope limitations are retained.
The browser runtime ships the bulk lexer in `0.0.23-rustx.4`
(`72997c789` and `0f916f108`). The tail-list candidate remains an experiment.
The benchmark results above retain their original build identities.

## Follow-up to the November 23 review

The earlier integrated head `efef9f0441de1a31752187ed9287217ec53816fb`
passed [FULL CI #2249](https://github.com/Caismis/rustX/actions/runs/44699398145),
including all three browser shards and [Merge verification](https://github.com/Caismis/rustX/actions/runs/44699398145/job/136181776506).
GitHub also recorded a separate failing Devin Review status for that head.

The cancellation follow-up exercises real Host HTTP operations with a
controllable native socket. The regressions cover retirement before
acknowledgement, repeat cancellation, unexpected native errors, and unknown
settlement. The streaming STL checks use the actual source scheduler and
content owner with a disposable Worker test double. They verify size rejection
before any page read, capacity, replacement and refresh, and unknown settlement.

The local checkout regression runs the real rebuild recipe and Git archive
against small repositories, with a sentinel at compiler configuration. It
rejects staged and unstaged edits and excludes untracked and ignored C++ files.
An additional preparation-only run compared all 16 importer-tree files against
the pinned Git blobs and reviewed overrides. The recipe hash was refreshed.
The bundled JS/WASM were verified by checksum and were not rebuilt for this
follow-up.

See the [evidence archive](evidence.md) for the immutable research inputs,
raw outputs, failures, and extraction checksums. The PR description links
final-head verification for the review follow-up.

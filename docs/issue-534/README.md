# Issue #534: Segmented Source-Reading Evidence

Related: [Issue #534](https://github.com/Caismis/rustX/issues/534), [Issue #516](https://github.com/Caismis/rustX/issues/516), and [PR #517](https://github.com/Caismis/rustX/pull/517).

This set of measurements evaluates only the read phase for a complete geometry source. The implementation direction is: after `readGeometrySource` determines the length/version on the initial read, it reads the remaining 2 MiB pages through two rolling lanes; the byte path in `readWorkspacePreview` reads directly into page buffers, avoiding retention of 64 KiB chunks and the final concatenation. Existing capacity, permission, and version checks remain in force.

This round implements only read-ahead and reduced copying; persistent source transport and the native import path were not changed.

The formal results are in [source-reading.json](source-reading.json), with 30 samples total. The environment was Node v24.21.0, Linux 6.14.0-37-generic, Intel Core i9-14900K, 32 logical CPUs. The final run was performed in the isolated worktree `/tmp/rustx-issue-534`, at HEAD `01a9b5d9681da3107c7e7a5591ee9dcb128f968c` (implementation commit). The following SHA-256 hashes also pin the candidate production source files:

- `geometry-source.ts`: `1d913cc4e281bf76e0bb89547e77431f2b729d7154a9321239ddce70e06eadce`
- `workspace-preview.ts`: `eae8e3930df8ff9b24d10d1732cf68e70fdc014ca9e99e649f59050b2969e6d3`

The raw durations below are ordered by pair number 1/2/3 and are in ms. “Reduction” is calculated from the two medians; a negative value indicates a regression.

| Group | Baseline raw durations | Candidate raw durations | Baseline median | Candidate median | Reduction |
| --- | --- | --- | ---: | ---: | ---: |
| Single source, 0 ms artificial delay | 128.295 / 126.762 / 130.905 | 134.110 / 125.348 / 133.657 | 128.295 | 133.657 | -4.18% |
| Single source, 10 ms artificial delay | 1314.203 / 1281.032 / 1289.083 | 678.080 / 802.039 / 704.295 | 1289.083 | 704.295 | 45.36% |
| Single source, 50 ms artificial delay | 5286.483 / 5297.920 / 5291.783 | 2775.462 / 2819.394 / 2728.958 | 5291.783 | 2775.462 | 47.55% |
| Shared two sources + download demand, 10 ms | 106.736 / 96.213 / 100.417 | 95.334 / 96.497 / 93.347 | 100.417 | 95.334 | 5.06% |
| Workbench hot local | 270.371 / 276.288 / 277.555 | 122.079 / 122.333 / 121.392 | 276.288 | 122.079 | 55.81% |

The median in the zero-delay group is 5.362 ms slower (4.18%). Of the three pairs, the first and third were slower, while the second was faster. The sample is too small to conclude either a stable regression or no regression; no improvement is claimed for this group. The zero-delay adapter performs synchronous copying before returning a Promise, so the observed active physical-read count is 1. In the 10/50 ms groups, observed concurrency changed from 1 to 2, while page requests remained at 100 each; the advantage comes from overlapping waits. In the hot-local group, actual `fs.read` calls fell from 3200 to 100, and maximum physical-read concurrency rose from 1 to 2.

All six samples in the shared group had 17 reads and a maximum of 2 active physical reads. The download demand was consistently the third admitted request; both geometry sources then completed, and the download was not displaced to the end by subsequent read-ahead. Download completion times were 20.646 / 19.918 / 20.210 ms for the baseline and 21.399 / 20.651 / 20.641 ms for the candidate. The two baseline sources already occupied both shared slots, so this group yields only a small benefit, as expected. The download here is a controlled raw-byte demand submitted to the actual shared admission controller; browser download delivery was not executed.

Segment medians are below, in ms, with each cell showing “baseline → candidate.” Each column is independently median-aggregated; adding the medians does not reconstruct the overall median.

| Group | First page completed | Union of active page-read intervals | Remainder outside page-read intervals |
| --- | ---: | ---: | ---: |
| Single source, 0 ms | 0.653 → 0.364 | 46.368 → 40.478 | 81.927 → 93.179 |
| Single source, 10 ms | 9.822 → 10.757 | 1071.008 → 655.433 | 218.075 → 73.588 |
| Single source, 50 ms | 50.755 → 50.904 | 5070.924 → 2710.409 | 220.860 → 65.906 |
| Shared two sources + download demand | 10.594 → 10.587 | 99.097 → 94.272 | 1.320 → 1.063 |
| Workbench hot local | 1.464 → 0.797 | 182.341 → 93.373 | 95.214 → 28.321 |

The hot-local group also has a median of 117.048 → 89.574 ms for the union of actual `fs.read` callback intervals, with a remainder outside those intervals of 160.507 → 32.251 ms. First-page completion means only that the byte page is available; it does not mean that the first screen has rendered.

The medians of each sample's process-memory `sampledMax` are below, in MiB, with each cell showing “baseline → candidate.”

| Group | RSS | heapUsed | external | arrayBuffers |
| --- | ---: | ---: | ---: | ---: |
| Single source, 0 ms | 584.164 → 594.180 | 8.015 → 8.027 | 496.781 → 504.781 | 482.202 → 482.202 |
| Single source, 10 ms | 552.062 → 572.352 | 8.279 → 8.412 | 614.781 → 484.781 | 602.202 → 472.202 |
| Single source, 50 ms | 551.957 → 549.984 | 8.926 → 8.738 | 612.781 → 428.781 | 602.202 → 418.202 |
| Shared two sources + download demand | 398.160 → 366.285 | 9.260 → 9.251 | 280.781 → 268.781 | 270.202 → 258.202 |
| Workbench hot local | 571.250 → 570.520 | 10.415 → 9.909 | 443.719 → 450.781 | 433.140 → 436.202 |

This round does not claim to reduce peak process memory. In the 10 ms group, median sampled RSS increased by 20.289 MiB while `arrayBuffers` decreased by 130 MiB; in the 0 ms group, RSS increased by 10.016 MiB, and in the hot-local group, `arrayBuffers` increased by 3.062 MiB. Changes across these different counters and groups are inconsistent. These process counters include the 200 MiB memory fixture and are affected by allocator and GC reclamation/accounting timing. This group cannot determine the specific cause of the observed differences, and these counters are not the number of active geometry page buffers or an exact resident-memory peak.

The new rolling lanes have a logical active-page budget of at most 2 pages (4 MiB) per owner. Two mounted owners can reference at most 4 pages (8 MiB) in total; global physical active reads remain limited to 2 by the shared admission controller. The logical page budget is not an allocator/GC retained-memory limit. The complete output buffer is separately bounded at up to 200 MiB per source.

Run command (from the repository root; during formal runs, avoid running alongside builds, tests, or other benchmarks):

```sh
node --experimental-transform-types --expose-gc web-console/scripts/geometry-source-benchmark.ts --output docs/issue-534/source-reading.json
```

`--smoke --output /tmp/source-reading-smoke.json` uses 8 MiB and one pair for a quick check; it is not formal performance evidence.

The script pins the two baseline production source files from `git show c076354d647c1a1832e9af5e005696ecb5579285` and runs them alongside the current production functions using a Node module-loading hook; it does not duplicate the algorithms under test in the script. Other relative dependencies come from the same checkout. The JSON records the SHA-256 hashes of the two production files and loaded dependencies, the revision, worktree state, Node/OS/CPU details, and raw command. Each formal group has at least three paired runs; the baseline runs first in odd-numbered pairs and the current version first in even-numbered pairs.

| Group | Data and path | Purpose |
| --- | --- | --- |
| controlled-single | 200 MiB; 2 MiB pages; 0/10/50 ms artificial delay added to each read | Observe separately the effect of serial reads versus two lanes under fixed per-page latency |
| controlled-shared | Two 16 MiB sources and one 2 MiB download; 10 ms artificial delay; uses actual `previewDemands().privateReads` | Verify that shared physical concurrency is at most 2 and that the queued download gets the next admission slot after the initial two pages |
| workspace-hot-local | 200 MiB; actual `readGeometrySource → readWorkspacePreview → fs.read` | Observe the hot path for a recently written local file, including safe directory traversal and version revalidation |

Timing excludes module initialization, file creation/writing, explicit GC, result hashing, and report output. Every result is checked after timing for full SHA-256 and version. Only one temporary file, at most 200 MiB, is created and is cleaned up at the end. The formal JSON records each page's submission/admission/completion times and offset, the start/callback times and byte count for each actual `fs.read`, actual read counts, and maximum physical concurrency.

Segmentation uses the union of page-read active intervals and `fs.read` callback intervals; overlapping intervals cannot be added. The remainder outside those intervals mixes scheduling, admission, validation, allocation, copying, cooperative yielding, and sampling/instrumentation overhead. It is **not pure assembly CPU time and is not a complete waterfall**. Process memory is sampled every 5 ms, plus at the beginning and end; `sampledMax` is only the maximum sampled value, not an exact peak.

Artificial delay does not represent a real network. This group does not cover HTTP, native delivery transport, the browser, parsing, or GPU work; it cannot prove the 200 ms first-screen or 50 ms interaction targets, and it does not mark #534 complete. Hot-local file results do not represent cold disks or remote filesystems.

## Validation

[validation.json](validation.json) records commands, implementation revision, production file hashes, and results. In the isolated implementation worktree, TypeScript, the production build, and the provenance/geometry runtime/i18n/notices checks passed; 504 tests across 45 related test files passed in one run. The run reused the checkout's locked Web/TUI dependencies and unmodified native filesystem build artifacts. The temporary Vite test configuration only allowed dependency paths outside the worktree; it did not modify product configuration, assertions, timeouts, or test selection. The missing locked icon dependency was restored with `corepack pnpm install --frozen-lockfile --ignore-scripts`; the lockfile was not modified.

The local Chrome production geometry-fixture supplemental diagnostics passed 8 of 9 checks. They covered cancellation during code loading, two owners, controlled full-model LOD, focus, late results after retirement, and an approximately 200 MiB STL. One natural LOD test failed its ordering assertion that the coarse-model GPU/next-frame proxy time must precede the final model: the record shows the coarse model was submitted before the final model, but the proxy polling results appeared in the opposite order. The same original assertion also failed once in three additional baseline runs, then passed in three candidate runs; [all results and observations](browser/functional-validation.json) are retained together, and the later passes do not replace the original failure. The fixture is 168,057 bytes, with EOF at the first page, so it does not enter concurrent reading; the CAD Worker, renderer, and test were not modified. This comparison supports the judgment that the behavior was already timing-sensitive; it does not prove the absence of other browser regressions.

The current-version results for [large STL](browser/stl-200mib-latency.json), [STL plane](browser/stl-735000-vertices-latency.json), and [CAD assembly](browser/cad-96-parts-latency.json) retain the raw interaction and GPU/next-frame proxy timings. These are not paired before/after browser improvement evidence and do not certify strict budgets. Docker/Podman was unavailable in the environment. **At the time these local diagnostics were recorded, pinned-container browser acceptance and remote CI were still pending.**

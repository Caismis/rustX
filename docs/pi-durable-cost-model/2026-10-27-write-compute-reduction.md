# PR #485: eliminate repeated computation and final-only writes, 2026-10-27

> Historical F round. The [joint runtime report](2026-10-27-joint-major-runtime.md) identifies the latest delivered source, fresh Pi/DeepSeek comparisons and remaining CPU costs.

**Retained F removes final-only staging writes and repeated encoding/validation, but does not achieve overall Pi parity.** Same-engine short-turn complete writes fall about 2–2.5%; timing and memory results are mixed. Fresh F/Pi warm short-turn medians remain **7.32–10.34×** for wall and **8.15–9.07×** for complete process writes. All **284 formal products** passed their declared oracles; adverse observations remain below.

## What still separates rustX from Pi

Both retained D and Pi already use SQLite WAL/NORMAL. The gap is not evidence that rustX needs weaker fsync settings. Fresh diagnostics on the retained production binary identify more transactions, more derived data and more computation. These are production HTTP/SSE workloads with 32 measured short turns after 8 or 64 seed turns. `no-live-subscriber` still retains the Runtime Client publication path for a late reader; it does not mean that all observation work is disabled.

| Diagnostic, 32 measured turns | D, 8-history | Pi, 8-history | D, 64-history | Pi, 64-history |
| --- | ---: | ---: | ---: | ---: |
| WAL write-commit markers | 512 | 160 | 512 | 160 |
| Markers per turn | 16 | 5 | 16 | 5 |
| WAL frames | 10,722 | 1,683 | 11,748 | 1,688 |
| Frame bytes, including 24-byte frame headers | 44,174,640 | 6,933,960 | 48,401,760 | 6,954,560 |

The **16 = 7 native + 9 publication-readiness** split is corroborated by each repeated transaction's page families and the source call order. It is not 16 independent application facts. The seven native commits are inbound acceptance, adoption, attempt start, turn start, request start, publication-stream open, and final settlement. The nine releases occur only after their owner state is installed: accepted transcript, adoption, attempt, turn, request, then provider/canonical/turn/attempt completion. D already combines a single installed batch's canonical transcript and Journal release; the four separate final callbacks remain. The readiness-only transactions write 640 frames in either cohort, about 5.4–6.0% of all frames. Eliminating their cost would still leave most writes.

Pi's five commits are admission (user entry/submission/pending task/live state), scheduler transition to running, request checkpoint, generation live marker, and atomic final assistant/submission/task/live settlement. Stable system/tool declarations are only appended when they change. Pi still validates and copies JSON and compares schemas; it does not simply append two strings without checks. The final task UPSERT replaces its running request checkpoint with a terminal outcome. Consequently, its persisted history does not include rustX's full immutable per-request options/configuration catalog. The benchmark's Pi history check also retains request cutoffs in its own `captures` array. Dropping rustX's historical request inspection would remove existing functionality.

The large amount of derived work is directly visible. In the small/large D cohorts, event rows and their indexes account for 3,855/4,090 attributed WAL frames; event-readiness rows/indexes add 832/832. Request source/native-input pages and indexes contribute 1,192/1,395, and request snapshots/catalogs/indexes 553/604. The transient final frame/block/count family contributes 224 in each cohort even though those rows are deleted before final commit. These are endpoint `dbstat` page-owner attributions, not exclusive semantic byte accounting; intermediate page reuse cannot be ruled out, and 2/1 D frames remain unmapped. Ordinary short turns do not rewrite Goal/Todo/compaction State bodies.

Independent CPU signal sampling on D supports the computational distinction:

| Inclusive stack, numerator / all CPU samples | No-live, small | No-live, large | Live, small | Live, large |
| --- | ---: | ---: | ---: | ---: |
| Request read-layout construction | 47 / 253 | 53 / 269 | 41 / 374 | 69 / 503 |
| PublicationDraft::encode | 44 / 253 | 53 / 269 | 80 / 374 | 103 / 503 |
| SQLite statement preparation | 39 / 253 | 43 / 269 | 77 / 374 | 104 / 503 |
| ReadDomainCut::read_with_trace | not reported | not reported | 98 / 374 | 127 / 503 |

These are one diagnostic run per cell at a requested 4 ms interval, across product threads and the in-process driver. Inclusive stacks overlap and must not be added, interpreted as wall-clock percentages, or used to predict the eventual speedup. Control runs and raw profiles are retained. The previously rejected broad read-statement cache and deferred request-index trial are not restored.

## The implemented reduction

Candidate F is source `2800be25a650305d5daf9f543ffbf7d8f62ea79b`, `src` tree `a86302ed48b415de91cac782bd156dc935b8e517`. Schema 76 and WAL/NORMAL are unchanged. Five local reductions preserve the current reader and recovery contract:

1. Final settlement validates stream/message/sequence/proposal ownership through the same helpers as live publication, then consumes final frames directly. It no longer creates final-only frame JSON rows, live content/indexes or frame receipts that the same transaction would discard. Ordinary live-prefix staging remains. Raw tool argument suffixes are retained exactly, after a healthy live prefix or a fallback reconstruction from original staged frames; errors still roll back the complete outer transaction.
2. `seal_execution` writes the successful bounded encoding it already produced, avoiding a second encoding of the same execution header.
3. `PublicationDraft` moves checkpoint collection/content descriptors into the outer metadata instead of cloning and then discarding/clearing them. Existing old-path differential tests verify complete event bytes, metadata, cursor and failure behavior.
4. Native checkpoint length probes count the exact same serde JSON bytes without allocating a byte Vec that is immediately thrown away. Admission, escaping, number handling, serialization errors and eviction budgets remain unchanged. This does not remove the JSON traversal.
5. Request Fact capture reuses a successfully validated bounded ancestor interval for contained subtrees. Both existing node counters cap at 33; the new path still creates all eager previews, field/child facts, authenticated pages and byte spans. Differential tests compare the entire indexed representation, plus duplicate/alias keys, deep/numeric errors and 16 KiB boundaries. This is not the rejected E approach of reconstructing indexes during reads.

These changes do not consolidate the nine readiness transactions or remove the full scalar capture. A meaningful end-to-end gain must therefore be measured, and a small write reduction alone cannot establish Pi parity.

### WAL verification after F

A separate F/Pi diagnostic confirms the final-only frame/block/count pages no longer appear in measured WAL writes. The net frame reduction is modest; page split/layout changes in other families mean it is not valid to assign every removed byte to one source edit.

| 32 measured turns | D frames | F frames | Change | F commits | Pi frames / commits |
| --- | ---: | ---: | ---: | ---: | ---: |
| 8-history | 10,722 | 10,513 | −1.95% | 512 | 1,683 / 160 |
| 64-history | 11,748 | 11,458 | −2.47% | 512 | 1,688 / 160 |

F therefore still makes 16 write commits per turn, and writes about 6.25–6.79× Pi's WAL frame bytes in these diagnostic windows. Frame bytes exclude the WAL file headers; these are not device writes, complete-process write counters or timing ratios. The unchanged commit count and remaining indexed event/request work explain why the implementation is a bounded reduction rather than a structural convergence to Pi. [Rust transaction map](evidence/write-compute-20261027/rustx-transaction-cost-map.txt), [Pi source map](evidence/write-compute-20261027/pi-transaction-cost-map.txt), [independent F WAL audit](evidence/write-compute-20261027/wal-f-audit.json).

## Same-engine F/D runtime comparison

All ratios below are median within-pair F/D, lower favors F. The initial cohort has three alternating pairs per case and mode (72 products); complete counters run from exec through cleanup, including startup and verification. Short cases measure 32 turns after 8/64 history turns. The other cases measure one turn. The provider, full same-engine requests, native tools, late snapshot, historical inspection and close/reopen oracle are unchanged. All 846 initial paired requests passed.

| Mode | Case | Turn wall | Turn CPU | Complete CPU | Complete writes | Sampled anon PSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| no-live | short-small | 0.9409 | 0.9464 | 0.9388 | 0.9753 | 1.0031 |
| no-live | short-large | 1.0199 | 1.0275 | 1.0074 | 0.9758 | 1.0082 |
| no-live | stream | 1.0157 | 1.0588 | 1.0645 | 0.9982 | 1.0037 |
| no-live | large | 0.9912 | 1.0000 | 1.0000 | 0.9561 | 1.0066 |
| no-live | tool | 1.0706 | 1.0000 | 0.9474 | 0.9812 | 0.9947 |
| no-live | cancel | 1.0037 | 1.0000 | 1.0000 | 1.0000 | 0.9897 |
| live | short-small | 0.9719 | 0.9752 | 1.0237 | 0.9802 | 1.0045 |
| live | short-large | 0.8531 | 0.8624 | 1.0025 | 0.9754 | 1.0060 |
| live | stream | 0.9829 | 0.8000 | 0.8485 | 1.0000 | 0.9945 |
| live | large | 1.0300 | 0.7143 | 1.0000 | 0.9561 | 1.0114 |
| live | tool | 0.9549 | 0.9000 | 1.0000 | 0.9812 | 0.9992 |
| live | cancel | 1.2067 | 1.2500 | 1.0667 | 1.0000 | 0.9982 |

The initial observations are mixed. Short-turn complete process writes fall about 2–2.5%; large first-output writes fall 4.39%, tool writes 1.88%, and stream/cancel writes are effectively unchanged. No-live short-large median wall/turn CPU are 1.020/1.027, live cancel wall is 1.207, and a live short-large pair reaches 2.852 for wall (1.304 for turn CPU). This is 122.115 ms versus 42.815 ms; its cause is unexplained and the sample remains included. No-live short-small, short-large and stream, and live short-large sampled anonymous PSS are higher in every initial pair, by 0.03–2.21% depending on the cell/pair. No-live short-large anonymous PSS increases by 1.64–1.78 MiB in every initial pair. No universal speed or memory improvement is claimed.

Initial no-live stream turn CPU is adverse in all three pairs: F 180/180/200 ms versus D 170/170/180 ms. Live cancel complete CPU is also adverse in all three: F 170/190/160 ms versus D 160/170/150 ms. This triggered a separately recorded seven-pair confirmation of stream and cancel in both modes (56 additional products, 28 paired requests). It does not replace the initial cohort or convert the selected follow-up into a predeclared global nonregression test.

Confirmation F/D medians **[complete seven-pair range]**:

| Mode | Case | Turn wall | Turn CPU | Complete CPU | Complete writes | Sampled anon PSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| no-live | stream | 0.988 [0.967, 1.037] | 1.000 [0.778, 1.333] | 1.000 [0.794, 1.148] | 1.000 [0.994, 1.000] | 1.007 [0.995, 1.013] |
| no-live | cancel | 0.999 [0.792, 1.230] | 1.000 [0.750, 1.250] | 1.000 [0.938, 1.125] | 1.000 [1.000, 1.000] | 0.992 [0.986, 1.030] |
| live | stream | 0.993 [0.966, 1.025] | 0.889 [0.789, 1.167] | 0.935 [0.824, 1.172] | 1.000 [0.998, 1.000] | 0.996 [0.980, 1.029] |
| live | cancel | 0.949 [0.794, 1.148] | 1.000 [0.800, 1.250] | 1.000 [0.842, 1.188] | 1.000 [1.000, 1.000] | 1.004 [0.975, 1.007] |

The two originally consistent adverse CPU cells are mixed in the confirmation, with median ratios 1.0. Their regression is not independently reproduced as a consistent effect; this is not proof of zero regression. CPU accounting has 10 ms resolution, material for the single-turn cases. All paired values, absolute counters, lifecycle wall, sampled total PSS/RSS and exploratory log-t intervals are retained in the raw recomputations.

## Fresh F/Pi comparison

These are newly measured F/Pi within-pair medians, three alternating pairs per case and mode. Historical D/Pi ratios are not multiplied by F/D. Rust/Pi use seven/four native tools and preserve their respective request/observation contracts; the core request oracle passes, while full cross-engine request equality is explicitly false.

| Mode | Case | Turn wall | Turn CPU | Complete CPU | Complete writes | Sampled anon PSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| no-live | short-small | 7.3216 | 4.0417 | 1.8041 | 8.1457 | 1.1326 |
| no-live | short-large | 7.3985 | 4.6364 | 3.5608 | 9.0391 | 1.3076 |
| no-live | stream | 0.9292 | 0.3125 | 0.2936 | 5.0182 | 0.8677 |
| no-live | large | 0.5968 | 0.5000 | 0.2931 | 5.8314 | 1.0745 |
| no-live | tool | 0.6006 | 0.5000 | 0.3016 | 5.8875 | 0.9271 |
| no-live | cancel | 0.4380 | 0.3333 | 0.2623 | 6.0252 | 0.9611 |
| live | short-small | 9.1656 | 6.3750 | 2.6082 | 8.1734 | 1.1906 |
| live | short-large | 10.3436 | 8.0952 | 5.2877 | 9.0735 | 1.3329 |
| live | stream | 0.9797 | 0.3556 | 0.2979 | 5.0182 | 0.8593 |
| live | large | 0.6305 | 0.6154 | 0.3103 | 5.8314 | 1.0937 |
| live | tool | 0.7447 | 0.6923 | 0.3548 | 5.8875 | 0.9773 |
| live | cancel | 0.4211 | 0.3636 | 0.2833 | 6.0252 | 0.9721 |

Absolute warm costs below are separate per-engine medians F / Pi, not the calculation used for paired ratios:

| Mode | Case | Turn wall ms | Turn CPU ms | Complete writes MiB |
| --- | --- | ---: | ---: | ---: |
| no-live | short-small | 27.402 / 3.743 | 30.312 / 7.500 | 72.293 / 8.875 |
| no-live | short-large | 28.876 / 3.903 | 31.875 / 6.875 | 187.809 / 20.777 |
| live | short-small | 35.398 / 3.862 | 47.812 / 7.500 | 72.539 / 8.875 |
| live | short-large | 40.638 / 3.971 | 53.125 / 6.875 | 188.523 / 20.777 |

Warm short-turn paired medians remain 7.32–10.34× Pi wall, 4.04–8.10× Pi turn CPU, 1.80–5.29× Pi complete CPU, 8.15–9.07× Pi complete process writes. Overall Pi parity is not achieved. Three pairs are exploratory and do not satisfy the seven-pair, per-case/per-mode uncertainty criterion in [comparison-policy.md](comparison-policy.md). Favorable first-request latency is not warm or overall parity.

## Native DeepSeek Harness reference

The additional reference uses pinned DeepSeek full SDK source `687f282e8625c6b8d11ff58211a90524be2e4317`, its native 25 tools, HTTP/SSE loop, compressed JSONL, checkpoint policy, notifications and native resume. Pi remains `90d863f7c1183b965f099855715d6e88c7b8a52c`. The CLI launches through `tsx` from source with the recorded `sdk-source` profile patch that disables the unavailable generated Typert loader; this is not a built-CLI startup measurement. Three Latin-order rotations cover four supported cases (36 products). Tool and mid-turn cancellation remain unsupported by this adapter.

DeepSeek `session/wait` idle does **not** await final flush/fsync. Prefix checkpoints are awaited before a model request; turn-end flush is asynchronous, and `allSettled` shutdown can suppress errors. Closed/resumed readability is checked, but is not proof of sync success or power-loss safety. SDK notifications expose committed session events/status and final assistant messages, without the incremental partial-text snapshots of the Rust/Pi live-subscriber adapters. Native prompt/tool surfaces and CPU boundaries also differ. Consequently these are absolute native completion costs, with no equal-durability speedup ratio or full-lifecycle ranking. [Adapter and contract](../../benchmarks/runtime/deepseek-reference.md).

Values are median **[three-repetition range]**. Wall is each product’s mean turn time, writes are per measured turn, and memory is maximum sampled turn anonymous PSS rather than peak. Rust/Pi count self CPU including in-process drivers; DeepSeek counts CLI/extant descendants and waited children, excluding its external Python driver.

| Case | Engine | API completion ms | CPU ms/turn | Writes MiB/turn | Sampled anon PSS MiB |
| --- | --- | ---: | ---: | ---: | ---: |
| short-small | rustX F | 47.200 [36.741, 49.191] | 61.875 [49.688, 65.000] | 1.818 [1.811, 1.822] | 112.105 [111.246, 113.555] |
| short-small | Pi Durable | 3.835 [3.795, 3.900] | 7.500 [7.500, 7.812] | 0.214 [0.214, 0.214] | 106.438 [92.160, 107.332] |
| short-small | DeepSeek SDK | 15.591 [14.547, 16.730] | 17.812 [17.188, 19.375] | 0.021 [0.021, 0.021] | 243.102 [241.113, 243.297] |
| short-large | rustX F | 44.718 [43.519, 50.521] | 59.062 [57.500, 67.188] | 2.113 [2.110, 2.115] | 131.520 [129.766, 131.586] |
| short-large | Pi Durable | 4.356 [4.185, 4.433] | 7.500 [7.188, 7.812] | 0.214 [0.214, 0.214] | 134.953 [134.758, 139.535] |
| short-large | DeepSeek SDK | 14.891 [14.835, 16.315] | 16.875 [16.875, 18.438] | 0.020 [0.020, 0.020] | 266.668 [259.156, 268.148] |
| large | rustX F | 56.665 [46.672, 57.369] | 60.000 [50.000, 70.000] | 1.516 [1.516, 1.516] | 87.996 [87.266, 88.543] |
| large | Pi Durable | 89.191 [88.413, 91.553] | 120.000 [120.000, 120.000] | 0.281 [0.281, 0.281] | 78.406 [77.660, 79.781] |
| large | DeepSeek SDK | 167.952 [167.266, 169.402] | 190.000 [190.000, 200.000] | 0.031 [0.027, 0.031] | 195.902 [194.867, 197.078] |
| stream | rustX F | 1565.374 [1562.745, 1593.520] | 160.000 [140.000, 180.000] | 1.922 [1.922, 1.922] | 88.359 [88.254, 88.434] |
| stream | Pi Durable | 1696.761 [1626.796, 1759.007] | 600.000 [490.000, 670.000] | 0.492 [0.477, 0.512] | 101.902 [98.973, 103.637] |
| stream | DeepSeek SDK | 1611.018 [1595.884, 1612.763] | 320.000 [320.000, 330.000] | 0.027 [0.027, 0.027] | 208.223 [207.723, 218.398] |

## Canonical append control

The 48 additional D/F products use histories 0/64/512/2048, 32 appends per product and three alternating pairs. Warm and reopened connections remain separate; reopened does not mean an evicted OS page cache. This control exercises canonical acceptance without provider request construction. F/D medians **[complete pair range]**:

| Connection | History | Append wall | Append/reopen phase CPU | Complete CPU | Complete writes | Sampled anon PSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| warm | 0 | 1.106 [0.976, 1.172] | 1.500 [1.000, 1.500] | 1.333 [0.750, 2.000] | 1.000 [1.000, 1.000] | 1.000 [0.999, 1.000] |
| warm | 64 | 1.004 [0.942, 1.008] | 1.000 [1.000, 2.000] | 0.875 [0.778, 1.000] | 1.000 [1.000, 1.000] | 1.000 [1.000, 1.001] |
| warm | 512 | 1.003 [0.994, 1.016] | 1.000 [1.000, 1.500] | 1.000 [0.977, 1.024] | 1.000 [1.000, 1.000] | 1.001 [1.000, 1.001] |
| warm | 2048 | 0.946 [0.722, 1.084] | 1.000 [1.000, 1.000] | 0.972 [0.812, 1.034] | 1.000 [1.000, 1.000] | 1.000 [0.999, 1.001] |
| cold | 0 | 1.009 [0.867, 1.082] | 1.059 [0.941, 1.062] | 1.059 [1.059, 1.062] | 1.000 [1.000, 1.000] | 1.000 [0.999, 1.000] |
| cold | 64 | 1.023 [0.921, 1.050] | 1.000 [0.944, 1.062] | 1.043 [1.000, 1.043] | 1.000 [1.000, 1.000] | 1.001 [1.000, 1.001] |
| cold | 512 | 1.030 [1.001, 1.061] | 1.000 [1.000, 1.059] | 1.000 [1.000, 1.018] | 1.000 [1.000, 1.000] | 1.000 [0.999, 1.000] |
| cold | 2048 | 0.993 [0.977, 1.547] | 1.000 [0.950, 1.050] | 1.022 [1.009, 1.202] | 1.000 [1.000, 1.000] | 1.001 [1.000, 1.001] |

The append control also contains adverse observations. Cold history 0 complete CPU increases by exactly one 10 ms tick in every pair (170→180, 170→180, 160→170 ms). Cold history 512 append wall is higher in every pair, by 0.07–6.11% (median 3.01%). Cold history 2048 complete CPU increases by 20/450/50 ms; one lifecycle lasts **11.703 s versus 2.533 s (4.62×)** and remains unexplained. All append write counters are equal within pairs. These results do not establish a canonical-append improvement or general nonregression.

## Validation, identities and scope

F's ordinary optimized conformance binary is SHA-256 `7ca19d99e1a241e5e33627819fe837a81788d26609072d8c41a1a0ab805ef109`; D is `8ec26a4a08bde085f46ee40159d3870f7ead675bec39f42cb4f3e55df1d1d233`. F was rebuilt with root artifacts invalidated and `fresh:false` for the product library, binary and conformance executable. Both binaries use the same Rust compiler and six-file runtime harness. The delivered production source tree remains `a86302ed48b415de91cac782bd156dc935b8e517`; later evidence/documentation commits do not change it.

Correctness validation passed **1,323 unique Rust tests**, with one measurement-only test ignored: SQLite 435, Runtime Client 532, conversation runtime 94, read layout 26, content 27, durable integration 147 and process integration 62. The independent audit deduplicates test names by executable and verifies the saved commands/artifacts. This selected correctness suite uses all features, release profile with **root rustx optimization level 0**; it is distinct from the ordinary optimized production binaries used for every formal measurement. No full ordinary-O3 unit-suite run is claimed. Formatting and all-target/all-feature Clippy with warnings denied passed. The first Clippy attempt found two documentation backtick warnings; those were fixed and the failed log remains included.

Final settlement tests cover late-reader/reopen text and tool bytes, buffered and live-prefix behavior, degraded optional indexes, invalid identities/sequences, raw-content failure rollback and canonical block/tool mismatches. Native length tests compare byte-buffer and counting paths, including Unicode, escaping, raw JSON, integer admission, exact budgets and serializer failure. Request Fact tests compare complete authenticated pages with an independent legacy capture implementation, capped-node and 16 KiB boundaries, reserved aliases, duplicates and hidden invalid subtrees. These checks retain eager reader indexes; they do not revive the rejected E design.

This round records **284 accepted formal products**: initial F/D 72, selected stream/cancel confirmation 56, F/Pi 72, append 48, and three-engine 36. Separate diagnostic products are four D CPU profiles plus four controls, four D/Pi WAL products and four F/Pi WAL products. An initial Pi WAL import failure is retained alongside the fixed attempt. Formal products ran serially without task-owned builds, tests, profiling or compression; unrelated host activity is not excluded. Host details and timestamps are retained: Linux 6.8, i9-14900K, ext4, Rust 1.98.1, Node 24.19.0, 100 CPU ticks/s.

Independent readers recompute numeric values from raw counters and check recorded oracle outcomes; they do not independently reimplement every semantic oracle. D's WAL runner reused its accepted Rust directory during the Pi import repair, and its global identity was recorded afterward. Its preserved binary/source path and independent hash audit support provenance, but this is weaker than F's new per-product identity frozen before launch. F records binary/build/driver/runner hashes, argv and pinned Pi revision before every WAL product and checks hashes again afterward. Both WAL diagnostics instrument production with strace and synchronous snapshot handshakes, so their times never enter the formal performance tables. Endpoint page attribution cannot exclude intermediate page reuse; unexplained/unmapped pages remain explicit.

The earlier [B/C report](2026-10-27-performance.md) and [D/E continuation](2026-10-27-continuation.md) remain historical evidence. No new browser E2E/full repository CI run or main-branch integration is claimed. Existing PR conflicts remain, and the PR stays a draft.

## What is retained and what remains

F is retained as a tested removal of unobservable final writes and repeated work, with modest measured write savings. The complete performance evidence is mixed, including the adverse append controls; neither global CPU/latency improvement, all-metric nonregression nor Pi parity is claimed. Retention is an engineering tradeoff for a bounded write/work reduction, not a passed nonregression gate; the unexplained wall outliers have not been dismissed as harmless noise. The seven-pair follow-up does not erase the original adverse samples or small memory increases. Multiple changes were measured together, so their separate speed contributions are not established.

A next, explicitly separate structural change could publish the final provider/canonical/turn/attempt family once, after all its in-memory owners have installed their projections. The current source suggests a reduction from 16 to 13 commits and six WAL frames per turn, not a measured gain. It needs tests for historical/intermediate cuts, foreign-owner holes, failure before complete installation, independent readers, tools and pending adoption. Loosening those visibility rules silently would change functionality.

A larger opportunity is sharing indexed immutable request configuration/tool fragments and binding every request to its exact version, as the references avoid repeatedly persisting unchanged declarations. Existing content-addressed Source fragments could support this, but field ordinals, byte offsets, aliases, configuration changes, retry/compaction, reopen and corruption behavior must remain exact. The first layer still needs exact serialization/content hashing where the native model exposes no immutable configuration identity. This design is documented as a future candidate, not an implemented or measured speedup, and must not move capture work into slower historical reads.

## Evidence and reproduction

[Raw evidence archive](evidence/write-compute-20261027/raw.tar.xz) and its [SHA-256 member manifest](evidence/write-compute-20261027/raw.tar.manifest.json) retain both confirmation and initial cohorts, raw requests/counters, native SDK resume/log records, profiles, WAL traces/snapshots, failures, build identities, commands and independent audit scripts. Executable binaries and dependency checkouts are excluded.

- [F build identity](evidence/write-compute-20261027/f-build-identity.json), [D build identity](evidence/write-compute-20261027/d-build-identity.json), [initial plan](evidence/write-compute-20261027/plan.json), [confirmation plan/commands](evidence/write-compute-20261027/confirmation-commands.json), [final commands](evidence/write-compute-20261027/final-commands.json).
- [Direct and confirmation audit](evidence/write-compute-20261027/paired-cohorts-audit.json), [F/Pi recomputation](evidence/write-compute-20261027/pi-recomputed.json), [append audit](evidence/write-compute-20261027/append-audit.json), [three-engine audit](evidence/write-compute-20261027/three-engine-audit.json).
- [D diagnostic audit](evidence/write-compute-20261027/diagnostic-audit.json), [F WAL audit](evidence/write-compute-20261027/wal-f-audit.json), [test/build audit](evidence/write-compute-20261027/validation-audit.json), [retention caveats](evidence/write-compute-20261027/f-retention-audit-notes.txt).

Download the separate [DeepSeek source identity](evidence/write-compute-20261027/deepseek-source-identity.json) alongside the archive; it is required by the three-engine reader and is not inside this archive. For portable numeric reproduction after extraction, run `recompute-direct.py --root direct-cohorts`, `recompute-confirmation.py --root confirmation-cohorts`, `recompute-pi.py --root final-cohorts --raw-only`, `audit-final-append.py --root final-cohorts --binary-verification off --output append-reproduced.json`, and `recompute-three-engine.py --dataset final-cohorts/three-engine --raw-only --freeze /path/to/deepseek-source-identity.json --output three-reproduced.json`. Raw-only replay checks archived arithmetic/evidence; it deliberately does not claim to re-verify the original external binaries, Git checkouts or dependencies. Strict identity audits were performed on the measurement host before packaging.

The [independent archive verification](evidence/write-compute-20261027/archive-verification.json) safely extracted and twice hash-checked all 1,984 payload files (1,569,478,641 uncompressed bytes), then reproduced all five cohorts (284 products). Numeric/product/group records agree; path relocation and deliberately omitted external identity checks are recorded separately. The separately downloaded DeepSeek identity is hash-checked as a sidecar. The [second final-products review](evidence/write-compute-20261027/final-products-review.json) rechecks raw counters and confirms 414 main provider requests per engine plus 12 native SDK resume requests. These review/verification sidecars were created after packing and are not claimed to be inside the archive.

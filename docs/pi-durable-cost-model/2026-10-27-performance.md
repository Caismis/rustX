# PR #485: measured performance continuation, 2026-10-27

> Historical B/C round. The [D/E continuation and DeepSeek reference](2026-10-27-continuation.md) identifies the currently delivered source and its fresh comparisons.

The candidate reduces median complete-process writes for warm short cases by **14.5–15.0%** and complete CPU by **16.7–23.6%** against recovery 623. Turn latency improves unevenly: short-small medians improve in both modes, while live short-large is nearly unchanged and some cold/streaming costs increase. **Pi parity is not achieved:** warm short-turn wall remains **7.3–12.9× Pi**, and complete writes remain **8.4–9.3× Pi**.

## Scope and identities

The final matrix contains **144 accepted products**, three alternating pairs per case, all six cases and both observation modes, in four separate cohorts: candidate/baseline and candidate/Pi. All 1,692 ordered request comparisons passed their declared semantic scope. Rust/Rust comparisons include native tool definitions; Rust/Pi comparisons retain the seven-versus-four native tool/schema difference, so full cross-engine request equality is false. Both use the same sibling HTTP/SSE provider, fixed model-facing time `2026-10-19 00:00:00`, ordered history and scripted responses/tool/cancellation gates. No real inference service is measured.

| Product | Source identity | Ordinary optimized executable SHA-256 |
| --- | --- | --- |
| Recovery baseline | `623b41b869a95c888eb04f907cbf607232699a2f` | `fc4f95b1363c554196a93c16d742eda9a8c7cd7cb6fb65caa98ea702edd7cb89` |
| Retained candidate (B) | `cb6e026b4ac84842703dfc6adbaec43d700bff24` | `f80f3b7ba0f2be6035c7459c87cf04e69ec596e64ccba0d72d43ddce7d8ebdb3` |
| Actual Pi Durable SQLite | `90d863f7c1183b965f099855715d6e88c7b8a52c` | Source revision and checkout are verified per raw sample. |

Candidate production tree is `e3f821550dbe5ba8d54c269251b6e0ce5152b3b4`; baseline tree is `19ff7d6bd64993a6b9fc08bbcd687787dacfe08e`. Build identities retain matching harness hashes, Rust 1.98.1, ordinary release settings and rebuilt root artifacts. These performance executables do not use the reduced root optimization used for correctness tests. The four runtime cohorts ran serially from 18:13:41 to 18:18:15 UTC on October 26 (October 27 locally), after the candidate build and before the separate CPU profiles.

## Candidate / recovery baseline

All cells are **medians of within-pair cost ratios**; smaller is better. Complete CPU/writes are absolute product self counters since exec through cleanup, including startup and verification. The short cases measure 32 turns after 8/64 history turns; other cases measure one first request and include cold work. Observer modes are never pooled.

| Mode | Case | Turn wall | Turn CPU | Complete CPU | Complete writes | Sampled anon PSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| no-live-subscriber | short-small | 0.8253 | 0.8480 | 0.8326 | 0.8505 | 0.9973 |
| no-live-subscriber | short-large | 0.8320 | 0.8689 | 0.7641 | 0.8545 | 0.9999 |
| no-live-subscriber | stream | 1.0064 | 1.1250 | 1.0000 | 0.9787 | 1.0006 |
| no-live-subscriber | large | 1.1268 | 1.2000 | 1.0000 | 0.9776 | 1.0043 |
| no-live-subscriber | tool | 0.9081 | 1.0000 | 1.0000 | 0.9574 | 0.9959 |
| no-live-subscriber | cancel | 0.9390 | 0.6667 | 1.0000 | 0.9796 | 0.9978 |
| live-subscriber | short-small | 0.7946 | 0.8035 | 0.7809 | 0.8542 | 1.0053 |
| live-subscriber | short-large | 0.9735 | 1.0046 | 0.8203 | 0.8542 | 0.9931 |
| live-subscriber | stream | 1.0043 | 1.1000 | 1.0294 | 0.9787 | 0.9960 |
| live-subscriber | large | 0.9780 | 1.0000 | 1.0000 | 0.9776 | 1.0082 |
| live-subscriber | tool | 0.9162 | 0.9091 | 0.9600 | 0.9574 | 1.0009 |
| live-subscriber | cancel | 0.9723 | 0.8000 | 1.0000 | 0.9796 | 1.0038 |

Write reduction is consistent across these pairs. The warm short-small wall intervals still include no improvement: no-live 90% log-t interval [0.690, 1.097], live [0.693, 1.046]. No-live short-large wall/CPU intervals are [0.714, 0.970]/[0.750, 0.982], while live short-large remains inconclusive at [0.812, 1.210]/[0.841, 1.219]. Adverse medians are retained: no-live large wall/CPU rise 12.7%/20.0%; streaming turn CPU rises 12.5% without a subscriber and 10.0% with one. Live short-small sampled anonymous PSS rises 0.53%, with interval [1.001, 1.008]. This is a useful reduction in some costs, not uniform acceleration.

## Candidate / pinned Pi Durable

| Mode | Case | Turn wall | Turn CPU | Complete CPU | Complete writes | Sampled anon PSS |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| no-live-subscriber | short-small | 7.3050 | 4.2273 | 1.9355 | 8.3719 | 1.1251 |
| no-live-subscriber | short-large | 8.6988 | 5.3182 | 3.9362 | 9.2850 | 1.3084 |
| no-live-subscriber | stream | 0.9393 | 0.3000 | 0.2752 | 5.0182 | 0.8660 |
| no-live-subscriber | large | 0.5688 | 0.4167 | 0.2881 | 6.0988 | 1.0965 |
| no-live-subscriber | tool | 0.7073 | 0.5833 | 0.3651 | 6.0000 | 0.9385 |
| no-live-subscriber | cancel | 0.4537 | 0.3333 | 0.2833 | 6.0252 | 0.9520 |
| live-subscriber | short-small | 12.1681 | 8.3043 | 3.2626 | 8.3763 | 1.2506 |
| live-subscriber | short-large | 12.8562 | 9.5909 | 5.6127 | 9.2905 | 1.3172 |
| live-subscriber | stream | 0.9263 | 0.3065 | 0.2768 | 5.0182 | 0.8738 |
| live-subscriber | large | 0.5894 | 0.5000 | 0.3000 | 6.0988 | 1.0941 |
| live-subscriber | tool | 0.7610 | 0.7500 | 0.3492 | 6.0000 | 0.9217 |
| live-subscriber | cancel | 0.5248 | 0.4167 | 0.3016 | 6.0252 | 0.9659 |

For scale, the following are **separate per-engine medians, candidate / Pi**, from the Pi cohorts above. They are not divided to obtain the paired ratios.

| Mode | Case | Turn wall (ms) | Turn CPU (ms) | Complete writes (MiB) |
| --- | --- | ---: | ---: | ---: |
| no-live-subscriber | short-small | 31.147 / 3.638 | 32.188 / 7.188 | 74.301 / 8.875 |
| no-live-subscriber | short-large | 33.705 / 3.869 | 36.562 / 6.875 | 192.918 / 20.777 |
| live-subscriber | short-small | 50.121 / 3.806 | 67.188 / 7.500 | 74.340 / 8.875 |
| live-subscriber | short-large | 50.121 / 3.899 | 65.938 / 6.875 | 193.031 / 20.777 |

The no-live short-small Pi wall comparison has a particularly wide 90% interval [2.133, 15.095], so its 7.305 median is not a precise gap estimate. Even the favorable cold large/tool/cancel wall and CPU results coexist with roughly 5–6× complete writes. They cannot establish warm steady-state or overall parity. Three pairs are exploratory; the existing [comparison policy](comparison-policy.md) requires seven and the per-case/mode uncertainty criterion for a parity claim.

## Durability and the work removed

The pinned [Pi Node SQLite adapter](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/storage/sqlite/node.ts) uses WAL/NORMAL, 1,000-page automatic checkpoints, serialized `BEGIN IMMEDIATE` transactions, statement reuse and a truncating checkpoint at close. rustX already uses WAL/NORMAL; this change does not gain speed by disabling persistence. Pi's [generation loop](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/harness/generation.ts) throttles partial commits (default 100 ms), allows one in flight, and awaits it before terminal handling. Its [context reader](https://github.com/earendil-works/pi/blob/90d863f7c1183b965f099855715d6e88c7b8a52c/packages/durable/src/harness/context.ts) reuses immutable ranges and scans appended suffixes. Matching these persistence settings is not proof of identical recovery or tool-execution contracts.

At recovery 623, `persist_event_tx_internal` writes the native event, runs multiple optional observation builders, constructs an event Source, copies its bytes, writes field rows, then reads selected fields back to build a witness. Publication and explicit inspection add further Source/Trace reads and encoding. The earlier [cost map and profile](final-production-profile.md) observed 11 Source constructions per short turn, including eight event envelopes, plus substantial SQL preparation and observation work. Those historical counts locate candidate work; they are not fresh attribution of the final binary or timings to combine with this matrix.

| Retained change | Reduced work and preserved contract |
| --- | --- |
| Inline small Journal Sources (`ef73676e4`) | Store exact structural bytes and a separate fixed extent for Sources up to 16 KiB, derive the witness from the same Source, and navigate fields on explicit reads. Avoid segment/field inserts followed by rereads. Preserve journal bytes, historical cuts, aliases, pagination and deferred previews; larger events retain segmented storage. Schema 76 rejects older layouts without migration. |
| Event-specific optional builders (`c8d08ad0e`) | Skip empty event branches and reuse SAVEPOINT/RELEASE/ROLLBACK statements. Actual builders retain independent rollback and unavailable-evidence behavior. |
| Source/context lookup reuse (`3b5bb186c`) | Combine repeated lookups while preserving original order, historical selection and missing/corrupt evidence checks. |
| Native statement reuse and capacity (`576a92621`, `91a00da4d`) | Reuse node-shape queries; set a fixed capacity of 64 for reader/writer connections. Static expansion finds roughly 29–31 hot SQL statements in an ordinary turn, exceeding the default 16. This is a working-set argument, not a count of prepare call sites. |
| Seven fixed journal statements (`d1825d0d2`) | Cache the journal-frontier read, event insert, observation JSON/witness/inline-source writes, sequence update and availability update. SQL meaning and transaction boundaries are unchanged. |
| Publication metadata ownership (`f363c3cd6`) | Move owned metadata into transitions and recover it for replacement, retaining necessary checkpoint copies and equivalent cursor/eviction/budget behavior. |
| Cheap Trace admission (`0ea9017dc`) | Avoid full encoding when the cheap budget predicate decides; retain the encoded fallback for boundary and large-native-integer cases. |

The candidate retains events, request capture, tool results, cancellation evidence and historical inspection. It does not import the broad geometry/scoped-cache branches: [fresh-host evidence](https://github.com/Caismis/rustX/blob/a2bf3640e97ae4e2ae386f57e5d0e7d84e67cea0/docs/pi-durable-cost-model/evidence/fresh-source-20261020/README.txt) found a 1.483 live short-large wall ratio for geometry and no established live short-turn benefit for scoped caching. Removing Source capture would remove inspection functionality. Request Context indexes are also not an O(history) target: they store only newly introduced Context; both baseline short smoke measurement windows introduced zero such rows.

## Validation and evidence limits

`validation.json` identifies the same final source/tree and records **1,211 Rust tests passed, one ignored**, all-target/all-feature Clippy with warnings denied, **473 web recovery tests**, **1,162 TUI tests with seven skipped**, and **48 Python + 24 Node harness tests**. Correctness builds use release dependencies with root-package opt-level 0, separately from the ordinary optimized performance builds. Rust scale groups, full browser E2E and full repository CI were not run. No main merge was performed.

The [independent raw reader](evidence/local-performance-20261027/recompute.py) verifies the complete matrix, accepted statuses, pinned Pi revision, process/manifest bindings, recorded request comparisons and reopened inputs. It computes counters directly from raw `/proc` stat/io/smaps records, without importing producer statistics. `recomputed.json` retains all absolute arithmetic, 576 paired metric rows and 192 summaries, including auxiliary lifecycle wall/RSS/PSS. Every summary includes median, range, geometric mean and two-sided 90% log-t interval (n=3, df=2, critical 2.91998558; upper endpoint is also a one-sided 95% bound). These intervals estimate the geometric mean ratio, not the table median. This assumes independent approximately normal log ratios; it does not make the small cohort universal. All final ratios are determinate; failed/slow diagnostic records and preliminary smoke/stage-A evidence remain separate from this final matrix.

CPU includes product threads and driver/verification and has 10 ms tick resolution; small cold-request CPU ratios are coarse. Writes are process-attributed, not physical-device traffic. Memory is a sampled maximum, not a true peak; allocations are unavailable. Late inspection, close/checkpoint/reopen and cleanup remain visible; post-cleanup evidence serialization is excluded. The reader independently checks arithmetic and recorded oracle results, not a second implementation of the full semantic oracle. A ratio table cannot identify causal CPU shares.

## Append diagnostics

All **48 append products** passed: three pairs at histories 0, 64, 512 and 2,048, separately warm and reopened before each append. “Cold” means a connection reopen, not eviction of the OS page cache. The independent [append audit](evidence/local-performance-20261027/append-audit.json) matches every producer metric and preserves all slow samples. This is a separate baseline/candidate test, not a Pi comparison. Each product measures 32 appends.

Cells are paired candidate/baseline medians; brackets show the full observed pair range, not a confidence interval. Complete writes and database/file footprints are identical within every pair. The CPU columns are particularly coarse at this scale.

| Mode | History | Append wall [range] | Append CPU | Complete CPU | Lifecycle wall [range] | Sampled anon PSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| warm | 0 | 0.867 [0.807, 0.920] | 1.000 | 1.000 | 0.826 [0.807, 0.875] | 1.006 |
| warm | 64 | 1.018 [0.991, 1.045] | 2.000 | 1.000 | 1.030 [0.947, 1.059] | 1.004 |
| warm | 512 | 0.991 [0.956, 1.034] | 0.800 | 1.000 | 1.025 [0.994, 2.591] | 1.003 |
| warm | 2,048 | 0.974 [0.970, 1.018] | 1.250 | 0.986 | 0.988 [0.370, 1.020] | 1.001 |
| cold | 0 | 0.977 [0.946, 1.029] | 1.000 | 1.000 | 0.979 [0.933, 1.013] | 1.001 |
| cold | 64 | 1.012 [0.981, 1.062] | 2.000 | 1.045 | 1.022 [1.012, 1.063] | 1.004 |
| cold | 512 | 1.003 [0.980, 1.004] | 1.200 | 0.983 | 0.990 [0.979, 0.992] | 1.003 |
| cold | 2,048 | 1.080 [0.950, 3.073] | 1.429 | 1.018 | 1.021 [0.994, 3.498] | 0.999 |

The retained cold-2,048 pair 2 takes 9,031 ms candidate versus 2,582 ms baseline from initialization through cleanup. Of the extra 6,449 ms, 5,909 ms is history seeding and 538 ms is the append/reopen phase. Self CPU increases only 490 ms (2,770 versus 2,280 ms), while writes and file footprints are identical. The counters cannot distinguish I/O waiting, scheduling or other environmental causes; they neither prove a code regression nor justify excluding the sample. Warm outliers are retained symmetrically: history-512 pair 2 has 12.413/12.980 ms candidate/baseline mean append times (other pairs about 1 ms), with lifecycle 4,597/1,774 ms; history-2,048 pair 2 has lifecycle 2,327/6,283 ms. No general append-speed or stable-tail claim follows from these data.

At 100 CPU ticks/second, a 32-append average has 0.3125 ms units. Cold append CPU sums 32 separately rounded brackets, excluding reopens; zero brackets do not mean zero CPU, and rounding can accumulate. For example, cold-64 append CPU ratios range 0.571–7.000, while whole append/reopen-phase CPU ratios range only 1.000–1.0625. The [additional phase notes](evidence/local-performance-20261027/append-audit-notes.txt) retain these distinctions.

## Fresh CPU attribution

The separate **16 accepted diagnostic runs** cover baseline/candidate, both modes, both short cases, and CPU sampling/control. The ordinary optimized binaries are unchanged. Locally unpacked gperftools 2.15-3build1 samples product threads at 250 Hz; the sibling provider is not preloaded. There is one profile/control pair per cell. Their observed mean-turn ratios span 0.755–1.151 and turn-CPU ratios span 0.769–1.157, so these controls do not establish a precise profiling tax or candidate improvement. Diagnostic timings never enter the formal tables.

Each cell below is **baseline sample count/total → candidate count/total**, for the 32 measured turns. These are inclusive CPU-stack samples; rows overlap and must not be added. Counts are independently verified against complete binary profile records. A missing no-live read row is not treated as a measured zero.

| Mode / history | SQLite prepare | Publication encoding | Request layout indexing | Live read-cut refresh |
| --- | --- | --- | --- | --- |
| no-live / small | 67/289 → 36/276 | 59/289 → 62/276 | 49/289 → 53/276 | — |
| no-live / large | 67/309 → 41/284 | 63/309 → 62/284 | 55/309 → 52/284 | — |
| live / small | 140/530 → 83/410 | 136/530 → 93/410 | 64/530 → 51/410 | 111/530 → 109/410 |
| live / large | 127/539 → 87/530 | 123/539 → 146/530 | 72/539 → 73/530 | 106/539 → 101/530 |

The remaining work has concrete contracts: publication freezes replay/read-cut metadata, fits the bounded checkpoint and validates the wire budget; live refresh decorates transcript response/process metadata; request indexing retains field navigation and exact byte slices. No-live mode still maintains publication state for later readers. `publish_attempt::{closure#5}` spans the whole spawned attempt, so its inclusive samples are **not** isolated publication cost. The bounded Source fact spool already caches its SQL; these profiles do not justify increasing its memory bound or the statement cache beyond 64.

The [profile analysis](evidence/local-performance-20261027/profile-analysis.json) ties residual prepare stacks to explicit uncached witness/completion/read queries. Those queries were tested in the separate SQL-cache experiment below and the change was reverted. Wider removal of Source capture, validation or late-reader publication would change functionality. Pi's inspected storage paths serialize document/table values without constructing these rustX-specific field indexes and bounded publication structures.

## Rejected follow-up: additional native read statement reuse

Stage C (`8760f570999a4693f59db5c446804afce34b74c8`, production tree `807eb84ba5bfcb9d68ba6c39988aaff157e298a6`) changed 22 query call sites to reuse 18 distinct fixed SELECT statements for witness/completion and native response/process/execution reads. It retained the same SQL, parameters, optional/error behavior and cache capacity. Its ordinary optimized binary SHA-256 is `43310780974a2acdbc9b6b5c10eaa20682956d782053e7595587babb74379f4f`; it separately passed the same 1,211 Rust tests and Clippy before measurement.

The direct C/B experiment accepted **72 products and 846 ordered request comparisons**, with three alternating pairs for all six cases in both modes. No build, profile or compression overlapped measurement. These are fresh paired ratios against B, not products of ratios from earlier experiments. Full same-engine semantic/schema comparisons passed. The [independent audit](evidence/read-cache-trial-20261027/retention-audit.json) checks all raw/process hashes, actual binary hashes, build identities and all 96 metric summaries; the [raw reader output](evidence/read-cache-trial-20261027/direct-recomputed.json) contains the full intervals and absolute costs.

All cells are median C/B ratios. Lifecycle wall is an auxiliary metric from initialization through cleanup, separate from measured-turn wall.

| Mode | Case | Turn wall | Turn CPU | Complete CPU | Complete writes | Anon PSS | Lifecycle wall |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| no-live | short-small | 0.8951 | 0.8850 | 0.9417 | 0.9984 | 1.0120 | 0.9451 |
| no-live | short-large | 1.0939 | 1.0342 | 0.9861 | 1.0025 | 1.0103 | 0.9868 |
| no-live | stream | 0.9920 | 1.1667 | 1.1481 | 1.0000 | 1.0000 | 0.9956 |
| no-live | large | 1.0254 | 1.2000 | 1.0500 | 1.0000 | 0.9986 | 1.0146 |
| no-live | tool | 1.0095 | 1.1667 | 1.1000 | 1.0000 | 0.9995 | 1.1216 |
| no-live | cancel | 0.9272 | 1.0000 | 0.9444 | 1.0000 | 0.9990 | 0.9801 |
| live | short-small | 0.9054 | 0.9043 | 0.9544 | 0.9989 | 0.9998 | 0.9687 |
| live | short-large | 0.9611 | 0.9609 | 0.9620 | 0.9981 | 1.0074 | 0.9817 |
| live | stream | 1.0032 | 0.9524 | 1.0000 | 1.0000 | 1.0040 | 1.0028 |
| live | large | 1.1089 | 1.1667 | 1.1111 | 1.0000 | 0.9949 | 1.1147 |
| live | tool | 1.0455 | 1.0000 | 1.0000 | 1.0000 | 1.0142 | 1.0331 |
| live | cancel | 1.0621 | 1.0000 | 1.0000 | 1.0000 | 0.9992 | 0.9521 |

There is a clear local benefit: no-live short-small saves 3.30–3.60 ms wall and 4.0625 ms CPU per turn in every pair. Its wall/CPU geometric-mean 90% intervals are [0.892, 0.898]/[0.877, 0.896]. Live short-large complete CPU also improves, with interval [0.948, 0.991]. However, all three no-live short-large wall ratios exceed one (median 1.0939, interval [0.995, 1.186]); all live large/tool lifecycle wall ratios also exceed one, with intervals [1.0423, 1.1691]/[1.0217, 1.0555]. The latter increases are 16.4–33.8 ms and 8.8–16.0 ms and span initialization, provider work, close and verification. Cold CPU is coarse; neither these timings nor their intervals establish that statement reuse caused each adverse delay.

Under the predeclared wider-cost retention criterion, C was conservatively **rejected and reverted** by `caaab5f2b6d6c3ec9d0f19a35249c5fe70e367a8`. All tracked files then matched measured B byte for byte (`git diff --exit-code cb6e026b4ac84842703dfc6adbaec43d700bff24 HEAD` exited zero); the retained production tree remains `e3f821550dbe5ba8d54c269251b6e0ce5152b3b4`. The decision retains positive and adverse evidence and claims none of C's gains for the final code. Fresh C/recovery-623, C/Pi, append and profiling cohorts were not run because C was not retained. The final B performance and validation evidence above therefore still identifies the delivered source.

The separate [trial archive](evidence/read-cache-trial-20261027/raw.tar.xz), [manifest](evidence/read-cache-trial-20261027/raw.tar.manifest.json), [retention record](evidence/read-cache-trial-20261027/retention.json) and [extraction verification](evidence/read-cache-trial-20261027/archive-verification.json) preserve the experiment. The archive contains 436 payload files plus the identical embedded manifest; its SHA-256 is `545152495c76d4175b26739a8309b03902b85e67e99158dc92d837d720b6c434`. Independent extraction verified every file and reproduced all 72 products/846 comparisons byte for byte. Reproduce its arithmetic with `python3 recompute-direct.py --root /path/to/extracted/trial` using the [standalone reader](evidence/read-cache-trial-20261027/recompute-direct.py).

## Reproduction and archive

The [raw archive](evidence/local-performance-20261027/raw.tar.xz) contains all four final runtime cohorts, 48 append samples, preliminary smoke/stage-A runs, 16 separate profile/control runs, build identities, commands, host information and validation logs. The [manifest](evidence/local-performance-20261027/raw.tar.manifest.json) lists 1,559 payload files; the embedded identical manifest is a 1,560th archive member. The [independent extraction check](evidence/local-performance-20261027/archive-verification.json) verified every payload size/SHA-256, then reproduced [recomputed.json](evidence/local-performance-20261027/recomputed.json) **byte for byte** from the extracted raw files: 144 accepted products and 1,692 request comparisons. The final external append audit adds warm-outlier analysis to the earlier archived audit snapshot; all 146 raw append directory files remain identical.

The raw archive SHA-256 is `4f406110811a7d82a440f4187cc18a9fab42575a6e70cb16bdc932b4104bffa1`. Original binaries remain preserved locally; binaries are not embedded in Git. `baseline-build/identity.json` and `candidate-final-build/identity.json` retain exact build commands, toolchains, harness hashes and executable/library hashes. `measurement-commands.json` records all formal commands and start/end times; `run-measurements.py` reproduces their ordering after adjusting paths. Profile packages, bridge provenance, raw stacks and symbolization commands are separately recorded. The [attribution supplement](evidence/local-performance-20261027/profile-attribution.tar.xz) supplies collapsed stacks and the analysis script generated after the raw archive was frozen.

```sh
python3 docs/pi-durable-cost-model/evidence/local-performance-20261027/recompute.py \
  --root /path/to/extracted/raw > recomputed.json
```

Use `benchmarks/runtime/build-revision.py --repo CHECKOUT --target TARGET --output NEW_DIR` on clean checkouts of the recorded revisions to rebuild preserved binaries. It invalidates root-package release artifacts and verifies both library and conformance artifacts were freshly compiled. Then run the recorded `compare-revisions.py`, `controller.py` and `compare-append.py` commands serially, without builds, profiling or compression. The fake-provider Python environment and pinned Pi packages must first be prepared as recorded under `setup/`; historical absolute paths in manifests are identity records, not portable installation paths.

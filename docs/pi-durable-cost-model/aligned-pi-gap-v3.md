# Current strict-input gap against Pi Durable SQLite

All **72 fresh products pass**: three alternating Rust/Pi pairs, both aligned observation modes and all six cases. The external-controller matrix completed in **129.86 seconds**, without compilation, profiling, compression or another benchmark overlapping it. It checked 1,692 complete local HTTP/SSE requests and 846 ordered core-semantic pairs. This is a diagnostic current-gap comparison, **not seven-pair parity acceptance**.

Rust is the published `300c8d2b` production tree, unchanged from source `b87ae908b17dc9b63c2e6993a0c81d10c0960177`, tree `f0943a155a83249499c335e3352b2a9ccba7ed72`. Its freshly built, preserved normal optimized executable has SHA-256 `52240c89502584f7ef85cc5ce7b6425ed204c8d70f82d97fca0375a9d0323f45`. Pi is actual Durable SQLite revision `90d863f7c1183b965f099855715d6e88c7b8a52c`, clean tracked source, Node 24.19.0, production `openAICompletionsApi()`. The newer checkpoint proof at `0f79f812` is **excluded**; its unfinished build was interrupted before this cohort and its failure identity/logs are retained. Matching preserved Rust/native-supervisor executables were restored and hash-checked before measurement.

Both production adapters use the same controller-owned sibling provider and identical protocol, model parameters, ordered history, scripted response/chunks, tool/cancellation gates and model-facing time `2026-10-19 00:00:00`. Runtime/elapsed/deadline clocks remain real. Each engine independently validates complete expected/wire/captured/frozen/reopened inputs and results, useful live updates when enabled, late snapshots and complete cleanup. The provider survives product cleanup and is reaped by the controller. There is no real inference service, synthetic matching delay or substituted in-process provider.

**Full native request equality is false in all 846 pairs.** Rust has seven native tools; Pi has four, with different native schemas. The comparator retains their names and schema hashes and compares every other normalized request field exactly. Only text representation, JSON argument spelling and the exact fixture path receive the previously declared semantic normalization. Time text is exact. Literal HTTP-byte equality and feature/durability equivalence are not asserted. Earlier unequal-observer or timestamp-normalized numbers are not used to calculate this gap.

Values are medians of **within-pair Rust/Pi ratios**. Complete CPU and writes are absolute product self counters since exec through cleanup. The short cases have 32 measured turns after 8/64 warm-history turns. Other cases measure one first request; their lower wall/CPU ratios include cold first-request work and must not be generalized to warm steady operation.

| Mode | Case | Turn wall | Turn CPU | Complete CPU | Complete writes | Sampled anon PSS |
|---|---|---:|---:|---:|---:|---:|
| no-live-subscriber | short-small | 7.0545 | 3.9000 | 1.8387 | 9.0370 | 1.0866 |
| no-live-subscriber | short-large | 7.7387 | 4.7600 | 4.0511 | 9.8527 | 1.2817 |
| no-live-subscriber | stream | 0.9316 | 0.3111 | 0.2456 | 5.2074 | 0.8583 |
| no-live-subscriber | large | 0.3954 | 0.2857 | 0.2361 | 6.1221 | 1.1465 |
| no-live-subscriber | tool | 0.5455 | 0.5000 | 0.2727 | 6.2708 | 0.9428 |
| no-live-subscriber | cancel | 0.3160 | 0.2667 | 0.2162 | 6.1572 | 0.9586 |
| live-subscriber | short-small | 9.4397 | 7.0000 | 2.7252 | 9.0392 | 1.2456 |
| live-subscriber | short-large | 11.7024 | 8.9286 | 5.8889 | 9.8445 | 1.3602 |
| live-subscriber | stream | 0.9453 | 0.3265 | 0.2523 | 5.2304 | 0.8678 |
| live-subscriber | large | 0.4281 | 0.3571 | 0.2361 | 6.1221 | 1.1234 |
| live-subscriber | tool | 0.6053 | 0.5882 | 0.3067 | 6.2708 | 0.9685 |
| live-subscriber | cancel | 0.3326 | 0.3333 | 0.2125 | 6.1635 | 0.9806 |

For scale, separate per-engine median short-small wall costs are 34.22/4.77 ms without a live subscriber and 51.27/5.30 ms with one. Short-large is 38.61/4.75 ms and 57.64/4.91 ms. These separate medians are not divided to produce the paired ratios above. Complete writes are about 80.2/8.875 MiB for short-small and 204.6–204.8/20.781 MiB for short-large. Substantial steady-state and write gaps remain; no overall parity claim follows from favorable single-request cases.

The [180 paired metric rows](evidence/aligned-pi-gap-v3/paired-costs.csv) retain every pair and absolute cost. [All 60 intervals](evidence/aligned-pi-gap-v3/paired-uncertainty.json) use the paired log-t method, df 2, critical 2.91998558: two-sided 90%, equivalent to a one-sided 95% upper bound. The small cloud sample assumes independent approximately normal log ratios. No failed or slow sample was removed, no observer modes pooled and no unrelated medians subtracted.

CPU is self user+system across product threads and driver, with 10 ms ticks. It includes verification; it is not pure engine/DB CPU. Process evidence serialization follows the cleanup endpoint and is excluded. Startup/provider/control, initialization, steady turns, late snapshot/verification, shutdown, checkpoint/final close, reopen and cleanup are separately recorded. RSS/PSS are sampled maxima, not peaks; allocations are unavailable. Writes are process-attributed, not physical-device writes; DB/WAL/SHM and Source extents remain in the raw samples. Source/SQL work is not inferred from these wall ratios; separate diagnostic profiles remain necessary. Native tool, observer and recovery contracts remain explicit.

The [verified 409-file archive](evidence/aligned-pi-gap-v3.tar.xz), 1,373,788 bytes, and [manifest](evidence/aligned-pi-gap-v3.tar.manifest.json) retain every full input/result/counter, process/provider report, comparison, build identity, environment, native executable hash, command and elapsed case progress. The producer's first arithmetic attempt assumed Pi's `requests` field was a count; it is a list. Correcting that analysis shape did not change or rerun any product sample. The standalone reader below imports no producer summaries: it verifies every archive hash, all 72 accepted statuses, 846 schema-explicit comparison records, captured/reopened identity, all 180 metric rows and 60 intervals directly from raw counters.

```sh
python3 docs/pi-durable-cost-model/evidence/aligned-pi-gap-v3/recompute.py \
  --archive docs/pi-durable-cost-model/evidence/aligned-pi-gap-v3.tar.xz \
  --readable docs/pi-durable-cost-model/evidence/aligned-pi-gap-v3
```

Published `300c8d2b` passed [FULL/Merge 39207219920](https://github.com/Caismis/rustX/actions/runs/39207219920). Parent's independent prior SQL arithmetic review does not constitute independent review of this new raw archive or comparison. Incremental live-view reuse and demand-prepared deep inspection remain unfinished. The [strict cancellation control](observer-demand-cancellation-v3.md) does not reproduce the earlier slowdown but still permits one. #485 remains independent cloud exploration; #484 was still open when checked during packaging. No base retarget, rebase, reset or merge was performed. Future integration must verify actual ancestry/squash history and revalidate against then-current main; local feature development need not wait for this exploration.

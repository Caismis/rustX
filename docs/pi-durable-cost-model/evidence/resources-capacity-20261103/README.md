# Resource and native capacity evidence

[Report](../../2026-11-03-resources-capacity.md) · [All values](../../2026-11-03-resources-capacity-tables.md).

Final production source is **1039cc44deff7e13b8a8275ab12e8b2b09cb7ad2**, private schema 93. Initial two-table source **e44a6e545** / schema 92 is retained as a separate trial. Benchmark driver/provider are committed at **654bf7e6e**; the later reload-barrier correction is separately smoke-tested, and `capacity-harness-measured.py` retains the exact measured source. No production inputs changed after final checks/build.

- `ordinary-pairs`: initial schema-92 trial, 32 ordinary products, four balanced pairs per cell. Its long-history write regression remains visible.
- `refined/ordinary-pairs`: final model-only schema-93 trial, another 32 ordinary products. Each cohort independently checks 1,088 request comparisons. Do not pool trials or choose only favorable pairs.
- `columns-*`, `wal-*`, and `refined` counterparts: independent logical column and WAL diagnostics. Raw strace, endpoint dbstat, consistent database images, SHA/page/free counters, all transaction page counts and strict header/payload coverage are retained. Eight WAL products are replayed, separately from ordinary timings.
- `capacity-before-refined`, `capacity-after`, `refined/capacity-final`: one actual native App Server process each, 128 sessions, eight seed turns each, 1/4/8 active attempts, 1,235 requests per process. Only native PID resource counters are reported; driver/provider are siblings.
- `refined/history-512-{before,final}`: two sessions ×512 seed turns, 1/2 active attempts, 1,075 requests per process. Single-process growth checks, not a complexity or throughput guarantee.
- `capacity-cpu-long`, `refined/capacity-cpu-final`: diagnostic-only native 250-Hz CPU windows, 1,771 /1,758 samples. Each has 512 active turns in its profiled window. Seeds/driver/provider are excluded. Raw record totals equal collapsed/text denominators. Symbolized text is retained; inline/ICF/type-name limitations remain.
- `capacity-heap-bounded`: diagnostic-only replacement allocator, baseline native library, eight sessions/one history turn. Six 1-MiB-capped gperftools-2.15 heap dumps retain global headers and incomplete serialized buckets. Coverage/omissions are audited; pprof's subset total is never treated as whole heap. An earlier intrusive 128-session calibration was aborted and excluded.
- `capacity-unload-barrier-smoke`: final harness fix, eight sessions, 37 requests. The old heap diagnostic sampled unloading=1 at its first reload point; this is retained explicitly. The corrected loop waits loaded/loading/unloading zero, without altering old measurements.
- `prior-memory-counter-excerpts`: projections from 32 **previously committed** fceaf/Pi records. Full original member hashes match the previous archive before projection. These locate prior lifecycle peaks; Pi and DeepSeek are not rerun, and no new cross-batch ratios are calculated.
- `checks-*`, build identities, frozen fixtures, profiler package provenance, original failed setup/audit sources and logs are retained. `exclusions.json` explains unsuccessful trials. Initial archive replay failed because a broad `runtime` path exclusion dropped frozen benchmark sources; the failed manifest/script/log and correction are retained. Products were not rerun to repair packaging.

Final ordinary O3/default conformance SHA-256:
`6274155c7721d0ebb2a55324c9d6c22d8d15fac44e067320408c198e41725425`.
Final native executable:
`2add0f8a10f8c29238b24124be4f10cfde9cd9ee34d7b6a97fa63f0ba19876ea`.
Baseline conformance is saved fceaf O3. Baseline native main is an explicitly recorded O3 relink against that immutable default-feature library and matching dependencies, not a fresh library build. Root-O0/all-feature correctness artifacts never supply formal timings. Final 971-input maps agree across checks/build/current production source; 2,379 tests pass, one existing ignored; Fmt/Clippy, 48 Python and 24 Node checks pass.

## Offline replay

From the repository root, with Python standard library and enough temporary space
for approximately 3 GB of uncompressed evidence:

```sh
python3 docs/pi-durable-cost-model/evidence/resources-capacity-20261103/replay.py \
  --archive docs/pi-durable-cost-model/evidence/resources-capacity-20261103/raw.tar.xz \
  --manifest docs/pi-durable-cost-model/evidence/resources-capacity-20261103/manifest.json \
  --output /tmp/rustx-resources-replay.json
```

The output file must not exist. Replay authenticates every regular member, reparses both ordinary cohorts, all eight WAL products, nine native products with full provider histories and closed SQLite content oracles, CPU/heap denominators, prior memory projections, and regenerates tables byte for byte. It executes **no products, compiler, git or network**. Build/source/executable identity records and existing symbolization text are authenticated; absent executable bytes are not freshly verified and symbolization is not rerun.

Live runtime/workspace copies, executables/rlibs, environment files and caches are omitted. Closed consistent images remain. Provider stdout repeats request bodies already present in full raw products/reports and is omitted by documented selection; failed setup results are still retained. The raw archive contains synthetic provider/session data, not real user conversations.

Verified archive: **3,233 regular members**, **43,738,656 bytes**, SHA-256
`ce218fff8c32a9e1227b52b46abfbf86df32c16debe0f516fd4436b101b31f33`.
[Recorded successful replay](replay-result.json) authenticates all members and
reproduces both audits, native content proofs, profile denominators and tables.

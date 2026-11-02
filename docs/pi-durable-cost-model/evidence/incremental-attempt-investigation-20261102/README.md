# Post-formal regression investigation

Six separate live/64 profiler products executed after the formal and column/WAL batches: two heap and four CPU products, all controller-accepted, with the same ordinary O3 binaries. They are not additional formal repetitions, and their elapsed/CPU/write counters are not pooled with the formal results. No Pi/DeepSeek product runs.

CPU phase `before_measured_turns -> after_turns` has only 94/101/96/98 samples at requested 250 Hz. Baseline totals 192, candidate 197. Publication encoding remains a material existing cost; its overlapping cumulative counts are baseline 17/9 and candidate 20/19. SQLite step is baseline 23/16 and candidate 24/11. These sparse, variable observations do not establish which change caused the formal approximately 5% long-history regression. No claim assigns it to scheduling noise, a lost covering scan, or any particular cache.

Both heap products emitted approximately 1 MiB stack listings whose rows fail to sum to their own header totals. `heap-completeness.json` records this failure. The retained `baseline-heap-summary.json` was generated before that check and uses the earlier initialization-to-warmup cut (files 0003 -> 0004) and is only an incomplete-prefix calculation: it must not be used as full allocation attribution. Header-only measured-window deltas are 1,220,379 / 317,812,315 requested allocations/bytes for baseline and 1,226,190 / 320,294,436 for candidate (files 0004 -> 0005, bound to control-log phase names). Heap mode replaces the allocator and includes driver/profiler effects; it is not an ordinary-runtime allocation count or a precise cause of wall-time differences.

The synthetic SQLite probe checks identical results and covering/range ordering for explicit trailing sequence versus implicit rowid: each tested query takes one fewer VM instruction with the narrow index. It is a limited SQLite 3.45.1 probe, not whole-rustX attribution or proof against every query-plan regression. Production bundled-SQLite correctness tests independently verify the range/covering behavior.

The four requested optimizations are implemented and all functional checks pass, but the formal long-history regressions remain observed. This investigation did not identify another safely removable operation that would explain and eliminate them. The report therefore retains the adverse results and makes no whole-matrix speedup or Pi-parity claim.

The initial supplemental archive `969bff06f064e07dcabfee96d96b336bfe7a86f40a9fe87af71b285ff1395874` is superseded: its phase arithmetic used 0003 -> 0004, which is initialization-to-warmup rather than the measured window. Its unchanged bytes remain separately retained. The corrected verifier enforces control-log-bound 0004 -> 0005 and independently recomputes those header deltas. Formal measurements and CPU profile counts were unaffected; no products were rerun.

Verify in a fresh directory (Python 3.11+ standard library; no product/build execution):

```sh
python3 verify-supplement.py --work-dir /new/external/verification-directory
```

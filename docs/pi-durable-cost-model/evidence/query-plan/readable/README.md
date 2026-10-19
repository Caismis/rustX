# Readable query-plan comparison inputs

All **84 samples** are exposed here as per-pair numeric inputs: 36 runtime, 24 warm append and 24 cold append. The three `*-summary.json` files and `runtime-uncertainty.json` are byte-identical copies of the corresponding files in the [original verified archive](../../query-plan-results.tar.xz). `provenance.json` records their hashes and the hashes of all 84 raw samples. Original files and archives remain unchanged; no samples were rerun or filtered.

`paired-inputs.csv` presents every numeric before/after value and its paired ratio. Blank ratios mean a zero or unavailable denominator, not a favorable result. Runtime JSON additionally retains lifecycle phase inputs; append JSON retains warm/cold counter scopes and DB/WAL logical/allocated sizes. CPU is product self user+system including driver, at 10 ms resolution; writes are process-attributed, not device writes. Memory values are sampled maxima, not peaks.

Run from any directory:

```sh
python3 docs/pi-durable-cost-model/evidence/query-plan/readable/recompute.py
```

This independently divides the input values, checks **594 paired ratios**, recomputes every before/after/paired median, and reproduces all **12 reported runtime intervals**. `recomputed.json` and the CSV are regenerated from the inputs. Intervals use paired log ratios and df=2 Student t, two-sided 90% (equivalently the upper bound is one-sided 95%). Three pairs are exploratory; short-small wall and CPU intervals cross 1. The 10–12% short-turn median reductions are provisional, not established universal improvements.

To also verify original raw hashes and rederive inputs with the repository's original phase/counter summarizer, extract the archive to a directory containing `pi-parity-qpsg/` and run:

```sh
python3 docs/pi-durable-cost-model/evidence/query-plan/readable/recompute.py --raw-root /path/to/extracted
```

The recorded check used `/workspace/issue-481-evidence` and passed both levels. Large-case CPU ratio **1.1429**, cancellation wall and warm-append adverse samples are retained. Query-plan stability disables parameter-sensitive optimization; six workloads and four append histories do not establish favorable plans for arbitrary production data distributions or larger scales. No Pi parity or exact-head FULL success is inferred from these arithmetic checks.

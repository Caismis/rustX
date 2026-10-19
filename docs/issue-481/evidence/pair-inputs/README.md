# Reviewable paired inputs

These small text files expose every retained pair, grouped by stage and case.
They contain measured elapsed arrays, counter endpoints, sampled memory maxima
and derived metrics, without duplicating request/history payloads. `index.json`
records the summary and archive-manifest hashes. The exporter verified each raw
sample and status file against the original manifests before reading it.

For example, `canonical-runtime-short-small.json` retains all six pairs behind
the reported wall regression. Recompute its headline ratio with:

```python
import json, statistics
p = json.load(open('canonical-runtime-short-small.json'))['pairs']
print(statistics.median(x['after']['derived']['mean_ms'] /
                        x['before']['derived']['mean_ms'] for x in p))
```

For runtime elapsed arrays, divide microseconds by 1,000; for marginal append,
divide nanoseconds by 1,000,000. Average each run's measured array, then form the
matched pair ratio, then take the median across pairs. CPU is self user+system
tick delta × 10 ms / measured operations. Lifecycle writes are cleanup minus
initialization counter endpoints. RSS/PSS maxima are sampled, not true peaks.

The rust/Pi files use explicit `rust` and `pi` labels. Their native tool schemas
and durability contracts differ; these inputs do not assert feature parity.
The underlying full ordered request/results/oracle evidence remains in the
compact archives. Source-only has three pairs per case; canonical incremental,
combined and marginal append have six; each rust/Pi matrix has three.

# Exploratory stream replication

Exploratory replication before implementation. Separate from the final formal matrix; not premeasurement-preregistered and not pooled.

Before implementation: preceding Rust production 2184461 / binary 8b198 versus a664c5a / binary 62c2e9. Both schema 87. CPU 2,8; six alternating-order pairs per mode. No Pi/DeepSeek runs.

| Mode | Wall paired median % [min,max] | Window CPU % | Full CPU % | Full writes % |
|---|---:|---:|---:|---:|
| no-live-subscriber | 0.625 [-0.514,2.764] | 14.545 | 9.416 | 0.000 |
| live-subscriber | -0.837 [-2.446,0.026] | -11.538 | -8.167 | 0.000 |

All 24 products and 12 semantic comparisons pass the independent arithmetic/admission audit. All metric values, pair reversals and unavailable denominators remain in replication-audit-final.json. Different batches are not pooled; this replication does not identify a unique source cause or invalidate the previous adverse matrix. The first auditor attempt failed to initialize the independent math module; that failed report remains, and the accepted auditor explicitly binds its existing pinned hash.

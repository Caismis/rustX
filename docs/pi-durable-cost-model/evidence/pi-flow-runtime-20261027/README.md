# Pi-flow runtime evidence

This archive also retains **8 additional exploratory short-small CPU/control products**
under `supplemental-small-profiles/`. They are separate from the 18 standard
diagnostic products and all 276 formal products. Their strict audit, raw CPU profiles,
independent analysis and portable replay tools are retained. The external
`supplemental-portable-verification.json` confirms all eight replayed successfully
from the same fresh extraction; `portable-verification.json` covers the standard
formal/diagnostic audits and the complete archive inventory.

`raw.tar.xz` contains 120 direct revision runs, 120 fresh Pi runs and 36 native
three-engine reference runs. Each direct/Pi case and observer mode has five
predeclared pairs. Exploratory screens, logical columns, SQL preparation and
write-transaction stacks, WAL syscall evidence, and CPU/control runs are retained
separately and are never pooled with formal medians.

The baseline is the previously validated complete-costs candidate. Its original
build receipts are retained with prior strict reports under `previous-round/`;
no prior measurements are pooled into this round and no prior successful checks
substitute for the explicitly bound Pi-flow correctness queue. The current
24-product screen and 18 diagnostic products are separate from the formal 276.

All retained failed and successful correctness queues, isolated repros, lint
probes, exact commands, source snapshots and build identities are included.
`summary.json` records the archive hash and size. `MANIFEST.json` inside the
archive records every payload hash, size, provenance path and exclusion reason.
Absolute paths inside captured records are historical provenance, not replay
inputs. No compiled product, dependency cache, or Python bytecode is included.

Extract into an empty directory and run the retained read-only auditors:

```sh
mkdir raw
tar -xJf raw.tar.xz -C raw
cd raw
python3 audit-complete.py --root . --raw-only --stage all --baseline-identity baseline-build/identity.json --candidate-identity candidate-complete-build/identity.json --freeze reference-pins/deepseek-final-source-identity.json --expected-harness-revision d22ce66c5a1cda895cac382ce2c350f345933876 --expect-rust-sha256 775b6426d1ce59e1f80e746000d52527e876917f117361841ce120b183d042bb --output ../archive-complete-audit.json
python3 audit-diagnostics.py --root . --raw-only --baseline-identity baseline-build/identity.json --candidate-identity candidate-complete-build/identity.json --formal-audit complete-all-audit.json --expect-rust-sha256 775b6426d1ce59e1f80e746000d52527e876917f117361841ce120b183d042bb --output ../diagnostic-raw-audit.json
```

Both commands use only relative archived identity/freeze inputs. Their outputs
stay outside `raw`. Python 3.11+ and its standard library are sufficient; no
product, package install, build, profiler, SQLite database or native session
decompression is run. `verify-archive.py` provides safe fresh extraction,
manifest checks and comparisons of the recomputed results with the preserved
strict reports. Run its `--help` for an explicit fresh verification directory.

`baseline-build/` holds the prior complete-costs candidate build receipt and Cargo logs;
`candidate-complete-build/` holds this Pi-flow candidate's receipt and logs. The complete
candidate src/tests/Cargo input hash map is retained in its identity and final
correctness source snapshots. `frozen-harness/` includes the exact repository
benchmark scripts, fixed harness sources and provider sources (excluding caches). Root-level producer wrappers,
pinned arithmetic helpers, `audit-complete.py`, `audit-diagnostics.py`, native
DeepSeek source/dependency freeze metadata, and every raw command output remain.
The saved strict reports are `complete-all-audit.json` and `diagnostic-audit.json`.

DeepSeek compressed native sessions, resume evidence, wire/provider logs, profile
configuration and native projection-cache state are included. All cohort stdout
and stderr are preserved. CPU `.cpu` files and their symbols are included; new
symbolization requires the matching excluded executable. SQL preparations do
not count cached statement executions; WAL commit markers omit read-only
commits. Logical column bytes cover main storage only; connection-local TEMP readiness is not visible to the collector and its memory is unavailable rather than zero. They are not process writes, WAL bytes or device I/O.

Raw-only acceptance verifies retained identities and recomputed arithmetic. It
explicitly does not revalidate historical host binaries/current Git checkouts.
Missing counters remain unavailable, zeros and adverse samples remain visible.
Five pairs per cell do not establish the full seven-pair parity gate. Native
DeepSeek SDK idle does not await final JSONL flush/fsync and is an additional
reference, not an equal-durability comparison.

The baseline uses schema 79; Pi-flow uses schema 80. Correctness runs use release
with rustX package O0/all-features; measured binaries are separately cleaned and
built with ordinary O3/default features. Readiness/direct state now belongs to
a connection epoch. Reopening adopts committed J/T as baseline and does not
durably retain whether the previous process installed or published them. This
is the deliberate Pi commit-then-adopt tradeoff, not a promise that every old
corruption or recovery behavior remains unchanged.

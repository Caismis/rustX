# Complete-costs runtime evidence

`raw.tar.xz` contains 120 direct revision runs, 120 fresh Pi runs and 36 native
three-engine reference runs. Each direct/Pi case and observer mode has five
predeclared pairs. Exploratory screens, logical columns, SQL preparation and
write-transaction stacks, WAL syscall evidence, and CPU/control runs are retained
separately and are never pooled with formal medians.

The original first candidate is frozen under `pass1/`, with its own original
producers, build identity, all failed/successful checks, 24 exploratory screen
products and 11 diagnostic products. `pass1-freeze.json` records its exact file
hashes and phase scope; those checks never validate the final candidate and
those measurements never enter the final 276 products. Absolute recorded paths
remain original provenance; the matching compiled binary is excluded.

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
python3 audit-complete.py --root . --raw-only --stage all --baseline-identity baseline-build/identity.json --candidate-identity candidate-complete-build/identity.json --freeze deepseek-setup/final-source-identity.json --expected-harness-revision f5cf22a8fbd2b1f0b3abc7fc2082a586f1126e20 --expect-rust-sha256 8d033129f343660821b5fd7cef957a424e6d769c55948011f2798c6e2f3a0cb7 --output ../archive-complete-audit.json
python3 audit-diagnostics.py --root . --raw-only --baseline-identity baseline-build/identity.json --candidate-identity candidate-complete-build/identity.json --formal-audit complete-all-audit.json --expect-rust-sha256 8d033129f343660821b5fd7cef957a424e6d769c55948011f2798c6e2f3a0cb7 --output ../diagnostic-raw-audit.json
```

Both commands use only relative archived identity/freeze inputs. Their outputs
stay outside `raw`. Python 3.11+ and its standard library are sufficient; no
product, package install, build, profiler, SQLite database or native session
decompression is run. `verify-archive.py` provides safe fresh extraction,
manifest checks and comparisons of the recomputed results with the preserved
strict reports. Run its `--help` for an explicit fresh verification directory.

`baseline-build/` holds the prior joint candidate build receipt and Cargo logs;
`candidate-complete-build/` holds this candidate's receipt and logs. The complete
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
commits. Logical column bytes are not process writes, WAL bytes or device I/O.

Raw-only acceptance verifies retained identities and recomputed arithmetic. It
explicitly does not revalidate historical host binaries/current Git checkouts.
Missing counters remain unavailable, zeros and adverse samples remain visible.
Five pairs per cell do not establish the full seven-pair parity gate. Native
DeepSeek SDK idle does not await final JSONL flush/fsync and is an additional
reference, not an equal-durability comparison.

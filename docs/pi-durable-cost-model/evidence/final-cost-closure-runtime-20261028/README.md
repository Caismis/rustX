# Final cost closure runtime evidence

This archive retains 120 fresh Rust products (two revisions × six cases × two
observer modes × five interleaved pairs) and 20 separate diagnostics (four
columns, twelve CPU/control and four WAL). Failed checks, audit attempts and all
other regular evidence are retained without being counted as successful products.
No Pi or DeepSeek product executes in this round. The baseline uses schema 81 and the candidate uses schema 82.
The first candidate remains a separate repository archive. Its four pinned reports, accepted portable receipt, source identity and 120/20 scope are checked; its archive body hash is checked at packing. Portable replay checks that retained chain without requiring the separate archive body or pooling its products.

The previous 60 Pi and 12 DeepSeek products are a fixed, separately measured
reference dependency. Its exact accepted report, archive SHA, summary, original
raw-only command and prior portable-verification receipt are in `reference-pins/`.
The old raw archive is not duplicated or re-audited here. Those comparisons are
ratios of separate batch medians, never fresh paired estimates. DeepSeek SDK idle
does not await flush/fsync; native exit CPU is separate and full-process writes
remain unavailable rather than zero.

`summary.json` binds the archive SHA/size and `MANIFEST.json` inside `raw.tar.xz`
binds every retained file's SHA/size and all exclusions. All members must be
regular files; duplicate, unsafe, missing or extra members are rejected by the
standalone `verify-archive.py`. Absolute captured paths are historical provenance,
not replay inputs. Compiled executables/libraries, dependency caches, bytecode
and prior archive/extraction trees are excluded; SQLite snapshots, raw CPU data,
WAL traces and native compressed evidence remain included.

After extracting into a fresh `raw/` directory and changing into it, the exact
read-only replay commands are:

```sh
python3 direct-audit.py --root . --raw-only --output ../direct-raw-audit.json
python3 diagnostic-audit.py --root . --raw-only --formal-audit ../direct-raw-audit.json --output ../diagnostic-raw-audit.json
python3 summarize-results.py --root . --audit ../direct-raw-audit.json --output ../report-data.json --markdown ../report-tables.md
```

Use the standalone verifier for safe extraction and complete manifest validation,
then all three commands and strict-result comparisons. Run `verify-archive.py
--help` for a fresh work directory and this evidence directory. Its output
`portable-verification.json` is written only after an actual verification attempt.
All replay outputs and logs are outside extracted `raw/`; a final full inventory
and hash pass detects any raw mutation. Python 3.11+ standard library suffices.
No product, compiler, profiler, database query or historical executable/source
lookup runs during replay. A portable pass does not claim fresh host identity.

`baseline-build/` and `candidate-complete-build/` retain original Cargo/clean logs
and build identities. Candidate O3/default-feature inputs bind the separate
successful O0/all-features correctness queue, including all tests and source hashes.
`frozen-harness/` holds hash-bound repository fixture/harness sources. Retained
strict reports are `direct-audit.json` and `diagnostic-audit.json`; `report-data.json`
contains all metric values, missing ratios and adverse pairs. Replayed complete
formal datasets, diagnostic profiles/columns/WAL and numerical tables must equal
these reports, allowing only explicit timestamp, raw-mode and relocated identity
fields. Diagnostic replay binds the newly recomputed formal report.

Main-only logical column bytes exclude connection-local TEMP readiness and are
not WAL, process write bytes, or device I/O. Retained CPU symbol reports support
sample/count consistency, not a fresh independent symbolization of raw PCs.
Inclusive sample buckets overlap; sampled shares are not measured speedups.

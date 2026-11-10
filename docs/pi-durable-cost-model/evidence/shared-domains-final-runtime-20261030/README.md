# Shared domains and bounded aggregate codec runtime evidence

This archive retains 144 fresh Rust products (two revisions × six cases × two
observer modes × six interleaved pairs) and eight separate diagnostics (four
columns and four WAL). The formal comparison covers 72 pairs and 1,692 matched
requests. All fresh products use CPUs 2,8, with three pairs in each execution
order per cell. Failed checks, audit attempts and other regular evidence remain
retained without being counted as successful products. No Pi or DeepSeek product
executes in this round. The baseline uses schema 85 and the candidate uses
schema 86. There are no profile or supplementary products.

A source correction before this phase defines a second candidate. The first
candidate's complete accepted reports, every adverse pair and unavailable metric,
original build identity and archive/portable-verification receipts are copied
verbatim in `previous-phase/` and bound by `previous-phase-pins.json`. Its raw
products remain in the separately verified, hash-pinned first-phase archive.
This package verifies the copied report bindings; it does not recompute that old
raw archive or pool first- and final-candidate measurements.

The previous 60 Pi and 12 DeepSeek products are a fixed, separately measured
reference dependency. Its exact accepted report, archive SHA, summary, original
raw-only command and prior portable-verification receipt are in `reference-pins/`.
The old raw archive is not duplicated or re-audited here. Historical distributions
are presented separately; there are no new current/historical ratios. DeepSeek
SDK idle does not await flush/fsync; native exit CPU is separate and full-process
writes remain unavailable rather than zero.

`summary.json` binds the archive SHA/size and `MANIFEST.json` inside `raw.tar.xz`
binds every retained file's SHA/size and all exclusions. All members must be
regular files; duplicate, unsafe, missing or extra members are rejected by the
standalone `verify-archive.py`. Absolute captured paths are historical provenance,
not replay inputs. Compiled executables/libraries, dependency caches, bytecode
and prior archive/extraction trees are excluded; SQLite snapshots, WAL traces
and native compressed evidence remain included.

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

`baseline-build/` retains both its unchanged original build receipt and a copy
with only the executable path relocated. Both revisions retain original Cargo
and clean logs. Candidate O3/default-feature inputs bind the separate successful
O0/all-features correctness queue, including tests and source hashes.
`frozen-harness/` holds hash-bound repository fixture/harness sources. Retained
strict reports are `direct-audit.json` and `diagnostic-audit.json`; `report-data.json`
contains all metric values, missing ratios and adverse pairs. Replayed complete
formal datasets, diagnostic columns/WAL and numerical tables must equal these
reports, allowing only explicit timestamp, raw-mode and relocated identity
fields. Diagnostic replay binds the newly recomputed formal report.

Main-only logical column bytes exclude connection-local TEMP readiness and are
not WAL, process write bytes, or device I/O. CPU affinity evidence records the
controller's observed mask and topology; it does not establish historical
per-thread CPU residency or remove every source of scheduling variation.

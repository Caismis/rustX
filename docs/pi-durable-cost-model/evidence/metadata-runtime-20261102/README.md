# Shared cut metadata and indexed merge runtime evidence

This archive retains 144 fresh Rust products (two revisions × six cases × two
observer modes × six interleaved pairs) and eight separate diagnostics (four
columns and four WAL). Another 144 targeted diagnostic products isolate five
optimization knobs plus the pre-round Rust code as a replication control,
only for 64-history turns in two modes. Their 72 pairs and 6,912 matched requests
are audited separately, never pooled with the formal matrix. In total, this
archive retains 296 fresh Rust products. The older replication control uses
schema 86; all other diagnostic products use schema 87. The formal comparison covers 72 pairs and 1,692 matched
requests. All fresh products use CPUs 2,8, with three pairs in each execution
order per cell. Failed checks, audit attempts and other regular evidence remain
retained without being counted as successful products. No Pi or DeepSeek product
executes in this round. The baseline uses schema 87 and the candidate uses
schema 87. There are no CPU/heap profile products; the separate targeted
ablation products are identified above.

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
python3 ablation-audit.py --root . --raw-only --output ../ablation-raw-audit.json
python3 direct-audit-corrected.py --root . --raw-only --output ../direct-raw-audit.json
python3 diagnostic-audit-corrected.py --root . --raw-only --formal-audit ../direct-raw-audit.json --output ../diagnostic-raw-audit.json
python3 summarize-results.py --root . --audit ../direct-raw-audit.json --output ../report-data.json --markdown ../report-tables.md
```

Use the standalone verifier for safe extraction and complete manifest validation,
then all four commands and strict-result comparisons. Run `verify-archive.py
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
strict reports are `direct-audit-final.json` and `diagnostic-audit-final.json`; `report-data.json`
contains all metric values, missing ratios and adverse pairs. Replayed complete
formal datasets, diagnostic columns/WAL and numerical tables must equal these
reports, allowing only explicit timestamp, raw-mode and relocated identity
fields. Diagnostic replay binds the newly recomputed formal report.

`audit-correction.json` records a post-measurement auditor-only correction.
The original premeasurement receipt froze 17 scripts; the original formal auditor
still required the earlier 11-name inventory. All 17 original hashes remain
unchanged, and the original failed report is retained as `direct-audit.json`.
The corrected formal auditor changes only that expected set; the diagnostic
auditor changes only its formal-auditor/report binding. The packer and portable
verifier enforce those exact source transformations. Product/protocol/semantic/
statistics code and all raw measurements are unchanged. The corrected auditors
were not premeasurement-frozen and are identified separately.

Main-only logical column bytes exclude connection-local TEMP readiness and are
not WAL, process write bytes, or device I/O. CPU affinity evidence records the
controller's observed mask and topology; it does not establish historical
per-thread CPU residency or remove every source of scheduling variation.

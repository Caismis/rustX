# Source checkpoint

The separate baseline syscall profile found that 402 of 442 Source encodings
fit in 4 KiB. This checkpoint allocates a bounded in-memory SQLite navigation
index for exactly that case, selected after the existing structural encoder
finishes. It keeps the single original byte encoding, parser, node metadata,
field order, previews, ranges, pagination and copy/witness path.

At most 4,096 encoded JSON bytes can contribute nodes and key bytes. SQLite
index pages are additionally capped at 256 × 4,096 bytes (1 MiB); temporary
sorting is limited by the admitted node set. This is a per-Source bound, not a
claim that a SQLite connection's total allocator footprint is exactly 1 MiB.
The bounded connection is retained until the Source drops and reused by its
synchronous reads. Larger Sources retain the disk index, release construction
connections at seal, and use the same physically bounded cold read path.

There is no durable format change, authority cache, delayed indexing, witness
regeneration or native commit-policy change. Index SQL/node work remains;
the selected optimization removes small-source index files and read opens.
Original byte files remain intentionally unchanged.

Focused validation: all 20 `runtime::read_layout` tests passed, including new
4,096/4,097-byte admission, dense-node memory bound and pagination, raw lexical
JSON/key order/u64 regressions, plus existing large-read/copy/lifecycle tests.
The broader durable owner tests and paired performance checkpoint passed as recorded below.

## Paired cloud results

Source production checkpoint: `e369dee3`; optimized executable SHA-256
`035afcda49aa17ef3787c00871112077c04004def35d8546dfb20a76dfd8ffe3`.
The same baseline executable was rerun interleaved, three pairs for every case.
All 36 samples and 18 full normalized request comparisons passed. These are
medians of three per-run means/counters, not differences of unrelated runs.

| Case | Before/after turn ms | Before/after CPU ms per turn | Before/after sampled RSS MiB |
|---|---:|---:|---:|
| short-small | 74.24 / 68.70 | 104.38 / 98.44 | 204.12 / 205.35 |
| short-large | 77.01 / 75.00 | 108.12 / 105.62 | 288.59 / 289.39 |
| stream | 1580.49 / 1594.54 | 200 / 190 | 158.11 / 156.71 |
| large | 212.46 / 207.52 | 220 / 220 | 158.13 / 160.00 |
| tool | 117.70 / 109.52 | 140 / 140 | 157.91 / 159.58 |
| cancel | 50.31 / 49.22 | 70 / 70 | 157.11 / 157.16 |

The small memory increase is a tradeoff of retaining a bounded SQLite
connection. Streaming wall time regressed slightly; the provider's deliberate
chunk schedule dominates that case. Three repeats are descriptive evidence,
not a significance claim. CPU resolution is 10 ms. All baseline scope and
native-contract qualifications still apply. Lifecycle writes remained essentially
unchanged (short-small 128.08/128.05 MiB, short-large 330.74/330.66 MiB).

Separate syscall profiling found 442 exact byte files unchanged, 40 index files
instead of 442, 160 index opens instead of 1,207, 1,000 positional index reads
instead of 4,948, and 480 index writes instead of 2,892. Successful logical index
write bytes fell from 11,845,632 to 1,966,080. SQL/node construction remains;
this is file/connection work reduction, not a claim of fewer SQL statements.
The profile overlapped a test-harness rebuild and is used only for operation
counts, never timing or CPU comparisons. Both profiles completed with zero
unresolved syscall records. `/tmp` is tmpfs, so these are not device-write savings.

All 361 durable SQLite owner tests passed after the focused 20 layout tests.
The marginal-append fixture initially failed before measurement because its
ConversationId was not a production UUIDv7; every failed record remains in
`source-append`. The corrected benchmark is measured separately.

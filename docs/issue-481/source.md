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
The broader durable owner tests and paired performance checkpoint are in progress.

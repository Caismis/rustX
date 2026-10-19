# Bounded fact spooling and compact authenticated pages

Stage 1's fresh profiles retain substantial SQLite preparation work and unchanged
Source carrier/index counts. This stage targets fact-spool SQL and persisted page
encoding, without replacing generic Source navigation or original-byte carriers.

Small producers retain at most 256 `(key, value)` facts and 65,536 bytes of owned
String/Vec buffer capacity. Capacity accounting also bounds encoded length; Vec
record overhead is separately limited by the count (at most 12 KiB on this target).
Sorting is in-place UTF-8 byte order, matching SQLite BINARY ordering. Duplicate
keys fail at insertion. Exceeding either limit transfers prior facts to the same
SQLite temporary spool and continues on disk. Large Sources never collect their
whole fact set in memory. The caller's existing optional-observation savepoint
rolls back a failed spill/capture; successful finish clears the temporary rows.
No new connection or persistence queue is introduced.

SQLite development gate 71 replaces only authenticated index-page JSON/base64
with a compact codec. Original source chunks and serialized field facts are
stored as raw value bytes; their JSON lexical content is unchanged. Root/template
metadata keeps its existing format. Every page hash still authenticates the exact
stored bytes, and original request capture witnesses are neither regenerated nor
replaced when reading/reusing content. Gate 70 is rejected without migration.
Archive capture still opens through the durable store gate and exports the same
decoded records, not these internal page blobs. Its existing manifest records
`durable_schema: 71`; the archive/protocol record formats do not change.

## Binary layout

All lengths/counts use little-endian unsigned integers. The header is `RXSP`,
one byte version `1`, one byte kind (`0` leaf, `1` branch), and a u16 count.

- Leaf entry: u16 key-byte length, UTF-8 key bytes, u32 value-byte length, raw value.
- Branch entry: u16 last-key-byte length, UTF-8 last-key bytes, u8 child depth,
  and the raw 32-byte SHA-256 child hash.

A page has 1–64 entries and at most 65,536 encoded bytes. Keys are at most 4,160
bytes; values are at most 40,960 bytes. Both leaf and branch builders split before
exceeding either count or byte limits. The largest admitted single leaf entry
occupies 45,134 bytes including its page header. Readers reject unknown versions
or kinds, bad counts/lengths, invalid UTF-8, truncation and trailing bytes before
unbounded allocation. The existing authenticated navigation checks still require
strict key order, consistent child depth and the referenced final key.

There is no production JSON-page decoder, legacy fallback, dual write, migration
or compression algorithm. Test-only inspection converts an already decoded typed
page to a JSON value to mutate adversarial fixtures; it does not read legacy
persisted JSON pages.

## Bounds and ownership

The existing 64 KiB physical page handoff, depth below 16, 64 emitted range pieces,
512 preview pieces and 32 preview-node limits remain. Cold reads navigate pages;
they never parse/reindex a complete large Source. A decoder holds one bounded raw
page and bounded decoded entries; traversal drops unrelated children before the
next level. Construction holds one bounded leaf and at most one byte/count-bounded
reference group per level. Larger fanout can make an individual cold page read
larger even while reducing tree depth and total page work; measurements must not
claim every lookup transfers fewer bytes.

Native selected-input nodes remain the selection authority. Paths, child ordinal
references, ranges, historical cuts, frozen request identity, lexical numbers/u64,
key order, original capture witnesses, same-buffer verification, fail-open optional
observations, lifecycle pins and independent Fork/archive closure retain their
existing owners and checks. Publication cadence and canonical P<U<C transactions
are unchanged.

## Validation

All-target/all-feature Clippy passed with warnings denied; formatting passed.
The focused run passed 21 tests. After correcting two stale schema-version
assertions, the complete durable SQLite run passed 393 tests in 262.96 seconds.
The first run's 391 passes and two version-assertion failures remain retained.
No timeout, gate, interleaving or semantic assertion was relaxed.

New cases cover exact independently specified binary bytes, every truncation,
unknown versions/kinds, invalid UTF-8, excess lengths/counts, trailing bytes,
legacy JSON rejection, byte/count splits, ordering/depth authentication, and
exact maximum entry sizes. Spool cases compare against independent SQLite BINARY
ordering, exercise count/capacity spill boundaries, retain every fact, reject
duplicates and disk failures, and verify savepoint rollback of spilled rows.
Existing differential metadata tests now use an independent original SQL fact
producer; original lexical fields and child references remain checked.

Broader durable tests retain historical cuts, original witness tamper rejection,
cold physical bounds, optional-observation failure behavior, canonical atomicity,
recovery and independent Fork closure. Final exact-head FULL/Merge verification
and independent review remain required; performance results are reported separately.

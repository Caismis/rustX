# Stage 2: bounded resident Source bytes

A Source whose completed original encoding is at most 4,096 bytes now keeps
those bytes in an immutable shared allocation beside its existing bounded
navigation index. It creates no scratch directory, byte file or SQLite index.
Larger encodings spill once into the Source-owned directory; the prefix and
remaining bytes are written without loss or a second serialization. Serializer
write size cannot increase the resident threshold. Existing bounded file reads
and disk navigation remain the large-Source implementation.

The byte carrier is private to the Source owner. Sealing closes the writer or
freezes resident bytes; readers borrow the Source and have independent positions.
Cloned layout pins retain the same owner. No global cache or durable authority is
introduced, and no persistence format, witness, history cut or publication
contract changes in this stage. Failure to create/write spill storage aborts
construction and is observable; it never substitutes truncated resident bytes.

Tests cover the threshold on both sides with split writes, exact raw lexical JSON,
u64 maximum, key order/navigation, independent readers and owner pins, a failed
spill collision, sealed writes and out-of-range reads. Existing large-file
physical bounds, truncation and destination-write failures remain authoritative.
Validation: all 26 Source tests pass (1.08 s), all 398 durable SQLite tests pass
(326.65 s), and all-target/all-feature Clippy with warnings denied passes. A
naming-only Clippy finding was fixed; the initial and successful check logs are
retained. Release measurements follow in a separate evidence commit.

# Profile-selected byte residency bound

The initial 4 KiB carrier removed 402 of 442 temporary byte files but showed no
end-to-end performance benefit. Its separate syscall profile found that all 40
remaining byte files were only 12,038–12,895 bytes. They still caused 27,356
`pread64` operations, largely because indexed navigation/canonical emission reads
many small spans. The initial measurements remain in [resident-source.md](resident-source.md).

Raise only the byte carrier's fixed bound to 16,384 bytes. The navigation-index
admission bound remains 4,096 bytes: a 12 KiB Source now uses resident bytes with
disk navigation, not an unbounded resident node tree. Larger byte encodings still
spill once without reserialization. Readers, historical ownership, original bytes
and persistence formats are unchanged. Each resident byte allocation is bounded;
this does not claim lower total process memory.

The threshold tests now exercise both independent boundaries (4 KiB and 16 KiB),
including split writes, resident bytes with a disk index, zero byte-file work
under the byte bound, exact spill bytes and failure at the new threshold. End-to-end
and syscall comparisons will use the preserved 4 KiB executable as their before
identity; no timing gain is assumed from the operation-count diagnosis.

Validation with the subsequent scoped-index change present: 400 durable tests
passed (252.72 s), 26 Source tests passed (1.04 s), and all-target/all-feature
Clippy passed with warnings denied. The byte-bound change is committed separately
for a clean, independently built measurement identity.

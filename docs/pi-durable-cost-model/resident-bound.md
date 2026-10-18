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

## Isolated comparison

The root-clean release build verified both fresh root artifacts at
`c34de111f5da991e5b92db40a9391d60842b8c49` (production tree
`e6ece79fc60123814ec239d201aaa2530916e878`). After executable SHA-256:
`725d8d5dde5aba95a38a2e2fdf304cad4f62fb72247552bd5e9297e0af992668`.
Before is the preserved 4 KiB build at `e1823321`, executable SHA-256
`246adf48a1855ad7a0021ec9644d324e8081c1dc5afc8b514e6361d1cc3ecd70`.
All five fixture hashes match. All 36 samples / 846 requests / 18 paired full
request comparisons pass. No compiler, test suite or profiler overlapped formal
measurements; every slow sample remains included.

Median **paired** after/before ratios (not ratios of independent medians):

| Case | Turn wall | Turn CPU | Lifecycle writes | Sampled anonymous PSS |
|---|---:|---:|---:|---:|
| short-small | 0.96411 | 0.95527 | 0.99871 | 0.99543 |
| short-large | 0.97765 | 0.98142 | 1.00053 | 0.99654 |
| stream | 1.00106 | 0.95455 | 1.00000 | 1.00081 |
| large | 0.92862 | 0.95455 | 0.99970 | 0.99756 |
| tool | 0.98563 | 0.91667 | 0.99897 | 0.99120 |
| cancel | 0.87622 | 0.83333 | 0.99906 | 0.99118 |

Short-small paired turn wall/CPU improve by about 3.6%/4.5% in this run. This is
not a universal gain: stream lifecycle CPU ratio is 1.02857 and wall ratio is
1.00515; tool lifecycle wall is 1.03601. Short-large lifecycle writes increase
slightly (1.00053). All phase counters and individual samples are retained.
CPU is whole-product-plus-driver process CPU at 10 ms resolution; single-turn
ratios are coarse. Memory values are sampled, not peaks, and no general memory
saving is established. Allocation counts and physical-device writes remain
unavailable. Provider startup/teardown remains separate; runtime evidence output
occurs after cleanup counters. No unrelated medians are subtracted.

The separate syscall profile passes all 40 request oracles and contains zero
unresolved records. Relative to the preserved 4 KiB diagnostic, byte files fall
40 to 0, byte-file opens 240 to 0, writes 40 to 0, `pread64` calls 27,356 to 0 and
logical byte-file writes 482,467 bytes to 0. Navigation remains 40 files, 160 opens,
1,000 `pread64` and 480 `pwrite64` calls (1,966,080 logical written bytes).
These are measured operations, not CPU percentages or device writes; `/tmp` is
tmpfs, and the trace filter omits ordinary `read` calls. No timing from the traced
run is used in the headline table. The existing 4 KiB profile is preserved in the
prior stage's archive.

```sh
python3 benchmarks/runtime/compare-revisions.py --before /workspace/issue-481-evidence/pi-resident-build/conformance --after /workspace/issue-481-evidence/pi-resident16-build/conformance --output /workspace/issue-481-evidence/pi-resident16-matrix --pairs 3
python3 benchmarks/runtime/summarize-comparisons.py /workspace/issue-481-evidence/pi-resident16-matrix
python3 benchmarks/runtime/profiling/syscalls.py --binary /workspace/issue-481-evidence/pi-resident16-build/conformance --output /workspace/issue-481-evidence/pi-resident16-source
python3 benchmarks/runtime/profile-source.py /workspace/issue-481-evidence/pi-resident16-source/calls.strace
```

[Phase/pair summary](evidence/resident-bound-summary.json),
[Source profile counters](evidence/resident-bound-source.json),
[verified raw archive](evidence/resident-bound.tar.xz) and
[manifest](evidence/resident-bound.tar.manifest.json) preserve the evidence.
Originals remain in `/workspace/issue-481-evidence`; the archived build identity
and commands reproduce the preserved executable without committing binaries.

# Stage 2: bounded resident Source bytes

The initial 4 KiB implementation stores completed encodings of at most 4,096
bytes in an immutable shared allocation beside the bounded navigation index. It creates no scratch directory, byte file or SQLite index.
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

## Measurement: fewer file operations, no demonstrated end-to-end speedup

Three interleaved pairs across all six scenarios compare the frozen template
stage `989d74d12d211a38eee5ad1f07ec18c0251a0de5` to resident bytes at
`e1823321fa0dce1e3969f571e4eb61bb85aa2924`. Both clean root-crate builds are
identity verified, have distinct executable hashes, and share all five measured
fixture hashes. The after executable is SHA-256
`246adf48a1855ad7a0021ec9644d324e8081c1dc5afc8b514e6361d1cc3ecd70`.
All **36 samples / 846 requests / 18 paired comparisons** pass their full ordered
oracles. No build, test or profile overlapped formal measurement.

| Median paired after/before | Wall | Self CPU | Lifecycle writes | Sampled anonymous PSS |
|---|---:|---:|---:|---:|
| short-small | 1.01966 | 1.03378 | 1.00082 | 1.01064 |
| short-large | 1.00315 | 1.00633 | 1.00029 | 0.99784 |
| stream | 1.00078 | 1.05263 | 1.00000 | 0.99520 |
| large | 1.02770 | 0.95652 | 0.99970 | 0.99632 |
| tool | 0.98022 | 1.00000 | 0.99949 | 1.00354 |
| cancel | 0.99747 | 1.00000 | 0.99906 | 0.98828 |

The short-small CPU/wall and large-output wall regressions remain explicit.
There is no general end-to-end speedup or memory improvement in this sample.
Single-turn CPU ratios are especially coarse at 10 ms resolution. These are
runtime-plus-driver counters, not pure Source costs; sampled memory maxima are
not peaks. Full lifecycle includes checkpoint, close, reopen and cleanup, with
verification/evidence output after the product counters. Provider startup/control/
teardown are separate recorded costs. Allocation counts and physical-device writes
remain unavailable. Do not multiply this stage's ratios by earlier stage ratios.

Two separate sibling-tracer diagnostics passed all 40 request oracles each. They
are excluded from the formal timing results. The unchanged analyzer reports:

| Source byte carrier work | Before | After |
|---|---:|---:|
| Distinct written byte files | 442 | 40 |
| Byte files at most 4,096 bytes | 402 | 0 |
| Byte-file `openat` calls | 1,649 | 240 |
| Byte-file `write` calls | 442 | 40 |
| Byte-file `pread64` calls | 40,753 | 27,356 |
| Successful byte-file write bytes | 692,618 | 482,467 |
| Distinct navigation index files | 40 | 40 |
| Navigation index `openat` calls | 160 | 160 |

Both traces have zero unresolved syscall records. The `pread64` row is not a
count of all reads: parser `read` syscalls are outside this trace filter. Index
opens are an approximate connection-work proxy, not SQL statement counts.
`/tmp` is tmpfs in this cloud, so fewer temporary files and logical writes do not
establish device-write savings; process-attributed lifecycle writes are effectively
unchanged. File-backed page cache and resident anonymous bytes also have different
accounting, so process PSS alone cannot establish total-system memory savings.

Reproduce from the frozen resident checkout after the root-clean build:

```sh
python3 benchmarks/runtime/compare-revisions.py --before /workspace/issue-481-evidence/pi-template-build/conformance --after /workspace/issue-481-evidence/pi-resident-build/conformance --output /workspace/issue-481-evidence/pi-resident-matrix --pairs 3
python3 benchmarks/runtime/summarize-comparisons.py /workspace/issue-481-evidence/pi-resident-matrix
python3 benchmarks/runtime/profiling/syscalls.py --binary /workspace/issue-481-evidence/pi-resident-build/conformance --output /workspace/issue-481-evidence/pi-resident-source-after
python3 benchmarks/runtime/profile-source.py /workspace/issue-481-evidence/pi-resident-source-after/calls.strace
```

Repeat the separate syscall command with the before binary for its diagnostic.
[Formal pair/phase summary](evidence/resident-summary.json),
[before counters](evidence/resident-source-before.json),
[after counters](evidence/resident-source-after.json),
[verified raw archive](evidence/resident-source.tar.xz) and
[manifest](evidence/resident-source.tar.manifest.json) retain reproducible inputs.
The archive preserves the initial Clippy finding and an interleaved retry log;
`pi-resident-clippy-final.log` is the clean successful final invocation. Originals
remain under `/workspace/issue-481-evidence`; no failed or slow sample is removed.

The subsequent [16 KiB byte-bound refinement](resident-bound.md) preserves these
4 KiB results as historical stage evidence and keeps navigation at its 4 KiB bound.

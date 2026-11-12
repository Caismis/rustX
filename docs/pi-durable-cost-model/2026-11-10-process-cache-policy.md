# Process-wide SQLite soft-budget experiment: reject

## Decision and identities

Reject the process-wide soft-budget mechanism. It can reduce SQLite's reported
allocation without reducing GNU anonymous PSS, and the 256-session workload has
substantial additional reads and adverse CPU. Do not select a favorable quota or
introduce an adaptive quota system. The implementation and its new test were
removed; the existing connection-local 30-second page and 120-second statement
reclamation remain.

All measured products use the preserved ordinary GNU-allocated CLI built from
`1920e5141523a3e921d7c32f4e61889437e745e2`, Rust tree
`ed64a1b1f99937f6bc45f1c923d98247cc366fbf`, SHA-256
`5c27b2671920f687ed520243e285b38fb3f7a15807ddc006905d59ced8f0b699`.
The fixture commits `df578f376` and `943049761` do not change that production
tree. Pi Durable `1cedd327` and pi-rust `4056176b` were source references only;
neither they nor DeepSeek were rebuilt or rerun.

These are diagnostic products, not ordinary comparative timing runs. A small
preload resolves the three native SQLite API symbols from this exact PIE ELF,
validates the offsets against its executable segments, sets the soft limit at
startup, and samples SQLite memory/soft/hard counters at acknowledged phases.
It leaves GNU allocation, page size, WAL/NORMAL, checkpoint policy and semantic
commits unchanged. The effective hard limit is zero in every sample. The
[identity records](evidence/process-cache-20261110/probe-identity.json) and
[320 MiB extension identity](evidence/process-cache-20261110/probe-320-identity.json)
bind the helpers, fixture and executable hashes.

SQLite's bundled build enables `SQLITE_ENABLE_MEMORY_MANAGEMENT`. Its
[soft limit](https://www.sqlite.org/c3ref/hard_heap_limit64.html) requests cache
reclamation rather than imposing a hard allocation failure. This is useful for
testing the working-set hypothesis, but does not bound required schema,
statement, temporary-state or other allocation.

## Diagnostic result

One unbalanced product per workload/quota; do not infer significance or an exact
isolated penalty. The 128-session products establish 32 turns per session, with
8/16/32 depth endpoints. The 256-session products establish eight turns. Each
then executes 256 active turns with rotating four concurrency. Timing and
process counters below cover that active phase, not setup or final close.

| Workload | Soft quota MiB | Seeded GNU anonymous PSS MiB | Native SQLite allocation MiB | Server CPU ms/active turn | Server rchar KiB/turn | Server read syscalls/turn |
|---|---:|---:|---:|---:|---:|---:|
| 128 x 32 | 0 | 1203.90 | 416.42 | 16.37 | 745.67 | 315.50 |
| 128 x 32 | 192 | 1244.21 | 191.88 | 16.37 | 1048.88 | 467.48 |
| 128 x 32 | 256 | 1028.49 | 255.63 | 16.80 | 432.94 | 159.15 |
| 128 x 32 | 320 | 1103.92 | 319.76 | 15.31 | 427.45 | 156.18 |
| 256 x 8 | 0 | 1370.29 | 511.64 | 18.59 | 284.99 | 19.13 |
| 256 x 8 | 192 | 1222.87 | 301.01 | 22.27 | 3738.39 | 1752.20 |
| 256 x 8 | 256 | 1222.34 | 301.01 | 22.34 | 3742.48 | 1754.23 |
| 256 x 8 | 320 | 1500.50 | 319.88 | 21.09 | 638.95 | 196.34 |

`rchar` and read syscall counts include the server's files and network traffic;
they are not authenticated SQLite page-read counts or device read bytes.
Diagnostic process writes remain about 978 KiB/active turn for 128 x 32 and
647-661 KiB for 256 x 8. No write saving is established. All values and the
failed product remain in the [complete summary](evidence/process-cache-20261110/probe-summary.json).

At 256 sessions, both low quotas still leave approximately 301 MiB of native
SQLite allocation while read activity increases sharply. A 320 MiB quota also
has adverse PSS and CPU. At 128 sessions, reducing the native counter from
416 to 192 MiB instead increases observed GNU PSS. The cause of nonmonotonic
PSS is not established: neither allocator fragmentation nor publication count
has been isolated. Do not subtract native SQLite allocation from GNU PSS and
label the remainder as allocator waste.

In the unlimited 128-session product, SQLite allocation at 8/16/32 turns is
255.82/313.96/416.42 MiB. Same-process GNU PSS is
690.71/875.21/1203.90 MiB. These diagnostic endpoints do not replace the earlier
four-pair ordinary 128 x 8 result. They confirm that active working sets and
retained presentation must be reduced structurally; forcing page eviction is
not an adequate capacity solution.

## Correctness and retained evidence

Eight accepted diagnostic products pass the current-protocol provider,
cancellation, browsing, slow-subscriber, native close/reopen and reload oracles.
An independent audit verifies all 1,536 consistent native backup hashes,
integrity/FK checks, schema 105, exact canonical content/order/counts, and the
absence of the cancelled provider tail. These products contain 26,648 accepted
finite provider requests. See the [backup audit](evidence/process-cache-20261110/native-backup-audit.json).

The first 256-session attempt failed before history establishment at the
inherited 1,024-descriptor process limit. It remains archived. Every subsequent
256-session quota, including its unlimited control, uses the same child-only
4,096 descriptor limit; the host limit is unchanged. No successful product was
repeated to obtain a favorable number. Sampled host monitors observed no
compiler during accepted active windows; this is not an exclusivity guarantee.

An unretained standalone-App-Server implementation at 192 MiB passed
all-target/all-feature Clippy and a child-process pressure regression, followed
by 1,652 focused library tests with three ignored. Its pressure test preserves
TEMP facts, manifest/response proof owners, old-cut bytes, rollback and current
corruption detection, and permits a write beyond the advisory quota. Those
checks apply to the archived preliminary source, not to a shipped policy.
Initial Clippy failures and exact source snapshots remain in the archive.

Only fixture improvements remain: intermediate loaded-history phase recording
and per-phase notification counts. No new production soft limit, hard limit,
cache TTL, allocator policy or runtime tuning interface is retained. Further
work should target retained cut/metadata ownership and actual page maintenance,
not another quota value. Keep PR #485 Draft; this report is not a merge gate.

The [archive](https://drive.google.com/file/d/1Pm-q9oRDza2AWTzQ6wQNJ4dTx7y_ka1l/view?usp=drivesdk "Original file: docs/pi-durable-cost-model/evidence/process-cache-20261110/evidence.tar.xz") and
[manifest](evidence/process-cache-20261110/manifest.json) preserve every accepted
and failed product, diagnostic helper, native backup, preliminary source and
check log, excluding product binaries, dependency trees, credentials and live
runtime/workspace roots. Helpers retain historical absolute paths; no portable
replay claim is made.

Detailed evidence links open the containing Drive bundle. See [download and checksum instructions](../performance-evidence/README.md) for original member paths and byte-level verification.

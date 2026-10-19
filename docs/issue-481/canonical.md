# Canonical append checkpoint

The marginal benchmark exercises only public `append_canonical`, with fixed
messages and fresh processes/stores. It excludes provider and request context
construction, measures 32 appends at histories 0, 64, 512 and 2,048, and verifies
the entire ordered history after close/reopen. Counter endpoints precede evidence
output; DB/WAL/SHM logical and allocated sizes are recorded before and after close.

The pre-canonical production checkpoint is the Source implementation, schema 69.
Corrected fixture executable SHA-256:
`48948866836e0e944772ebbf3d677a49eaf9556f143752bbdcfa1703066eb649`.
Six fresh repetitions at each size passed (the paired driver used the same
executable in both slots; these are repeatability samples, not a before/after
comparison). Median per-run mean append times were 0.865, 0.898, 1.250 and
2.459 ms respectively. All original failed fixture runs are preserved separately.

## Consumer audit and implementation

- `load_head` derives revision from the Ledger maximum, validates Surface
  frontier metadata and the selected activated projection, checks checkpoint
  base/Original dependencies, and materializes ordered unique active identities.
  Readers that need those identities retain that exact materialization path.
- The two `read_surface_head` consumers use only frontier metadata for bounded
  reading. Neither reads `active_json` as authority. They now select only the
  retained revision/generation columns.
- The three Surface head writers are canonical append, lineage bootstrap and
  a test-only bulk history fixture. Bootstrap still replays and validates the
  complete Surface operation history; the fixture still validates its final
  native head. Only the unused JSON serialization/write is removed.
- Ordinary append still validates the same bounded suffix, including UTF-8,
  SQL type, byte width and the exact pre-append working-set boundary. It borrows
  identity bytes instead of allocating a vector, `MessageId`s and `BTreeSet`.
  Ledger identity uniqueness is already a durable constraint. Checkpoint appends
  retain full materialization so Original dependency, deduplication and suffix
  semantics do not acquire another implementation.
- `append_message_ledger` remains the duplicate authority for Ledger and Pending,
  including the explicit adopted pending identity. Canonical acceptance, Surface
  operations, native events and staging remain in their existing transaction.
  No persistence queue, publication timing or durability-policy change is made.

Schema 70 removes `active_json`; current readers reject both older versions and
a forged current version retaining that obsolete column. No migration, dual
write or fallback exists. Current architecture/reading docs and schema tests move
with the gate. Retired-column corruption setup is removed from five read tests;
their native-body/snapshot corruption and read assertions remain intact.

This removes history-wide rewrite/allocation work, not every history-dependent
operation. The ordinary integrity scan remains bounded O(n); checkpoints retain
the full validation path. There is no whole-request O(1) claim or new cache
of active-history authority.

A separate instrumented pre-canonical 2,048-history lifecycle completed with
648,483 `pread64`, 318,475 `pwrite64` and 352 `fsync` calls. It includes seeding,
32 appends, close/reopen verification and cleanup; these are not marginal-append
counts. Its 26.75-second instrumented wall time is not a formal timing sample.
The syscall-summary percentages describe instrumented syscall time, not CPU
percentages. The profile overlapped compilation and is used only for counts.

Implementation commit: `eb7a9be6c960e71e8626134feff4dae24cd1d3bb`.
Combined optimized executable SHA-256:
`82a273f78f8e845e26cbca304a48c946f2afce185b377ecbb703e13c74e5881e`.
The fixed build uses the same Rust/compiler flags and default features as the
baseline. The corrected marginal fixture is identical on both sides.

All 20 focused append tests and 367 durable SQLite tests passed. New regressions
cover sparse frontiers, invalid SQL types/UTF-8/oversize identities, atomic
rejection, the exact 8,192 pre-append boundary, inconsistent revision, retired
layout rejection, and checkpoint Original dependency loss. Two initial corruption
fixtures failed to install their corruption because foreign keys correctly
rejected it. They now explicitly disable foreign keys only while installing
external-corruption fixtures; all product rejection assertions remain unchanged.
The initial failed test log is retained.

The separate after profile had 621,622 `pread64`, 262,649 `pwrite64` and 283
`fsync` calls over the same seeded lifecycle. Fewer WAL pages change automatic
checkpoint work; application transaction and publication boundaries are unchanged.
This profile overlapped other validation, so its 68.69-second instrumented wall
is not comparable performance evidence. No syscall percentage is reported as CPU.

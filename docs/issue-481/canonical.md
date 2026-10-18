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

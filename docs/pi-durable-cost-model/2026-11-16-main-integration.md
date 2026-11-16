# Main integration and PR finalization — 2026-11-16

This is a correctness/integration record, not a new runtime performance comparison.

## Inputs and history

- Cleaned PR parent: `bef6c1963fc8a6301c1c3a1bf789bc681e72df7f`.
- Integrated main: `d82d7f9b8d9dd4f5064dba8d377103c8c6049d28` (Rust 1.100.0).
- The approved history rewrite preserved the complete PR tree. Its frozen-public-tip pack measurements, archive checksums, limitations, and commit mapping are in [history cleanup](../performance-evidence/history-cleanup.md). No old PR ancestry was merged back.

## Combined contracts and repairs

The combined gates are App Server 80, Runtime Client 87, SQLite 114, session catalog 20, subagent IPC 39, and runtime manifest 5. Old peers and stores are rejected; no compatibility reader or migration is introduced.

- Preserve main's permission review, Workspace Python, Workflow run snapshots, and current client surfaces alongside the PR's shared owners, bounded observation work, and coalesced publication.
- Persist permission mode and task identity in frozen native requests. Shared configuration does not acquire request-specific permission authority.
- Advance the recovered latest primary request and unresolved adoption task with the existing transaction-owned frontier. Authenticate the primary request's exact identity, source Journal row, and Agent owner. Authenticate the adoption task against its exact adoption fact and canonical provenance. Settled history is not rescanned to recover these facts.
- Compact private frontier keys preserve the existing single-page idle root constraint. An initial integration candidate overflowed that root; the final representation fixes the layout rather than weakening the assertion or removing identity validation.
- Match Workflow forced-index queries to the generated-column partial indexes. Keep exact `origin.node_id` association distinct from a node-only fallback. Query-plan regressions require indexed, bounded reads.
- Retain generated JSON Schema meta validators and the no-file/no-network resolver boundary with the shared schema dependency version. This avoids restoring the redundant meta-schema trees removed earlier in the PR.
- Regenerate the combined protocol and native numeric codec assets. Fixtures must be emitted using the current codec plans, not hand-edited to hide numeric-domain errors.
- Adapt ownership-aware test fixtures and Rust 1.100 lints without weakening outcome assertions.

## Verification at the integration commit

All native targets compile with `--all-targets --all-features --locked`. The repository preparation tool staged four package binaries beside 22 emitted test harnesses. Focused recovery, frontier, permission, native-request, schema, and Workflow tests passed: **122 passed, zero failures, zero ignored**.

Earlier integration checks passed protocol/TUI/Web typechecks, TUI tests (1,367 passed), and Web provenance/i18n checks. The initial Web run had 11 missing-binary failures; after building the native binaries, all 24 tests in those three affected files passed. Benchmark comparator/oracle unit tests passed (36); this did not run a performance benchmark.

Final strict lint/doc/protocol drift checks and exact-head FULL CI are tracked on PR #485. This document does not treat historical or parent-commit CI as final-head validation. The PR must not be merged with failed or incomplete required checks.

No Pi, pi-rust, or DeepSeek performance product was rerun for this integration.

## Integrated fixed-work counter baseline

The old exact counter test described the pre-main contract. Two independent
current runs produced the same new text/tool tuples. Exact assertions are
retained with the integrated baseline:

| Per completed warmed turn | Text before | Text integrated | Tool before | Tool integrated |
| --- | ---: | ---: | ---: | ---: |
| SQL calls | 621 | 622 | 1,392 | 1,393 |
| Fresh prepares | 18 | 18 | 38 | 38 |
| SQLite VM steps | 20,413 | 20,491 | 45,266 | 45,410 |
| Commits | 7 | 7 | 13 | 13 |
| WAL frames (512-byte pages) | 258 | 260 | 553 | 555 |
| Event index entries | 58 | 58 | 112 | 112 |

The additional keyed canonical read establishes the adopted message's task
provenance. The combined schema also evaluates the new Workflow agent-node
index predicate. These are structural counters, not elapsed-time or device-write
measurements. The table records the net integration cost; it does not attribute
every VM step or WAL frame to one sub-change. The independent idle-root test
still requires one page, and query-plan tests retain bounded indexed reads.

## Finalization regressions

The broad local library run initially passed 6,230 tests with five failures and
28 intentional ignores. Investigation distinguished the failures:

- Permission recovery loaded the complete frozen request and projected history
  just to recover permission metadata and compare activation IDs. It now uses
  authenticated request metadata and the current projection checkpoint. The
  existing cold-attach history-growth bounds remain unchanged.
- The exact warmed-turn counter expectations are updated above; no thresholds
  were widened.
- Merge resolution accidentally restored the old physical sparse-file addition
  assertion. Restore the PR's existing `skip-worktree` coverage regression, as
  documented by the private-index design; production behavior is unchanged.
- The Workspace Python generation-race fixture now replaces the pin inode at
  its synchronization gate. It no longer relies on same-length writes receiving
  distinct filesystem mtimes; production generation handling is unchanged.
- The multi-interpreter boundary fixture needed a second installed Python minor.
  After installing Python 3.13 alongside 3.12, the unchanged fixture passed.

Strict rustdoc links were also repaired. Follow-up test and exact-head CI results
are recorded in the PR; the initial failing run is not final acceptance.

The focused permission/native-request/bootstrap run passed **52 tests, zero
failures, zero ignores**. Cold attachment at 64, 512 and 2,048 text turns now
uses exactly 85 returned rows and 1,672 SQLite VM steps (55 statements) at each
size. Tool histories at 64 and 512 turns use exactly 222 rows and 4,959 steps
(66 statements). These are deterministic structural measurements, not new
end-to-end latency measurements. The unchanged growth bounds pass.

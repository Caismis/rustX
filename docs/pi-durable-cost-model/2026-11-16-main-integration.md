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

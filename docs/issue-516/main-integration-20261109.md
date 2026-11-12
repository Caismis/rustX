# PR 517 upstream integration and final verification

## Integration

Upstream `main` at `8bf2f5cc1` was merged into PR head `1e45c5e2f` together with
the Scheduler documentation correction. Nineteen text
conflicts were resolved across the dual-sidebar shell and navigation, Workbench
and Navigator geometry visibility and bounded residency, composer approval-read
feedback, source inventory, and tests.

New app-server calls and imports were adapted to v75 `thread/start` and
`thread/resume`. The adaptation preserves native authority and receipt
semantics. The startup Session hint remains available while Home reauthorization
is held: the real-App regression verifies saved selection is retained and cold
explicit selections remain ephemeral. Related test repairs preserve Workspace
defaults and the task New dialog, update fixtures to the current v3 preference
shape, and keep Settings on the persistent route.

The 245k-triangle source grid may be admitted unchanged or as an exact planar
retriangulation within the existing optional preparation budget. The outline
assertion follows the actual admitted count and conservative admitted position
plus Uint32-index byte bound, capped at 100k triangles / 16 MiB. Resource and
idle-complete shape checks remain, including the within-cap default-on case.
No resource limit, deadline, or timeout was changed.

## Verification

Passed: `cargo build --bins --example document_artifact_fixture --locked`,
`cargo fmt`, Web production build, TypeScript, i18n, provenance validation (347
records / 355 notices / 2048 MiB WASM maximum), and
`pnpm exec vitest run test/shell.test.tsx` (5 tests).

Final Web/Host suite passed: `pnpm exec vitest run --maxWorkers=4` (local socket
permissions), 322 test files passed and 1 skipped; 4,678 tests passed and 4
skipped (4,682 total), in 160.05 seconds.

Installed Chrome 157.0.8103.38 / SwiftShader verification passed with
Playwright 1.63: the final run passed 2/2 cases, bringing the aggregate to 29
distinct functional cases across geometry, GLB validation, delivery and
Workbench, actual page reload, eight native task creation locale/theme/width
variants, exact IANA timing, and normalized recovery. The original 120-second
timeout and zero retries were retained; no screenshot baseline assertions or
updates were used. Aggregate evidence: [main-integration-browser.json](main-integration-browser.json).

All listed native build, formatting, Web/Host suite, production build,
TypeScript, i18n, provenance, shell test, and installed-Chrome checks passed
locally. Pinned-browser CI remains pending. The strict 200 ms first-model and
under-50 ms interaction acceptance remains open; see the linked performance
evidence in [comment status](comment-status.md). PR #517 remains draft; this
report does not certify readiness, merge, or auto-merge.

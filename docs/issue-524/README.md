# Model reasoning order validation

Issue: [#524](https://github.com/Caismis/rustX/issues/524).

Branch: `codex/issue-524-reasoning-profile-order`. The branch started from current
`origin/main`, `7e9010035fcb457ac415aa23fb946aba51bd1588`, matching the issue's
investigation baseline. The implementation commit is
`36bca97e94756bda6d7397ac0733bbc37412f45e`; the PR records its final head and CI.

## Contract

One optional Model-owned `reasoning_profile_order: Option<Vec<ModelProfileId>>`
declares a complete permutation of that Model's Profiles, lowest first. Omission
remains unconfigured and serializes as omission. Empty, incomplete, duplicate,
unknown, foreign-Model and no-Profile declarations fail shared native catalog
validation; invalid identities use the existing identity validator. Detailed
diagnostics identify the offending or missing Profiles, while safe classification
retains only the fixed field and reason.

Authoring/TOML uses `reasoning_profile_order`; full catalog contracts use
`reasoningProfileOrder`. App Server v72 and Runtime Client v83 carry the optional
metadata. The Rust-generated `v72.schema.json`, `v72.ts`, serialized fixtures,
strict client decoding, type contracts and `schemas/rustx.schema.json` are updated.
The old App Server generated pair is removed. Native authoring remains schema 10
and Model Catalog schema 1; execution/storage schemas do not change.

Both ordinary configuration writes and manual source repair validate complete
Model definitions in the shared native encoder before publication. Rejected
orders and stale revisions preserve file bytes and the accepted revision. A
Workspace replaces the complete Model, including intentional omission of order;
removing the override restores the User definition. Arrays never merge across
scopes.

## Editor and execution boundaries

The existing Profile editor provides explicit configure/clear and keyboard move
controls. Configure seeds a deterministic identity-sorted draft. Configured adds
append with a placement notice; deletes remove the same identity from both
collections, preserving survivor order. Final deletion omits the collection and
order. The default and all invocation parameters stay independent. Invalid JSON
buffers survive reorder and block panel switching. Reset and explicit Cancel
discard the complete unit through its transaction actor; Close retains drafts.

The dedicated editor alone displays semantic order. Catalog enumeration,
`profile_position`, pagination and workflow/model selection retain their existing
identity order, and stale selection tokens remain rejected after publication.
Full owned/borrowed catalog views preserve exact order. Bounded selection headers
omit complete ordering metadata, and frozen authorities omit it rather than
fabricating a singleton or reading live catalog state.

The native matrix compares omitted, configured and reordered Models across
default/explicit primary selection and Session-following/explicit
Summary/Title/Classification/BTW policies. Frozen invocation bytes and effective
budgets/parameters agree. Adapter tests compare actual request bodies for Chat,
Responses and Anthropic. An existing real fake-provider gate proves an admitted
Attempt retains its invocation while metadata is published. Catalog publication
advances normal revisions without preparation or capability rebuilding.

**Reasoning-parameter mapping and task-specific automatic selection are not
implemented.** Compaction and title behavior remain unchanged.

## Local verification

Toolchains: Rust 1.98.1, Node 24.22.0, pnpm 11.13.1, Python 3.12. Browser tests use
the repository's digest-pinned Playwright 1.63.0 container. The Browser plugin was
unavailable; the repository Playwright workflow was used. No paid model calls ran.

Commands below run from the repository root or the stated subdirectory.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| In `test-support/fake-provider`: `uv sync --frozen && uv run --frozen pytest` | 55 passed |
| `cargo build --bins --all-features` | Passed |
| `RUST_TEST_THREADS=8 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features` | 5,880 passed, 20 ignored, zero failures or filtered tests |
| `cargo run --all-features --example check_test_lanes` | Linux lane coverage passed |
| In `protocol/app-server`: `pnpm generate`, `pnpm check`, `pnpm typecheck` | Passed |
| In `tui`: `pnpm typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 1,314 passed, 1 skipped |
| In `dev`: `pnpm typecheck && pnpm test` | 42 passed |
| In `web-console`: `pnpm typecheck && pnpm test` | 4,055 passed, 4 skipped |
| In `web-console`: `pnpm check:i18n && pnpm check:provenance` | Passed |
| In `web-console`: `pnpm exec vitest run test/settings-units.test.tsx` | 43 passed, including configured order surviving an unrelated edit |
| In `web-console`: `pnpm test:e2e test/e2e/settings-reasoning-order.spec.ts test/e2e/settings-model-editor.spec.ts test/e2e/settings-presentation.spec.ts --output=/tmp/rustx-524-browser-head` | 29 passed |
| `git diff --check` | Passed |

Browser coverage includes production native save/reload of non-lexical order,
configure, clear, append/delete, final deletion, reset, Cancel versus Close,
keyboard focus, invalid JSON retention and a long identity at 1440 and 390 pixels.
Four affected screenshot references were regenerated and inspected; comparison
and rasterizer-noise policies were not changed.

An initial 32-thread native run encountered existing MCP/deadline failures and a
stalled Scheduler gate. All three cases passed in isolation; the complete final
run above passed at eight threads without filtering. The full browser suite and
macOS checks are delegated to required final-head **FULL** CI, rather than claimed
as local coverage. The PR records that run's exact head, link and current status.

## Source recovery after semantic validation

Settings uses the existing source-intrinsic native validator for source health,
ordinary editing admission, whole-document repair admission and every encoded
candidate. A strictly parseable source with invalid Model metadata is presented
with a safe diagnostic and the existing complete-source repair form. That form
admits only a fully valid replacement on the exact observed source revision.
Failed candidates and stale requests preserve the original bytes and revision;
repairing a healthy source is refused. Successful repair restores ordinary unit
editing and clears an unresolved automatic-repair receipt only after native
health is proven.

This health decision does not require Catalog reference closure, a default Model
or Provider definitions supplied by another scope. Those remain composition
facts. Parser diagnostics keep their existing value-redaction rules, and semantic
diagnostics expose only the native constant category and schema path.

A deterministic regression first failed on the prior implementation because the
source was incorrectly projected as editable. It now covers two parseable invalid
Models in both User and Workspace sources, partial-repair rejection, stale-revision
conflict, complete valid replacement and ordinary editing after reread. Native
repair tests additionally cover non-Catalog semantic errors, receipt withdrawal,
and an incomplete source with an absent Provider and default selection. The real
browser regression repairs both Models through Settings, rejects a partial fix,
reloads the committed order and edits it through the normal Model form without
creating a Session or making a provider request.

The full local verification table above records the feature implementation before
this recovery revision. The PR separately records the recovery revision's targeted
checks and the final-head FULL CI verdict.

| Recovery revision check | Result |
| --- | --- |
| `RUST_TEST_THREADS=8 RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib --all-features configuration::` | 254 passed |
| Web `pnpm exec vitest run test/settings-document-authoring.test.tsx test/settings-surface-ownership.test.tsx test/settings-auto-repair.test.tsx` | 90 passed |
| Web `pnpm typecheck && pnpm check:i18n && pnpm check:provenance` | Passed |
| Web `pnpm test:e2e test/e2e/settings-reasoning-order.spec.ts test/e2e/settings-ownership.spec.ts` | 5 passed |
| Pinned browser rerun of the new repair scenario with a Model-specific physical-source assertion | 1 passed |
| Independent read-only review | No remaining actionable findings |

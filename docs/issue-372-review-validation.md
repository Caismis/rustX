# Issue #372 / PR #379 review validation

## Git and protocol base

- Previous HEAD: `3c6c3a9309985bd4f4ba07096fab8716ccffd3d2`.
- Previous base: `d9453ce878f098dd717e1cd8663ca1135d7f147b`.
- Rebased onto `e0ded7766a834ce44baa21e36a42d667ff7fb274`, the merge of PR #378.
- Only `/home/caismis/Documents/codes/rustX-issue-372`, branch
  `issue-372-trace-native-presentation`, was edited. It was clean at entry.
- Base App Server 12 / Runtime Client 42 → branch App Server 13 / Runtime Client 43.
- Generated v13 replaces v12; no old decoder, alternate DTO or fallback negotiation.
- A structural schema comparison retained all 55 main methods and proved the
  `subagent/transcript` request schema is unchanged. v13 also contains #379 Trace
  presentation, exact `CertifiedExtensionIdentity`, and native Tool correlation.

## Context semantics

`context_semantics` validates canonical source and kind in one match. The previous
independent `context_source` / `context_family` functions are removed.

| Canonical source | Canonical kind | Projected source | Projected kind |
| --- | --- | --- | --- |
| Runtime | GoalStatus | Runtime | GoalStatus |
| Runtime | RuntimeToolObservation | Runtime | RuntimeToolObservation |
| Runtime | AgentStatus | Runtime | AgentStatus |
| Extension { contributor } | ExtensionEnvironment | CertifiedExtension { exact contributor } | ExtensionEnvironment |

Every other pair yields the existing `ConversationStoreError::InvalidReference`.
The guard precedes identity/content projection; no source coercion, new wire
variant, Context Engine change or Web-side inference was introduced. Identity
and ordering still come from `RequestSnapshot.request_context_ids`, with keyed
canonical Message Ledger reads.

## Deterministic regressions

All names below belong to `runtime_client::trace::tests`:

- `all_context_assembly_semantic_pairs_project_from_durable_request_start` proves
  all four legal pairs, exact frozen order, and exact extension identity.
- `runtime_extension_environment_is_rejected_after_durable_request_start`.
- `extension_runtime_tool_observation_is_rejected_after_durable_request_start`.
- `extension_goal_status_is_rejected_after_durable_request_start`.
- `extension_agent_status_is_rejected_after_durable_request_start`.
- `non_admitted_context_provenance_is_rejected_after_durable_request_start` covers
  Human, Agent, Fleet and ExternalSystem.

Each rejection fixture commits through `commit_model_turn_start`, verifies both
the frozen IDs and exact stored canonical message, then reads Trace and asserts
InvalidReference. AgentStatus fixtures include the required prepared metadata;
they do not fail structural admission before reaching Trace. No sleeps.

Existing `request_scoped_context_that_is_not_an_admitted_context_fact_never_commits`
continues proving that a non-Context request reference is rejected structurally.
`two_certified_extensions_stay_distinguishable_by_exact_contributor_identity`
continues proving exact provenance across paging/reopen.

The accepted anchor/summary/lifecycle split is preserved. `anchor.rs`,
`lifecycle.rs`, `record.rs`, `live.rs`, `mod.rs` and `types.rs` are unchanged from
reviewed HEAD; only `summary.rs` gains the semantic-pair guard. Both `a_lifecycle_refresh_resolves_no_immutable_presentation_relationship`
and `refreshing_many_request_cursors_reconstructs_no_immutable_presentation` retain
their `(0, 0)` immutable relationship probe assertions. The remaining Trace suite
retains predecessor/page-boundary/history, retry/recovery, compaction/request-only,
exact Tool correlation, bounds, read-cut and lifecycle repair coverage.

## Validation environment

Linux x86_64. Existing locked dependencies were installed. Docker is unavailable;
the repository supports `CONTAINER_ENGINE=podman`, used with its unchanged
pinned Playwright image and browser suite. No screenshot references were changed.
The macOS CI command selection is also exercised locally on Linux; native macOS
OS behavior remains for CI to verify.

An initial Trace run had 71 passes / 2 failures because the two new AgentStatus
fixtures omitted required prepared start metadata. The fixtures were corrected;
the subsequent final Trace suite passed all 73 tests. A malformed conflict
resolution was caught by formatting, discarded, and the rebase redone before
validation. Neither was waived. No existing test was removed or weakened.

Raw local logs: ignored `target/issue-372-review/` in the task worktree.

## Final command results

All final commands exited 0. CI selections were read from the rebased `.github/workflows/ci.yml`.

| Command | Result |
| --- | --- |
| `pnpm --dir protocol/app-server install --frozen-lockfile` | Pass |
| `pnpm --dir protocol/app-server generate` | Pass; v13 regenerated, v12 removed |
| `pnpm --dir tui install --frozen-lockfile` | Pass |
| `pnpm --dir web-console install --frozen-lockfile` | Pass |
| `pnpm --dir dev install --frozen-lockfile` | Pass |
| `(cd test-support/fake-provider && uv sync --frozen)` | Pass |
| `(cd test-support/fake-provider && uv run --frozen pytest)` | Pass; 51 tests |
| `pnpm --dir protocol/app-server check` | Pass |
| `pnpm --dir protocol/app-server typecheck` | Pass |
| `pnpm --dir tui typecheck` | Pass |
| `pnpm --dir web-console typecheck` | Pass |
| `pnpm --dir web-console test` | Pass; 529 tests / 36 files |
| `pnpm --dir web-console check:provenance` | Pass |
| `cargo fmt --all -- --check` | Pass |
| `pnpm --dir web-console build` | Pass |
| `pnpm --dir dev typecheck` | Pass |
| `pnpm --dir dev test` | Pass; 37 tests |
| `cargo test --lib --all-features runtime_client::trace` | Pass; 73 passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| `cargo build --bins` | Pass |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm --dir tui test` | Pass; 854 tests, zero skipped |
| `cargo test --lib --bins --examples --all-features -- --skip boundary_suites::` | Pass; 2983 passed / 1 existing ignored |
| `cargo test --test contracts --test provider --all-features` | Pass; 27 passed, 166 passed / 5 existing ignored |
| `cargo test --lib --all-features -- boundary_suites::` | Pass; 231 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output` | Pass; 30 passed, 5 passed, 23 passed, 130 passed, 53 passed, 53 passed, 130 passed |
| `CONTAINER_ENGINE=podman pnpm --dir web-console test:e2e` | Pass; 51 browser tests, unchanged pinned image via Podman |
| `cargo test --lib --bins --all-features -- --skip scripted_suites:: --skip local_runtime::session_runtime_manager::tests::` | Pass; 2519 passed / 1 existing ignored |
| `git diff --check` | Pass |
| `git diff --cached --check` | Pass |

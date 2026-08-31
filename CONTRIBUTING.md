# Contributing

rustX is currently pre-1.0 and architecture-first.

The normative repository rules are defined in [`docs/repository-policy.md`](docs/repository-policy.md). This document summarizes the contributor workflow.

## Language policy

All repository content must be written in English, including:

- source code
- comments
- documentation
- tests
- CLI output
- errors
- issue titles and bodies
- pull request titles and bodies
- review comments
- commit messages

## Compatibility policy

Do not add compatibility layers for legacy runtimes, old schemas, or previous abstractions unless compatibility becomes an explicit product requirement.

During pre-1.0 development, prefer a breaking change over preserving an abstraction that weakens correctness, separation of concerns, or long-term maintainability.

## Architecture policy

Dependencies must point inward toward runtime-owned abstractions.

Do not expose external SDK types through public runtime-core interfaces.

Do not make the agent kernel depend directly on:

- provider SDKs
- MCP SDKs
- databases
- HTTP frameworks
- process implementations
- control-plane schemas

## Issue workflow

Use the repository Issue Forms.

- Use **Bug report** for reproducible defects.
- Use **Feature request** for new capabilities with a clear problem and acceptance criteria.
- Use **Architecture proposal** for changes to runtime contracts, invariants, layer boundaries, or execution semantics.

Non-trivial implementation work should have a related issue before merge.

## Pull request requirements

Keep a pull request focused on one coherent architectural or functional change.

A merge-ready pull request should:

- link a related issue unless the change is trivial repository administration;
- explain architectural and runtime-semantic impact;
- identify intentional breaking changes;
- include deterministic tests for execution semantics when practical;
- update architecture or invariant documentation when required;
- pass formatting, Clippy, and test checks;
- avoid unrelated refactors;
- avoid compatibility shims unless explicitly approved.

Use the repository pull request template and complete its checklist before review.

## Change requirements

A change that modifies execution semantics should include deterministic tests whenever possible.

Changes to any runtime invariant must update `docs/invariants.md` and explain why the invariant changed.

Changes to architectural boundaries must update `docs/architecture.md`.

Persistence-facing type changes should include serialization or round-trip tests where applicable.

## Testing

The preferred test order is:

1. pure unit tests
2. deterministic mock-runtime tests
3. live integration tests

Core correctness must not depend only on live model behavior.

### Baseline Rust checks

Before merge, the baseline local Rust checks are:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

### Full pre-PR and CI validation

The baseline commands are only the Rust starting point. The full CI workflow
also requires the provider emulator checks and the TUI checks. Run the
following from the repository root with Python 3.12, `uv`, and the Node LTS
line available:

```bash
# Provider emulator, used by both Rust and TUI integration checks.
(cd test-support/fake-provider && uv sync --frozen)
(cd test-support/fake-provider && uv run --frozen pytest)

# Rust validation with provider-emulator tests required rather than skipped.
# One command covers every target, conformance included; tests/README.md
# documents the domain targets for focused runs.
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-targets --all-features

# TUI's locked pnpm workflow.
nvm install --lts
nvm use --lts
corepack enable
(cd tui && corepack install)
(cd tui && pnpm install --frozen-lockfile)
(cd tui && pnpm typecheck)
cargo build --bin rustx
(cd tui && pnpm test)

git diff --check
```

The GitHub Actions workflow is authoritative if this list and CI ever differ.

## Commit style

Use short imperative English commit messages, for example:

- `Define canonical message blocks`
- `Implement attempt state machine`
- `Add complete-message compaction fixture`

Keep commits focused on one coherent architectural or functional change.

## Merge style

Squash merge is preferred. The squash commit title should describe the resulting repository state in concise imperative English.

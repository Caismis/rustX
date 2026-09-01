# Repository Policy

This document defines the contribution and review policy for rustX during pre-1.0 development.

## 1. Language

All repository content must be written in English. This includes source code, comments, documentation, examples, tests, CLI output, error messages, issue titles and bodies, pull request titles and bodies, review comments, and commit messages.

## 2. Compatibility

rustX does not preserve compatibility with previous runtimes, legacy schemas, or flawed abstractions during pre-1.0 development.

Do not add compatibility shims unless a future project policy explicitly requires them.

When an abstraction is wrong, prefer a breaking change over preserving it.

## 3. Architecture

Dependencies must point inward toward rustX-owned contracts.

External SDK types must terminate at adapter boundaries. Provider SDK types, MCP SDK types, database models, HTTP framework types, process implementation details, and control-plane schemas must not appear in agent-kernel interfaces.

Test seams must not be published API. A fixture that substitutes a runtime-owned dependency — a provider adapter behind a validated catalog binding, a summary service behind a context runtime — must be `#[cfg(test)] pub(crate)` and must be exercised from the crate's own test build. `#[doc(hidden)] pub` hides a seam from documentation but leaves it callable by a consumer, and is not an acceptable substitute. Suites needing such a seam live under `tests/scripted/` (deterministic contracts) or `tests/boundary/` (in-crate boundary conformance) and compile into the crate through `src/lib.rs`; `tests/*/main.rs` binaries use published API only.

Composed Agent Loop conformance must not use a seam at all. There is one canonical external provider-emulation boundary — `test-support/fake-provider`, an external Python 3.12 process managed by uv — and a test that exercises the Agent Loop, the context engine, the tool runtime, or the capability plane end to end must reach it through the real catalog, adapter, HTTP client, and stream parser. A scripted injected adapter and the raw Rust HTTP fixture remain valid for internal state machines and for single-adapter translation tests respectively; neither is an implementation of composed conformance. Python is test-support only and must never become a rustX production runtime dependency.

Changes that alter architectural boundaries must update `docs/architecture.md`.

Changes that alter runtime invariants must update `docs/invariants.md`.

## 4. Issues

Use the repository Issue Forms.

A non-trivial implementation should have a related issue before code is merged. Architecture changes should use the architecture proposal form.

Issues must describe the problem, scope, non-goals where relevant, and acceptance criteria. Implementation details may evolve, but the acceptance criteria should remain testable.

Do not use issues to preserve compatibility by default. If compatibility is proposed, it must be justified explicitly as a product requirement.

## 5. Pull requests

Pull requests must be focused on one coherent architectural or functional change.

Every non-trivial pull request must:

- link a related issue;
- explain what changes and why;
- identify architectural or runtime-semantic impact;
- include deterministic tests for execution semantics when practical;
- update architecture or invariant documentation when required;
- pass formatting, linting, and test checks;
- avoid unrelated refactors;
- avoid compatibility shims unless explicitly approved;
- avoid leaking external SDK types into the runtime core.

Large changes should be split by stable layer boundary instead of by arbitrary file count.

Draft pull requests are acceptable for early architectural review, but merge-ready pull requests must satisfy the full checklist.

## 6. Review requirements

Review should prioritize, in order:

1. correctness of runtime semantics;
2. preservation of documented invariants;
3. layer boundaries and dependency direction;
4. deterministic behavior and recoverability;
5. test quality;
6. operational clarity;
7. code style.

Do not approve a change merely because it preserves existing behavior. Existing behavior has no special status before 1.0.

## 7. Merge policy

The preferred merge strategy is squash merge.

The squash commit title should be a short imperative English statement that describes the resulting repository state.

Do not merge with failing CI.

A pull request that changes a persistence-facing schema, runtime event contract, message contract, attempt/turn semantics, cancellation semantics, or recovery behavior must describe the breaking change explicitly.

## 8. CI gates

The baseline CI gate is:

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features`
- `uv sync --frozen` and `uv run --frozen pytest` in `test-support/fake-provider`

The Rust job installs Python 3.12 and uv and runs with `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`, so the Agent Loop conformance suite fails on a missing toolchain instead of skipping itself.

Additional deterministic runtime fixtures will be added as the executor becomes functional.

## 9. Branch protection

The intended `main` branch policy is:

- pull requests required for changes;
- required status checks must pass;
- stale approvals dismissed after material changes;
- CODEOWNERS review required for protected changes;
- direct pushes disabled except for repository administration emergencies;
- force pushes disabled;
- branch deletion disabled.

These controls are repository settings and should mirror this document.

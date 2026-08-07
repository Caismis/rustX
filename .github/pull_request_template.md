## Summary

Describe what this pull request changes and why.

## Related issue

Closes #

Use `N/A` only for trivial repository administration or documentation-only changes.

## Architectural impact

Describe any impact on runtime contracts, dependency direction, message semantics, attempt/turn behavior, compaction, tool execution, cancellation, recovery, or persistence-facing data.

Use `None` when there is no architectural impact.

## Breaking changes

List intentional breaking changes. Pre-1.0 breaking changes are acceptable when they improve the architecture.

Do not add compatibility shims unless explicitly approved.

## Validation

Describe the tests and fixtures used to validate this change.

## Checklist

- [ ] All repository content in this PR is written in English.
- [ ] The PR is focused on one coherent architectural or functional change.
- [ ] A related issue is linked for non-trivial changes.
- [ ] External SDK types do not leak past their adapter boundary.
- [ ] The agent kernel does not gain a dependency on provider SDKs, MCP SDKs, databases, HTTP frameworks, process implementations, or control-plane schemas.
- [ ] No compatibility shim was introduced unless explicitly approved.
- [ ] Runtime-semantic changes include deterministic tests when practical.
- [ ] Persistence-facing type changes include serialization/round-trip coverage where applicable.
- [ ] `docs/architecture.md` is updated when layer boundaries change.
- [ ] `docs/invariants.md` is updated when runtime invariants change.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo test --all-targets --all-features` passes.
- [ ] Failure, cancellation, and recovery behavior were considered where relevant.

# Contributing

rustX is currently pre-1.0 and architecture-first.

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
- commit messages

## Compatibility policy

Do not add compatibility layers for legacy runtimes, old schemas, or previous abstractions unless compatibility becomes an explicit post-1.0 product requirement.

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

## Change requirements

A change that modifies execution semantics should include deterministic tests whenever possible.

Changes to any runtime invariant should update `docs/invariants.md` and explain why the invariant changed.

Changes to architectural boundaries should update `docs/architecture.md`.

## Testing

The preferred test order is:

1. pure unit tests
2. deterministic mock-runtime tests
3. live integration tests

Core correctness must not depend only on live model behavior.

## Commit style

Use short imperative English commit messages, for example:

- `Define canonical message blocks`
- `Implement attempt state machine`
- `Add split-turn compaction fixture`

Keep commits focused on one coherent architectural or functional change.

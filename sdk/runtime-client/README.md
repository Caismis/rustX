# Runtime Client Session deletion contract

Protocol 28's transport-independent deletion requests and results. Rust is the
ownership and durability authority. The existing TUI protocol imports these types;
this package does not implement confirmation UI or filesystem operations.

Run `pnpm install --frozen-lockfile`, `pnpm test -- --runInBand`, and
`pnpm exec tsc --noEmit`. The serial spelling maps to Node's native test runner.
The JSON fixtures are also roundtripped by Rust's deletion contract test.

See [the lifecycle contract](../../docs/session-deletion-lifecycle.md).

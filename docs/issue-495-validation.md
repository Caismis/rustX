# Issue 495 validation

Development started in a fresh worktree at `origin/main` commit
`5c8be3af0fe08c0fb35c5553901a32d6cf17b9d1`. The change adds one conservative
native JSON Schema capability and a per-Call structured output contract, with
the existing Auxiliary executor retaining lifecycle and physical settlement
ownership. Title and Summary remain text consumers.

## Contracts and evidence

- `model::structured` validates the closed schema vocabulary, depth, encoded
  size, aggregate enum limits and output byte budget before dispatch. Complete
  output parsing and `jsonschema` validation happen before typed JSON acceptance.
- Production adapters are exercised through local scripted HTTP servers for
  all three protocols. Exact outbound bodies cover native formatting, prompt
  requests, sibling parameters, collisions before network entry and the absence
  of a hidden prompt fallback on native rejection.
- A separate internal test consumer uses the real Auxiliary executor and
  SQLite Store. Channels and consumed settlement gates prove confirmed physical
  completion, cancellation cleanup and frozen retry inputs. Invalid JSON,
  mismatched schemas, refusal, reasoning-only output, truncation, output limits
  and context fit never return a business value.
- Fingerprints and snapshot reconstruction retain the schema and strategy.
  Cross-protocol estimates include prepared schema/prompt bytes. Acceptance
  diagnostics omit schema contents and generated output.
- Native configuration and Web tests cover omitted=false, effective
  intersection, Profile/frozen invocation retention, User/Workspace whole-object
  replacement, and EN/ZH UI strings. Pinned-browser tests cover real native
  persistence and desktop/mobile layout.

## Independent review

A separate agent that did not implement the change reviewed the complete diff.
It found two P2 schema-boundary defects: missing aggregate provider enum limits
and semantic traversal before bounded input checks. Both were fixed, with
regressions for aggregate enums, large string enums, oversized keys/arrays and
excessive nesting. Independent re-review found no remaining actionable issues.

## Validation environment

Validation uses Rust 1.98.1, Node 24.19.0, pnpm 11.13.1, local scripted providers
and the repository-pinned Playwright container. No live model credentials are
required by the deterministic tests.

The final full Rust library run with four workers completed 4,599 tests
successfully, with five existing boundary-fixture liveness timeouts and four
ignored cases. All five timeout cases passed individually against the same
final binary. The final full Web run with four workers completed 3,194 tests
successfully, with two workbench-host timeouts and four skipped cases. The
complete workbench-host file then passed all 22 tests after concurrent native
test load ended. These are full runs plus successful isolated reruns, not
single uninterrupted green full-suite commands. No deadlines were extended.

An earlier Web setup used a `node_modules` symlink that Vite rejected for PDF
worker access. Installing the pinned dependencies directly in this worktree
removed that path issue. All temporary setup changes were removed. Canonical
configuration and request-fingerprint expectations were updated for the new
field; the final process suite passes all 65 cases.

## Initial implementation results

| Check | Final result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| Focused structured schema, request and executor tests | 31 passed; the flat fingerprint oracle also passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features -- --test-threads=4` | Library: 4,599 passed, five liveness timeouts, four ignored; all five passed isolated reruns |
| External Rust integration targets | CFG3 catalog 26, managed output 5, conformance 35, contracts 31, durable 147, process 65, provider 187, subagent 47, tools 135 passed; three conformance and five provider cases ignored |
| `cargo test --all-features --bins --examples` | 27 passed |
| `cargo test --doc --all-features` | Nine passed |
| Linux FULL test-lane coverage checks | `rust-contracts` and `rust-boundaries` passed |
| CI scripts, benchmark controllers, provider emulator | 64, 37 and 55 Python tests passed, respectively |
| Schema generation and `protocol/app-server` `pnpm check` | Passed; generated artifacts have no drift |
| Protocol, Web and TUI type checks | Passed |
| Web Vitest suite with four workers | 3,194 passed, two timeouts, four skipped; entire affected file passed all 22 cases on a quiet rerun |
| TUI full test suite | 1,191 passed, one skipped |
| Web production build, i18n and provenance checks | Passed |
| Pinned Playwright browser tests | Three passed: real native Settings persistence and desktop/mobile layout |
| Independent code review | Two findings fixed and regression tested; final re-review clear |

The external Rust targets were run separately after the full command stopped
at library timeouts. The repository's remote FULL workflow is requested through
the PR's `full-verification` label; local results do not substitute for its
platform and merge-gate checks.

## Format boundary review follow-up

Review of commit `2b6c5678e5a4cf5e3909070614cc3fd35b21c7a5` identified two
additional format boundaries that must advance with this feature:

- SQLite format 76 replaces 75. The extra response-schema tuple member and
  serialized capability change even ordinary text-request fingerprints. Both
  `open` and `open_existing` reject format 75 with `SchemaVersionMismatch`
  before reading history, leaving the database bytes unchanged. File-backed
  regressions reopen current-format text requests with both capability values,
  plus Native and Prompt requests, and compare complete requests, snapshots and
  fingerprints.
- App Server v53 replaces v52 across initialization, WebSocket negotiation,
  generated artifacts and all current client imports. The serialized
  `jsonSchemaOutput` capability is incompatible with v52's closed object schema.
  The real server handler rejects v52 before initialization or attachment;
  WebSocket and Web/TUI tests reject old peers. Strict TUI decoding remains in
  place. Obsolete v52 artifacts are removed.

There is no database migration, old-fingerprint fallback, relaxed decoder or
protocol downgrade. A fresh independent agent reviewed these corrections and
found no remaining actionable code issues; its wire-field documentation
correction was applied.

Focused revision validation:

- SQLite format/reopen regressions: two passed.
- Native schema-related tests: 141 passed.
- Native protocol admission/conformance tests: ten passed.
- External contract tests: 31 passed; durable tests: 147 passed. The first
  combined external run was interrupted by SIGTERM without an assertion
  failure; the durable target was rerun independently to completion.
- External process tests: 65 passed, including the real WebSocket version gate.
- Web client/transport tests: 97 passed; TUI strict decoder/client tests: 82 passed.
- Protocol, Web and TUI type checks: passed.
- Authoritative protocol generation and provenance checks: passed.
- The staged v53 schema, TypeScript and fixtures passed `pnpm check` without drift.
- All-target/all-feature Clippy, formatting and whitespace checks: passed.

The initial implementation's [FULL CI run](https://github.com/Caismis/rustX/actions/runs/39919564464)
completed successfully on `2b6c5678e5a4cf5e3909070614cc3fd35b21c7a5`. The format
correction is pushed with the existing `full-verification` label, so its own
commit must pass a new FULL run before merge.

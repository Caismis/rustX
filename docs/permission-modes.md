# Fixed execution-permission modes (Issue #544)

rustX ships exactly three built-in execution-permission modes: `plan`,
`automatic`, and `full_access`. They are Rust-owned, not TOML presets, and
never change Tool Registry membership, model-facing names, schemas, MCP
discovery, or capability selection.

## Protocol regeneration note

`protocol/app-server/v78.ts` is generated from Rust schemars. Only the
`ApprovalMode` type alias and the inline `effective_approval_mode` union were
string-replaced to `plan | automatic | full_access` so WebUI/TUI typecheck
against the new vocabulary. Regenerate `v78.ts` / `v78.schema.json` from the
Rust `ExecutionPermissionMode` (exported as `ApprovalMode`) to refresh the
surrounding doc comments and any other generated ApprovalMode descriptions.
Do not hand-edit the rest of that generated file.

## Modes

| Mode | Default behavior |
| --- | --- |
| `plan` | Native reads and approved bookkeeping allowed; Write/Edit may change only workspace-root `PLAN.md`; Bash requires a proven read-only classification; other defaults refuse without `elevation_reason`. |
| `automatic` | Keep Plan's deterministic fast paths; remaining defaults go through the shared Auxiliary permission reviewer; denied calls may escalate with `elevation_reason`. |
| `full_access` | Defaults allowed without Auxiliary or ordinary human review. Effective explicit `deny` / `ask` resource settings still apply. |

Session stores the selected mode. Child Agents inherit the invoking
execution's frozen mode; they do not own a separate mode picker.

## Resource overrides

A resource may omit permissions or declare all three mode entries with values
`default`, `allow`, `ask`, or `deny`. Exact-tool objects replace source
defaults. Workspace completely shadows Global for the same canonical resource.
Effective `deny` cannot be overridden by elevation, review, or Full Access.

## Elevation

Every model-facing tool call uses `{arguments: {...}, elevation_reason?: ...}`.
Model-selectable tools also require an outer `execution_mode`. Business schemas
remain independent of these outer runtime fields, including same-named business
properties. A nonblank elevation reason requests human escalation for the exact frozen
invocation. It never reaches business executors and never becomes a persistent
grant.

## Bash analysis

Bash classification is conservative and fail-closed. Parse failures,
unsupported constructs, and unresolved behavior are `unproven`, not proven
read-only. Plan elevates unproven Bash; Automatic reviews it.

### Upstream pin and provenance

| Item | Value |
| --- | --- |
| Upstream | [zai-org/ZCode](https://github.com/zai-org/ZCode) |
| Pinned commit | `b95ebaffeab641907b38652f45c1502bd762972d` (`chore: sync ZCode source v3.18.0`) |
| Upstream license | Apache-2.0 |
| Reference modules | `bash-command-parser.ts`, `bash-semantics.ts`, `bash-readonly-policy-argv.ts` (+ policy tables / callbacks) |
| Rust parser | [`brush-parser`](https://crates.io/crates/brush-parser) 0.4.x (MIT) — POSIX/bash AST; replaces ZCode's TypeScript `unbash` |

Adopted rules: bounded AST walk over simple commands / pipelines / `&&` `||` `;`
sequences; dynamic expansions (command/process/parameter/arithmetic/brace) and
unsupported compounds fail closed; data-driven per-command `safeFlags` /
allow-any / multiword policies; known write options (`sed -i`,
`find -delete/-exec`, `tree -o`); safe env-assignment and
input-redirect allowlists; `command` / `builtin` / `noglob` wrapper stripping;
conservative refusal for executable behavior that cannot be proven from argv.

Intentional differences from ZCode:

- `brush-parser` instead of `unbash` (Rust-native AST).
- Unquoted globs (`*`, `?`, `[`) are unproven (post-expansion argv is unresolved).
- Path-qualified executables (`/usr/bin/sed …`) are unproven.
- brush `[[ … ]]` ExtendedTest / brace-group / subshell / function / loop /
  conditional compounds are unsupported → unproven.
- Git is always unproven: repository configuration can execute textconv, clean
  filters, signature verifiers and other helpers even for nominal read commands.
  Full Access retains normal Git behavior and real patch output; Plan requires
  elevation and Automatic requires review.
- Windows CMD dialect handling is omitted (rustX Bash is POSIX).
- Bare `env` is command-only read-only; `env …` wrappers stay unproven.


## Acceptance evidence

P01–P22 mapping and remaining gaps: `docs/permission-modes-p01-p22.md`.

## Auxiliary permission review

Automatic-mode `Review` uses the shared #543 Auxiliary model selection (default
lowest Profile). The reviewer returns a strict `{decision, explanation}` JSON
object (`allow`|`deny`). Invalid, timed-out, or unavailable review fails closed.
Native review facts include the canonical task's Human/Agent provenance or the
admitted scheduled Claim, business arguments, mutation target, matrix decision
and structured Bash analysis. Missing or oversized evidence fails closed;
model-authored explanations never establish user authority.
A nonblank `elevation_reason` may escalate a denial or reviewer failure to
human Ask for the exact frozen invocation.

## Sparse reminders

A bounded permission-mode reminder is appended to native context only:

1. before the first model request in a context with no consumed reminder;
2. before the first request after a successfully committed compaction;
3. when a different mode becomes effective at a model-request boundary.

Eligibility is consumed only at successful model-request-start commit, so
pre-start cancellation cannot lose it and retries do not duplicate it.
Reconstruction uses the last committed primary request's mode and context
activation. Unchanged context and mode remain quiet across ordinary resumes.

## Settlement

Permission decides whether a prepared invocation may start. Independent
authorized calls may start as soon as their own decision is Allow, without
waiting for unrelated Ask/Review siblings. ToolResults still commit atomically
in original model call order. Tool-level Sequential/Parallel authoring is
removed; Bash still selects foreground|background via `execution_mode`.

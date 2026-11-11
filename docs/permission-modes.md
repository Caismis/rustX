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

`elevation_reason` is optional runtime metadata on every model-facing tool
schema. A nonblank reason requests human escalation for the exact frozen
invocation. It never reaches business executors and never becomes a persistent
grant.

## Bash analysis

Bash classification is conservative and fail-closed. Parse failures,
unsupported constructs, and unresolved behavior are `unproven`, not proven
read-only. Plan elevates unproven Bash; Automatic reviews it.

## Auxiliary permission review

Automatic-mode `Review` uses the shared #543 Auxiliary model selection (default
lowest Profile). The reviewer returns a strict `{decision, explanation}` JSON
object (`allow`|`deny`). Invalid, timed-out, or unavailable review fails closed.
A nonblank `elevation_reason` may escalate a denial or reviewer failure to
human Ask for the exact frozen invocation.

## Sparse reminders

A bounded permission-mode reminder is appended to native context only:

1. before the first model request of a new admitted execution;
2. before the first request after a successfully committed compaction;
3. when a changed mode becomes effective at the next independent admission
   (covered by (1) for that new admission).

Eligibility is consumed only at successful model-request-start commit, so
pre-start cancellation cannot lose it and retries do not duplicate it.

## Settlement

Permission decides whether a prepared invocation may start. Independent
authorized calls may start as soon as their own decision is Allow, without
waiting for unrelated Ask/Review siblings. ToolResults still commit atomically
in original model call order. Tool-level Sequential/Parallel authoring is
removed; Bash still selects foreground|background via `execution_mode`.

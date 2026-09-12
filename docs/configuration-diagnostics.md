# Configuration authoring and diagnostics

See [offline Workflow authoring](workflow-authoring.md) for `rustx workflow check <id>`
and `rustx workflow explain <id>`, compiled explanations and small executable templates.

See [canonical named Agent resources](subagent-resources.md) for schema 8
Agent files, discovery/admission, bounded roots, source provenance, and frozen
reload/child contracts.


Rust owns this interface. The TUI forwards arguments and terminal streams; it
does not discover, merge, validate, trust, or interpret configuration.
`config show` is the **prospective next launch**, never a running Session or
attempt. Live configuration projection is outside this contract (CFG-07).

## Finite command grammar

```text
rustx --help
rustx init --template openai-chat|openai-responses|anthropic
  --provider ID --model-id ID --endpoint URL --credential-env NAME
  --context-window TOKENS --max-output TOKENS
  --tool-calls true|false --reasoning true|false [--compat TOML] [--json]
rustx init --template custom --provider ID --endpoint URL
  --credential-env NAME --model-document PATH [--json]
rustx config check [SELECTION] [--json]
rustx config show --sources [SELECTION] [--json]
rustx doctor --probe [--prepare] [SELECTION] [--json]

SELECTION := [--models PATH] [--config PATH] [--workspace DIRECTORY]
  [--runtime-root DIRECTORY] [--model PROVIDER/MODEL]
  [--skill PATH ...] [--no-skills]
  [--no-tools | --no-builtin-tools | --tools NAME,...]
  [--exclude-tools NAME,...]
```

Switches cannot repeat; `--skill` is repeatable. Tool conflicts and exact-name
selection are the ordinary launch contract: `--no-tools` also conflicts with
`--exclude-tools`. Diagnostic commands reject trust changes, Session selection,
conversation inspection, and Session names. Ordinary startup remains
`rustx [launch flags]`; see [launch configuration](launch-configuration.md).

| Exit | Meaning |
| --- | --- |
| 0 | Complete initialization or help (no execution-readiness claim) |
| 1 | Probe failed/timed out/cancelled, or output failed |
| 2 | Invalid configuration/arguments, or initialization conflict/publication failure |
| 3 | Incomplete configuration or unresolved readiness |

Bare `init` reports missing declarations (3), without prompting or writing.
Malformed/incomplete explicit initialization arguments return 2. An untrusted
workspace returns 3 even with a valid native-only configuration. Missing implicit
model configuration is incomplete; a missing explicitly selected file is invalid.
Validity means local structural/semantic correctness. Readiness means prospective
execution readiness, not completion of static analysis. Every selected provider
has unresolved credential/connectivity/compatibility facts, so even a valid
native-only configuration returns 3. Neither credential values nor environment
variable presence are read. Literal credentials do not establish connectivity
either. Doctor currently always
has an unresolved provider result, so returns 3 unless a probe fails (1) or a
local static source failure exists (2). Independent sources are still probed;
a probe failure takes precedence over static invalidity in the final doctor exit.

Human output shows classification, reasons, corrections, and redacted structured
values. `--json` emits a version-1 object with `operation`, `scope`, `validity`,
`readiness`, `diagnostics`, `launch`, `partial`, and `initialization` members.
Validity is `valid`, `invalid`, or `incomplete`. Execution readiness currently
has only the state `unresolved`: static launch reports cannot establish ready because
provider execution is unverified. Initialization has `readiness: null`: it
reports file publication, not next-launch readiness. Invalidity takes exit-code
precedence (2) over unresolved readiness (3).
Absent complete values are null, not guessed. Reports carry
`projection_omitted: false` normally. If the pretty projection
would exceed 256 KiB, both renderers omit `launch`/`partial`, append a structured
`projection_limit` diagnostic, and set `projection_omitted: true`. Existing
diagnostics, validity, readiness, and exit semantics are preserved. If diagnostics
alone still exceed the bound, the first authoritative invalid/incomplete cause
is retained first, followed by a fitting prefix of the remaining diagnostics in
original order. Both omission and `diagnostics_truncated` warnings are reserved.
An individually oversized causal record keeps its classification/location and
UTF-8 prefixes of file/path/reason/correction (1024 bytes per text field), with
explicit truncation markers. Only already-redacted structured values are used;
JSON records are never byte-cut. The common budget includes the human header
and trailing newline, so human and JSON output retain identical diagnostics.
Diagnostics carry
classification, category, source file, field/reference path, reason, correction, and optional
parser line/column. Positions are supplied only when the TOML parser knows them;
semantic errors do not manufacture positions. Doctor emits two ordered records:
`phase: probe_plan` with typed targets/effects, then `phase: probe_results` with
individual states and exact verification claims. There is no global `allGood`.

## Minimal initialization

On both Linux and macOS, user configuration is `$XDG_CONFIG_HOME/rustx`, or
`$HOME/.config/rustx` if XDG_CONFIG_HOME is absent. Relative HOME/XDG roots fail.
Initialization writes exactly:

- `<user configuration directory>/models.toml`: one explicitly declared
  provider and model, endpoint, protocol, credential reference, limits, and
  capabilities;
- `<user configuration directory>/settings.toml`: only `model.model`.

It does not serialize runtime defaults. It never writes project `rustx.toml`;
a normal project needs no configuration file. Runtime state remains under
`$XDG_STATE_HOME/rustx` or `$HOME/.local/state/rustx`, with disjoint per-workspace
identity directories. Initialization does not create runtime or trust state.

Templates select only their named protocol. IDs never imply limits, reasoning,
Tool support, or compatibility. The OpenAI templates require explicit `--compat`
using the actual catalog compatibility object; no compatibility is inferred.
The Anthropic template uses its native protocol contract. Text input/output is
the explicit scope of these three templates. The custom route reads one complete
current model-authoring object and supports the capabilities the native parser
accepts. It is not a provider catalog or compatibility promise.

For example, after verifying your provider's declarations, substitute your own
values in this shape (the endpoint below deliberately cannot serve a model):

```sh
rustx init --template openai-chat --provider example --model-id demo-model \
  --endpoint https://provider.invalid/v1 --credential-env RUSTX_API_KEY \
  --context-window 128000 --max-output 4096 --tool-calls true --reasoning false \
  --compat 'chat_reasoning_replay = "omit"'
rustx config check
rustx --trust grant
rustx config show --sources
```

`--credential-env` accepts a variable **name**, not its value; TOML contains
`$RUSTX_API_KEY`, never the API key. Secret values must be supplied outside these
documents. Do not put credentials in ordinary environment, headers, arguments,
endpoints, or IDs. Literal catalog credentials remain accepted by the
catalog's existing contract, but init never emits them and diagnostics never
expose them.

Both destination paths are preflighted before publication. Each complete file is
staged in the destination directory, synced, and published with no-clobber OS
semantics. An existing file or racing creator is never overwritten. Repeated init
reports conflicts and leaves bytes unchanged. This is **not a multi-file
transaction**: if models succeeds and settings fails, models remains and the
result lists that exact partial outcome. No rollback deletes user data. Directory
creation can remain after a failure. Publication does not promise crash-durable
multi-file atomicity.

## Static analysis and admission

`launch::analyze` is the one prospective semantic path: bounded TOML loading,
field authority, fixed precedence/defaults, path rebasing, catalog selection and
context validation, resource references, native Workflow compilation, and
source/Tool metadata admission. `launch::resolve` reuses that result and then
requires actual workspace trust before capturing credentials. Runtime composition
and Session publication remain downstream owners, not checker dependencies.

Host path discovery reads HOME/XDG paths, not a credential snapshot. Check/show
never inspect secret environment values, even to test whether a reference is
populated. Provider references and deferred verification are safe to project.
Command arguments, environment maps, request parameters, and auth headers are
redacted, not emitted as a runnable source configuration dump.

| Operation | Local read | Resolve secrets | Subprocess | Network | Session/runtime state | Trust write |
| --- | --- | --- | --- | --- | --- | --- |
| init | explicit custom model only; destination preflight | no | no | no | no | no |
| config check/show | bounded authorized files | no | no | no | no | no |
| doctor provider | static analysis | no | no | no | no | no |
| doctor admitted MCP | static analysis | declared refs at admitted use | stdio only | possible | no Session | no |
| doctor admitted Python with --prepare | package discovery | no configured secret refs | yes | possible | environment store only | no |

Static analysis does not invoke Git, uv, Python, version helpers, package
preparation, MCP, providers, Tools, Workflows, Subagents, SQLite, resource
publication, recovery, or reconnect. Workspace discovery uses filesystem markers,
including `.git` files/directories, not Git commands. Documents are limited to
1 MiB each; only regular files are read. Configuration admits at most 128 sources,
128 roles and 128 Workflows. Skill discovery bounds roots/packages, directory
entries, nesting, file count, and aggregate package bytes in the Skill owner.

Validity and readiness are independent. Unknown fields, forbidden host overrides,
bad known references and Workflow compiler failures are invalid. Disabled and
unconfigured sources are known inert states: no package or capability loading.
Enabled online sources are unresolved until discovery; no schemas or executors
are fabricated. Untrusted project resources remain unread and unresolved. A
source probe cannot enable them or grant workspace authority.

Managed Python discovery records directory identity only. `discovered_package`
distinguishes these sources from MCP settings; readiness remains inert until
admitted preparation demand exists. Static discovery never reads package code or
dependencies, creates environments, runs uv/Python, spawns MCP, or captures credentials.
Missing roots and unprepared packages do not fail native-only startup. Invalid
identities and escaping/symlinked canonical packages fail bounded static discovery.


The projection includes the selected model, paths, safe provider metadata,
resolved config, origins, precedence reasons, source activation/readiness,
main-model Tool policy/selection and exclusion reasons, discovered Workflows and
local Skills. Origins distinguish builtin, user, trusted project, CLI, and
untrusted prospective project declarations. Online Tool identities remain null
until known; `--no-tools` is a known empty selection independently of activation.

## Explicit probes

`doctor --probe` selects the configured targets and flushes the effect plan before
credential capture, spawn, preparation, or connection. Each target discloses
process, network, preparation and credential effects plus a finite deadline.
Stdio processes may themselves use the network, so network disclosure is
conservative. Disabled/unconfigured sources are skipped; untrusted sources are
unavailable. Probe selection never changes activation or trust.

Managed Python preparation requires the additional `--prepare` switch. Without
it, even an enabled Python source is unavailable and no package preparation is
attempted. With it, the existing Python store/discovery/preparation owner is used;
doctor does not implement uv commands or an alternate environment manager.

MCP uses `McpServerRuntime::connect_owned`, not a doctor-specific runtime or the
recovering coordinator. Success verifies handshake and capability discovery only;
it never calls `tools/call`. A 15-second target budget covers preparation and
connection. Deadline, Ctrl-C or SIGTERM cancellation signals the owner and **awaits** it;
successful connections are explicitly closed before returning. Settlement may
take longer than the operation deadline. Physical-settlement failure is reported
as failure, never as verified cleanup. No recovery task is installed to reopen a
probe source. Doctor creates no Session and executes no business Workflow/Tool.

The current provider adapters expose generation, not a safe metadata probe API.
Their probe result is therefore `unresolved`, with all effects false. No paid turn
is sent; endpoint reachability, accepted credentials, Tool calling, and reasoning
capabilities are not claimed. Results are individually `skipped`, `unavailable`,
`failed`, `timed_out`, `cancelled`, `verified`, or `unresolved`.

## Schema authority and example layers

`schemas/{models,settings,rustx,workflow,subagent}.schema.json` are generated from native
Rust authoring types and metadata with schemars. Partial settings describe omitted
fields, not fully resolved runtime structs. Project schemas exclude structurally
host-owned fields. Serde names, enum tags, nullability and unknown-field rejection
remain authoritative. Regenerate offline with:

```sh
cargo run --example generate_schemas
cargo test --lib cfg235_checked_in_schemas_match_authoritative_generation --all-features
```

Drift compares canonical generated bytes to checked-in artifacts. Editor schema
association does not allow `$schema` in documents whose parser rejects it. Schema
validation is structural assistance, never a second Workflow compiler or authority
for cross-resource references, trust, source readiness, or provider compatibility.

- `examples/local-runtime/minimal`: one explicit model and minimal settings;
  native Tools, no project config, Python, uv, MCP, Subagent or Workflow prerequisite.
- `examples/local-runtime/workflow-basic`: one existing fixed Workflow using native
  Read, compiled through the current native compiler.
- The existing full reference workspace and Workflow conformance fixtures remain
  comprehensive executable evidence, not mandatory first-run setup.

`cfg235_all_example_layers_use_real_resolver_and_workflow_compiler` checks all
three layers with the actual resolver/compiler. Existing Workflow conformance
suites remain in the CI boundary partition.

## Limits

Offline analysis cannot know online MCP Tool identities, endpoint reachability,
credential presence, live provider compatibility, or future source availability.
Untrusted resource contents are deliberately not validated. Failed analysis may
return only the paths and partial declarations established before the failure;
it does not invent a complete launch. Diagnostics stop at the first authoritative
failure instead of accumulating speculative follow-on errors. Canonical role
authoring (CFG-05) and Workflow authoring (CFG-06) build on this same path;
live configuration projections (CFG-07) remain separate work.

## Deterministic regression evidence

The `cfg235_` prefix selects the new source-module regressions. The two binary
tests run in CI's `process` target. No race/zero-effect proof uses sleeps.

| Requirement | Concrete test(s) |
| --- | --- |
| Oversized invalid projection preserves its real package error and redaction in both renderers | `cfg235_oversized_invalid_projection_preserves_authoritative_diagnostics` |
| Oversized partial projection preserves the incomplete cause | `cfg235_oversized_incomplete_projection_preserves_incomplete_diagnostic` |
| Diagnostic-only overflow retains the first cause, deterministic ordering and explicit truncation, including an oversized UTF-8 causal record | `cfg235_diagnostics_only_overflow_preserves_first_cause_deterministically` |
| Trusted enabled package enters native parser, stays unresolved, no environment store/effects | `cfg235_enabled_python_package_is_locally_validated_without_preparation` |
| Missing enabled package is a precise static source failure | `cfg235_enabled_missing_python_package_is_a_precise_static_source_failure` |
| Malformed enabled package is a precise static source failure, redacted | `cfg235_enabled_malformed_python_package_is_a_precise_static_source_failure` |
| Missing requirements, invalid package name, symlink contract reused | `cfg235_python_local_contract_reuses_name_file_and_symlink_validation` |
| Disabled malformed content never enters package parser | `cfg235_disabled_malformed_python_package_remains_inert` |
| Unconfigured malformed content never enters package parser | `cfg235_unconfigured_malformed_python_package_remains_inert` |
| Untrusted enabled content never enters package parser | `cfg235_untrusted_python_package_contents_are_not_read` |
| Environment reference/literal credential and provider+MCP: valid, unresolved, exit 3, no credential lookup | `cfg235_provider_readiness_is_unresolved_without_credential_lookup` |
| Generated minimal config uses real parsing/analysis; optional project; no implicit trust or source preparation | `cfg235_minimal_init_validates_with_real_analysis_without_trust_or_sources` |
| Native Session startup without four explicit paths or Python/MCP | `cfg235_generated_init_launches_native_session_without_four_paths` |
| Explicit provider templates, deterministic bytes, no raw key | `cfg235_templates_are_explicit_and_deterministic` |
| Existing/preflight conflicts; no publication on conflict | `cfg235_preflight_conflict_writes_nothing_and_never_enters_publication` |
| Racing creator preserved; exact partial two-file outcome | `cfg235_racing_creator_is_not_overwritten_and_partial_result_is_honest` (channels) |
| Publication failure preserves published bytes and removes only private staging | `cfg235_failed_publication_preserves_prior_content_and_staged_bytes_are_not_published` |
| Failed/partial staging write cannot publish truncated config or damage existing data | `cfg235_partial_staging_write_failure_never_publishes_truncated_configuration` (injected writer) |
| Check/show zero process, connect, provider, Session, runtime/trust write, preparation, credential entries; valid/invalid/online/disabled/unconfigured/untrusted inputs; human/JSON/Debug redaction | `cfg235_static_check_show_have_zero_effects_and_redacted_outputs` (eight effect-owner counters) |
| Unknown field, forbidden override, missing instruction file, malformed TOML location and correction | `cfg235_diagnostics_keep_source_field_classification_and_correction` |
| Missing credential reference remains unread; explicit missing file invalid; malformed Workflow and native compiler failure; incomplete partial projection | `cfg235_incomplete_missing_explicit_workflow_and_credential_reference_states` |
| Same startup/show values and origins; exact exclusion; no live-Session claims | `cfg235_prospective_values_and_origins_equal_runtime_resolution` plus zero-effect test |
| Bounded structured output with honest omission | `cfg235_projection_has_a_structured_size_bound_without_changing_validity` |
| Schema drift | `cfg235_checked_in_schemas_match_authoritative_generation` |
| Minimal, workflow-basic and full reference use real native parsers/compiler | `cfg235_all_example_layers_use_real_resolver_and_workflow_compiler` |
| Exact command grammar, conflicts, repeated switches, flag-valued paths, help/exit text | `cfg235_finite_command_grammar_and_switch_values` |
| Real binary init/check/show framing, success/invalid/incomplete exits, repeated init, no runtime writes | `cfg235_binary_init_check_show_exit_and_machine_contract` |
| Plan-before-result framing; mixed skipped/unresolved outcomes without fake success | `cfg235_binary_doctor_discloses_plan_and_preserves_mixed_results` |
| Missing Python source does not block independent verified MCP; disabled/unconfigured skipped, untrusted unavailable, zero business calls | `cfg235_probe_verifies_mcp_without_business_calls_and_respects_inert_sources` |
| Deadline/cancellation await owner, not a detached waiter | `cfg235_timeout_and_cancel_await_physical_settlement` (oneshot gates) |
| Real stdio timeout/cancellation reaps supervisor before return | `cfg235_probe_stdio_timeout_and_cancel_reap_owned_process` (ownership pause, injected expiration, waitpid/ESRCH assertions) |
| Connection close must settle; cancellation during close; failing close is never verified | `cfg235_probe_close_is_awaited_and_failed_close_is_not_verified` (close gates; zero business calls) |
| Explicit preparation opt-in reaches existing Python owner; denied preparation inert; failed builds retire; no secret-bearing output | `cfg235_preparation_requires_authorization_and_uses_existing_python_owner` (existing store with recorded runner) |
| Probe credential success/failure and all renderers/Debug redact sentinel | `cfg235_probe_credential_use_and_failures_never_leak_values` |
| TUI opaque arguments, exit propagation and signal listener cleanup | `tui/test/configuration-command.test.ts` |

The existing CFG-01/02/03, native process ownership, Python build cancellation,
MCP settlement/recovery and Workflow conformance tests remain authoritative and
run in their existing CI partitions. Tests that previously waited for composition
to reject static resource errors now assert the same rejection at launch analysis;
no invalid reference has been made acceptable.

MCP and Managed Python selection uses [the shared ToolSource contract](tool-source-selection.md).
Definition/enablement is not Agent exposure; offline discovery is inert, and
only admitted demand enters native source preparation.

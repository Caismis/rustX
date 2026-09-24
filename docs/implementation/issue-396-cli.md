# Public CLI contract (issue #396)

Base audit: e8f700dae5b252e7cdf5b78a9e0000b05edeee04. Reviewed the three
parsers, serve/main, their callers and CI. Review correction rebased the existing
branch without conflicts onto da43450b77d3c195d95b06818be8142317663c91, including
merged PR #401 and its protocol v20, Trajectory, TUI and Product Host changes.

## Command/parameter matrix frozen before implementation

All value flags are optional unless marked required. Selection = `--model`,
`--config`, `--workspace`; process binding = `--runtime-root`.

| Command | Legal inputs | Conversion / native effects | Output / exit |
| --- | --- | --- | --- |
| default launch | selection, process binding, `--session`, `--node`, `--name`, `--inspect-conversation` | LaunchRequest; native resolution/composition; inspect is read-only | protocol stdout, diagnostics stderr; 0/1/2 |
| workflow check/explain | required id; selection, `--json` | WorkflowId + LaunchRequest; offline analysis only | report stdout; 2 invalid, 3 unresolved |
| config check | selection, process binding, `--json` | LaunchRequest; offline analysis | report stdout; 2/3 |
| config show | check fields + exactly one of `--sources` / `--agent` | prospective sources or agent inspection | report stdout; 2/3 |
| doctor | check fields + required `--probe`, optional `--prepare` requiring probe | native probe plan before effects; preparation only when permitted | reports stdout; 1 failure, 2 invalid, 3 unresolved |
| init | required template/provider/endpoint/credential-env; optional model-id/context-window/max-output/tool-calls/reasoning/compat/model-document; `--json` | typed declarations; native template rules, validation, create-only publication | report stdout; 0/2 |
| app-server | required `--listen`; optional config/runtime-root/token-file | distinct process request; native path/transport validation and composition | protocol stdout; diagnostics stderr; 0/2 (existing forced drain 3) |
| help | root, each nested command, `help <command>` | generated grammar only; no host capture or composition | stderr; 0 (1 on output failure) |
| internal --subagent-child | exactly this one token | inherited control channel; existing child runtime | IPC stdout, diagnostics stderr; existing child exits |

## Lexical decisions

The old launch/init/server loops accepted flag-looking values positionally,
rejected equals syntax, and had differing empty/duplicate checks. The old
switch remover could treat `--json` as a value; diagnostic error routing was a
separate token scan. Root help was stdout, server help stderr, nested help
mostly failed. These incidental differences are replaced together:

- The production entry uses `std::env::args_os()`: OS-native arguments reach
  fallible clap parsing without prior Unicode conversion. PathBuf values retain
  exact OS-native paths. Text fields require valid Unicode; invalid Unicode text
  returns the normal stderr-only lexical error (exit 2), without a pre-clap panic.
- Every public grammar uses clap; no alternate parser. No defaults manufacture
  model, workspace, Session, source, or transport intent.
- Both `--flag value` and `--flag=value` work. A dash-leading value requires
  equals syntax (`--workspace=--json`); `--workspace --json` is invalid.
- Empty lexical text/path values are rejected, but opaque non-empty values are
  never globally trimmed. Clap's inferred PathBuf parser preserves exact paths;
  NonEmptyStringValueParser preserves Init declarations, App Server listen and
  diagnostic agent names. Native owners validate their domains. Compatibility
  TOML may be explicitly empty, since an empty document is meaningful.
- Historical launch text normalization is explicitly scoped to conversion into
  LaunchRequest: model selection (also used by diagnostics), Session display
  name, Session/node/conversation identities trim surrounding whitespace and
  reject blank/invalid results. The lexical representation retains the original
  strings. Paths and Init/App Server strings never use this launch policy.
- Integers use u64 context-window and u32 max-output; booleans require explicit
  `true` or `false`. Domain validity (including positive limits) remains native.
- Duplicate options/switches, unknown flags/commands and extra positional values
  fail. `--` ends options; only the workflow ID positional can follow it.
- Node requires Session; inspection conflicts with Session/node/name. Removed
  `--continue` is simply unknown. Diagnostics cannot accept Session operations;
  workflow cannot accept runtime-root or probe/prepare.
- All lexical failures are stderr-only exit 2, including malformed commands
  containing `--json`. JSON is selected only by a successfully parsed diagnostic
  intent; native semantic failures still produce JSON reports. No token scan.
- All help is stderr-only and generated from the parser, with exit 0. No color.
- Empty init is now a missing-required-arguments lexical error (2), rather than
  an incomplete initialization report (3).

Native resolution, credentials, model validation, environment preparation,
Session/MCP effects and exit policy remain outside clap. Init custom/template
relationships and OpenAI compatibility requirements remain document policy.

## Ownership and architecture review

`Cli::try_parse_from` returns either explicit native `Command` intent, rendered
help, or a rendered lexical error. `serve::run_process` owns the destination
stream and exit class. `main` returns the ordinary process exit code. The existing
App Server emergency drain deadline/second-signal forced exit is unchanged;
parsing introduces no exit calls.

Selection groups convert directly to `LaunchRequest`. Init grammar converts to
`InitializationRequest` and native `Template`; `documents` retains template
relationships, explicit compatibility, credential-reference validation, model
validation and publication. App Server has a separate `AppServerArgs` converted
to native `process::Request`, with transport selection checked before composition.
No clap types enter Agent Loop, protocol DTOs, providers, history, or resolvers.

Review answers: lexical grammar only; native resolution/effects retained;
omission remains absent; one authoritative grammar; Init no longer reparses;
App Server no longer loops over argv; help/parse failures cannot use stdout;
help precedes host capture; static dispatch retains effect isolation; child
mode remains exact and private; obsolete tables/setters/help constants removed;
no lower-layer clap dependency; no generic framework; tests include production
`run_process` and spawned executable entry paths.

## Acceptance mapping

| ID | Concrete proof |
| --- | --- |
| CLI-01 | `cli::tests::cli01_grammar_is_consistent_and_every_command_converts`; `tests/process/configuration_commands.rs` public entry matrix and existing init/check/show/workflow/doctor process tests; App Server process tests |
| CLI-02 | `cli02_lexical_failures_and_cli03_finite_permissions`, `cli05_explicit_false_and_typed_init_values`, process stream matrix |
| CLI-03 | cli02 finite show/doctor/workflow tests; `cfg235_binary_doctor_discloses_plan_and_preserves_mixed_results`; native probe tests |
| CLI-04 | `cli04_omission_and_identity_conversion`; `process::tests::cli04_app_server_omitted_bindings_stay_absent`; unchanged launch resolver/prospective resolution tests |
| CLI-05 | cli05 typed booleans; `initialization::tests::cli05_native_custom_model_and_declaration_policy`; retained deterministic template, native launch and create-only/racing/staging failure tests; real init publication |
| CLI-06 | `cli06_equals_dash_values_and_terminator`; real public entry stream matrix with malformed nested JSON and JSON-as-path |
| CLI-07 | `cli01_cli02_cli06_cli07_public_entry_stream_matrix`, `cli07_cli08_help_precedes_host_capture_and_has_no_effects`; existing process startup/diagnostic tests |
| CLI-08 | `cli08_production_static_dispatch_has_zero_prohibited_effects` measures all 12 counters around production dispatch; `launch_tests` retains successful static inspection counter tests; real help and filesystem invariance |
| CLI-09 | `cli09_internal_child_rejects_all_extra_arguments`; existing subagent IPC suites; real App Server process suite, TUI real-child suite, Product Host browser acceptance |
| CLI-10 | source audit: no old flag tables, switch remover, setter helpers, raw Init argv or App Server parser loop; generated-help tests and this contract |

## Dependency evidence

Selected clap 4.6.7; explicit features `std`, `derive`, `help`, `usage`,
`error-context`; defaults disabled. License: MIT OR Apache-2.0. Its exact source
package's derive documentation and `clap_builder::Parser::try_parse_from` source
were inspected after consulting Context7. Exact version documentation:
https://docs.rs/clap/4.6.7/clap/_derive/index.html and
https://docs.rs/clap/4.6.7/clap/trait.Parser.html.

Cargo.lock adds only clap/clap_builder/clap_derive 4.6.7 and clap_lex 1.1.1.
All four declare Rust 1.85. Existing anstyle, heck, proc-macro2, quote and syn
are reused; no existing dependency was upgraded. The repository keeps MSRV
1.92 and `unsafe_code = deny`. `cargo +1.92 check --all-targets --all-features
--locked --target-dir target/msrv` passed locally on Linux.

A first local development binary build completed in 2m22s (including dependency
compilation and lock contention); its unstripped debug binary was 989,740,248
bytes. These are bounded observations, not a controlled before/after benchmark
or startup/performance claim. No release-size comparison was performed.

## Validation environment and recovered setup attempts

Linux x86_64; Rust 1.95.0 stable plus explicit Rust 1.92 validation; Node
24.21.0; pnpm 11.13.1. CI was reread, including the protocol, TUI, development
launcher and full Web lanes. The CI workflow itself is unchanged.

Initial `uv sync --frozen` / `uv run --frozen pytest` encountered a PyPI network
failure fetching Hatchling. `uv sync --frozen --offline` and `uv run --frozen
--offline pytest` succeeded using cache; the exact original online commands
then both passed (51 tests). The first Clippy attempt caught two introduced
style issues (numeric separators and a redundant let-return); both were fixed.
An accidental `pnpm typecheck` at repository root found no package manifest;
the command was rerun successfully from the intended package directories.
The first `pnpm test:e2e` could not find Docker; the supported
`CONTAINER_ENGINE=podman pnpm test:e2e` ran the same digest-pinned browser image
and passed all 82 tests. No test assertions or image references were relaxed.

macOS was not executed locally. Existing ignored/live-provider tests remain
ignored; no live-provider credentials or external model calls were used.

### Completed validation commands (original pre-rebase run)

Commands below run from the isolated worktree root unless a directory is shown.
All use the committed dependency lockfiles. Normal Rust validation uses Linux
stable; the additional MSRV command uses 1.92.

| Command | Result |
| --- | --- |
| `git diff --check` | pass |
| `cargo fmt --all -- --check` | pass |
| `cargo check --all-targets --all-features --locked` | pass |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | pass |
| `cargo build --bins --locked` | pass |
| `cargo +1.92 check --all-targets --all-features --locked --target-dir target/msrv` | pass |
| `cargo test --lib --all-features --locked local_runtime::cli::tests` | pass, 7 |
| `cargo test --test process --all-features --locked configuration_commands` | pass, 7 |
| `cargo test --lib --bins --examples --all-features --locked -- --skip boundary_suites::` | pass, 3,063; 2 existing ignored |
| `cargo test --test contracts --test provider --all-features --locked` | pass, 27 + 166; 5 live-provider tests ignored |
| `uv sync --frozen` (test-support/fake-provider) | pass after cache recovery |
| `uv run --frozen pytest` (test-support/fake-provider) | pass, 51 |
| `pnpm install --frozen-lockfile` (tui, dev, protocol/app-server, web-console) | pass in all four |
| `pnpm typecheck` (tui, dev, protocol/app-server, web-console) | pass in all four |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` (tui) | pass, 852 |
| `pnpm test` (dev) | pass, 37 |
| `pnpm check` (protocol/app-server) | pass; no generated drift |
| `pnpm test` (web-console) | pass, 906 |
| `pnpm check:provenance` (web-console) | pass |
| `pnpm build` (web-console) | pass; existing chunk-size warning |
| `CONTAINER_ENGINE=podman pnpm test:e2e` (web-console) | pass, 82; real native binary/Product Host/TUI |

Dependency inspection commands: `cargo info clap@4.6.0` (candidate),
`cargo info clap@4.6.7` (selected), `cargo fetch`, `cargo tree --locked -p clap
-e features`; all succeeded. Exact downloaded package manifests and source docs
were inspected. No unrelated dependency upgrades occurred.

Additional mandatory boundary gates:

- `cargo test --lib --all-features --locked -- boundary_suites::`: passed,
  225 tests, including real managed FastMCP tasks and child IPC.

- `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --locked --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output`: passed, 410 total (durable 129, process 55, subagent 43, tools 130, conformance 22, catalog 26, managed-output 5).

The original pre-rebase run had no unresolved validation failure. Local macOS execution, ignored live-provider tests and release/startup performance benchmarking were not performed. The implementation changes no platform gates.

## Review correction: exact values and current-main integration

Starting head: 810e000876c810342fed155f6864e0a966e53a19. Rebased implementation:
5fc617431af947791b159e75445041ff818e7174. Integration base:
da43450b77d3c195d95b06818be8142317663c91. No conflicts; no protocol v20 or newer
client/test behavior was reverted. No dependencies changed in this correction.

The shared `text_value`/`path_value` helpers incorrectly made lexical parsing
own normalization: they could change the filesystem object named by a path,
make an invalid credential environment name valid, or turn `" stdio "` into
`stdio`. Both helpers are removed. Exact typed CLI values now reach explicit
field/native conversion. Init still owns credential/template/model policy;
App Server still owns transport/path policy. Historical launch trimming remains
only in LaunchRequest conversion, verified against the pre-clap parser on main.

Pre-validation architecture review: clap owns only lexical grammar; opaque
values round-trip unchanged; credential names and transports cannot be repaired
by CLI parsing; paths are neither trimmed nor canonicalized; launch-only
normalization is explicit after parsing. There remains one public grammar,
with no fallback, flag tables, raw Init parser, App Server loop or JSON scan.

New regressions:

- `cli::tests::exact_paths_survive_public_cli_conversion`: all launch/diagnostic
  path fields and Init model-document, including leading/trailing/only spaces.
- `cli::tests::exact_init_strings_reach_native_credential_policy`: exact Init
  strings and native rejection of padded/blank credential names; exact agent.
- `cli::tests::exact_empty_values_are_rejected_without_blanket_whitespace_rejection`:
  genuinely empty values remain invalid.
- `cli::tests::launch_normalization_is_explicit_after_exact_lexical_parsing`:
  exact lexical strings followed by intentional launch normalization.
- App Server `app_server_paths_and_listen_survive_public_cli_conversion`: exact
  config/runtime-root/token paths and padded listen value.
- Process `configuration_commands::exact_values_reach_native_process_owners`:
  distinct real padded/unpadded configuration files, native credential rejection
  and native rejection of padded stdio, with stream and zero-publication checks.

The earlier validation results above describe the original implementation run;
post-rebase validation is recorded below.

### Post-rebase validation finding

`pnpm test` in web-console: 898 passed, 1 failed. The failure is
`test/settings-sensitive.test.tsx:120`, “S1-15 a Provider credential is never
read back from a shadowed definition”: after Escape, `getByLabelText('Endpoint')`
cannot find the field. `pnpm exec vitest run test/settings-sensitive.test.tsx`
reproduces it (5 passed, 1 failed). The test calls the mocked `cfg3Client` and
never invokes the Rust CLI. `git diff --exit-code origin/main -- web-console`
passes, confirming the entire Web source, fixtures, configuration and lockfile
are identical to the integration base. This existing frontend-only failure was
not repaired or hidden by the bounded CLI correction.

### Post-rebase validation commands

All commands run against the rebased implementation plus this correction on
Linux. The table distinguishes the new run from the original evidence above.

| Command (worktree root unless noted) | Result |
| --- | --- |
| `git diff --check` | Pass |
| `cargo fmt --all -- --check` | Pass |
| `cargo check --all-targets --all-features --locked` | Pass |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Pass |
| `cargo build --bins --locked` | Pass |
| `cargo +1.92 check --all-targets --all-features --locked --target-dir target/msrv` | Pass |
| `cargo test --lib --all-features --locked local_runtime::cli::tests` | Pass: 11 |
| `cargo test --test process --all-features --locked configuration_commands` | Pass: 8 |
| `cargo test --lib --bins --examples --all-features --locked -- --skip boundary_suites::` | Pass: 3,070; 2 existing ignored |
| `cargo test --test contracts --test provider --all-features --locked` | Pass: 28 contracts, 166 provider; 5 live-provider ignored |
| fake-provider: `uv sync --frozen`, `uv run --frozen pytest` | Pass: 51 |
| tui, dev, protocol/app-server, web-console: `pnpm install --frozen-lockfile`, `pnpm typecheck` | All pass |
| tui: `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | Pass: 852, including real child launch |
| dev: `pnpm test` | Pass: 37 |
| protocol/app-server: `pnpm check`, `pnpm typecheck` | Pass: protocol v20 generated fixtures have no drift |
| web-console: `pnpm test` | Fail: 898 passed, 1 unrelated Settings failure described above |
| web-console: `pnpm exec vitest run test/settings-sensitive.test.tsx` | Same failure: 5 passed, 1 failed |
| web-console: `pnpm check:provenance`, `pnpm build` | Pass |
| web-console: `CONTAINER_ENGINE=podman pnpm test:e2e` | 86 passed, 1 navigation interruption described below |
| web-console: `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh test/e2e/agent.spec.ts --grep 'composer primary seat, uploads and context stack light 390'` | Pass: 1 |

The browser failure was `page.evaluate: Execution context was destroyed, most
likely because of a navigation` in `screenshot.ts:46` while capturing the
composer fixture. Its targeted rerun passed with unchanged assertions and
references. All real-binary Product Host, App Server, TUI integration and
PR #401 Trajectory scenarios passed in the full browser run. The pinned
Playwright Linux image was used through the supported Podman override.

The first post-rebase in-crate boundary run passed 223 and failed 2:
`managed_selection::fastmcp4_availability_selection_request_and_invocation_share_one_authority`
and `runtime_client::python_capability::capability_projection_covers_python_origins`.
Both failed with native managed-Python `SourceUnavailable` / `source preparation
failed`. These tests call native capability preparation directly, without the
public CLI; their fixtures and capability code are unchanged from main.
The fresh full run passed all 225 tests, including both preparation failures
from the first run, without code or assertion changes. The precise cause of
the first preparation failures was not established; they are not hidden by
the successful rerun.

- `cargo test --lib --all-features --locked -- boundary_suites::`: rerun passed,
  225 tests. The initial 223/2 outcome is documented above.

- `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --locked --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output`: passed, 411 total (129 durable, 56 process, 43 subagent, 130 tools, 22 conformance, 26 catalog, 5 managed-output).

No macOS execution, ignored live-provider tests or performance benchmarks were
run locally. The remaining validation failure is the unrelated reproducible Web
Settings unit test documented above. All requested Rust gates passed on the
rebased correction; initial failures and reruns remain disclosed.


## Review correction: OS-native argv

Starting head: `9bbfee07d73950e5536d78e5787b671d17400d82`; base remains
`da43450b77d3c195d95b06818be8142317663c91`. The previous outer `env::args()`
could panic on non-Unicode Unix argv before clap saw a path. Main now uses
`args_os()`. `serve`, `run_process`, `parse_arguments` and `parse_command` accept
bounded `Into<OsString> + Clone` items. They never coerce the argv vector to
Unicode; the one fallible clap grammar still constructs typed native intent.

Only the private child discriminator examines raw argv before clap, using
OsStr equality: the exact singleton enters the inherited-control-channel mode;
the same token plus any other argument returns exit 2. Public help excludes it.
No protocol, semantic policy, dependency or launch-normalization change is made.

OS-native regressions (Unix argv; Linux where filename materialization is required):

- `cli::tests::os_argv_paths_survive_typed_conversion`: exact non-Unicode launch
  config/workspace/runtime-root and Init model-document PathBuf values.
- `configuration_commands::os_argv_non_unicode_model_document_selects_exact_file`: real
  binary opens the invalid-UTF-8 filename, not the deliberately invalid lossy
  sibling on Linux; normal JSON success/exit 0, exact published model, unchanged input
  workspace and absence of runtime storage are asserted.
- `configuration_commands::os_argv_non_unicode_text_is_a_lexical_failure`: real
  App Server text value fails via clap with invalid-UTF-8 diagnostic, empty
  stdout, exit 2 and unchanged filesystem.
- `configuration_commands::os_argv_private_child_discriminator_is_exact`: exact
  OsString child token reaches native control-channel validation; non-Unicode
  extras on either side are rejected, and generated help keeps the mode private.

The previous exact-value and CLI-01–CLI-10 assertions remain unchanged.


The initial config-file regression exposed a native source-manifest encoding
panic. The subsequent source-revision correction below fixes that defect and
restores the real non-Unicode config-file regression; it is no longer an
accepted limitation. The Init model-document regression remains in place.

### OS-native correction validation

Linux only. Current CI was inspected. No dependency or generated protocol changes.

| Command | Result |
| --- | --- |
| `git diff --check` | Pass |
| `cargo fmt --all -- --check` | Pass |
| `cargo check --all-targets --all-features --locked` | Pass |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Pass |
| `cargo build --bins --locked` | Pass |
| `cargo +1.92 check --all-targets --all-features --locked --target-dir target/msrv` | Pass |
| `cargo test --lib --all-features --locked local_runtime::cli::tests` | 12 passed |
| `cargo test --test process --all-features --locked os_argv_` | Final: 3 passed |
| `cargo test --test process --all-features --locked configuration_commands` | 11 passed |
| `cargo test --lib --bins --examples --all-features --locked -- --skip boundary_suites::` | 3,071 passed; 2 ignored |
| `cargo test --test contracts --test provider --all-features --locked` | 28 + 166 passed; 5 live-provider ignored |
| fake-provider: `uv sync --frozen`; `uv run --frozen pytest` | Pass; 51 tests |
| tui: `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 852 passed |
| protocol/app-server: `pnpm check`; `pnpm typecheck` | Pass; no generated drift |
| web-console: `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh test/e2e/dev-launcher.spec.ts test/e2e/workspaces.spec.ts` | 2 passed; real binaries/hosts |

The first new-process run exposed the native config-manifest defect (fixed below)
(2 passed, 1 failed). The first Init variant also needed its assertion corrected
for native-added default fields; the final assertion checks every declared model
field and the final run passed all 3. Existing assertions were not weakened.

Full Web unit/screenshot suites were not repeated for this bounded Rust entry
change; affected real-binary launcher cases were run. The previous local Web
Settings failure remains recorded in the earlier validation history; all seven
remote CI jobs on the starting head had since passed. No macOS or ignored live
provider tests were run locally for this correction.


- `cargo test --lib --all-features --locked -- boundary_suites::`: 225 passed.
- `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --locked --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output`: 414 passed (129 durable, 59 process, 43 subagent, 130 tools, 22 conformance, 26 catalog, 5 managed-output).

Final source review confirms: main uses args_os; no pre-clap Unicode/lossy argv
conversion; PathBuf values stay OS-native; text Unicode checks belong to clap;
child dispatch uses OsStr equality only; one public parser; no path normalization;
protocol stdout stays clean; help/error exits are unchanged; native semantic
owners and the prior PR architecture remain intact. All final executed gates
passed. The source-manifest defect discovered during this run is corrected below.


## Native source-revision correction

Starting head: `5e5bfcfaeb23d711db50d0b673a3fb375e7451ca`; base remains
`da43450b77d3c195d95b06818be8142317663c91`. OS-native argv and clap are unchanged.
The configuration owner now computes source revisions directly with SHA-256 over:

1. The fixed ASCII domain/version tag `rustx-source-manifest-v1`.
2. Entry count as unsigned 64-bit big-endian bytes.
3. For each entry in existing PathBuf BTreeMap order: path byte length (u64 BE),
   exact Unix OsStr bytes, revision byte length (u64 BE), revision UTF-8 bytes.

The internal encoding is infallible on supported Linux/macOS pointer widths,
independent of JSON representability, and unambiguous across fields and entries.
No lossy path identity, new dependency, source discovery or precedence change.
Identical manifests hash identically independent of insertion order; framing
prevents concatenation ambiguity before SHA-256. Normal cryptographic collision
properties still apply; this is not a mathematical claim of collision freedom.

Restoring `configuration_commands::os_argv_non_unicode_config_selects_exact_file`
also exposed path serialization in diagnostic provenance. The diagnostic owner
now uses the existing `projection_omitted` contract if a path-bearing report
cannot be represented in JSON: projections and file labels are absent, a
`projection_encoding` warning is present, and native validity/readiness/exit
classification and causal diagnostic text remain unchanged. No path is rewritten.
The Linux real process test proves valid exact-file selection (exit 3, empty stderr),
invalid lossy-sibling selection (exit 2), normal structured reports and unchanged
filesystem state. The non-Unicode source reaches source revision calculation.

Native tests in `configuration::source_manifest_tests` cover same-input
repeatability, distinct byte paths with equal lossy projections, revision changes,
field/entry boundary distinctions and reversed insertion order. Diagnostic test
`path_projection_tests::non_unicode_projection_preserves_native_failure_and_cause`
proves native failure classification and causal text survive partial projection.
All earlier OS-argv/model-document/text/child and exact-space tests remain.

Bounded path audit: launch config and workspace sources, and App Server config
when resolving Sessions, share this source-revision owner. Runtime-root bindings
are not source-manifest keys; workspace identity already uses exact Unix bytes.
Init model-document and App Server token-file are native file reads outside this
manifest. Diagnostic workspace/runtime-root/provenance paths use the bounded
projection behavior above. Both resource resolution and configuration application use this encoding; unrelated resource, protocol and filesystem identities were
not redesigned. No broader filesystem guarantee is claimed.


### Source-revision correction validation

Linux only; current CI was reread. No dependencies, protocol fixtures, frontend
source, CLI grammar or native resolution precedence changed.

| Command | Final result |
| --- | --- |
| `git diff --check` | Pass |
| `cargo fmt --all -- --check` | Pass |
| `cargo check --all-targets --all-features --locked` | Pass |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Pass |
| `cargo build --bins --locked` | Pass |
| `cargo +1.92 check --all-targets --all-features --locked --target-dir target/msrv` | Pass |
| `cargo test --lib --all-features --locked local_runtime::configuration::` | 8 passed, including both new manifest tests |
| `cargo test --lib --all-features --locked path_projection_tests` | 1 passed |
| `cargo test --lib --all-features --locked local_runtime::cli::tests` | 12 passed |
| `cargo test --test process --all-features --locked os_argv_non_unicode_config_selects_exact_file` | 1 passed |
| `cargo test --test process --all-features --locked configuration_commands` | 12 passed |
| `cargo test --lib --bins --examples --all-features --locked -- --skip boundary_suites::` | 3,074 passed; 2 ignored |
| `cargo test --test contracts --test provider --all-features --locked` | 28 + 166 passed; 5 live-provider ignored |
| fake-provider: `uv sync --frozen`; `uv run --frozen pytest` | Pass; 51 tests |
| tui: `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 852 passed |
| protocol/app-server: `pnpm check`; `pnpm typecheck` | Pass; no generated drift |
| web-console: `CONTAINER_ENGINE=podman bash scripts/browser-tests.sh --config .manifest-review.config.ts test/e2e/dev-launcher.spec.ts test/e2e/workspaces.spec.ts test/e2e/trajectory-integration.spec.ts` | 3 passed |

The default browser command initially could not start because port 5173 was
occupied. The successful run used a temporary config inheriting the repository
config and changing only preview/dev ports to 52831/52832 and baseURL accordingly.
Assertions and pinned browser image were unchanged. The temporary file was
removed; existing listeners and unrelated worktrees were untouched.

The first restored config regression exposed the additional diagnostic-projection
panic; it now passes with explicit partial projection as described above. Initial
Clippy documentation/function-length findings were corrected before the final run.
A bounded Python subprocess audit also exercised non-Unicode workspace and
runtime-root arguments through the real binary: both returned valid partial
JSON, exit 3, empty stderr, and no home/runtime publication.

No macOS, ignored live-provider tests or full Web unit/screenshot suites were run
locally for this correction. Affected real-binary configuration adoption and
launcher cases were run. Prior local Web findings remain historical evidence,
not changes included in this correction.


- `cargo test --lib --all-features --locked -- boundary_suites::`: 225 passed.
- `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --locked --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output`: 415 passed (129 durable, 60 process, 43 subagent, 130 tools, 22 conformance, 26 catalog, 5 managed-output).

Final source review: args_os and the single OS-native clap boundary are unchanged;
source identity uses exact Unix bytes, deterministic BTreeMap order and fixed-width
framing; JSON source-manifest serialization and its panic are removed. Restored
config-file and retained model-document/text/child process tests pass. Paths are
not normalized or converted to lossy identity. Diagnostic partial projection is
explicit and retains native outcomes. No unrelated architecture changed. All
final executed validation gates passed; the former config panic is fixed.


## Platform contract correction

Starting head: `b4a5e708aa81e3be2730e8984411300091a2e5cd`; main remains
`da43450b77d3c195d95b06818be8142317663c91`. The reviewed macOS job failed while
creating two invalid-UTF-8 filenames, before rustX launched (OS error 92).
OS-native argv representability does not imply filesystem filename representability.

The exact-file config and model-document regressions are now Linux-only and
retain all exact-byte/lossy-sibling assertions. Unix-wide typed conversion tests
need no filesystem access and continue asserting exact OsString/PathBuf equality.
The Unix real-binary test
`os_argv_non_unicode_missing_config_reaches_native_owner` creates only Unicode
fixture directories, then supplies a non-Unicode missing path as argv. It accepts
native invalid (2) or incomplete (3) classification according to filesystem
behavior, requires normal JSON stdout and empty stderr, preserves causal text,
and proves zero filesystem changes. A retained non-Unicode projection must use
`projection_omitted` and `projection_encoding`; early native rejection without a
path-bearing projection need not omit one. No platform error prose is asserted.
The existing path-projection unit test also runs on macOS without materializing
a filename and proves failure classification and causal text are retained.

The bounded audit found one additional configuration-application manifest hash
using JSON path keys (`input manifest`). It now calls the same exact-byte
`source_manifest_revision` helper as resource resolution. Linux native test
`settings_e2e::application_capture_preserves_non_unicode_source_identity` covers
repeatable application capture from an exact non-Unicode file. No new encoding,
parser, path abstraction, or configuration policy was introduced.

Audit of cli/configuration/diagnostics/initialization/App Server/serve and process
tests: production argv remains args_os -> clap -> native PathBuf; no lossy identity
conversion or pre-owner UTF-8 path coercion was added. Lossy conversions in these
changed tests deliberately construct/assert distinct display siblings, never
select the native source. Display formatting in existing errors is presentation,
not identity. Only physical invalid-byte filename fixtures use Linux gates;
text rejection, private child dispatch, typed conversion and projection tests
retain their Unix coverage. Native filesystem owners remain authoritative about
which paths are usable. No arbitrary-invalid-filename guarantee is made for macOS.


### Platform correction validation

Linux local results; macOS verification is owned by the new-head CI run.

| Command | Result |
| --- | --- |
| `git diff --check` | Pass |
| `cargo fmt --all -- --check` | Pass |
| `cargo check --all-targets --all-features --locked` | Pass |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | Pass |
| `cargo build --bins --locked` | Pass |
| `cargo +1.92 check --all-targets --all-features --locked --target-dir target/msrv` | Pass |
| `cargo test --lib --all-features --locked local_runtime::cli::tests` | 12 passed |
| `cargo test --lib --all-features --locked local_runtime::configuration::` | 8 passed |
| `cargo test --lib --all-features --locked application_capture_preserves_non_unicode_source_identity` | 1 passed |
| `cargo test --lib --all-features --locked path_projection_tests` | 1 passed |
| `cargo test --test process --all-features --locked os_argv_non_unicode_missing_config_reaches_native_owner` | 1 passed |
| `cargo test --test process --all-features --locked configuration_commands` | 13 passed |
| `cargo test --lib --bins --examples --all-features --locked -- --skip boundary_suites::` | 3,075 passed; 2 ignored |
| `cargo test --test contracts --test provider --all-features --locked` | 194 passed; 5 live-provider ignored |
| `cargo test --lib --all-features --locked -- boundary_suites::` | 225 passed |

Additional checks: fake-provider `uv sync --frozen` and `uv run --frozen pytest`
passed (51 tests); TUI `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` passed (852);
protocol/app-server `pnpm check` and `pnpm typecheck` passed with no fixture drift.
Check, Clippy and MSRV were rerun after the final test assertion change and passed.
The new missing-path test initially assumed exit 2; it was corrected to the native
invalid/incomplete distinction (2/3), and its final run passed. Existing exact-file
assertions were not weakened. Full Web suites and macOS were not run locally.
The prior macOS failure was confirmed from run 36070757610, job 107870904576;
macOS success must be verified on the pushed correction before declaring the
platform blocker resolved.


External gate:
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --locked --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output`
completed with 415 passed and one timeout:
`tools::mcp_managed::a_dependency_conflict_fails_only_its_own_managed_source`
at `tests/tools/mcp_managed.rs:1189` exceeded its 120-second preparation guard.
The fixture directly calls CapabilityCoordinator, without CLI parsing or the
configuration source-revision path. Its source and capability/Python owners are
unchanged. This unrelated liveness failure was not repaired or hidden.


The isolated rerun
`RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --test tools --all-features --locked a_dependency_conflict_fails_only_its_own_managed_source`
passed unchanged (1 test, 17.81 seconds). The original timeout remains recorded;
its underlying cause was not established. No assertions/timeouts were relaxed.

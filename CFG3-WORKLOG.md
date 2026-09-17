# Issue #332 implementation worklog

Status: **in progress; not a release specification or completion report**.

Base: `a105022cf0bbcd0cf7eab501a8f4f881a55b3db9`. Original worktree:
`/home/caismis/Documents/codes/rustX`, clean `main`. Implementation worktree:
`/home/caismis/Documents/codes/rustX-issue-332`, branch `issue-332-cfg3`.
The complete issue and its one comment (runtime layout/UUID amendment) were
read before editing. No implementation PR has been opened.

## Owner/contract map before implementation

| Domain | Current owner | Required disposition |
| --- | --- | --- |
| Authored configuration | `local_runtime/authoring.rs`, `config.rs`, `model/authoring.rs` | Replace split settings/catalog inputs with one strict partial rustx document; independently keyed Provider and Model domains. |
| Resource discovery | `agent_resources.rs`, `workflow_resources.rs`, `managed_python_resources.rs`, `resource_directory.rs`, `skills/{source,materialization,catalog}.rs` | Retain domain readers; converge two roots, shadow identities before parsing, preserve invalid winners with bounded diagnostics. |
| Semantic overlay | `authoring.rs` | Retain native typed owner; replace recursive record inheritance with explicit atomic domain replacement. |
| Effective resolution | `configuration.rs` / `UserConfigManager` | Replace split source bindings and trust capture; resolve current source captures together. |
| Credentials | `credentials.rs`, `model/catalog.rs`, MCP transport boundary | Retain bounded references and redacted resolved values; resolve only selected winning destinations. |
| Agent capability profiles | `config::AgentProfileDocument`, `runtime/agent_profile.rs`, `capabilities/selection.rs` | Retain profile resolution; Root and named profiles independently select capabilities; remove Session narrowing and caller Tool/Plugin ceilings. |
| Tool invocation policy | `tools/types.rs`, `config.rs`, `tools/native.rs` | Retain canonical policy; expose User/Workspace atomic policies separately from Agent selections. |
| External lifecycle | `capabilities/coordinator.rs`, `tools/mcp`, Python environment owners | Retain lifecycle and cancellation owners; replace enabled activation and eager child demand with admitted finite demand. |
| Runtime generation | `runtime/resources.rs`, `conversation_runtime::reload_resources`, `composition::LocalRuntimeResourceLoader` | Extend existing off-side candidate/coordinator publication boundary to model bindings, policy, and profiles; remove resource-only reload contract. |
| Session persistence | `session::SessionPersistentState`, `session_controller.rs` | Keep cwd and deliberate explicit model selection; delete configuration path and Tool/Skill narrowing; refuse obsolete durable schema. |
| Source authoring/CAS | `settings.rs`, `configuration/{settings,integrations}.rs` | Retain lock/revision/no-secret primitives; replace split writers and DTOs. Cooperating writer lock plus revision check must not be described as a filesystem CAS against arbitrary uncooperative editors. |
| App Server projection | `app_server/protocol.rs`, `runtime_client/settings.rs`, source settings | Replace V5 vocabulary and generated artifacts with one native CFG3 source/effective/reload contract. |
| TUI projection/control | `tui/src/commands`, `ui`, `app-server` | Replace obsolete trust/narrowing controls; call one reload; reconstruct observations on reconnect. |
| Web drafts/presentation | `web-console/src/app/settings` | Replace WEB-08/09 views with Effective/User/Workspace and structured editors, including named Agents. |
| Durable identity/storage | `session.rs`, `runtime/identity.rs`, `runtime/local_storage.rs`, durable stores/deletion | Keep Conversation SQLite ownership and locking; use typed UUIDv7 identity independently of explicit ordinal metadata; change topology and process root binding. |
| Managed Tool output | `tools/managed_output.rs`, background registry | Retain Conversation ownership and bounded canonical result; replace scanned spill sequence and execution-ID reseeding; collisions must never reclaim another identity's file. |

`session_runtime_manager.rs` remains the residency owner, not a second config
generation owner. `live_inspection.rs` remains routing/inspection only.
`app_server_policy.rs` has process budgets whose process ownership must remain
explicit. `schemas.rs` must generate from final types rather than maintain a
second authoring model. `initialization.rs` must cease writing two documents.

## Validation inventory

The current CI splits deterministic and boundary Rust suites, preserves macOS
filesystem/process coverage, and checks protocol generation, TUI, fake-provider,
Web/browser conformance, and `dev` launcher typechecking/tests. The user-requested
full command set remains required; add the current `dev` package checks.

## Delivery gates still outstanding

All phases B–I remain delivery gates until explicitly verified. In particular,
neither an isolated resolver change nor passing focused tests constitutes CFG3
delivery. No partial PR, compatibility bridge, or completion claim is authorized.

## Concrete seams identified for subsequent implementation

- `ConversationRuntime::reload_resources` acquires the lifecycle/reload gate,
  prepares outside the coordinator mutex, then commits capability and swaps
  `state.resources` under that mutex. It emits one `Resources` observation.
  This is the publication seam to extend; a separate model publication would
  permit mixed generations.
- `CapabilityCoordinatorConfig.extension_tools` currently sits outside
  reloadable inputs. Changing only the resource snapshot cannot implement
  reloadable Plugins: their actual Tool-plane owners must participate.
- `composition::admitted_source_demand` eagerly includes allowlisted named
  Agents and every discovered Workflow's dependencies. It must become
  admission-specific; merely filtering the final Tool list is insufficient.
- `SessionCatalog::allocate_ids`, lineage preparation/admission, and
  `native_successor` parse ordinals out of identity strings. UUID conversion
  must replace those contracts with explicit durable metadata, not just change
  formatting in constructors.
- `ManagedToolOutput::allocate_background_output` removes an existing path on
  its first collision. UUID allocation must remove that reclaim assumption and
  refuse/retry without deleting a different allocation's bytes.

## Implemented intermediate overlay changes

The existing typed overlay now replaces context, model timeout, Tool deadline,
capacity, and model-selection objects atomically. Empty identity maps no longer
clear other identities. Native Tool entries remain atomic per identity.
Defaulted members receive the winning complete object's provenance.

Nine focused CFG3 tests were added. Against the original resolver: three passed
and six failed. After the resolver change, all nine passed, together with the
six existing authoring tests (15 total). This is only an intermediate portion
of phase B: the split source schema, Provider/Model separation, Tool-selection
units and per-Plugin units still require replacement.

### Deterministic test coverage in this checkpoint

All new tests live in `src/local_runtime/authoring_cfg3_tests.rs` and invoke the
real typed overlay and resolver, without clocks, sleeps, or external services.

| Test | Contract |
| --- | --- |
| `absent_atomic_dimensions_inherit` | Omission retains the lower whole unit. |
| `model_selection_replacement_cannot_inherit_request_fields` | A higher model replaces reasoning, parameters, output limit, and summarizer selection; member provenance follows the winning object. |
| `incomplete_higher_model_selection_does_not_borrow_lower_identity` | An empty/incomplete higher selection fails rather than borrowing a lower model. |
| `context_replacement_uses_product_defaults_for_omitted_members` | Omitted context members default inside the winning object. |
| `timeout_and_deadline_objects_never_splice` | Timeout/deadline members and provenance do not come from the shadowed object. |
| `explicit_empty_policy_objects_reset_to_domain_defaults` | Explicit empty policy/capacity objects differ from absence. |
| `native_policy_is_atomic_per_tool` | Same-name native policy replacement drops lower execution/concurrency; unrelated Tool policy remains. |
| `empty_identity_maps_do_not_erase_unmentioned_identities` | Empty containers name no replacement identities; an explicitly empty variable value replaces that variable. |
| `mcp_destination_replacement_drops_lower_secrets_and_headers` | Higher MCP destination does not inherit lower headers or secret references. |

The source-authoring regression
`workspace_model_without_identity_stays_unresolved_until_reset` also proves that
an incomplete higher model source remains visible for repair without producing
a borrowed effective model; deleting that authored unit restores lower selection.

### Validation of this intermediate checkpoint

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `git diff --check` | Pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass |
| `cargo build --bins` | Pass |
| `cargo test --lib local_runtime::authoring --all-features` | 15 passed |
| `cargo test --lib local_runtime:: --all-features` | 439 passed |
| `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --workspace --all-targets --all-features` | 3,733 passed; 6 ignored |
| `cargo test --doc --workspace --all-features` | 9 passed |
| `test-support/fake-provider: uv sync --frozen` | Pass |
| `test-support/fake-provider: uv run --frozen pytest` | 51 passed |
| `pnpm install --frozen-lockfile` in protocol, TUI, Web, dev | Pass in all four packages |
| `protocol/app-server: pnpm typecheck` | Pass |
| `protocol/app-server: pnpm check` | Pass; no generated schema/TypeScript drift |
| `tui: pnpm typecheck` | Pass |
| `tui: RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 819 passed |
| `web-console: pnpm typecheck` | Pass |
| `web-console: pnpm test` | 299 passed |
| `web-console: pnpm check:provenance` | Pass |
| `web-console: pnpm build` | Pass; existing bundle-size advisory |
| `dev: pnpm typecheck` | Pass |
| `dev: pnpm test` | 30 passed |
| `web-console: pnpm test:e2e` | Not run |

These results do not establish the unimplemented CFG3 contracts. The old source,
trust, resource activation, Session, protocol, runtime-storage, and client
architectures are still present. No CFG3 browser screenshots exist. Linux checks
above are local results; macOS CI has not run on this branch. A second fetch
confirmed upstream is still the original base; no upstream integration was needed.

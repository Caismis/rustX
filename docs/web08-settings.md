# WEB-08 implementation and review guide

Starting `origin/main`: `e15b55e0e067352d1720d66e2be3243921d6ea21`.
Branch: `feat/web-08-settings`.

## Ownership and concurrency

The existing native `UserConfigManager` owns source authoring. User and trusted
Workspace are canonical partial `ModelLayer` values. Session is an intentional
whole `SessionModelConfig`; these types are not interchangeable. Omitted source
reasoning/output fields inherit, explicit `catalog_default` resets to the catalog,
and named profiles/limits are explicit. Saving an unrelated field preserves all
other omissions. `None` removes only the chosen model layer (or Session selection),
never materializes effective state, and preserves unrelated TOML settings.

`UserConfigManager::capture_sources` owns strict canonical parsing, source
path/authority selection, trusted Workspace activation, path rebasing, overlay,
Session whole-state application and provenance in `SourceCapture`. Structural
`RuntimeLayer::resolve` lowering performs no runtime-domain semantic validation.
`resolve_model_candidate` consumes that capture and validates the bound catalog,
canonical `analyze_session_model_config` result, context policy and model/context
budgets. Settings source reads/staged mutations and `resolve_model_configuration`
for Session selection use this same seam, with the already-captured trust decision.

Full `resolve_session` additionally calls `resolve_runtime_configuration`:
`CurrentRuntimeConfig::validate`, Tool deadline lowering and Tool environment
validation still enforce MCP, timeout, Agent, Workflow, Subagent and Skill policy
semantics. Only then does `resolve_resources` prepare launch resources. No runtime
checks are ignored or weakened, and no second merge/provenance engine exists.

Successful WEB-08 model projection means model/source configuration is valid for
the model domain. It does not claim the entire runtime configuration is admissible.
Full Session resolution remains the authority that validates the remaining runtime
domains before admission.

`TrustEpoch` coordinates the existing membership-directory trust store through a
persistent per-workspace lock beside the User state directory. Read-only analysis
does not initialize durable state or trust membership. CLI grant/revoke now delegates to that native owner.
Lock order: **workspace trust, then sorted/deduplicated source documents**. The
membership create/remove is the grant/revoke linearization point; Workspace atomic
rename is its publication point, under the same epoch. Revoke first means the save
fails untrusted without publishing. Save owning the epoch means publication may
finish before revoke. Read activity and Project-derived provenance use one epoch.
Untrusted Workspace bytes are neither parsed nor exposed.


Static launch/configuration analysis captures one atomic membership observation and
passes that immutable decision through both native phases without creating lock or
state files. Settings source reads and Workspace publication instead retain the
shared trust lock through projection/publication. This preserves static CLI
read-only behavior while source authoring and trust mutation remain serialized.

Source CAS holds native document locks across exact-byte revision validation,
staging/sync and atomic rename. User, Workspace, catalog and Session remain distinct
CAS domains. External observed edits invalidate revisions. Noncooperating editors
can still race the final fingerprint check/rename; no stronger filesystem
transaction guarantee is claimed.

The global Session catalog mutex never spans source filesystem work. Source reads
capture `(revision, settings)`, release the mutex, obtain the native projection on
a blocking worker, then check the revision once. A changed Session returns typed
`stale_settings`; there is no unbounded retry. A source mutation that committed but
cannot return a coherent combined projection reports committed uncertainty. It is
never replayed. Session selection captures and verifies the expected revision,
validates outside the catalog mutex on a blocking worker, then uses the **original**
revision in existing `replace_settings` CAS. Reset can reveal unconfigured lower
sources; it does not invent a default model.

Loaded runtime configured/effective requests and frozen attempt primary/summary
requests come from existing native snapshots. Source primary/summary projections
remain prospective. The UI shows reasoning, effective output, request parameters,
and summary policy so same-model divergence is visible. Detached/unloaded views
do not claim current loaded state. Saves do not reload/cancel work; live `/model`
ownership and admitted-attempt freezing remain unchanged.

## Protocol

- `settings/sourcesRead { session_id }` returns `source_settings` with native
  projection and the exact Session revision/selection used for resolution.
- `settings/sourcesWrite { session_id, expected_revision, mutation }` accepts
  `catalog { providers }`, `user_model { authored: ModelLayer | null }`, or
  `workspace_model { authored: ModelLayer | null }`. Only User can author Providers.
- `settings/selectModel` retains `selection: SessionModelConfig | null` and the
  existing Session revision, returning `settings_replaced`.
- `SelectionSource.selection` and whole-state source mutation payloads are removed.
  `SelectionSource.authored` is the canonical partial authoring contract. There are
  no legacy/V2 variants or compatibility paths. `ModelLayer::from_selection` is gone.
- `SourceSettings.effective_summary` adds native resolved summary request facts.
- `source_conflict`, `untrusted_workspace`, and `stale_settings` remain structured.
  Changed combined reads use `stale_settings`. Committed but unavailable readback
  uses existing `committed_durability_uncertain`. Neither side-effecting method is
  read-retriable or automatically replayed.

Rust types, generated v5 schema and TypeScript change together. Existing Web/TUI
request classifications remain correct and require no additional error variants.

## UI and source reuse

The Settings tab uses the WEB-07 shell and shared primitives. The Workspace settings
entry opens the editor instead of its old status placeholder. The chat composer is
hidden while editing Settings. Effective/provenance facts, source targets, revisions,
trust and apply lifetime are visible. Model selectors use native catalog entries.
Provider cards author endpoint, credential reference, model identities/protocols,
limits, explicit capabilities, reasoning profiles, request defaults and compatibility.
There are no native display-name fields, secret-write API or bounded discovery/probe
API, so these are not fabricated by Web Console.

A failed CAS retains drafts, refreshes authoritative state and permits only explicit
retry/discard. Lost responses are repaired by reads without replay. Inputs are kept
in component state, never browser persistence. Credential values are never returned;
existing literal credentials use a retention marker understood only by native code.

Pinned Harness source: `deepseek-ai/deepseek-harness@c291e7961a515f6d7af9304e7fd1d257929aef26`.
Inspected Settings/general/models/selection source, tests, CSS and docs are recorded
in `web-console/source-inventory.json`; presentation reuse and excluded semantic
owners are recorded in `web-console/PROVENANCE.md`.

![Settings and native provenance](images/web08-settings.png)
![User Provider catalog editor](images/web08-catalog.png)

## Deterministic regressions and linearization proof

| Test | Synchronization and exact ordering | Commit/observation point |
| --- | --- | --- |
| `revoke_wins_before_workspace_publication_authority` | Save pauses before trust acquisition; native revoke completes; save resumes and refuses without source bytes | Membership removal precedes attempted Workspace publication |
| `workspace_publication_owns_trust_until_after_commit` | Save pauses immediately before rename while owning epoch; an independent OS `try_lock` proves authority contention; revoke waits and completes after save | Atomic source rename under trust + document locks |
| `reads_keep_one_trust_epoch_across_grant_and_revoke` | Read pauses after capturing epoch; grant/revoke runs concurrently; read returns matching activity and provenance, next read observes mutation | Epoch acquisition serializes membership observation through projection |
| `competing_catalog_writers_have_one_publication_and_one_conflict` | Barrier starts writers using one revision; native lock serializes publication; one succeeds, one conflicts | Rename under exact revision/document lock |
| `web08_source_document_wait_releases_catalog_and_rejects_mixed_session_revision` | Test holds source document lock; worker reaches document boundary; unrelated catalog operation and same-Session revision commit finish before source release | Final Session revision check rejects old projection; no mixed result |
| `web08_selection_validation_releases_catalog_and_commits_with_original_cas` | Validation pauses outside catalog mutex; another Session operation and competing settings commit finish; validation resumes | Original expected revision in `replace_settings` rejects stale commit |
| `web08_source_commit_with_changed_session_is_uncertain_and_never_replayed` | Source pauses before rename; Session revision commits; source publishes once; combined readback refuses | Source rename succeeded; final Session check yields committed uncertainty |

All gates use channels/barriers or lock probes, never sleeps/timing assertions.

Additional regressions:

- Partial User request-only and Workspace policy-only round trips retain absent
  model/output/summary fields. Explicit catalog-default markers remain distinct
  from omission. Reset preserves unrelated document fields and reveals lower state.
- Native precedence/provenance and whole Session request override semantics remain
  covered, as do independent stale source and Session CAS protocol failures.
- Invalid Workflow blocks full Session preparation but not model projection/save.
- Invalid models/context budgets and invalid catalog definitions fail before commit.
- Literal secrets remain natively retained without projection readback; add/delete
  and external-edit conflict regressions remain intact.
- The gated admitted-attempt test keeps `local/a` unchanged while catalog profile,
  output budget and parameters change. Prospective requests show new values while
  loaded/frozen requests stay old; a cold Session sees the new catalog.
- Web tests assert exact partial payloads, default/omission distinctions, reset,
  backend options/provenance, explicit conflict retry, uncertain-write no-replay and
  same-model prospective/current/frozen request differences.
- Browser flow: Settings → Workspace output-only partial save → inherited identity
  and changed prospective output, unchanged loaded output → selection/reset → User
  catalog save/reread. Desktop/mobile screenshots and zero Provider requests.

## Validation

Final command results are recorded in PR #325. Required commands cover formatting,
Clippy, all Rust targets/features, doc tests, emulator pytest, frozen package
installs, Web typecheck/tests/provenance/build/browser tests, TUI typecheck/tests,
and protocol generation drift/typecheck. Focused owner and App Server race tests
are rerun independently. Browser plugin is unavailable; the repository Playwright
workflow exercises the real App Server and local fixture Provider.

## Executed validation inventory (model-domain review fix)

All commands below were rerun on the final model-domain correction. Existing
regression expectations were not weakened.

| Working directory | Command | Result |
| --- | --- | --- |
| Root | `cargo fmt --all -- --check` | Passed |
| Root | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| Root | `cargo build --bins` | Passed |
| Root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --workspace --all-targets --all-features` | 3,708 passed; 6 existing ignored |
| Root | `cargo test --doc --workspace --all-features` | 9 passed |
| Root | `cargo check --all-targets --all-features` | Passed |
| Root | `cargo test --lib configuration::settings --all-features` | 13 passed |
| Root | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib web08_ --all-features` | 5 passed |
| Root | `cargo test --lib model_domain_remains_usable --all-features` | 1 passed |
| Root | `git diff --check` | Passed |
| `test-support/fake-provider` | `uv sync --frozen`; `uv run --frozen pytest` | Passed; 51 tests |
| Web, TUI, protocol packages | `pnpm install --frozen-lockfile` | Passed in each package |
| `web-console` | `pnpm typecheck`; `pnpm test` | Passed; 282 tests |
| `web-console` | `pnpm check:provenance`; `pnpm build` | Passed; 73 source records, 100 notices |
| `web-console` | `pnpm test:e2e` | 10 passed; real server; desktop/mobile |
| `tui` | `pnpm typecheck`; `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | Passed; 816 tests |
| `protocol/app-server` | `pnpm check`; `pnpm typecheck` | Passed; no generated drift |

No environmental validation blocker. Local tests ran on Linux; macOS coverage is
provided by repository CI. The existing Vite large-bundle advisory is unchanged.

## Model-domain semantic isolation regression

`model_domain_remains_usable_with_invalid_runtime_workflow_semantics` uses
canonical authorized Workspace TOML with `agent.workflows = ['check', 'check']`.
Settings returns the User model, Workspace output limit and correct provenance.
A CAS Workspace output-only edit publishes without gaining a model identity or
other defaults. Whole Session selection validates through the model seam, while
full `resolve_session` still rejects duplicate Workflow identity before resource
preparation. `web08_source_and_session_cas_cross_the_real_protocol_boundary` now
runs with the same semantic error: real typed source reads/writes and whole Session
selection succeed, stale revisions still fail, and full resolution still rejects
it. This supplements the existing invalid Workflow resource regression; it does
not substitute malformed resources for a semantic configuration failure.

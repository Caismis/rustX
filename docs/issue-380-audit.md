# Issue #380: current-main audit

This is a Phase 0 engineering record, not an implementation or an acceptance
report. None of T01–T16 is claimed satisfied by this document.

## Baseline and isolation

- Issue: <https://github.com/Caismis/rustX/issues/380>, read in full.
- Fetched `origin/main`: `1c5a493a87c15b30874cc39bd4f676525db9a87e`.
- Branch: `issue-380-configuration-auto-apply`.
- Worktree: `/home/caismis/Documents/codes/rustX-issue-380`.
- Open PR inventory at audit start: empty.
- PR #376 merged at `d9453ce878f098dd717e1cd8663ca1135d7f147b`;
  issue #373 remains open. Its typed permission authoring and conservative
  response-loss handling must survive removal of its publication workflow.
- Repository `AGENTS.md` requires replacement of obsolete semantics, simple
  explicit composition, modular ownership, and use of existing dependencies.
- App Server protocol is **13**; Runtime Client protocol is separately **43**.
  The runtime configuration schema is separately **9**.

## Actual owners and contracts

| Concern | Current owner and evidence | Required convergence |
| --- | --- | --- |
| Typed source CAS | `configuration/settings.rs::UserConfigManager::write_source_settings`: document lock, byte hash, staged file, repeated external-edit checks, rename, directory sync | Preserve source CAS; serialize native commit/job identity and final publication using one coordinator rule |
| Source reads | `read_source_settings`: sorted document locks, source and resource-directory revisions checked before returning | Keep source projections separate from immutable preparation inputs; a diagnostic read is not a prepared candidate |
| Resolution | `configuration.rs::capture_layers`, `capture_sources`, `resolve_resources` | Capture one finite input manifest and all consuming bytes, validate stable ingestion, then prepare without rediscovery |
| Resource preparation | `composition.rs::LocalRuntimeResourceLoader::prepare` currently rereads configuration via `reload_configuration` | Take captured input from the native coordinator; do not let the loaded runtime or RPC own discovery/scheduling |
| Runtime publication | `ConversationRuntime::reload_configuration` sets `resource_reload_in_progress`, prepares, then commits capability/resources/model under `CoordinatorState` | Remove generic Reload and its preparation-long admission gate; publish independent complete units and commit adoption at the Session gate |
| Attempt capture | `RuntimeInner::publish_attempt`, shared by inbound and continuation admission | Preserve immutable model, policy, resources, and capability lease capture; compose applicable units before capture under the same gate |
| Session lifetime | `SessionCatalog`/`SessionController` own identity; runtime residency is owned by `SessionRuntimeManager` | Put process-local adopted binding and its revision in the Session domain, independent of `ResidentRuntime` |
| Loaded-runtime replacement | `SessionRuntimeManager::compose` calls `resolve_session` from current files every time | Reconstruct from the retained adopted descriptor; unload/load must not accept new context |
| Model selection | `settings/selectModel` writes durable selection; `session/setModel` changes loaded state through `model_set_with_persistence`; omission can re-resolve defaults | Converge mutation to one Session operation; resolve default once for Session initialization; reuse preparation/adoption foundations |
| App Server process policy | `app_server/process.rs` reads policy at startup; `AppServerHost` owns connection/attachment/request admission, manager owns residency | Report actual process bindings; classify concrete limits by their owner/lifetime, not by the `app_server` table name |

### Existing strengths to retain

`RuntimeResourceSnapshot` already combines configuration, selected capabilities,
project context, Skills, named Agents, Workflows, and managed Python inputs.
`AttemptModelSnapshot` already freezes primary and summary resolution. Attempt
admission acquires the lease for the captured capability snapshot explicitly.
Capability/resource observations publish both halves together, avoiding a client
projection that shows a schema from one generation beside another registry.

Managed Python already captures package source and uses content-addressed
`packages/<fingerprint>` directories. It verifies reuse, rejects corrupt
published state, and leaves the old fingerprint untouched. This is a useful
physical-isolation foundation, not something to replace with a mutable shared
environment. Candidate cleanup and cache retention still need distinct proofs.

### Concrete conflicts with automatic publication

1. `LoadedSources::pending_reload` compares source revisions, not effective
   semantic change or Session-relative request shape.
2. `reload_configuration` rejects a live Attempt and keeps new admission gated
   throughout preparation. Calling it automatically cannot meet T01/T06.
3. Reload can choose the newest default when `explicit_model` is false.
4. `source_settings` finishes source persistence before returning to the RPC,
   but there is no native reconciliation owner to receive the application job.
5. `SubagentRegistry::prepare` overwrites a frozen child's model timeout, Tool
   deadline, and context policy with mutable registry state just before spawn.
   `publish_configuration` mutates that state. The quiescent Reload gate masks
   the issue today; automatic publication would permit a child derived from an
   older Attempt to receive newer policy. Workflow Agent nodes use this same
   child path. Those values must instead travel with the parent capture.
6. `subagents.max_concurrent` governs a shared registry (`max_active` and capacity
   waiters), not a simple per-request policy. Reductions must not revoke existing
   reservations; its ownership cannot be hidden in a timeout snapshot.
7. Source revisions include bounded resource-tree hashes, but project instruction
   discovery and profile file reads occur separately. A tree revision is change
   detection, not by itself proof that all resolved bytes share an ingestion cut.

## Finite domain map for implementation

This map describes intended units grounded in current types. It is not a claim
that these units are implemented. Policy and capability splits must preserve a
complete registry plus its model-visible definitions at every commit.

| Unit | Actual fields/inputs | Scope and preparation | Comparison and adoption | Acceptance |
| --- | --- | --- | --- | --- |
| Independent execution policy | `approval_mode`, `model_timeout_policy`, `tool_deadline_policy`; invocation axes in `native_tools` and `mcp_tool_policies` | User/workspace overlay; native validation; clone complete registrations when invocation policy is registry-carried | Compare the actual compiled model Tool definitions, including any policy-dependent schema; auto-apply only proven-preserving complete policy | T01, T02, T03, T04, T09 |
| Capability closure | `agent.tools`, `agent.skills`, plugins, selected `agent.agents`/`agent.workflows`, `.agents/{mcp.toml,tools,agents,workflows,skills}`, effective Tool environment | Off-side complete registry, discovered definitions, guidance, dependent child/Workflow catalogs, MCP/Python and environment ownership | Definitions, ordering, guidance and leases move together; transport-only change can auto-apply against the Session's retained independent instructions if request shape is preserved | T03, T05, T06, T09, T14 |
| Instructions/context | Root `instructions`, project instruction chain and `agents_md`; context policy validated against retained selected models | Immutable text and ordered source capture; coupled capability guidance stays in capability closure | Compare real compiled system contributions; no synthetic history messages; changed/unproven prefix requires explicit adoption | T03, T04, T08, T10, T11 |
| Provider/model binding | `providers`, `models`, Session selected primary/summary model, reasoning, request parameters, compat, capabilities, output/context limits | Credential binding and invocation resolution off-side; default selection belongs to initialization, not every publication | Compare actual adapter-emitted contributions plus provider/cache namespace; retain existing Session selection while resolving definition changes | T04, T08, T09, T10, T15 |
| Shared capacity | `subagents.max_concurrent`; host connection/attachment limits; manager resident count/idle grace | Apply through the existing reservation/admission owner; preserve granted permits and count old owners | Not a model prefix contribution; must not masquerade as a per-Attempt immutable limit or force unrelated context adoption | T01, T06, T14 |
| Process bindings | CLI `--listen`, token-file transport setup, `--config`, `--runtime-root`; shutdown policy has a process drain owner | Startup binding or explicit existing owner; expose actual values separately | Restart-only only where no safe hot owner exists | T03, T15 |
| Client presentation | Browser/TUI display preferences | Client state only | No runtime generation or resource preparation | T09, T16 |

There is **no authored listen-address field** in `RuntimeLayer` at this baseline.
`--listen` is parsed by `app_server/process.rs`. T03 should test the actual
process-binding representation rather than invent a TOML key or classify all
global settings as restart-only.

`agent_id` and Root `description` are independently authored scalar units today.
They require explicit treatment in complete bindings: agent identity also reaches
child routing, and description may affect model-visible catalog metadata. Neither
should be assigned an impact merely from its mutation variant.

## Request/cache comparison boundary

All three current adapters must participate:

- `model/adapter/openai/chat_completions.rs::translate_request`: translated
  system messages, Tools, model, compat-selected output field, then opaque
  request parameters through `finalize_provider_request`.
- `model/adapter/openai/responses.rs::translate_request`: instructions, Tools,
  model, stored/stateless mode, output field and final request parameters;
  continuation affects input construction.
- `model/adapter/anthropic/mapping.rs::translate_request`: system blocks, Tools,
  model, output limit and final request parameters.

Use those construction paths for comparison evidence. Do not create a second
approximation that only hashes `agent.instructions`. Hold non-configuration
history/continuation inputs fixed when comparing; ordinary history growth is not
a configuration impact. Namespace evidence belongs with provider bindings and
adapter semantics. `ContextProjection::fingerprint` is not that namespace.

The common result needs `Preserved`, `PrefixChanged`, `CacheNamespaceChanged`,
and an explicit unproven result. Global candidate equality cannot substitute for
comparison against each Session's adopted binding.

## Required control-plane ordering

1. **Source commit/input capture:** native source serialization owns desired
   revision and application-attempt allocation. External modifications observed
   before CAS conflict. Rescan checks the finite manifest around capture and
   reports retryable instability rather than publishing a mixed capture.
2. **Preparation:** bounded active work and one latest pending request; immutable
   inputs; deadlines; candidate-exclusive ownership. No Session admission lock
   across file discovery, MCP connection, Python preparation or model resolution.
3. **Publication:** input revision, application attempt, scope, and relevant base
   identity checks, pointer update, and per-unit state update in the same native
   transaction. Newer failure cannot revive superseded work.
4. **Attempt capture:** applicable independent policy/resource units compose with
   that Session's adopted context under the execution gate. Descendants receive
   that captured value, including the child policies identified above.
5. **Adoption/model commit:** concrete inspected candidate plus expected Session
   revision, idle check and complete binding swap under the same gate as Attempt
   admission. Busy/NotReady/Conflict have no cancellation/history side effects.
6. **Notifications:** scope plus monotonic native version; read authority on
   reconnect and response loss, never replay mutations. Candidate readiness and
   Session adoption are separate facts.

## Obsolete surfaces identified

- Native `reload_configuration`, `RuntimeResourceReload*`, Runtime Client
  `ReloadConfiguration` commands/results/errors and advertised operation.
- App Server `configuration/reload` and `LoadedSources::pending_reload`.
- `settings_e2e.rs::cfg332_save_is_cas_only_reload_publishes_and_cold_resolution_rereads`.
- Runtime tests asserting that preparation blocks admission or that current-file
  load implicitly picks up the newest context. Preserve their useful whole-pair
  publication/lease/cleanup checks under the replacement contract.
- `web-console/src/app/settings/Settings.tsx` Reload handler/banner and
  `AgentControls.tsx` “Apply saved policy” publication action.
- TUI `AppServerSession::publishPermissions`, `reloadConfiguration`, permission
  Ctrl+P publication, `/reload`, and configuration error wording.
- `docs/configuration.md`, `effective-settings.md`, `web-settings.md`,
  `app-server-protocol.md`, `tui-app-server.md`, and acceptance documentation.
- Web settings fixtures, unit/browser tests, TUI settings/configuration-command
  tests, native process configuration tests and protocol fixtures.

These surfaces must be removed together after the native replacement works;
renaming the current Reload function to “reconcile” would preserve the defect.

## Deterministic acceptance work still required

No new acceptance tests have been implemented or run. This table identifies
proof boundaries, not completed test mappings.

| ID | Required controlled observation |
| --- | --- |
| T01 | Hold Attempt A in a model/Tool channel, save policy, admit independent B after settlement; compare captures |
| T02 | Publish while A is held, then release retry/recovery/Tool/child creation; inspect every derived capture, including child IPC |
| T03 | One captured desired input with independent policy/context/process differences; inspect simultaneous per-unit authority |
| T04 | Two Sessions on P1/P2, policy-only save; compare each resulting request and adopted binding |
| T05 | Hold instructions pending, publish equivalent-shape resource revision; inspect complete definitions/registry/leases |
| T06 | Hold resource preparation at a channel; prove admission completes and old physical resources remain usable |
| T07 | Park before final publication, commit newer source/job; release older success/failure/retry and prove rejection |
| T08 | Park capture and Session/model resolution separately; mutate each fence and prove no mixed candidate commit |
| T09 | Count generations, MCP connections and Python preparations across no-op/shadowed/unselected/presentation edits |
| T10 | Compare adapter-emitted configuration contributions under identical history inputs; test conservative uncertainty |
| T11 | Race admission/adoption at the shared gate using barriers; assert ordering, Busy/NotReady/Conflict and unchanged history |
| T12 | Disconnect after persisted commit at a hook; observe completion headlessly; reorder notifications and reconnect clients |
| T13 | Fail preparation once, retry same revision with new attempt identity; rescan changed files and unchanged healthy state |
| T14 | Hold active preparation while successive saves replace pending work; count acquisitions/releases and old leases |
| T15 | Unload/load/reconnect retained binding; separate process restart; edit defaults without replacing Session model |
| T16 | Headless authority fixtures exercised by Web/TUI; one Save intent, native adoption/retry, stale-state rejection |

## Generation and CI commands

Generation is `pnpm generate` in `protocol/app-server`, invoking
`cargo run --manifest-path ../../Cargo.toml --example generate_app_server_protocol`
then `node generate.mjs`. Version references live in the native protocol/schema,
generator, package check script, type contracts, client imports and handshake.
Generated files must be regenerated, never hand-edited. The next breaking
generation is 14 if main remains on 13; old v13 artifacts must then be removed.

Commands read from `.github/workflows/ci.yml` and package manifests:

```sh
# Repository root
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
git diff --check
cargo build --bins
cargo test --lib --bins --examples --all-features -- --skip boundary_suites::
cargo test --test contracts --test provider --all-features
cargo test --lib --all-features -- boundary_suites::
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --all-features --test durable --test process --test subagent --test tools --test conformance --test cfg3_catalog --test cfg3_managed_output

# test-support/fake-provider (before boundary/client integration tests)
uv sync --frozen
uv run --frozen pytest

# protocol/app-server
pnpm install --frozen-lockfile
pnpm check
pnpm typecheck

# tui (real rustx binary and emulator prepared first)
pnpm install --frozen-lockfile
pnpm typecheck
RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test

# dev (Web CI also validates launchers)
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test

# web-console
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
pnpm check:provenance
pnpm build
pnpm test:e2e
```

Web E2E uses the repository's digest-pinned Playwright container script; a host
browser is not an interchangeable validation environment. CI has a separate macOS
platform lane. Neither the above Rust/client tests nor macOS validation has been
run as part of this audit.

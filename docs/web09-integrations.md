# WEB-09 integration settings review guide

Base: `58f17e3c90f60275c3f58ebf9f9061dc0c5412e7` (WEB-08 PR #325).
Pinned presentation reference: DeepSeek Harness
`c291e7961a515f6d7af9304e7fd1d257929aef26`.

## Ownership and semantic domains

`UserConfigManager` remains the source authoring authority. `capture_layers`
extracts WEB-08's strict parsing, authority checks, path rebasing, named-map overlay
and provenance into one source capture before model or runtime validation.
`capture_sources` consumes this capture and binds the model catalog/Session model.
`resolve_mcp_candidate` consumes the same layer capture and the same
`config::resolve_mcp_bindings` used by full runtime validation. It requires neither
a model identity nor valid Workflow/Agent semantic configuration. Full
`resolve_session` still validates models, all runtime domains, then resources.
MCP-domain success never asserts full runtime admissibility.

`McpDraft` projects existing canonical transport fields and reference metadata.
Rust reconstructs the selected source entry, validates the complete candidate MCP
domain (including a shadowed User candidate in User authority), serializes typed
TOML, and publishes. Unknown fields/scopes and invalid transport combinations are
rejected. URL/header/environment validation belongs to the canonical MCP owner,
so launch and Settings have identical semantics.

`SourceMutation::Mcp` addresses an `IntegrationScope` (User or Workspace), exact
logical identity, and the enclosing Settings request's exact source revision.
There is no Session, Effective or private Workspace definition scope. Deletion
removes only that entry. Removing the final authored entry removes the source
map, restoring inheritance; it does not publish the empty-map clear operation.
Unrelated TOML fields and comments survive. Native named-map replacement makes
Workspace `foo` replace User `foo` as one whole entry; other identities coexist.
User and Workspace authored entries and native winning origin are returned
separately, with document/revision/activity in the enclosing source projection.
Untrusted Workspace bytes are neither parsed nor returned.

## Trust, CAS and publication

The original WEB-08 lock order is unchanged: workspace `TrustEpoch`, then
sorted/deduplicated document locks. Trust grant/revoke membership mutation and
source atomic rename are their respective linearization points. The epoch is
retained through projection/publication, including integration provenance.
Host cwd authorization is independent and does not create native trust.

User, Workspace, catalog and Session revisions remain independent. The App Server
captures Session settings/revision outside source work and rechecks after the
blocking worker returns. A stale source revision publishes nothing; a successful
source publication with unavailable coherent readback reports committed
uncertainty. Writes are never classified as read-retriable or automatically replayed.
The UI retains drafts on failure, rereads authority, and requires explicit retry
or discard. Connection-generation changes no longer remount Settings drafts.
Drafts are memory-only. Noncooperating external writers can still race the last
fingerprint check and rename; no stronger filesystem transaction is claimed.

## Security and secrets

The canonical parser rejects Workspace Provider/approval/native Tool/MCP Tool
policy authority and any sensitive MCP declaration, including empty tables.
Whole-entry replacement cannot retain User credentials when Workspace changes a
destination. Session controls use the closed existing `SessionPersistentState`;
they cannot author those policies or MCP definitions.

Sensitive maps accept validated `$ENV_VAR` references only. Credentials are not
captured or resolved by Settings. Ordinary environment/header values already on
disk are also withheld: the editor receives only key names and can retain/remove
them through a native marker. No plaintext credential write API, browser secret
store or secret viewer is introduced. An absent reference at runtime remains the
native preparation owner's diagnostic, not a claim inferred by the browser.

## Inventory and activation

The source projection separates authored User/Workspace selections, prospective
selection provenance, and static `CapabilityInspection` from loaded resource and
capability snapshots. A failed full static resource resolution marks that inventory
unavailable while leaving valid MCP-domain edits usable. It does not manufacture
partial admission or connect sources.

- Skills retain source policy, native resource/shadowing provenance, root hidden
  identities, Session explicit paths/no-automatic selection and admitted catalog.
- Named Agent resources use existing whole-resource discovery and shadowing;
  root `agent.agents` selection is separate source authoring.
- Workflow admission facts use the existing native catalog; `agent.workflows`
  selection never edits program bytes.
- Native extension controls address only Todo, Goal and Agent Status activation.
  Rust reads the exact same-scope collection, changes one member and preserves
  siblings. Across scopes it still replaces the collection as a whole. Reset
  removes that dimension. No current Todo/Goal state enters serialization.
- Tools/Python use native availability and exposed Tool facts. Session controls
  narrow via the existing vocabulary without changing User security ceilings.

No content editing is provided for Skills, Agents, Workflows, extensions, Tools
or Python. Resource diagnostics are safe native facts, never raw parser payloads.

## Lifetimes and presentation

Save means source commit for fresh/cold resolution. It does not reload or replace
the loaded runtime, open an MCP connection, prepare a Python environment, or
change an admitted attempt. Effective shows native loaded resource/capability
revisions and the attempt's frozen resource revision when available. Source
activation is explicitly prospective and is not connection proof.

No MCP-specific probe/reconnect control was added. The existing whole-resource
reload operation has broader launch-pinned lifecycle semantics; this page does
not relabel it as an MCP probe or promise it applies every source change.
The Product Host has no dedicated write-only credential API here. New credential
material and ordinary literal environment/header editing remain absent; native
reference authoring and retention/removal are supported.

Settings uses the WEB-08 shell with Provider / Models and Integrations navigation.
MCP drafts keep exact scope and identity fixed; Workspace trust and revisions are
visible. Harness presentation inspection/adaptation and excluded semantic owners
are recorded in the shared Web provenance files.

## Deterministic evidence

`configuration::integrations::tests::web09_*` covers source roundtrips, independent
CAS, whole entries, shadowed mutation, secret withholding, semantic isolation,
closed scope/security authority and extension collection semantics. Trust tests
pause before epoch acquisition or before publication using channels. A native OS
lock probe proves a publication owns the trust epoch; revoke waits for release.
The real App Server regression changes Session revision while the MCP source
writer is gated before rename, proves committed uncertainty, then repairs through
read and exact CAS. No sleep-based race assertions or live third-party MCP service
is used.

Frontend tests exercise exact target/payload, retained draft after authoritative
refresh, explicit retry/deletion, untrusted editing and bounded extension mutation.
Playwright exercises the real App Server with local fixtures: User MCP add,
Workspace replacement, shadowed User edit, external-edit conflict, explicit retry,
Workspace reset, and desktop/mobile presentation. Disabled fixture definitions
must remain inert throughout; saving makes no Provider request.

Validation results are recorded below; only completed commands are reported as passing.

User MCP Tool policy is a separate closed `mcp_policy` source mutation. It has no
scope field: only User may author `mcp_tool_policies`. Approval, execution and
concurrency use the existing canonical enums. Workspace MCP replacement does not
replace these policies. Reset removes only that User policy entry; dangling policy
references remain invalid under the shared MCP semantic validator.
Catalog semantic validity is reported independently (`catalog.valid`), so invalid
model limits do not block MCP-domain source editing. Strict source parsing and full
runtime admission remain unchanged.


## Validation record (Linux)

Validated against fetched `origin/main` at
`58f17e3c90f60275c3f58ebf9f9061dc0c5412e7`, unchanged on final fetch.
The broad Rust invocation covers CI's Linux contract and boundary targets with the
Provider emulator required. macOS execution remains CI coverage, not a local claim.

| Directory | Command | Final result |
| --- | --- | --- |
| repository | `cargo fmt --all -- --check` | Pass |
| repository | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass |
| repository | `cargo check --all-targets --all-features` | Pass |
| repository | `cargo check --lib --all-features` | Pass |
| repository | `cargo build --bins` | Pass |
| repository | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --workspace --all-targets --all-features` | 3,720 passed, 6 existing ignored, 0 failed across 14 targets |
| repository | `cargo test --doc --workspace --all-features` | 9 passed |
| repository | `cargo test --lib configuration::settings --all-features` | 13 passed |
| repository | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib web09_ --all-features` | 11 passed |
| repository | `cargo test --lib web09_ --all-features` | Initial focused run: 7 passed before additional coverage |
| repository | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 cargo test --lib web08_ --all-features` | 5 passed |
| repository | `git diff --check` | Pass |
| test-support/fake-provider | `uv sync --frozen` | Pass |
| test-support/fake-provider | `uv run --frozen pytest` | 51 passed |
| web-console | `pnpm install --frozen-lockfile` | Pass |
| web-console | `pnpm typecheck` | Pass |
| web-console | `pnpm test` | 288 passed in 21 files |
| web-console | `pnpm check:provenance` | Pass: 74 records, notices for 100 production packages |
| web-console | `pnpm build` | Pass; existing bundle-size advisory remains |
| web-console | `pnpm test:e2e` | 11 passed, including real-server WEB-09 desktop/mobile flow |
| tui | `pnpm install --frozen-lockfile` | Pass |
| tui | `pnpm typecheck` | Pass |
| tui | `RUSTX_REQUIRE_PROVIDER_EMULATOR=1 pnpm test` | 816 passed |
| protocol/app-server | `pnpm install --frozen-lockfile` | Pass |
| protocol/app-server | `pnpm generate` | Pass; Rust-generated schema and TypeScript staged together |
| protocol/app-server | `pnpm check` | Pass; no generated drift |
| protocol/app-server | `pnpm typecheck` | Pass |

Development failures were repaired before final validation: an attempted root
`pnpm install --frozen-lockfile` had no package manifest (rerun in package
directories); initial Rust compilation/lint failures were corrected; a full Rust
run caught the canonical empty-URL diagnostic regression (fixed in its owner);
browser/unit tests caught an ambiguous argument label and duplicate child reset
keys (fixed in the UI). No test was disabled or weakened. Intermediate focused
and full Web runs were repeated after fixes; the table records final counts.

# CFG3 residue audit for PR #333

This is a **negative-contract audit**, not configuration instructions. Obsolete
spellings below identify rejected or historical behavior. The current contract is
[CFG3 configuration](configuration.md), including its exact overlay matrix.

The cleanup started at `69d644ab7e5d4adcc8b61bbd08ee9be6ec28d94e`, with a clean
`issue-332-cfg3` worktree and `main` at
`a105022cf0bbcd0cf7eab501a8f4f881a55b3db9`.

## Search scope and disposition

The tracked-tree audit covered `settings.toml`, `models.toml`, `--user-settings`,
`--models`, `--skill`, `--no-direct-tools`, `--no-builtin-tools`,
`no_direct_tools`, `no_builtin_tools`, `no_automatic_skills`, `skill_paths`,
`exclude_tools`, `skills.sources`, `[skills].sources`, `disabled_skills`,
`~/.agents`, global Skill roots, trusted/untrusted Workspace, `TrustAction`,
`TrustEpoch`, MCP enabled, enabled source, source enablement/activation/disabling,
`resources/reload`, resource reload, `XDG_CONFIG_HOME`, `.config/rustx`, v5,
removed approval controls, `agent.extensions`, and host-only policy claims.
Case-insensitive searches cover capitalization differences.

Current product and maintainer prose was corrected in README, DEVELOPMENT,
architecture/invariants/development-plan, App Server acceptance/protocol,
process-death and Workflow documentation, TUI README, and Web CHAT, COMPOSER,
CONFORMANCE, DOGFOODING, PROVENANCE, README and WORKSPACES. The specialized-Agent
example now describes admitted demand. The focused CFG3 references were inspected:
configuration, launch-configuration, configuration-diagnostics, runtime-resources,
source-activation, agent-profiles, subagent-resources, tool-source-selection,
capability-inspection, effective-settings, tui-app-server and web-settings.
`source-activation.md` is a retained link pathname; its content documents demand,
not an activation switch.

Current documentation teaches only two fixed resource scopes, User < Workspace
typed overlay, explicit Root capability selection, independent named profiles,
default-off Plugins, Skill prompt visibility, inert discovery, Save versus Reload,
one coherent publication, current-file cold composition, Session `cwd` plus optional
model, and Conversation-owned UUIDv7 runtime storage. App Server is v6 and exposes
`configuration/effective`, `configuration/sourcesRead`,
`configuration/sourceWrite`, and `configuration/reload`.

## Intentional remaining search hits

| Category | Paths | Why retained |
| --- | --- | --- |
| C: rejection/inertness | `src/local_runtime/authoring.rs`, `cli.rs`, `config.rs`, `initialization.rs`, `launch_tests.rs`, `session_controller.rs` | Old fields/flags/files are inputs to rejection or inertness tests, or assertions that initialization does not create obsolete files. The static-effect test also supplies invalid old MCP objects to prove inspection performs no effects. |
| C: rejection/inertness | `src/skills/package.rs`, `tests/cfg3_catalog.rs`, `tests/contracts/runtime_examples.rs`, `tests/process/runtime_process.rs` | Reject old Session fields/CLI input, ignore old files and Skill paths, and assert examples do not use obsolete roots. |
| C: guard vocabulary | `tests/contracts/documentation.rs` | Explicit forbidden strings and positive/negative guard tests. They are not accepted configuration. |
| D: historical | `web-console/VALIDATION.md`, `docs/tui-dogfooding-267.md` | Entire files are labeled historical pre-CFG3 records and link current instructions. Old approval/trust evidence is not current behavior. |
| D: removal record | `CFG3-WORKLOG.md`; historical sections of `web-console/CONFORMANCE.md`; version history in `docs/architecture.md` | Explicitly describes deleted architecture or dated protocol history. No compatibility reader is advertised. |
| D: negative inventory | This file | Records the exact obsolete vocabulary and its disposition. |
| E: emulator assertion | `test-support/fake-provider/README.md`, `src/fake_provider/scenario.py`, `src/fake_provider/scenarios/{conformance,tui}.py`, `tests/test_scenario.py` beneath that package | `no_direct_tools` asserts that a captured provider request contains zero Tools; it is not rustX authoring or Session intent. |
| E: local test helper | `tests/conformance/agent_loop.rs` | `skill_paths` is a local test setup field, not a serialized Session/configuration field. |
| E: local test names/values | `tests/scripted/tools/native_contracts.rs`, `tests/subagent/overrides.rs` | `no_direct_tools` names an empty selection in a test; no CLI or source reader exists for it. |
| E: environment sanitation | `tests/process/app_server.rs`, `tui/test/integration.test.ts` | Removes ambient `XDG_CONFIG_HOME` from fixture subprocess environments; does not discover configuration there. |
| E: other domains | Workflow admission documentation/tests; process lifecycle; Host routing | Workflow Enabled/Disabled is static program admission, process activation is lifecycle, and Host navigation authorization is routing. None activates MCP or gates Workspace configuration. |

No remaining old-term hit supplies CFG2 runtime compatibility authority. Internal
`RuntimeResourceReload*` names describe the existing implementation primitive;
there is only one public configuration reload operation.

## Maintainer ownership corrections

`CurrentRuntimeConfig` documents generation-owned User < Workspace approval,
context, model timeout, Tool deadline, per-Tool/per-source invocation policies,
environment variables and child capacity. MCP secrets belong to the complete
winning definition. Capacity can change only at the quiescent publication boundary.
Process bindings and User-only `app_server` policy remain process-owned.

Skill package documentation now describes fixed roots and shadow-before-parse,
not removed explicit paths or global precedence. Plugin comments distinguish
stable conversation domain owners from per-generation explicit capability
selection. Native Tool whitelist and Plugin composition are separate. Reload
comments and client-facing diagnostics use configuration publication vocabulary.

## Examples, fixtures and schemas

The final example tree is `examples/local-runtime/rustx.toml` plus `.agents/`
and bounded CFG3 subexamples. `examples/cfg2/`, the old minimal/settings/model
examples and public `settings.schema.json`/`models.schema.json` were already absent
at the starting head; no live obsolete example was retained. The four public
structural schemas are `rustx`, `agent`, `mcp` and `workflow`. App Server's generated
schema/TypeScript remain v6. Generated descriptions were regenerated from Rust.

The active Runtime Client fixture was replaced by `plugins.json`, retaining only
its tested Plugin projection states. Unused old `default_save` request/result and
launch/approval lifetime objects were removed with `settings-v26.json`. The old
fixture is not retained as a reader or alias.

Composer tests now prohibit the actual CFG3 source-write/reload methods when
operating Todo/Goal/Queue state, replacing checks for removed settings methods.

## Permanent guard and validation

`cargo test --test contracts documentation --all-features` scans current root,
docs, examples, TUI and Web Markdown/source examples using a small explicit string
list. Exact file exceptions are this negative inventory and the two labeled
historical records above. Rust comments and negative Rust fixtures are outside
that product-document scan. This test runs in the existing Linux contracts CI lane.

The starting head's CI exposed two unrelated validation defects: newer Clippy
rejected a redundant borrow, and macOS stalled at the reload test's publication
gate because its temporary Workspace alias differed from the canonical candidate
path. The borrow was removed; the test-only gate now canonicalizes its key. A
symlink-alias regression uses the actual gate and channel handshake, with a timeout
only as a liveness bound. Runtime reload semantics were not changed.

The first cleanup-head macOS run passed both reload tests and then exposed two
Python capability fixture failures: the fixture passed an uncanonicalized
temporary Workspace into discovery. The fixture now binds its canonical Workspace
before writes and discovery, matching production. A deterministic parent-symlink
regression checks the same fixture helper and verifies Python identity discovery
without materialization. This changes test setup, not runtime path policy.

Full local validation and exact pushed-head GitHub Actions results are reported
with the PR update. Local success is not evidence of GitHub CI success.

# Generation capability inspection

A published resource generation carries one immutable `CapabilityInspection`.
Agent Profile resolution, Skill discovery, ToolSource lifecycle and Workflow
admission own its typed facts. The resource candidate composes those facts in
`PreparedRuntimeResources::into_parts`, before capability commit. The runtime
publishes resources and their diagnostics together under the existing resource
reload gate/state lock and emits one `ResourceGenerationUpdated` observation.
Failed candidates publish nothing. Retained attempts, children, Tool sets,
Skill bindings and Workflow runs continue using their original generation.

A child uses the exact `ResolvedSubagentSpec` frozen by its invoking generation.
The selected-only materializer realizes its Tool definitions and verifies its
Skill bindings. Before commit, `PreparedCapabilityCandidate::with_frozen_child`
installs the native child profile and materialized Skill metadata/provenance.
Execution, the child resource snapshot, and inspection consume that same
capability snapshot. The child does not rediscover Skills or reread an Agent
file. Source selection intent and suppression cross as frozen facts; they
never authorize additional Tools. Child IPC version 24 carries these facts.

`config show` requires exactly one of `--sources` or `--agent`; combining them
or omitting the target is a typed CLI error.

`RuntimeResourceSnapshot::inspection()` reads the frozen projection. Runtime
Client protocol 33 copies it into `resources.inspection`; the TUI's effective
settings view renders those facts. Neither client resolves capabilities.

```sh
rustx config show --agent main --json
rustx config show --agent reviewer --json
rustx config show --sources --json
rustx workflow check review --json
rustx workflow explain review --json
```

Offline launch analysis constructs a prospective snapshot through the same
Agent resolver and Workflow admission traversal using native leaf metadata.
External sources remain unprepared, disabled or authority-rejected. No source
preparation, install, connection, credential capture, model call or execution
occurs. `enabled` means static dependencies were admitted in that snapshot;
it does not grant invocation approval or establish provider connectivity.

Agent inspection retains typed Tool selection intent (including an `all` source
that currently publishes no Tools) and lists active Tools with origins, active Skills with global,
workspace or explicit provenance, disabled Skill selections, child/Workflow
selections, native Extension state and typed suppression diagnostics. Missing
Agent capabilities suppress only their selection. A Workflow with one failed
static dependency is disabled atomically, omitted from model-facing exposure,
and non-executable; inspection reports the owner's node path and typed reason.

Source state preserves disabled, unconfigured, untrusted, unprepared, ready and
unavailable distinctions. A missing exact Tool from a ready source is distinct
from an unavailable source. Skill diagnostics retain invalid-package exclusion,
same-scope duplicate exclusion, workspace shadowing, missing/invalid roots and
source provenance. Missing root `disabled_skills` targets are Agent diagnostics.
Facts are deterministically ordered and never appended per model turn.

The wire projection contains identities and bounded provenance, no raw source
configuration, model parameters, credentials, instructions or Skill bodies.
External preparation/materialization errors and Skill parser details may contain
sensitive text; serialization preserves typed causes and omits those payloads.
Existing configuration redaction continues to protect config show output.

`--no-direct-tools` removes ordinary direct Tool exposure from main, leaving
independently selected Agent/Workflow dispatch and Extensions intact.
`--no-builtin-tools` restricts only ordinary built-ins. `--tools` is an exact
allowlist within admitted direct Tools; `--exclude-tools` subtracts from that
direct plane. Neither changes the independent `agent.agents`, `agent.workflows`,
or native Extension composition. `--no-automatic-skills`
disables automatic Skill roots while explicit `--skill` inputs retain their
separate launch authority. There are no aliases for the obsolete flag names.

See the [complete CFG2 example](../examples/cfg2/README.md) for source eligibility,
Agent selection, Workflow authority, Skill provenance and structured TOML
`request_params` together.

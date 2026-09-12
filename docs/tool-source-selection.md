# Ordinary Tool source selection

MCP configuration and canonical Managed Python packages define two concrete kinds
of `ToolSourceId`: a configured MCP identity such as `github`, and a Managed
Python identity such as `python:data-analysis`. Namespace parsing happens at the
strict authoring boundary. Resolution and materialization dispatch use typed
identities, never display names or prefix matching.

Agents and Workflow Agent overrides share one selection document:

```toml
[tools]
builtin = ["read", "grep"]

[tools.sources]
github = "all"
"python:data-analysis" = ["run_python", "inspect_dataframe"]
```

The main runtime configuration accepts the same `[tools]` document. When it is
absent, existing native defaults apply, with no external source exposure. An
explicit document replaces that main selection. Named Agent discovery does not
select every discovered Agent; existing main/Workflow Agent admission lists
remain responsible for which profiles create demand in this issue.

`All` is coarse source trust: every eligible ordinary Tool published by that
exact source in the admitted resource generation. `Exact` is fine-grained Tool
trust: only the source-qualified names in the array. Arrays reject malformed and
duplicate names. There are no wildcard, exclusion, inheritance, or alternate
Python selectors. Native Tools remain an explicit list. Extension-provided Tools
belong solely to native Agent Extension composition and cannot be selected
through either `builtin` or `sources`.

A Workflow Tool node selects one exact leaf with `origin: source`, `source_id`,
and `name`; its Agent override uses the shared `tools.sources` map. A leaf cannot
execute a source-wide `All` selection as if it were one Tool.

## Demand and ownership

Source definition/discovery is distinct from enabled/trusted eligibility,
materialization, Agent exposure, frozen admission, and invocation approval.
Selecting a source cannot define it, enable a disabled MCP server, grant host
trust, or bypass approval policy.

The composition owner collects finite demand from main selection, admitted
named Agent profiles, and admitted Workflow references. A sorted set coalesces
multiple references to one semantic source. The existing capability coordinator
prepares one off-side candidate:

- Enabled/trusted MCP definitions are connected only when demanded, by the
  existing MCP connection and generation owner.
- `.agents/tools/<package>` discovery records inert identities and paths.
  Only demanded discovered packages enter the existing `PythonToolStore`
  preparation owner. Unreferenced package code and dependencies are not parsed;
  no environment, uv operation, credentials, import, or process is requested.
- Python's prepared execution binding may use MCP internally. Its published
  ordinary Tool provenance remains Managed Python. This does not create a
  generic plugin runtime or a second connection manager.

Materialization alone does not expose Tools to main. Each Agent/Workflow resolves
its own source selection against the committed available catalog. Same-name
Tools from different sources never substitute for each other. Source-local
failures remain bounded and isolated from unrelated sources.

## Frozen generations and children

The existing resource publication owner commits capability and resource state
atomically. Selection uses only the immutable generation admitted to that
attempt or Workflow. R1 publishing `{a,b}` freezes `All` to `{a,b}`. R2 publishing
`{a,b,c}` lets a future `All` admission select `{a,b,c}` while R1 stays unchanged.
`Exact([a])` stays `{a}` in both generations.

Child plans contain the finite selected source Tools and their canonical
cross-process identities. They do not contain an instruction to expand `All`
again. Child materialization connects only the frozen bindings and verifies each
expected identity before constructing an executor. Missing or changed definitions
fail preparation; a later same-name Tool cannot replace a parent-authorized one.

## Resolution facts and policy

`resolve_source` returns typed facts: undefined source; inactive source with its
actual disabled/untrusted decision; unprepared source; materialization unavailable
with a bounded reason; or a ready source with selected definitions and missing
Exact names. Offline checks retain unprepared facts and never invent online Tool
metadata. Agent warning/suppression policy and atomic Workflow disable policy
remain owned by CFG2-04 and CFG2-05 respectively.

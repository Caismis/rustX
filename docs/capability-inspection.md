# Native capability inspection

Rust projects the same frozen profile resolution used by admission. The browser and
TUI do not infer availability from filenames or parse configuration. See
[CFG3 configuration](configuration.md) for the ownership contract.

A generation projection distinguishes Root-selected capabilities from merely defined
resources, and availability from preparation state. Every resource family exposes
its winning scope/path, lower shadowed path and validity. Invalid higher duplicates
remain visible as invalid identities; they never restore lower definitions.
Diagnostics are bounded, ordered and redacted.

Each resource diagnostic names its owner as a native `subject`: one resource
identity (`resource`, with family and name) or a family's source document or
collection as a whole (`collection`). Native attributes it from the catalog entry
keyed by that identity, never from a field path or from a file several identities
share — two MCP definitions in one `mcp.toml` are two owners, and a document that
does not parse is the family's diagnostic, not any identity's. `field` only locates
the failure inside `file`. Clients render a diagnostic on exactly the subject it
names and never infer another.

Preparation and admission are native observations. An MCP or Managed Python
identity with no `sources` entry, or a Workflow with no `workflows` entry, is
unobserved — not unprepared, unavailable or disabled — and the source-authoring
inventory publishes none of them, because it prepares and admits nothing.

Named-Agent inspection uses that Agent's complete profile. Omitted model selection
means invoking Attempt inheritance, not a lookup of current Root defaults. Root
Tool/Plugin selection is not a child ceiling. Skill selection describes prompt
visibility with resolved User and Workspace roots.

`session/effectiveConfiguration` describes a loaded published generation. Source reads and
`rustx config show --sources` describe current prospective resolution and explicitly
remain distinct. Admitted Attempt generation/model/resources can differ from current
published defaults. Clients render those native facts without recomputing them.

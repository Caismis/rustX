# Web Settings

Web owns forms, drafts and presentation. Rust owns parsing, semantic validation,
serialization, overlays, provenance, shadowing, CAS and runtime publication.
The [CFG3 reference](configuration.md) is the configuration contract.

The primary tabs are **Effective | User | Workspace**. Effective is read-only and
shows native values, source origins, selected/defined resources, readiness, generation
and explicit Session model. The User tab edits the bound User document and User
resources; the Workspace tab edits its partial document and resources. The actual
User config pathname is displayed separately from fixed `~/rustx/.agents`.

Structured groups cover Runtime General, Providers & Models, Tool Policies, Root
Model/Tools/Skills/Plugins/Agents & Workflows, and MCP/Python/Skill/Agent/Workflow
resources. Provider and Model edits address independent identities. Workspace
replacement edits complete objects. Native Tools use an explicit checkbox whitelist;
MCP, Python and Skill selections have all/exact/none controls. Skill guidance explains
prompt visibility and displays absolute collection roots. Plugins default off.
Named-Agent editing writes the complete resource profile, including its independent
Tools/Skills/Plugins, model inheritance, timeout and worktree settings.

**Save** submits a native typed semantic-unit mutation with the exact source revision.
It preserves an unsaved draft on conflict and does not reload. The native response
shows source changes and pending reload while the runtime remains on generation N.
**Reload** asks the runtime to prepare and publish a coherent generation; success
shows N -> N+1, while failure/busy leaves N authoritative with bounded diagnostics.
An uncertain write is repaired by rereading authoritative state, never blind replay.
Reconnect also reads current state rather than resending old actions.

Python, Skill and Workflow resource inventories do not imply full source editors.
Full Workflow program, Skill-package and Python-source editing are outside this
Settings scope. MCP definitions and named-Agent profiles have structured editors.

## Browser evidence

These captures come from the real App Server/provider-emulator acceptance suite:

- [Effective values and native provenance](images/cfg3-effective-provenance.png)
- [Named-Agent source saved with reload pending](images/cfg3-named-agent.png)
- [External-edit CAS conflict preserving the draft](images/cfg3-cas-conflict.png)
- [Failed reload retaining the published generation](images/cfg3-reload-failed.png)
- [Mobile named-Agent editor after publication](images/cfg3-settings-mobile.png)

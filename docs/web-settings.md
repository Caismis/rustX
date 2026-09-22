# Web Settings

Web owns forms, drafts and presentation. Rust owns parsing, semantic validation,
serialization, overlays, provenance, shadowing, CAS and runtime publication.
The [CFG3 reference](configuration.md) is the configuration contract.

Global Settings edits User definitions with no Session required. Workspace Settings
opens the exact registered Workspace through Product Host authorization without
changing Session focus or creating a runtime. The entry owns the target for the
whole lifetime of that Settings instance: the global entry is User authoring and
the Workspace object action opens `Workspace Settings — <workspace>` for that
exact authorized Workspace. There is no ordinary `Configuration owner` selector;
a Session focus change never retargets an open editor, and a revoked or
unregistered Workspace is fenced (its draft and error are retained) rather than
redirected to User or another Workspace. Resolved source previews are read-only and are
not the Session's adopted binding. The actual User pathname is displayed separately
from fixed `~/rustx/.agents`.

Structured groups cover Runtime General, Providers & Models, Tool Policies, Root
Model/Tools/Skills/Plugins/Agents & Workflows, and MCP/Python/Skill/Agent/Workflow
resources. Provider and Model edits address independent identities. Workspace
replacement edits complete objects. Native Tools use an explicit checkbox whitelist;
MCP, Python and Skill selections have all/exact/none controls. Skill guidance explains
prompt visibility and displays absolute collection roots. Plugins default off.
Named-Agent editing writes the complete resource profile, including its independent
Tools/Skills/Plugins, delegation lists, model inheritance, timeout and worktree settings.
Description and instructions remain optional; native projection and serialization
keep empty defaults omitted during unrelated profile edits. Root and named-Agent explicit Summary
Models each support independent model, reasoning profile, output limit and request
parameters. Changing Summary identity preserves its other authored fields; choosing
the Session/default option intentionally removes the explicit Summary settings.
General also edits the native Root identity and description semantic units.

**Save** submits a native typed semantic-unit mutation with the exact source
revision and transfers responsibility to native application. The browser keeps
four distinct stages: local draft plus exact base revision, native semantic-unit
mutation, committed acknowledgement, authoritative reread, and native per-unit
application observation. A write acknowledgement is never an application
observation, and a committed save followed by a failed reread is presented as
“saved plus current read/application status uncertain”, not as unsaved. Read
errors, write errors, CAS conflicts, application failures and unavailable/stale
observations are reported separately; a successful read clears only the read
error it answers. Connecting, loading, ready, stale and failed are distinct
states rather than one “loading” label. Change behavior (“Applies immediately” /
“Requires App Server restart”) is distinguished from the observed result
(Applied / Preparing / Failed / Restart pending), and any “Active” statement is
scoped to the native-confirmed unit rather than every Session. Conflicts preserve
the draft. Settings renders simultaneous applied, preparing, failed/retry,
process-restart state. The single **Adopt configuration** control lives below the
focused Session title, outside Settings. It sends the inspected candidate and
expected Session binding; native eligibility is advisory and the native admission
gate revalidates both. Failed preparation offers owner-specific Settings
navigation from the native source scope. Busy never cancels work or schedules
automatic adoption. Diagnostics offers native rescan. Uncertain writes/adoption
are repaired by rereading authority, never replayed. Reconnect rereads native
state.

Python, Skill and Workflow resource inventories do not imply full source editors.
Full Workflow program, Skill-package and Python-source editing are outside this
Settings scope. MCP definitions and named-Agent profiles have structured editors.
MCP transport may be implicit: a URL shows the HTTP editor and a command shows
stdio. Ordinary edits preserve that authored omission and retained credentials.

## Browser evidence

These captures come from the real App Server/provider-emulator acceptance suite:

- [Effective values and native provenance](images/cfg3-effective-provenance.png)
- [Named-Agent source editor](images/cfg3-named-agent.png)
- [External-edit CAS conflict preserving the draft](images/cfg3-cas-conflict.png)
- [Failed application retaining the effective configuration](images/cfg3-application-failed.png)
- [Mobile named-Agent editor after publication](images/cfg3-settings-mobile.png)

## Harness integration

[Settings architecture](../web-console/SETTINGS-ARCHITECTURE.md) describes the one
Harness shell, grouped sections, Provider/Model drill-down, structured exact-identity
rows, native resource cards and target-scoped unsaved drafts. A pure presentation
projection (`app/settings/projection.ts`) maps native facts — authoritative effective
value, authored membership and explicit presence, native provenance/availability,
and per-unit application observations — into display state without becoming
authority: it never recomputes inheritance and never materializes a native default
into a draft. General uses native source units; process settings are User-only and
consume native hot/restart classification. Theme is an intentionally browser-local
preference. Resource discovery never implies Root selection or native preparation.

The current image references are under
[`settings-presentation.spec.ts-snapshots`](../web-console/test/e2e/settings-presentation.spec.ts-snapshots).
The earlier CFG3 captures above document the native contracts before the Harness
presentation migration. Real-server browser runs continue to capture Save, CAS,
publication failure and narrow Settings evidence in `web-console/test-results`.

# Workflow admission and Agent selection

YAML defines a complete bounded program at `.agents/workflows/<name>.yaml`.
User and Workspace discovery follows [CFG3 identity shadowing](configuration.md).
Existence grants no Root authority: `agent.workflows` is an exact allowlist.
Unused invalid resources remain diagnostic entries. Selecting an invalid program
fails admission; no partial program executes.

Each Tool node selects one concrete ordinary Tool. Agent nodes select complete
named profiles and may replace supported dimensions through the native override
resolver. No Root Tool/Plugin ceiling narrows named profiles. Root gains no direct
access to a Workflow's internal capabilities by selecting that Workflow.

Discovery and offline check/explain perform no external preparation. Workflow
admission calculates its finite demand, resolves sources and credentials, and
uses the existing lifecycle owner to prepare exact capabilities. The admitted
program freezes Tool definitions and global policies, child profiles, model
bindings, Skill versions and its runtime generation. It never rereads files while
running. Later Save or Reload cannot alter admitted execution.

Workflow retains its existing language and progression/value/budget/settlement
owners. The outer Tool is foreground-only and sequential. Cancellation, native
candidate identity, human Review, bounded loops, terminal uniqueness and durable
recovery remain unchanged. See [offline authoring](workflow-authoring.md).

# Design for attached-idle execution residency

The durable Thread ID and Session catalog retain the conversation, node,
and configuration identity. A native historical read can acquire its own
finite storage lease. The live attachment binds a specific runtime
incarnation, read source, and subscription cursor.

Today `ManagedRuntimeClient::attach` acquires a `RuntimeResidencyPin`
under the registry lock. `SessionRuntimeManager::reap_idle` releases
idle SQLite page and statement caches, then excludes external pins from
unload. `ResidentRuntime` still owns its composition, Runtime Client host
and projection, publication workers, executable capabilities, and
connection-local evidence. Thread status already observes loaded and
unloaded state at the instance level. Sharing HTTP pools, schema owners,
and catalog records is orthogonal to retiring a composition.

## Choose a new incarnation on reactivation

Keep Thread identity and durable history independently available. Retire the
live observation source with the execution composition and require explicit
reattachment and resync for a later incarnation. This avoids a second
mutable projection owner while the first composition drains. A catalog
record and instance Thread status cover a settled Thread between activations.

An attached client would need a defined suspension notification and
reacquisition flow before its residency pin could be released. The current
attach and idle checks remain the implementation contract until that flow
exists. A user action that needs execution must acquire the new incarnation
before its native admission. An in-flight command receives its exact native
outcome or its existing uncertainty error.

## Activation and retirement

- The Session runtime registry owns one activation flight per Conversation.
  It validates the selected node and authoritative configuration generation,
  prepares the composition, and publishes a new incarnation only after
  startup succeeds. Competing operations await that same result. Failed
  activation destroys partial resources and leaves durable Thread identity
  available for reads and a later activation.
- Retirement acquires the registry and native idle claim under the same
  admission order used today. It fences new operations, captures the last
  committed observation boundary, and transitions the old live source to
  suspended. It waits for in-flight reads and physical worker settlement
  before dropping the composition. A failed claim preserves the prior
  incarnation and all existing pins.
- An active Attempt, foreground or background tool, Auxiliary writer,
  retained physical completion, pending Interaction, or runnable durable
  input excludes retirement. Scheduler activation and a configuration
  generation requiring native preparation participate in that same gate.
  Inert configuration and catalog changes remain durable and are resolved
  when the next incarnation activates.
- Disconnect and explicit detach retire their attachment identities
  idempotently. A reconnect during retirement waits on the retirement
  flight. Reattachment receives a new target and checkpoint, and old live
  ContentRefs fail by their existing source identity. A separately issued
  historical read keeps its own storage and cut lease until completion.
- Shutdown fences activation and retirement, drains native operations and
  physical workers, and preserves all committed facts. It closes live
  subscriptions at the same source boundary used for ordinary retirement.

The first validation should distinguish a catalog-only Thread, one loaded
idle composition, and one active composition with held operations. Track
independent store connections, observation heads and replay arrays, worker
owners, and the exact drop of the composition. A retained catalog identity
or allocator page is separate from a live execution owner. Use owner gates
to test concurrent activation, failed startup, configuration changes,
pending approval, a held old cut, and detach/reconnect during retirement.

# Runtime resource generations

A loaded runtime publishes one immutable `RuntimeResourceSnapshot` through the
existing capability coordinator. Its `RuntimeConfiguration` carries the execution
bindings native owners consume (the resolved configuration and the Provider/Model
binding registry) together with an inert `AuthoredConfiguration` record (component
and source revisions, field origins and the redacted effective document). The same
snapshot owns Skills, named Agents, Workflows, MCP/Python source facts, prepared
external capabilities and Root exposure. There is no parallel resource-only
configuration generation.

Ownership is split at that value. The application configuration owner
(`local_runtime::configuration`) captures sources (`capture`), compares and admits
captures (`prospective`), composes apply units (`units`) and composes the
configuration a generation carries (`generation`); its single preparation worker
(`application`) serializes publication. It runs off-side resource preparation
itself and hands the runtime finished values: a prepared resource candidate, a
`ContextBinding`, or a unit composition over the exact base generation the runtime
selects under its coordinator lock. The runtime (`runtime::adoption`) owns the
prepared candidate, adoption eligibility and refusal, the configuration each
admitted Attempt froze, and the one cache-impact rule; it never reads, redacts or
composes authored documents.

See the [configuration reference](configuration.md#save-automatic-application-and-session-adoption).
The native configuration coordinator captures immutable input bytes and a stable
manifest before preparation. Required definitions, registries, guidance, resource
bindings and leases form one complete capability closure. Preparation is off-side
and bounded; a failed closure leaves its prior effective snapshot available.
Independent policy can apply while another closure fails or awaits adoption.

The source/application fence and runtime gate order publication with new desired
input and Attempt admission. Preserved closures can publish while old Attempts
continue using old leases. Context-changing or unproven candidates require explicit
Session adoption. Retired resources close after lease settlement. Runtime
recreation restores the Session adopted binding and reprepares stale physical
candidates; it does not adopt pending context.

Unused malformed resources produce bounded ordered diagnostics. A selected invalid
resource fails resolution or admission. Workspace invalid duplicates shadow User
resources without fallback. Discovery itself has no connection/preparation effects.

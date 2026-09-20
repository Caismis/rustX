# Effective configuration and authored sources

The [CFG3 reference](configuration.md#exact-overlay-matrix) defines every typed
overlay unit. Rust is the sole parser, validator, serializer and provenance owner.
A winning Workspace Provider, Model, Tool policy or Plugin object receives no
fields from its shadowed User object. Defaulted members carry winning-object
provenance. Empty arrays and omitted fields have different meanings.

App Server `configuration/effective` returns the published generation, redacted
effective document and origins, Source revisions, Root profile, resource inventory
and readiness, explicit Session model, and admitted Attempt generation/model/resources.
`configuration/sourcesRead` separately reads current authored documents and inert
prospective resources. A source read does not publish configuration.

Typed `configuration/sourceWrite` CAS mutations persist source and transfer
application responsibility to the native coordinator. External edits invalidate
stale revisions. Conflicts preserve client drafts. Lost responses require an
authoritative reread, never blind replay.

Application is composable per unit: policy may already apply to future Attempts
while instructions await Session adoption, another closure failed, and a process
binding needs restart. The projection includes desired input revision, application
attempt, per-unit results, actual process bindings, latest complete candidate and
the addressed Session's adopted revision. Ready is distinct from adopted.

`session/adoptConfiguration` commits the inspected candidate with its expected
Session baseline. Busy never cancels work. Conflict requires a reread. An admitted
Attempt and all descendants keep their immutable execution snapshot.
`configuration/reconcile` owns external-file rescan and same-revision retry;
ordinary Save does not invoke it as a second action. Healthy no-ops do not create
fake pending state or resource churn. Scope-versioned notifications are native
authority; reconnect rereads them without replaying mutations.

See [application and adoption](configuration.md#save-automatic-application-and-session-adoption)
for ownership, finite units, request-shape comparison and publication fences.

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
prospective resources. Their revisions determine pending reload. A prospective
source read never changes the published runtime.

User and Workspace editors send typed semantic-unit CAS mutations through
`configuration/sourceWrite`. The native writer canonicalizes the complete document;
comments are not retained. External changes and competing writers invalidate stale
revisions. A conflict preserves the client draft. After an uncertain response,
clients reread and reconcile; they do not blindly retry the mutation.

There is one `configuration/reload`. The candidate includes all documents,
resources, policies, profiles and finite demand. Its single snapshot publication
advances the generation. Busy, cancellation or failed preparation leaves the old
generation authoritative. Save does not invoke Reload. Reconnect reads the current
projection without replaying either operation.

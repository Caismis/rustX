# Lazy native-state previews

Native state indexing retains original segmented bytes and the coordinates needed
for direct field/range/page reads. It now skips recursive preview construction
for every field and stores compact geometry instead of duplicated preview JSON.
The existing root row holds the source kind and extent; no extra descriptor table
or write is added. Navigation remains eager, so cold small reads do not parse or
reindex the full Source. This is the first bounded lazy-observation increment,
not a claim that all observation indexing is deferred.

A requested preview uses the same 32-node budget and 16 KiB original-span rule as
the previous eager implementation. Child lookup uses source/parent/ordinal keys;
individual metadata and byte reads are bounded. Long keys, reserved key paths,
original message overrides, large u64 and pagination retain their existing
meaning. Range-only reads bypass preview generation. Reads neither persist a cache
nor schedule background work; existing errors/readiness remain truthful at the
requested historical cut.

The SQLite development gate advances to 73, and exact table/index checks reject
unsupported layouts without migration, dual writes or fallback. Tests compare all
fixture fields against the independent eager oracle, exercise cold large Sources,
count indexed VM work and returned segment slices, and reject malformed bytes,
geometry and previous layouts. Returned slice counts are not physical-device I/O
measurements. Existing request-capture authentication is unchanged in this stage;
state previews are now derived from committed retained bytes rather than separate
stored preview copies. No stronger independent state-preview witness is claimed.

Validation passed: all-target/all-feature Clippy with warnings denied, all 404
durable SQLite tests, and all 26 Source tests. The four new differential, cold-read,
malformation and format-gate tests also passed separately. Checks and the initial
compile/oracle/lint failures are retained in `evidence/lazy-state-previews/`.

Isolated measurements remain pending. The eventual report must include
the extra SQL/byte work on first reads and any regressions, alongside savings for
unread previews. Final Web/TUI/protocol and recovery validation remains required.

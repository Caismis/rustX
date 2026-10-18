# Stage 1: reuse new-leaf template metrics

A new leaf used to load/authenticate/decode its template for each of 80 width
classes, encode the same base provenance hole 80 times, then load each template
again for path bounds. The matched #484 diagnostic found 64 single-template new
leaves across 32 steady turns: 5,184 loads in these two passes. Existing native
leaves already reuse their stored augmentation and do not repeat this work.

`nodes::leaf_geometry` now loads each capture-deduplicated template once, derives
its one-digit base width and maximum path bounds, then builds the same 80 widths
by adding the established class delta. It retains only at most 80 `u64` base
widths, not decoded templates or an identity cache. The bounded template bytes
are still checked and hashed before decoding the same buffer. Error handling and
persisted encoding are unchanged; this stage changes no format or guarantee.

Test-only counters distinguish template loads, new-leaf geometry evaluations and
base-hole encodes. Single- and three-template fixtures exercise all classes,
including maximum native u64 provenance lengths; malformed references, width
overflow and changed authenticated bytes remain rejected. A real RequestStart
fixture checks legacy full-byte/field/navigation oracles, then captures the same
native leaves again and requires zero new geometry/hole work. Existing durable
Source historical, witness, cold-bound, recovery and Fork tests remain in scope.

The operation reduction is not a wall-time claim. Stage measurements use a clean,
identity-verified release executable and the unchanged local HTTP/SSE provider
against #484's preserved production binary. The measured performance results and
validation outcomes are recorded separately once complete.

Validation: `cargo test --locked --lib durable::sqlite:: -- --nocapture` passed
398 tests (302.01 s); all-target/all-feature Clippy with warnings denied and
formatting/whitespace checks passed. The initial missing `Template` import was
corrected; its failed compile log remains in the cloud evidence directory.

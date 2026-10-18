# Partial publication cadence

The default oldest-payload deadline is 100 ms. This triggers a flush decision;
synchronous persistence and scheduling can add delay, so it is not a delivery SLA. Small responses that complete
before that deadline need no intermediate partial transaction. Tool proposal
start/completion remain distinct ordered frames but share the same commit batch.
The caller remains the single synchronous persistence writer, commits before
releasing frames, and drains final frames synchronously. There is no background
queue or separate precommit ContentRef authority and no pending write can race
past finalization. Cancellation/provider failure retains the current synchronous
audit drain; process crash retains only previously committed partials.

This is not identical to Pi's timer-only full-message snapshots. Rust also flushes
at 64 KiB conservative payload weight or 128 nonmergeable frames to bound pending
work. One incoming provider payload may exceed that threshold; it is flushed
without truncation. Text, tool identities and completed arguments contribute to
pressure. Argument sizing uses a bounded counting writer, not another JSON buffer.
String capacities and JSON container allocations are not measured allocation
counts; these are policy bounds, not a claimed exact heap-byte limit.

The injected-clock tests cover 99/100 ms, unchanged oldest deadlines, pressure
before the timer, empty payload frame limits, oversized completed tool arguments,
exact preserved order/content and terminal/reset fencing. The client also installs missing proposal facts from the existing bounded
canonical message descriptors before rebinding their native locators. A final
tool proposal therefore remains visible when no intermediate frame was published;
execution state still comes from native execution journal facts. No extra SQL or
persistence owner is introduced.

Validation: 10 coalescer tests, 606 scripted tests, 89 tests matching Agent
execution, 82 projection tests, 38 publication tests, and 70 process-death tests
passed, plus all-target/all-feature Clippy with warnings denied. These filters
overlap and are not an additive unique-test count. The process suite used the
freshly rebuilt product binaries and required the local fake provider. A mistaken
integration-binary invocation selected zero tests; its log is retained and is not
counted. The concurrent mailbox probe verifies publication retains its lease
through terminal notification while a producer attempts admission.

Initial failures are retained: stale frame-only client expectations, a real gap
in canonical-only tool proposal events, fixture UUID version, style lints, and
an ENOSPC link failure before tests ran. Only regenerable build caches were
removed to recover space; original evidence and benchmark executables remain.
Formal isolated/combined measurements and exact final-head FULL/Merge CI remain
pending. See evidence/partial-validation for raw checks and a hash manifest.

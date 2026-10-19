# Remaining short-turn gap: matched diagnostic

The final formal short-small medians remain Rust **65.679 ms wall / 96.25 ms
self CPU** versus Pi Durable **4.502 ms / 9.375 ms** per turn. These are complete
runtime-plus-driver measurements, not pure engine or database costs. This note
investigates their difference; it does not replace the formal results or propose
weaker persistence, observation or publication contracts.

## Same production inputs, separate diagnostic execution

The diagnostic uses the final fixed Rust executable (SHA-256
`78e044bee701759c17884e2b3813c8f10990ee4e92996008b6a8e65ef243c223`) and actual Pi
Durable SQLite at `90d863f7c1183b965f099855715d6e88c7b8a52c`, Node 24.19.0.
Both production HTTP/SSE adapters call the same local fake provider, with the
same 8 seed turns and 32 steady turns. Both SQLite adapters use WAL/NORMAL.
Native tool schemas remain Rust 7 / Pi 4; ordered core requests match, complete
requests do not claim schema equality. Full expected/wire/capture/reopen oracles
passed for both products, including all 40 requests each.

The external controller owns product, provider and tracer as siblings. The
provider remains alive past product cleanup. Existing synchronous Rust diagnostic
boundaries pause for external read-only database introspection; Pi's diagnostic
copy of the driver adds the same boundaries. Pi's production source is unchanged.
Its Node SQLite native methods are wrapped for counts and elapsed observations,
and V8 is sampled during the 32 steady turns. No Rust executable rebuild or
allocator replacement occurs. These operations intentionally perturb execution:
the traced means were **267.345 ms Rust / 15.384 ms Pi**, far above formal values.
Do not subtract their timings, compare their CPU-profile denominators, or use
these as new speed ratios. Native-call elapsed observations include tracer stalls.

Two earlier diagnostic pairs passed their product/request oracles but omitted
Rust socket `sendto` from tracing. Their traces cannot establish steady-phase
commit counts and are retained as incomplete instrumentation. The final trace
includes `sendto`/`sendmsg`; the analyzer rejects missing phase markers instead
of reporting zero work. All six diagnostic product samples and original files
are preserved in the compact archive. None enter headline benchmark samples.

## Persistence: substantially more committed work

| Same 32 steady turns | Rust | Pi Durable |
|---|---:|---:|
| WAL write-commit markers | 670 | 160 |
| WAL frames | 20,002 | 1,683 |
| Frame bytes, including 24-byte headers | 82,408,240 | 6,933,960 |
| Pi native `BEGIN IMMEDIATE` / `COMMIT` calls | unavailable | 160 / 160 |
| Newly prepared SQL statements | unavailable | 0 |
| Native statement `run` / `get` / `all` calls | unavailable | 832 / 1,568 / 160 |

These count successful WAL frame headers with nonzero committed database size,
not read-only transactions, fsyncs, logical application events or physical-device
writes. Phase boundaries are the benchmark's observed steady-turn boundaries;
the Rust count is **20.94 per turn in this run**, not a claim that every turn has
exactly 21 native transactions. Pi's five write transactions per turn are
independently corroborated by its SQL counter. Rust's P<U<C ordering is only part
of its broader native persistence work, not a three-transaction whole-turn claim.

External `dbstat` snapshots at both boundaries associate WAL page numbers with
tables/indexes. Rust has one unmapped frame; intermediate page reuse cannot be
excluded, so this is page-owner attribution, not a row-change audit. Of its frames:

- 8,997 map to `events` and its `events_*` / primary-key indexes (45.0%).
- 2,056 map to state observation fields and their child index (10.3%).
- 1,348 map to content segments and 831 to content frontiers.
- 1,136 map to request Source pages/nodes/roots and their indexes (5.7%).
- Other touched owners include ledger/surface/pending state, transcript readiness,
  request snapshots, publication streams/frames, lifecycle state and trace data.

The Rust steady interval adds 256 event rows, 3,008 state observation field rows,
704 content segments, 736 content frontiers, 32 request snapshots, 224 trace
definitions, 452 Source pages and 285 Source nodes. It also adds the expected 64
ledger/transcript entries. Net row counts do not reveal transient insert/delete
work: pending/publication staging tables have writes despite unchanged counts.

Pi's frames instead map to metadata (160), tasks and task indexes (774), document
revisions and their indexes (288), entries and their index (133), submissions and
their indexes (256), record IDs (64) and schema page (8). Net growth is 64 entries,
32 tasks, 32 submissions and 128 global record IDs. Document revision pages change
without net growth. The full SQL text/counts and table/page inventory are retained.
This establishes a different volume and shape of durable work; it does not make
Pi's native records equivalent to Rust's audit, Source and publication contracts.

## Context, request and publication work

Pi's generation preparation reads context, plans system entries and commits its
request checkpoint; request execution commits live generation state, reads the
frozen cutoff context and invokes `beforeRequest`. Its scheduler's
`readContextFrom` retains immutable context ranges: with an unchanged head marker,
it scans only new entries, reuses an equal cutoff, or derives an earlier cut from
the retained range. This run made 32 bounded `id >= ? AND id <= ?` entry scans,
not one full-history scan per observer/context call. The connection's SQL-text
statement cache produced zero new prepares in the steady interval. The benchmark
observer additionally reads context, compares it and clones messages.

Rust also retains native manifest selection and incrementally appends history,
but constructs authenticated request Sources, ordered observations and client
publication/read surfaces. Its existing final native CPU profile contains:

| Cumulative stack, small steady profile | Samples / 655 |
|---|---:|
| SQLite preparation (`sqlite3Prepare`) | 154 |
| Publication encoder (`PublicationDraft::encode`) | 134 |
| Template load (`fragments::load_template`) | 7 |
| Driver process-counter sampler | 19 |

These overlapping native stack samples cannot be summed. The 267 samples in
`publish_attempt::{closure#5}` belong to the **attempt task parent**, which calls
`run_attempt().await` and `finish_attempt`; they do not show that publication alone
costs 40.8%. The historical attribution label is corrected accordingly. Pi's V8
profile has 542 samples, including native wrappers and idle observations; it is
not a directly comparable process-CPU percentage profile. Neither profiler
provides an exclusive millisecond breakdown of the entire Rust/Pi difference.

Driver costs remain part of both formal results. Rust drains/decodes native
publications; Pi waits on native submission completion and runs the capture hook.
Both sample process counters and footprints per turn, verify results, close,
reopen and clean up. Rust's sampled counter function alone is 19/655 cumulative
samples, not all driver cost. Pi's faster steady turns coexist with slower startup
(348.037 ms vs Rust 4.156 ms in the formal medians). Provider startup, control and
complete lifecycle are already reported separately. These are workload boundaries,
not a justification to subtract unrelated medians or erase native differences.

## Repeated template work: real, bounded to new leaves

Boundary snapshots show **64 newly captured leaves**, each with 80 width classes
and exactly one template selected by every class. The unconditional production
loops in `leaf_widths` and the path-bound pass therefore execute:

- 64 `leaf_widths` evaluations;
- 5,120 width-pass template loads and base-hole encodes;
- 64 additional path-bound template loads;
- 5,184 total template loads for those new-leaf passes, versus 64 per-leaf unique
  template selections (global digest uniqueness is a separate question).

These counts are derived from successful persisted leaf geometry and source
control flow, not function-entry probes. Existing native leaves use `stored_hash`
and do not repeat this construction; do not multiply by all history. Each
`load_template` executes a cached SQL query, reads bounded bytes, verifies SHA-256
and decodes the same JSON buffer. `prepare_cached` avoids preparing the statement,
not those reads/hash/decode operations. The node `created` map caches node digests,
not templates. A leaf with multiple templates still has 80 width-pass loads plus
one load per stored template in the path pass; this run only exercises the
single-template shape.

A capture-local, bounded unique-template reuse experiment is a justified next
candidate, provided exact widths, page/root bytes and tamper behavior remain
unchanged. It has **not** been implemented or assigned a measured speedup here.
The final existing small/large CPU profiles attribute only 7/655 and 11/761
cumulative samples to `load_template`. They do not separate SQL reads, SHA-256
and JSON decode exclusively; no allocation or exclusive-time claim is available.
This repeated work is concrete, but the evidence does not make it the dominant
remaining gap. Event/index persistence, SQL preparation and publication encoding
have stronger evidence of scale and must be investigated within their contracts.

## Reproduction and limits

The [3.63 MB archive](evidence/remaining-pi-gap.tar.xz), verified against its
[109-file manifest](evidence/remaining-pi-gap.tar.manifest.json), contains the
controller, diagnostic Pi wrapper/driver, all raw traces,
phase snapshots, SQL counters, CPU profiles, oracle results, analyzer and binary
identity. Run `run-complete.py` from the repository with its recorded paths, then
`analyze.py`; use fresh output directories. The [summary](evidence/remaining-pi-gap-summary.json) is also published plain.
The fixed final executable remains preserved in the cloud evidence directory.

This is one final matched diagnostic pair, not a new three-pair performance claim.
We can identify commit/write amplification and specific repeated work; we cannot
assign every millisecond of the remaining gap to an exclusive owner. No production
architecture, publication cadence, persistence queue or guarantee was changed.

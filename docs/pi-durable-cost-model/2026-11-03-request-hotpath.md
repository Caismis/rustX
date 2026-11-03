# Request preparation and committed proof ownership

This round follows the Statistics history refactor. It first measures the current
gap and investigates the prior long-history reversal, then removes three proven
forms of repeated preparation. It does not extend incremental storage to Execution.

**The changes remove real work, but do not establish a large overall speedup or Pi
parity.** Four fresh descriptive pairs per cell give warm median-turn changes of
−2.289%, −4.404%, −1.107%, and −0.761%. Reversals remain, including +16.008% in one
live long-history pair. Process writes are essentially unchanged. Current warm
candidate/Pi ratios are **2.131–2.452**, rather than the old fivefold description.

[Full values](2026-11-03-request-hotpath-tables.md) and
[raw evidence / portable replay](evidence/request-hotpath-20261103/README.md)
retain the unfavorable observations and explain the measurement boundaries.

## What was actually redundant

### A read-only rollback invalidated committed writer proofs

SQLite can invoke its rollback hook when a read transaction is dropped. The shared
hook unconditionally invalidated both the Statistics fold and canonical append
validation owner. Consequently a read operation could force the next writer to
recover state and validate old immutable content again, despite no main-database
mutation having occurred.

The old diagnostic binary records, per 32 no-subscriber warm turns:

- 192 statistical fold calls: **160 hits, 32 misses**;
- 32 history recoveries, **all at distance 2**, not distance 15;
- no read-domain refresh in the measured window;
- about 2.9 ms total *inclusive diagnostic wall time* in history recovery.

The existing preupdate observer now marks main-database writes in a
connection-owned atomic flag. Rollback invalidates proofs only if such a write
occurred. The flag is cleared after the connection actually returns to autocommit,
not prematurely by a commit hook. Written transaction rollback, savepoint fences,
external `data_version`, mutation tracking, and unknown-schema rejection remain.
This is a change to ownership invalidation, not a public-reader cache.

Regression tests exercise read-only rollback followed by three warm appends/folds,
and real write rollback followed by reuse of the same positions. The latter must
reject stale retained batches and must not publish rolled-back statistical facts.
Public historical reads still authenticate the current stored dependencies.

Implementation: [connection lifecycle](../../src/durable/sqlite.rs),
[preupdate owner](../../src/durable/sqlite/manifest.rs),
[append rollback tests](../../src/durable/sqlite/append_head_tests.rs), and
[Statistics history tests](../../src/durable/sqlite/observation_statistics_history_tests.rs).

### Stable Request Source work happened before the sharing lookup

The native request already serializes and hashes its exact configuration. Request
Source nevertheless encoded its complete stable inspection view before discovering
that the view was already stored. The lookup therefore saved storage but still paid
for prompt/tool encoding on each request.

The native start operation now passes its freshly authenticated configuration
identity into Source capture. A key combines that identity and the remaining
allowlisted scalar view, streaming the latter directly into SHA-256. The key omits
prompt/tool bodies; their exact identity is already included through native
configuration. A hit authenticates current registry metadata and geometry before
reusing the stable fragments. Full stable Source construction happens on a miss.
Changing model, protocol, prompt, tool order/schema, revision, allowlisted options
or scalar view remains covered by the differential/reopen tests.

This does **not** remove native configuration serialization, the changing prefix,
or all Source capture. It does not cache inspection results across calls. For small
roots, exact retained suffix bytes are still read to preserve whole-root numeric,
depth and node admission around 16 KiB. Large roots check their empty `messages`
geometry without materializing the whole suffix. Bad shared Source evidence fails
closed; it is not silently rebuilt as a cache miss. This gives capture a dependency
on those retained Source chunks. Native request facts remain independent, and an
optional Source failure cannot roll back native request start. No automatic repair
of a corrupt inspection domain is promised.

### Identical authenticated paths were traversed more than once

Request length and path validation walked the same rank-digit boundaries. A fused
geometry traversal now authenticates each selected boundary node and metadata once.
Length errors are deferred until path checks finish, preserving the old staged
error priority. Both endpoints of each width class are checked, including the
8,192 rank bound.

An independent staged test oracle confirms **8→4 node/metadata loads** for its
boundary case, and preserves path failure before length overflow. Historical
inspection still uses the existing length reader; no new cross-operation cache
or persistent summary is introduced.

Similarly, reading up to nine consecutive Source chunks now groups index point
lookups. Shared authenticated ancestors are loaded once in the operation; only
selected subtrees are read. The test oracle measures **17→5 page loads** and checks
current-page corruption, rollback restoration and input bounds. These are operation
counts in explicit tests, not percentages of total runtime.

The obsolete encoded-Source carrier and its unused whole-source digest path were
removed. Private schema **91** refuses schema 90; there is no migration or legacy
reader. Public DTOs and normal functionality remain covered by the checks below.

Implementation: [stable capture](../../src/durable/sqlite/observation_request/composed/scalar.rs),
[native configuration](../../src/durable/sqlite/native_request.rs),
[fused geometry](../../src/durable/sqlite/observation_request/composed/nodes.rs),
[authenticated batch lookup](../../src/durable/sqlite/observation_request/composed/index.rs),
and [differential scalar tests](../../src/durable/sqlite/observation_request/composed/scalar_tests.rs).

## What the previous regression does and does not mean

The previous Statistics report's +5.395% / +2.656% long-history wall reversals stay
unchanged. A fresh targeted repeat uses the same two saved ordinary binaries,
four balanced pairs per observer mode, and the same 64-history/32-warm workload.
It does not stably reproduce that direction:

| Long history repeat | Median paired warm wall change | Warm CPU change |
|---|---:|---:|
| No subscriber | −0.132% | 0% |
| Live subscriber | −3.644% | −6.579% |

This does not prove there was no regression. It prevents attributing the old fixed
percentage to suffix replay without evidence. The counter diagnosis establishes
an avoidable invalidation separately; roughly 0.09 ms inclusive recovery time per
no-live turn cannot explain several milliseconds of total Pi difference.

Ordinary 250 Hz profiles of the two old binaries retain six CPU/control pairs.
Warm windows contain only 72–95 samples per profile. About a fifth of no-live
samples are in the benchmark's `/proc` counters/footprint work. Request Source is
visible; current Statistics writer/history samples are sparse. This is too little
sampling resolution for an exclusive function-level explanation of a 5% reversal.
Nested Source/Statistics timer records must not be added or called CPU shares.

Old instrumented Source capture is approximately 846–864 µs per request, with
capture 574–586 µs, stable scalar 88–90 µs, and separate path/length walks about
89–91 / 74–75 µs. Stable scalar alone was therefore not a plausible multi-fold
optimization. This round also merges the measured repeated walks.

The final pinned diagnostic products confirm the mechanism. In both no-live
windows, all **192 folds hit**, with **zero** writer misses/history recoveries.
Both live windows also have 192 writer hits; public read-domain history reads
remain and still validate current storage. Per-request Source node-load median
falls **63→41**. Warm stable full encodings are **zero** in all four candidate
products (the old build did not expose that counter). No-live Source median is
**752 / 752.5 µs**, compared with **853 / 846.5 µs** before. Candidate scalar is
71–75 µs; capture remains 570–612 µs. These are intrusive diagnostics in independent
products, not paired formal latency percentages or an exclusive CPU decomposition.

## Fresh ordinary results

The final cohort is **48 products**: before rustX, candidate rustX, and Pi, across
8/64 history turns and no/live subscriber, four ordered triads per cell. Rust
before/after products are adjacent with balanced order; Pi is first twice and last
twice. Each product measures 32 warm turns. Direct Rust comparisons and candidate/Pi
comparisons each check **1,088 requests**. No compiler or other product overlaps
this final cohort. CPU 2,8 are distinct physical cores with matching recorded class
evidence; controller descendants inherit the mask, without per-thread sampling.

Percentages below are medians of paired changes, not ratios of absolute medians.

| Scenario | Warm wall | Warm CPU | Warm process writes | Lifecycle CPU |
|---|---:|---:|---:|---:|
| No subscriber, 8 history | −2.289% | −5.000% | 0.000% | −1.670% |
| No subscriber, 64 history | −4.404% | −4.688% | −0.039% | −0.889% |
| Live subscriber, 8 history | −1.107% | 0.000% | −0.015% | −0.538% |
| Live subscriber, 64 history | −0.761% | −2.632% | +0.032% | +1.162% |

The small changes, reversals and 10 ms process-CPU endpoints preclude a precision
speedup claim. In particular live long-history full-cycle CPU increases, despite
its median warm CPU decrease. Full lifecycle includes seed history, frozen-request
checks, late snapshots, shutdown, reopen and cleanup; it must not be divided by 32
and described as ordinary chat overhead. Live mode excludes AppServer/WebUI.
Candidate lifecycle CPU is 1.816–1.937× Pi for short history and 3.061–3.103× for
long history; those are different ratios from warm wall and warm CPU.

Before development, a separate fresh 32-product c9/Pi cohort established a current
gap of roughly 2.2–2.4× warm wall. It is not pooled with the final cohort. DeepSeek
is not rerun, and no new ratio combines its historical unrestricted batch with
these pinned-CPU products.

## Why writes and overall runtime still exceed Pi

Pi has durable history, checkpoints and usage. Its usage document retains
`history: "latest"` and updates in the response commit. Request checkpoint is task
current state; termination preserves outcome. Its context reader reuses immutable
old entries under the same head, scanning and deriving only appended entries,
although view arrays are still copied. The comparison is not TypeScript versus
Rust instruction efficiency.

Pinned Pi sources: [usage](https://github.com/earendil-works/pi/blob/1cedd32724abfcb0915f76cc61b6827e2c16dbad/packages/durable/src/harness/usage.ts),
[generation](https://github.com/earendil-works/pi/blob/1cedd32724abfcb0915f76cc61b6827e2c16dbad/packages/durable/src/harness/generation.ts),
and [context](https://github.com/earendil-works/pi/blob/1cedd32724abfcb0915f76cc61b6827e2c16dbad/packages/durable/src/harness/context.ts).

rustX still retains historical frozen request input/configuration and inspection
evidence, per-event independent statistical facts, bounded historical cuts,
Execution/receipt/owner evidence, message/block navigation headers, Trace facts and
content frontiers. Those are active capabilities, not duplicate bytes that can
simply be deleted. This round changes CPU preparation; it removes no table, index,
normal event row or write transaction.

Current process-write ratios are **3.74×** Pi at short history and **4.41–4.42×** at
long history. Process accounting is not a device write measurement. The final
synchronized WAL/column diagnostics in the tables report page counts separately
from logical bytes. Five normal warm write commits remain per turn.

In this batch, WAL frames per 32 turns are **5,655→5,655** at short history and
**6,384→6,393** at long history. This is not an actual-write improvement. A single
diagnostic per side cannot establish a stable nine-frame regression either.

The investigated `events_kind_idx` and `events_trace_kind_idx` look similar on
healthy ordinary rows, but one indexes native event JSON and the other independent
optional observation kind. Replacing one authority with the other would change
fault isolation and selected-history access. Both remain. The existing width table
already uses count/digit deltas and run compression; it was not rewritten.

Subscription invalidation predicates are also not equivalent to completed reads:
the old no-live window records 448 predicate invocations and **zero** read-domain
calls. Live mode has 134/137 actual calls in the two diagnostic products. The
predicate counter does not record its returned boolean, so these counts cannot be
subtracted to manufacture a coalescing rate. This round does not redesign the
subscription pipeline or claim it explains the no-live gap.

There is no remaining identified fix from these three implementation changes left
unimplemented. Reducing retained historical evidence, merging lifecycle commits,
or dropping independent indexes requires a separate semantic/storage design and
proof of failure behavior. This report supplies no measured upper bound on such a
change and makes no promise of another multi-fold speedup.

## Source identities, checks and retained failures

Production candidate: **fceaf3d82548c8fef1844861a1788cbc7ef6f362**. Its clean ordinary
O3/default-feature conformance SHA-256 is
`08a82340870dfac036d8c0272488161c10bbdedcbdeee605ae565cdf1c8da3d7`.
Before is c9d5f73e, using saved ordinary binary
`1c3ee414569826e34eb289ab157c8c611e4e1c8ad0343d3e0caffc79408f9f22`.
The older Statistics comparison uses compiled 09e388f8 / f78-identical inputs and
binary `829d100e21ac52589ea46d1239429097e0073f84ab71482577e87654eadb5883`.

Pi is the clean source **1cedd32724abfcb0915f76cc61b6827e2c16dbad**, Node v24.19.0,
WAL/NORMAL. Offline installation and Chord/telemetry builds pass. AI initially lacks
ignored generated JSON; it is copied only after tracked model/provider/generator
hashes match, and model-data verification passes. **Official AI typecheck still
fails** in `kimi-coding.ts` and `opencode-go.ts`. Explicit `tsc --noCheck` emits
unchanged source for this runtime comparison; this is not a successful official Pi
build/typecheck. All runtime, request equality, native observer and reopen oracles
pass. The exact dependency/generated-data/source/dist provenance is retained.
Matching WAL/NORMAL settings and successful reopen checks do not prove identical
durability or failure semantics between the products. Native tool schemas also
differ; comparisons assert core request equality and retain full-request inequality.

Final checks: **2,378 distinct tests pass**, zero fail, one existing ignored;
SQLite accounts for 659. Fmt and release/all-target/all-feature Clippy with
deny-warnings pass. All 970 src/tests/Cargo inputs are identical across the final
check snapshots and ordinary candidate build. Python runtime harness unit tests and
Node Pi oracle/observer tests also pass. Sixteen additional ordinary Rust products
cover stream, large, tool and cancel in both observer modes; their request/reopen
oracles pass. Their single pairs are correctness checks, not speedup estimates.

Earlier compile/schema-fixture/Clippy failures and one explicitly stopped compiler
attempt remain archived. An auditor invoked before the first triad controller had
finished correctly rejects incomplete coverage. The first 48-product triad attempt
then overlaps its last product with the start of functional checks due to an early
yield from a controller wait. **All 48 trial products are excluded** from final
performance reporting and retained under `triads-overlap-excluded`. The complete
48-product final cohort runs again after functional checks exit; no selective pair
replacement or pooling is used. Functional overlap does not change their
correctness-only use.

Two later raw-audit harness mistakes are also retained: the historical repeat's
comparison filename differs from the formal cohort, and Rust's ignored test line
contains an explanatory suffix. Corrections change only those lookups/parsing;
they neither change counters/formulas nor rerun or select product data. The initial
audit source is retained (its hash matches the earlier arithmetic receipt).
The first candidate diagnostic child also starts before its parent is pinned to
CPU2,8. All four diagnostic trial products and the premature pin-receipt statement
are retained with a timestamp-based correction. Four pinned diagnostics replace
the whole trial; no ordinary formal sample is affected.
A control marker from the C profiler can interrupt Rust's formatted diagnostic
JSON in the middle of a key. The initial line-based counter parser rejects that
boundary. The retained corrected parser removes only the two fixed profiler/control
records, reconstructs complete JSON and assigns each record by its first emitted
byte relative to the retained markers. It does not discard malformed lines or
rerun products. Diagnostic timers completing in a window can include work begun
earlier; their inclusive durations remain unsuitable as exclusive phase costs.

Full repository CI, browser E2E and main-branch merge are outside these checks.
The PR remains draft with its existing main conflicts.

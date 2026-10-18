# Continued optimization: attribution checkpoint

This is a diagnostic checkpoint on production code at `fb6d7ff4`, before any
new production optimization. Prior green CI covers that head only. New profiling
hooks are test-harness-only. The unresolved original canonical short-small wall
ratio **1.3334** and real Source anonymous-memory increase remain unchanged.
No result below replaces the retained paired headline matrices.

## Facilities and scope

Locally unpacked Debian gperftools 2.16 provides CPU stack sampling and full heap
allocation tracking without changing host security settings or system packages.
Its packages, hashes and official URLs are recorded outside the repository in
`/workspace/issue-481-profilers/manifest.json`. CPU sampling uses the system
allocator, at a requested 250 Hz. Heap tracking uses tcmalloc instead, and its
very large overhead makes its wall/CPU/RSS unsuitable as production measurements.
Allocation totals are requested bytes/calls under that profiling allocator,
not physical memory or allocation counts previously available from ordinary runs.
The profiler's [CPU](https://gperftools.github.io/gperftools/cpuprofile.html) and
[heap](https://gperftools.github.io/gperftools/heapprofile.html) documentation
describe these distinct facilities.

The production Rust library is unchanged. Diagnostic conformance executable
SHA-256 is `b5c58303ec9960e67afe75dd23548de7e82d90aeed511f7c9953daa0235f8345`.
A preloaded C bridge controls the profiler over a synchronous Unix socket.
Safe Rust test code sends phase boundaries and waits for acknowledgement;
there are no signal races, sleeps, modified deadlines or unsafe Rust exceptions.
The bridge and allocator are loaded only into the product process. The same
HTTP/SSE provider remains a controller-owned sibling through product cleanup.
All diagnostic runs completed full request/result/reopen oracles successfully.

CPU profiles isolate initialization, seed turns, the next 32 measured turns,
live verification, shutdown/drop/close, reopen verification and cleanup. Phase
cuts include profiler control overhead; they are not formal timing observations.
Driver request submission, notification relay and counter/footprint sampling
remain in the process. Native publication/transport work reached via the relay
is product work, not automatically harness overhead. In the small steady profile,
16/751 samples include the counter reader and 3/751 include footprint scanning;
these identify explicit harness work but do not isolate every shared/inlined
function. No complete exclusive product/driver CPU percentage is claimed.

## Whole-turn diagnostics

Each row below is one clean control process, not a headline comparison or
statistical estimate. The small case seeds eight turns; the large case seeds 64;
both then measure 32 turns. CPU is self user+system ticks at 10 ms resolution,
not wall waiting. Process writes are OS-attributed bytes, not physical-device I/O.

| Case | Mean steady wall ms/turn | CPU ms/turn | Process writes MiB/turn | Init CPU ms | Live verification CPU ms | Shutdown/drop/close CPU ms | Reopen verification CPU ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| short-small | 71.89 | 103.75 | 3.187 | 130 | 430 | 30 | 440 |
| short-large | 80.21 | 114.69 | 3.644 | 140 | 2,300 | 40 | 2,450 |

One CPU-profiled small run averaged 72.90 ms wall / 103.75 ms CPU per turn;
large averaged 84.68 / 123.13. These single observations only show the scale of
instrumentation effects; they are not a controlled overhead ratio. An earlier
control overlapped heap-symbol analysis and is retained but excluded from this
table. The full heap run averaged 5,465 ms wall / 8,553 ms CPU per steady turn:
its overhead is substantial, not hidden or subtracted from headline samples.

The new clean runs do not reproduce the old 1.3334 ratio. They do not establish
its cause or invalidate it. Earlier waiting-burst and file-residency diagnosis
in `review.md` still applies.

## Sampled CPU and allocations

Small/large steady profiles contain 751/847 CPU samples. Cumulative stack
presence, **overlapping and not additive**, is approximately:

| Stack owner or operation | Small | Large |
|---|---:|---:|
| Native attempt publication | 44.1% | 46.6% |
| Read-domain materialization | 23.8% | 23.7% |
| SQLite statement preparation | 21.8% | 21.6% |
| Publication encoding | 19.7% | 18.8% |
| Request observation layout indexing | 15.2% | see raw profile |

The large request/observation cost is broader than small Source construction.
The small profile records just 10 samples in Source index initialization and
nine in sealing; these are not the entire Source subtree or exclusive CPU.

Same-process heap snapshots surrounding the 32 steady turns record **8,146,523
allocations / 1,687,264,799 requested bytes** (about 254,579 allocations and
50.29 MiB per turn). SQLite allocation/reallocation leaf sites account for
631,200,520 bytes, about 37.4% of that traffic. Runtime observation draining
accounts for 80,392,192 bytes in 1,025 allocation calls (4.8%). Named owner
attribution also highlights checkpoint encoding buffers, trace facts and
request-fragment persistence. Owner/caller tables overlap the leaf table and
must not be summed. Generic code sharing and absent inline expansion limit
fine-grained symbol attribution.

Inspection links the drain traffic to preallocating 32 transitions and 33 large
observation enum slots even for small journal batches. The existing ordering,
32-item bounds and owner publication semantics need no change to reduce that
allocation capacity. This is a concrete low-risk candidate, not a result yet.

## Write attribution

A separate controller-owned `strace` attaches to its product child before the
controller releases provider readiness. Product, provider and tracer are
siblings; startup before attachment is outside trace scope. The complete
40-turn lifecycle plus verification/close/cleanup writes:

| Destination | Logical syscall bytes |
|---|---:|
| Conversation WAL | 104,731,232 |
| Conversation database | 29,532,160 |
| SQLite temporary files on tmpfs | 26,809,900 |
| Source original byte files | 692,646 |
| Source disk indexes | 1,966,080 |
| Harness evidence JSON, after cleanup endpoint | 6,680,279 |

There are 442 Source byte files, including 402 at most 4 KiB, and 40 disk
indexes; trace reconstruction has zero unresolved calls. These are logical
writes across the stated lifecycle, not marginal steady DB writes. Database
checkpoint writes and WAL writes are distinct physical operations but not
physical-device measurements. Temporary tmpfs traffic is not device-write
savings. Per-turn process-write counters above exclude evidence JSON.

## Specific next changes to evaluate

1. Reduce the measured oversized temporary observation-vector reservations,
   preserving every item, batch order, bounds, wakeup and publication cadence.
2. Reuse preparation for repeated observation fragment/index SQL where evidence
   shows repeated preparation, with bounded statement retention and measured
   memory tradeoffs. This changes executable statement reuse, not data authority.
3. Evaluate a bounded small-Source navigation representation against the measured
   SQL/allocator costs; retain original bytes, lexical spans and large cold disk
   behavior. Do not assume it is the dominant whole-turn cost.
4. Profile marginal append at larger histories before changing validation scans.
   No authoritative corruption check is removed based on these short histories.

Only measured improvements will be retained. New paired whole-turn/Pi durable
controls, history scaling, deterministic contract tests and exact-head FULL
verification remain required after production changes. No merge is authorized.

Raw CPU/heap/syscall profiles, control samples, failed initial instrumentation
build, corrected build and all analysis attempts remain in
`/workspace/issue-481-evidence`. The failed unsafe-Rust probe was replaced with
the safe socket handshake; the repository's unsafe-code lint was not relaxed.
Reproduction tools are under `benchmarks/runtime/profiling/`.

## Attribution review correction

The heap analyzer now requires `--binary` and matches that exact recorded path,
instead of a hard-coded basename. Deterministic fixtures cover a renamed binary,
the ELF segment/file-offset gap, missing mappings and a missing zero-offset
mapping. The recorded allocation tables revalidate unchanged. The Python suite
now has 26 passing tests. [Plain CPU sample counts](evidence/attribution-cpu-counts.json)
expose phase IDs, raw/report hashes and denominators independently checked against
binary profile records: SQLite preparation is 164/751 and 183/847 samples, not
an exclusive share that can be added to other cumulative rows.

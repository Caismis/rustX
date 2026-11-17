# Reduce loaded-session retention without reducing replay/history

The requested first two changes are complete: replay versions share unchanged descriptor rows and transcript arrays; idle SQLite connections release reconstructible caches, followed by a batched GNU allocator return hint. The exact replay window, frozen history/cuts, protocol schema and durable transaction boundaries remain. This is a bounded retention change, not a claim that every resource now matches Pi.

Final production: **d50125842f45721e831d02474bf77bf2b7c62bc5**. Ownership/cache implementation: **f2e1273d2fd297fecdd0485d9edaa1a97520e73e**. Before is the saved production **e1d4a40dc7876d79ca0b3240064b560659b7d593**, documented by **e1a984771**. Trial products measure f2e; final products measure d501. They are kept separate throughout.

| Final ordinary-glibc endpoint | Before → final | Median paired change |
| --- | --- | --- |
| Subscribed 128 empty anonymous PSS MiB | 323.61 → 324.34 | +0.09% |
| Immediate 128×8 anonymous PSS MiB | 765.88 → 742.84 | -3.03% |
| Immediate 128×8 RSS MiB | 828.77 → 806.29 | -2.75% |
| 32 s idle 128×8 anonymous PSS MiB | 765.01 → 729.47 | -4.65% |
| 130 s mixed anonymous PSS MiB | 772.57 → 698.40 | -9.60% |
| Width 1 settled median ms | 9.55 → 9.71 | +0.33% |
| Width 1 CPU ms/turn | 11.88 → 11.88 | +0.00% |
| Width 1 process writes KiB/turn | 645.50 → 645.25 | +0.00% |

Immediate memory uses four balanced pairs, eight ordinary processes. Idle memory uses two additional balanced pairs, four separate processes. At 32 s all 128 retained/subscribed sessions still have exactly 8 turns. At 130 s four continuously revisited sessions have 14 turns and 124 idle sessions have 8; this point must not be labeled 128×8. There is no forced diagnostic allocator, preload, manual GC or external trim in final ordinary products. The allocator hint is production behavior.

[Every value and adverse cell](2026-11-03-idle-retention-tables.md) · [Authenticated raw evidence and portable replay](evidence/idle-retention-20261103/README.md) · [Previous native regression repair](2026-11-03-native-regression.md).

## What was retained, and what changed

`MessageDescriptorCommitted.head` previously owned a full `Vec<Descriptor>` for each retained publication. It now uses the existing `imbl` persistent vector with `Arc<Descriptor>` rows. Each historical version retains its exact membership/order/cut; unchanged rows share ownership. The event's individual descriptor shares the same row. Tool updates detach only matching rows. Exact locator lookup preserves distinct message-ID locators that share the same ordering sentinel.

The larger remaining replay payload was the transcript read-domain page. Content, collection, deferred-entry and entry arrays now use the project's existing immutable `MetadataFragment<Vec<T>>`. Internal views, prepared binding heads and replay/checkpoint fragments share these owners; explicit mutable access uses existing copy-on-write. Content/collection owner retention uses exact string/integer equality. Nonempty opaque JSON-bearing entries always adopt the new native fragment, so signed-zero and map-order differences cannot authorize stale-byte reuse. Explicit public snapshot materialization still produces its required value.

No new global object cache was added. The committed v52 schema and protocol fixtures remain byte-identical, verified by the existing artifact test. Native Rust container types changed intentionally; no obsolete compatibility path was introduced.

[Descriptor/transcript owners](../../src/runtime_client/projection/shared_snapshot.rs) · [Publication updates](../../src/runtime_client/projection.rs) · [Shared fragment](../../src/runtime_client/checkpoint/metadata_fragment.rs) · [Ownership/old-version tests](../../src/runtime_client/projection/shared_domain_detach_tests.rs).

## SQLite idle ownership and physical memory

Connection leases record their return time inside the existing connection mutex. With no foreground/background execution or interaction work, advisory maintenance tries that mutex without waiting. An outstanding lease, transaction, busy coordinator or poisoned owner cannot be reclaimed through this path. It preserves the open connection and all append/manifest/statistics proof owners.

After 30 s without database use, the connection attempts page-cache release. After 120 s it also flushes cached prepared statements. Active capacity remains 256; resumed database activity starts a fresh idle period. Attached runtimes previously performed durable idle queries every reaper pass before discovering that their attachment prevented unloading. That redundant query path is skipped while attached, allowing genuinely idle caches to cool. Attachment pins and unload claims remain in force.

SQLite's connection API attempts to free reusable connection heap; this does not promise an equal operating-system PSS reduction. The safe existing rusqlite API is used, with no SQL write, transaction or connection reopen. [SQLite API](https://www.sqlite.org/c3ref/db_release_memory.html).

The complete sampled heap experiment confirmed release of live cache objects. In the trial allocator counters, seeded allocated bytes were 776.16 MiB; after page idle they were 664.34 MiB, and after the mixed 130 s point 558.68 MiB. The old version remained 799.95/799.95/808.07 MiB at those same respective phases. These are replacement-tcmalloc class-charged allocations, **not ordinary-glibc PSS and not an additive memory partition**. The 130 s cut includes 24 new calls in four active sessions. Inclusive page-cache/prepare/JSONschema/checkpoint stacks overlap; all raw samples, exact headers and independent Poisson weighting are retained.

Trial ordinary-glibc PSS barely fell after those frees. A controlled same-ELF experiment at 130 s returned **45.78 MiB** from the trial version in **12.77 ms wall /12.71 ms control-thread CPU**; the old version returned 1.87 MiB in 0.34 ms. Both retained all 128 actors and passed the full workload. This is one causal sample per side, not a latency confidence estimate.

The final implementation calls `malloc_trim(0)` once after a reaper pass that actually completes at least one cache-reclamation stage, after all native/store/registry guards are gone. It is not called per connection or per chat turn. GNU libc documents it as MT-Safe and able to return unused whole pages; no live data is reclaimed. Other targets retain their system allocator's normal release policy. No allocator replacement, arena tuning or new dependency was added. [GNU API semantics](https://man7.org/linux/man-pages/man3/malloc_trim.3.html).

[Connection maintenance](../../src/durable/sqlite/idle_cache.rs) · [Runtime eligibility](../../src/runtime/conversation_runtime.rs) · [Batch and attachment ownership](../../src/local_runtime/session_runtime_manager.rs).

## Hot requests, cold resume, rotation and Inspector

| Final comparison | Before → final | Median paired change |
| --- | --- | --- |
| Width 4 settled median ms | 28.00 → 28.79 | +4.35% |
| Width 4 CPU ms/turn | 11.33 → 11.80 | +4.13% |
| Width 8 settled median ms | 55.97 → 55.03 | -1.75% |
| Width 8 CPU ms/turn | 11.60 → 11.64 | +0.34% |
| Cold rotation median ms,122 eligible | 10.08 → 13.14 | +30.52% |
| Cold rotation CPU ms/turn,all 128 | 11.48 → 14.88 | +29.59% |
| Warm rotation median ms,122 eligible | 9.61 → 10.04 | +4.57% |
| Warm rotation CPU ms/turn,all 128 | 11.37 → 11.45 | +0.69% |
| Kept-warm median ms,20 calls | 19.02 → 18.84 | +0.35% |
| Mixed 98 s CPU ms | 680.00 → 640.00 | -5.76% |
| Inspector/history/two Source reads ms | 12.52 → 14.32 | +14.42% |

Cold cache restoration has a real cost: prepared plans and database pages must be rebuilt on the next call. This is the deliberate idle-memory/first-resume tradeoff; it does not alter results, persistence, visibility or replay. Continuously active sessions retain their hot cache budget. The mixed 98 s CPU counter includes idle time and maintenance, not just 20 or 24 active calls. Cold latency excludes four kept-warm and two Inspector-primed sessions; CPU/write counters include all 128. Every cold and warm call and actual same-session gap is kept in the archive, including tails.

The measured write change is shown rather than inferred from smaller objects. This round changes no persisted table, index, event, commit boundary or checkpoint cadence; no large additional write reduction is established. Process `write_bytes` is not physical SSD traffic, and WAL page-owner diagnostics were not rerun. The earlier write/transport/catalog improvements remain the baseline.

The final width 4 cohort has **+4.35% paired median wall /+4.13% CPU**, while width 1 is **+0.33% wall /0% CPU** and width 8 **−1.75% wall /+0.34% CPU**. These adverse cells are not hidden. In all eight primary processes, the complete seed stage lasts 7.87–8.15 s and seed-end→width-8-end 1.95–2.03 s; every session has database writes during seeding, so these hot windows cannot meet the 30 s cache-idle threshold. The batched trim cannot explain that width 4 hot-window difference. The separate f2e trial, with the same ownership/cache implementation, showed width 4 CPU −3.21%; the direction is not stable across cohorts. This does not prove zero ownership overhead or a hot-path speedup. The delivered benefit is lower retained memory with a measured cold-restore tradeoff.

All products ran on a shared host. Two-second compiler observations and quiet preflight are retained; they do not prove exclusive execution. Finite P95/P99 samples and paired medians do not guarantee every-run or production-tail improvement.

## Pi reference, without rerunning Pi or DeepSeek

No Pi or DeepSeek product was executed this round. The previously authorized single Pi 128×8 memory sample remains **310.46 MiB anonymous PSS /373.75 MiB RSS**, unchanged Pi 1cedd327, 128 independent Harnesses/SQLite files/native watchers. Final immediate rustX seed8 memory is **2.39× anonymous PSS /2.16× RSS** relative to that reference. Rust uses four products, Pi one. Pi library/watchEvents does not expose the identical AppServer/Inspector/frozen-request/replay feature set; tool rosters also differ. The entire residual gap cannot be labeled redundant.

There is no matched prior Pi idle endpoint for this new cache policy, and no updated Pi latency/CPU/write comparison. The reference is authenticated against the prior archived audit and is not a new measurement.

## Validation, provenance and replay

Both production stages passed formatting, release/all-target/all-feature Clippy with warnings denied and 17 relevant command groups: **3,256 unique passed tests /three existing ignored**. Eight overlapping artifact-lineage cases count once. Coverage includes 663 SQLite tests, 621 RuntimeClient tests plus one ignored, 43 AppServer tests including exact artifacts, 585 agent tests and 878 local-runtime tests plus two ignored, with the other focused groups included in the unique total. New tests cover old-version immutability/row sharing, matching-row-only tool detach, shared transcript/Serde ownership, page-then-plan release, lease/transaction exclusion and fresh resumed idle periods.

Each stage's 973 source/test/Cargo files and two capacity helpers match its checks and fresh ordinary O3/default build. Correctness executables use a separate root-O0/all-features build. Final native SHA256: `afd7d0ee3124456be5f6449e69660164d332b67311e907b4fcdb7b5ff82fce3d`; conformance: `ab2a2824033d63229395ef14da1eec7d87b9437d6f18744a4c5a2feccb39398b`. Failed compilation/lint/schema/fixture attempts and partial validations remain, with fixes explained by the source diff. A trial post-clean optional numeric-probe coordinator failed before execution; it was not counted as a test/product. The final numeric probe runs before the deliberate package clean.

Final measurements contain eight primary ordinary products (1,235 requests each), four extended cache products (1,515 each) and eight stream/large/tool/cancel functional products. Trial evidence is separate: 12 ordinary/cache products plus two complete sampled-heap and two controlled-return products, and eight functional products. HTTP/native full-history matching, frozen/live/reopen request checks, cancellation partial-output absence, Trace/history/slow reader/detach/reload, SQLite integrity and foreign keys all pass. These mixed products are not pooled into a timing sample count.

The portable replay successfully authenticates **12,097 members**, reparses **28 native products /3,584 consistent SQLite images**, 16 non-chat products, 10 complete heap profiles and both controlled-return products, and reproduces the tables exactly. It executes no product/compiler/git/symbolizer/network. Stored prearchive byte proofs cover deliberately excluded executables, libraries and profiler helpers. Full mutable runtime workspaces, dependencies, ephemeral authentication and duplicated provider stdout are excluded; structured provider reports and consistent images remain.

No replay expiry/pressure policy, lightweight-observer lifecycle redesign, schema-library change or extra persistence structure was introduced. Full browser E2E/whole CI and merging main were not performed; PR485 remains draft with its pre-existing main conflicts.

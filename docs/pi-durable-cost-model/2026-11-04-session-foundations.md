# Reduce empty-session memory and repeated encoding

This round targets the fixed cost of the first loaded session and the repeated cost of each additional loaded session. It also removes an observed duplicate outbound serialization pass. Production is **57c7082ea63b82137c38ccd67e032565d9a1744d**, measured against the saved **d50125842f45721e831d02474bf77bf2b7c62bc5** executable. No Pi or DeepSeek product, build or performance benchmark was run.

| Ordinary endpoint | Before → after | Median paired change |
| --- | --- | --- |
| Subscribed 128 empty, anonymous PSS MiB | 323.61 → 169.03 | -47.75% |
| 128 × 8, anonymous PSS MiB | 743.29 → 585.51 | -21.17% |
| 128 × 8, RSS MiB | 806.79 → 649.13 | -19.52% |
| Width 4 fixed CPU ms/turn | 11.72 → 11.09 | -6.03% |
| Width 4 rotating CPU ms/turn | 13.10 → 12.55 | -6.24% |

These are fresh O3/default native AppServer products with the ordinary GNU allocator, 128 independent conversations/SQLite files/subscriptions and exactly eight seeded turns in every conversation at the memory endpoint. Each primary family has four balanced before/after pairs. Percentages are medians of paired changes; displayed endpoint medians need not have the same ratio. Fixed and rotating activity are separate workloads. Instrumented CPU/heap products are not pooled with ordinary results.

[Complete values, every pair and adverse cell](2026-11-04-session-foundations-tables.md) · [Authenticated raw evidence and portable offline replay](evidence/session-foundations-20261104/README.md) · [Previous ownership/cache round](2026-11-03-idle-retention.md).

## What made an empty session expensive

The previous implementation had two distinct growth patterns. Metadata-only discovery loaded no runtime. Loading the first empty runtime triggered a large process-wide initialization; loading subsequent runtimes repeated tool-schema and HTTP/TLS ownership. Treating the whole empty-session endpoint as SQLite cache or replay would have missed these owners.

| Ordinary endpoint | Before → after | Median paired change |
| --- | --- | --- |
| First runtime beyond metadata-only, MiB | 74.88 → 3.46 | -95.41% |
| Each additional runtime, average of 127, MiB | 1.76 → 1.11 | -37.09% |
| Eight seeded turns per runtime, MiB | 3.28 → 3.25 | -0.68% |

Three changes remove verified repeated foundations:

1. **Use generated meta-schema validation from the maintained dependency.** The old jsonschema 0.26.2 implementation initialized runtime validators for all five supported meta-schemas under one lazy initializer. The first ordinary registry validation retained those trees for the rest of the process. Updating the existing dependency to jsonschema 0.60.2 with its `macros` feature uses upstream-generated meta validators. Registration and invocation validation remain enabled; there is no skip-validation switch, vendored fork or permissive replacement. Tests cover all five drafts, local references, required/type/minimum/additionalProperties and malformed schemas. This does not promise byte-identical diagnostic wording for every invalid schema across dependency versions. [Upstream migration notes](https://github.com/Stranger6667/jsonschema/blob/master/MIGRATION.md).
2. **Share the credential-free default HTTP client.** OpenAI adapters previously constructed a client for the no-retry service and another SDK request client which remained retained after installing that service. Across sessions this duplicated pools and TLS configuration. Both OpenAI and Anthropic now clone one lazy default reqwest client; OpenAI constructs the SDK with that same client. Injected clients remain honored. Endpoint bindings, credentials, headers and retry ownership remain per adapter/request; the shared default has no authorization headers or cookie store. A persistent-connection test serves two SDK bindings with different paths and bearer keys and verifies both reuse and credential separation. Metadata-only commands still do not initialize provider transport.
3. **Share immutable tool input schemas.** Native schemas are generated once per closed native Rust input type. Canonical, selected capability, checkpoint and model-definition owners clone an `Arc<Value>` rather than deeply copying the JSON tree. Model-selectable decoration uses copy-on-write; edits cannot mutate an older definition or historical view. Authored/MCP schemas are not put into an unbounded global cache. The provider SDK receives an owned JSON value only at its required translation boundary. The old GoalUpdate per-registration schema rewrite is removed; its root-object constraint is generated once. Native Rust container types intentionally change; public serialized schemas do not.

The committed v52 wire schema and fixtures are byte-identical under the existing artifact test; they were not regenerated. Target serde_json 1.0.151 retains exactly the previous feature set, including `float_roundtrip`, `preserve_order` and `raw_value`. The newly separate host proc-macro serde_json artifact is not the product JSON implementation. The audited Cargo output records both explicitly.

[Shared transport](../../src/model/adapter/transport.rs) · [OpenAI client construction](../../src/model/adapter/openai/client.rs) · [Native schema ownership](../../src/tools/native/registration.rs) · [Schema validation and tests](../../src/tools/schema.rs) · [Tool definition owners](../../src/tools/types.rs).

## Four concurrent turns and the duplicate encoding pass

| Ordinary endpoint | Before → after | Median paired change |
| --- | --- | --- |
| Fixed width 1 settled median ms | 9.70 → 9.54 | +2.81% |
| Fixed width 1 CPU ms/turn | 11.88 → 11.25 | -5.26% |
| Fixed width 1 process writes KiB/turn | 640.00 → 641.12 | +0.18% |
| Fixed width 4 settled median ms | 29.71 → 28.34 | -3.72% |
| Fixed width 4 CPU ms/turn | 11.72 → 11.09 | -6.03% |
| Fixed width 4 process writes KiB/turn | 667.28 → 667.28 | -0.04% |
| Fixed width 8 settled median ms | 56.57 → 55.95 | -2.77% |
| Fixed width 8 CPU ms/turn | 11.95 → 11.56 | -3.53% |
| Fixed width 8 process writes KiB/turn | 693.05 → 693.06 | -0.00% |
| Rotating width 4 settled median ms | 34.99 → 33.04 | -6.54% |
| Rotating width 4 CPU ms/turn | 13.10 → 12.55 | -6.24% |
| Rotating width 4 process writes KiB/turn | 628.90 → 628.51 | -0.04% |

The earlier +4.13% width 4 CPU cell came from a small fixed-session window: 16 rounds, 64 calls, with the first four conversations growing unevenly. New fixed products retain that definition. The additional rotating family makes 512 calls across all 128 conversations, growing every history from 8 to 12; it is a separate four-pair cohort, not extra repetitions of the fixed case.

The native signal profile identified a complete serde traversal into `serialize::Budget` immediately before the same typed envelope was encoded into an already size-bounded transport record. The public envelope serializer retains that native-budget prepass for generic callers. The private typed transport path now uses the same structural wire encoder directly into the bounded record, making one traversal. The final structural representation preserves native values or adds decimal quotes, so its existing 1 MiB byte bound also bounds the native representation. No JSON cache, persistent buffer or unbounded output is introduced.

Exact UTF-8/escaping and numeric wire bytes match; tests count two public traversals versus one bounded-transport traversal, and exercise exact-limit/one-byte-overflow, oversize notification recovery, response-too-large/outcome-unknown publication and error paths. Queue, record and write budgets remain. Test-only traversal counters are absent from the ordinary executable.

The current-before diagnostic has 1454 samples, including 49 in the public native Budget traversal (3.37% inclusive). The candidate has 1377 samples and zero in that traversal. Serde-inclusive samples change 530→448. This verifies removal of the observed path; the combined production changes and finite sampling do not assign every saved CPU tick to it.

The new four-concurrency CPU result improves in both primary families. It does not retroactively prove that a single function caused the old +4.13% cell, or that memory ownership was entirely free. The older e1d4/current d501/new-candidate profiles are rotating diagnostics and cannot serve as exact attribution of the old fixed window. All profile denominators and inclusive stacks are retained; inclusive groups overlap.

Width 1 settled latency has a **+2.81%** median paired change despite displayed product medians 9.70→9.54ms; these are different arithmetic summaries, not contradictory measurements. Some individual pairs/tails regress. All before/after values remain in the table; this is not an every-run speedup or a production P99 guarantee.

The fourth rotating pair is explicitly adverse: CPU **+14.63%**, settled median **+16.35%**. Its full activity span is 5.04→23.67s, with a maximum free round about 2.29 s. Yet process `read_bytes` counters are 0 on both sides, process `write_bytes` counters are 329,601,024→329,412,608, read system calls 33,083→33,090 and write calls 226,788→227,108. This provides no evidence of a new large read/write amplification in that cell. It does not identify the precise source of its extra CPU or stalls. The other three rotating pairs and all four fixed pairs use less CPU; the finite/shared-host evidence supports an aggregate benefit, not a guarantee of no individual regression.

[Bounded typed transport](../../src/app_server/transport/mod.rs) · [Shared envelope codec](../../src/app_server/wire.rs).

## Remaining memory owners and idle behavior

The allocator diagnostic corroborates fewer live owners: subscribed-empty class-charged live bytes fall 328.57→168.65MiB; seed8 falls 776.61→617.86MiB. At candidate seed8, the three largest disjoint priority groups of estimated requested bytes are `sqlite` 231.88MiB, `other_observation` 132.83MiB, `checkpoint_capture` 108.87MiB. The report does not call all remaining observation or database objects redundant: SQLite schema/plans/pages belong to 128 independent open stores, and prepared observation/replay owners retain current and historical reads. Neither reducing the replay window nor unloading subscribed runtimes was used to obtain the gain.

| Empty subscribed priority group, estimated requested MiB | Before | After |
| --- | --- | --- |
| Schema-validation stack |65.89|0 sampled|
| HTTP/TLS stack |23.48|0.20|
| Tool-catalog stack |30.05|3.77|
| Prepared-capability stack |55.20|29.29|
| SQLite stack |70.01|71.26|

These are the disjoint priority groups within the sampled requested-byte denominator. “0 sampled” does not assert that validation uses no memory. In particular, SQLite was not reduced by sharing schemas or transport. At seed8 the remaining SQLite estimate includes about 231.88 MiB; after page/statement idle it is about 54.02 MiB at the mixed cut. The checkpoint-capture and other-observation stack groups include live heads and historical owners; they are not a direct measurement of replay-only bytes. Their continued presence is not proof that every retained object is necessary or that deleting a whole category would preserve behavior.

The complete sampler exports 12 phases on each side, including 1/8/32/64/128 empty loaded sessions, subscribed-empty, seeded8, page-cache idle, statement-cache idle and unloaded. Every raw header and record sum is checked; independent 64 KiB Poisson weighting matches archived symbolization within integer rounding. Allocator class-charged live bytes, estimated requested bytes and ordinary GNU-allocator PSS are separate denominators. Priority stack categories assign an allocation once by first match; they are useful attribution, not an exact PSS partition. SQLite preparation/page-cache and observation/publication inclusive groups also remain available and must not be added together.

The existing 30 s page-cache /120 s statement-cache idle policy is unchanged. In the candidate diagnostic, class-charged live memory moves 617.86→506.29MiB at 32 s, then 400.91MiB at the mixed 130 s point. The latter has four sessions ×14 turns and 124 sessions ×8 turns. In the separate ordinary cache pair, anonymous PSS at 32 s is 730.16→571.65MiB. Cold eligible 122-session median is 12.84→12.73ms; warm median is 9.58→8.91ms. This one cache pair is diagnostic coverage, not a speed confidence cohort. Kept-warm calls, all cold/warm rows and actual same-session gaps, Inspector/history/two matching Source reads and the 98 s mixed calls/idle/maintenance CPU are retained. Freed live allocation is not promised to produce an equal PSS return; allocator free space and partial pages still matter.

After full unload, candidate allocator live memory is 23.84 MiB (before 96.43 MiB), while ordinary anonymous PSS remains 653.58 MiB after the larger activity/verification phase. Those are different processes/allocators, so they cannot be subtracted as an exact free-space estimate; the live-allocation endpoint nevertheless shows that unloading releases the session owners. Ordinary allocator page retention remains distinct from the loaded-session live-object problem.

The replay window, all historical cuts, independent observation domains, frozen requests, source reads, current cache capacities, idle thresholds, transaction boundaries and durable schema 94 remain. This round does not remove a table, index, commit or history capability. Process write counters are measured rather than inferred from smaller memory owners; they are not physical device writes. No new WAL page-owner experiment was run.

## Pi reference and limits

The prior authorized single Pi sample remains 310.46MiB anonymous PSS /373.75MiB RSS at 128 × 8. Rust's immediate anonymous-PSS ratio changes 2.39×→1.89×; RSS changes 2.16×→1.74×. Subscribed-empty anonymous PSS is now 1.60× the unchanged 105.48MiB Pi reference. It is an authenticated reuse of the older sample, not a new Pi measurement.

Pi's existing sample uses independent Harnesses/SQLite files/watchers. It does not expose the identical native AppServer/Inspector/frozen-request/replay surface; tool rosters also differ. Four Rust products versus one older Pi sample do not establish an equivalent-feature capacity bound, and the residual gap cannot all be classified as redundant. No new Pi latency, CPU, write or idle comparison is claimed.

## Validation and reproducibility

The final source passes 22 command groups: formatting, release/all-target/all-feature Clippy with warnings denied, and focused plus broad model/tool/provider/contracts/SQLite/runtime/client/AppServer/agent/recovery/process/local-runtime tests. **4,379 unique tests pass; nine existing tests are ignored.** Overlapping filters are deduplicated by executable/test name. Credential-dependent provider smokes are not executed. All 974 source/test/Cargo files and two unchanged capacity helpers match the checked source and the fresh ordinary O3/default build. Correctness executables use a separate root-O0/all-features build.

Every native product validates all HTTP messages against session/turn ordering, complete native histories, cancellation without forbidden partial output, SQLite integrity/foreign keys and unload/reload barriers. A new pairwise audit compares every complete provider request body, including tool schemas, nulls and field order, by session/turn: it normalizes no body fields. The separate stream/large/tool/cancel products verify live/frozen/reopened requests; a single pair per case is functional coverage, not performance replication. Inspector/history/two equal Source reads, slow readers, detach and cancellation remain covered.

The successful portable replay authenticates **9,884 members**, reparses **23 native products /2,944 consistent SQLite images**, 8 non-chat functional products, 9 complete HTTP-body pairs, 3 CPU profiles and 24 full heap profiles, and reproduces all tables byte-for-byte. The 23 native products comprise 16 primary ordinary products, two extended ordinary cache products, three signal-CPU diagnostics and two replacement-allocator heap diagnostics. Mixed products do not form 23 timing repetitions. Replay runs only archived Python audits: no product/compiler/git/symbolizer/network. Stored prearchive proofs cover the deliberately omitted five executable images, two root libraries and profiler/helper bytes. Mutable runtime directories, dependency trees, authentication and duplicate provider stdout are excluded; structured full provider reports and consistent backups remain.

All new products run on a shared host. Two-second compiler observations and quiet preflight are retained; they do not prove exclusive execution or absence of interference. The full raw adverse cells and development build/type-adaptation failures remain. Constructor fixes use `Arc` conversion/copy-on-write and borrowed assertions; no test deadlines or expected contracts were weakened. Full browser E2E/whole CI and merging main were not performed. PR485 remains draft with its pre-existing main conflicts.

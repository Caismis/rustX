# Exact Source metadata reuse: fresh structural comparison

Production baseline is `5457e1c34106b8d49de0edfb40b49ca8267d0383`; production
candidate is `76ecc9d765e5c63080950e7dc5710a32b0b1caa7`. The later commits
242597cb/a42e7bfa repair summary validation and a test-only lint; 6b044654 repairs
the new cold benchmark fixture. They do not change the candidate production tree.
Verified origin/main remains `3c355f152d405a201482d53b6e45497df4f44a7f`.

The candidate borrows exact span/path/key/container/node metadata from Source's
existing byte index and selects fragment spans before generating previews. It
retains bounded subtree validation when a truncated preview cannot prove numeric
or depth validity. Original bytes, witnesses, authenticated pages and roots remain
byte-identical in differential tests; this stage changes no persisted format.

## Fresh paired results

Three paired, alternating runs per case, clean processes and stores, identical
optimized builds and local HTTP/SSE scripts. Each ratio below is the median of
same-pair candidate/baseline ratios; it is not a ratio of unrelated medians.
CPU includes the product and driver, at 10 ms counter resolution. Writes are
process-attributed lifecycle writes through cleanup, with evidence JSON emitted
after sampling; they are not physical-device writes. Anonymous PSS is the maximum
of observed samples, not a measured peak. Native allocator counts are unavailable.

| Case | Turn wall ratio | Turn CPU ratio | Lifecycle write ratio | Sampled anonymous PSS ratio |
|---|---:|---:|---:|---:|
| short-small | 0.9981 | 1.0063 | 0.9997 | 1.0078 |
| short-large | 0.9400 | 0.9579 | 1.0021 | 0.9993 |
| large | 1.0251 | 1.0000 | 0.9997 | 1.0208 |
| stream | 0.9836 | 0.9048 | 0.9975 | 1.0018 |
| tool | 0.9421 | 1.0714 | 1.0000 | 0.9998 |
| cancel | 1.0210 | 1.0000 | 1.0000 | 0.9928 |

Repeated large-history work improves by 6.0% wall and 4.2% CPU in this matrix.
Short-history performance is essentially flat; one-response large and cancellation
wall ratios are adverse, and Tool CPU is adverse. No general memory or disk gain
is established. These measurements retain every slow sample.

The marginal append control is intentionally separate from HTTP/context work.
The Source stage does not change native append; variation and adverse samples
remain visible. Warm CPU covers the whole append phase including driver overhead.
Cold CPU sums the recorded operation windows, which include counter/sampling
overhead; reopen/close work belongs to separately retained lifecycle counters.

| Mode | Seed history | Append wall ratio | Append CPU ratio | Lifecycle write ratio |
|---|---:|---:|---:|---:|
| warm | 0 | 1.0212 | 1.5000 | 1.0000 |
| warm | 64 | 0.9928 | 0.6667 | 1.0000 |
| warm | 512 | 1.0079 | 1.0000 | 1.0000 |
| warm | 2048 | 1.0691 | 1.2000 | 1.0000 |
| cold | 0 | 0.9972 | 1.0000 | 1.0000 |
| cold | 64 | 1.0553 | 0.6000 | 1.0000 |
| cold | 512 | 1.0632 | 1.0000 | 1.0000 |
| cold | 2048 | 1.0392 | 1.2857 | 1.0000 |

Each sample measures 32 appends. Coarse CPU ticks make ratios for these short
windows noisy; the raw counters and per-operation observations remain available.
The first cold attempt failed all 24 samples because the fixture used the read-only
management opener. Those failures and empty summary output are retained. The
corrected fixture uses the writable opener before each measured append. Both
production revisions were rebuilt with that identical fixture, and all 24 rerun
samples passed. The original successful runtime/warm/Pi samples remain unchanged.

## Same-provider Pi comparison

Actual Pi Durable SQLite is pinned to
`90d863f7c1183b965f099855715d6e88c7b8a52c`, using Node 24.19.0 and its production
OpenAI completions adapter. Both engines call the same local fake HTTP/SSE provider
as controller-owned sibling processes. Provider lifetime extends beyond product
cleanup; its child I/O cannot enter product counters. Native Tool schemas remain
explicitly different (rustX 7, Pi 4); only legitimate identities are normalized.
No real model API or inference credential is used.

The 36 successful paired engine samples contain 846 complete requests. The
Source revision comparison separately contains another 846 requests in 36
samples. Full ordered expected/wire/captured/frozen/reopened oracles pass, with
18 paired core-request comparisons in each matrix. In the Pi matrix, median
short-small wall is 66.39 ms/turn for rustX and 4.88 ms/turn for Pi; CPU is
98.13 versus 9.38 ms/turn. Lifecycle writes are 127.99 versus 8.88 MiB. These
include the documented driver and verification work and do not establish pure
engine/DB costs or equivalent native Tool schemas. This is not the old in-process
fake comparison and does not reuse its 49–57x ratios.

## Validation and reproducibility

Source stage validation passed 22 Source tests, 11 composed tests, 373 durable
SQLite tests, and 30 benchmark/summary regression tests. The differential oracle
checks exact generated pages/root and original chunks, lexical JSON/u64/key
aliases, dense fields, malformed numeric/deep input, and physical byte-read
reduction. Independent review found and fixed cross-directory warm/cold summary
mixing and omitted cold memory samples. Negative regressions cover both defects.
The structural baseline's FULL/Merge pass is historical evidence, not validation
of later production changes; final combined HEAD still requires FULL/Merge.

Binary SHA-256 identities:

- structural-baseline: `2b12bfb992ef706ffa74ca026bfebca69c5eb9f15c56b9ac34ae41d86c3fc757`; fixture `6a9c041588babca4c879facf5d002e8c1d4384dd1d506940da5c2fbf0255708a`.
- metadata: `5f006c64b0f207486888a1e2759564069709fe3731d596aaeaba5fb0918ca740`; fixture `6a9c041588babca4c879facf5d002e8c1d4384dd1d506940da5c2fbf0255708a`.
- structural-baseline-cold-fixed: `ab315719267cecb3c4f74527349c6f972fd4dbea839dbdb00585b2643f714e5c`; fixture `0c9b61828fe19270f32308eb382cc2dbe3f0dbb45abceff3790f1cf98e056f16`.
- metadata-cold-fixed: `ab315719267cecb3c4f74527349c6f972fd4dbea839dbdb00585b2643f714e5c`; fixture `0c9b61828fe19270f32308eb382cc2dbe3f0dbb45abceff3790f1cf98e056f16`.

The archived manifests retain commands, UTC boundaries, complete logical/allocated
DB/WAL/temp footprints, startup/steady/close/reopen/cleanup counters, provider
startup/control costs, and every successful/failed sample. Formal matrices ran
without concurrent builds or profilers. Diagnostic profiles run separately.

## Separate profile evidence

Four fresh 250 Hz CPU profiles retain independently checked raw denominators in
[metadata CPU counts](evidence/metadata-cpu-counts.json). Baseline/candidate steady
short-small totals are 747/698 samples, and short-large 806/772. SQLite preparation
has 138/747 and 160/698 cumulative samples for short-small, and 148/806 and 141/772
for short-large. These overlapping cumulative stacks are diagnostic attribution,
not additive elapsed-time percentages or a precise causal decomposition. Driver
counter helpers remain visible (23–27 samples). Profile timings include start/stop
and control overhead and are excluded from the headline matrices.

Two separate complete-lifecycle syscall traces reconstruct every operation with
zero unresolved lines. Both create/write 442 original-byte files (402 at most
4 KiB) and 40 disk-index files; Source-index writes remain 1,966,080 bytes. Thus
metadata reuse does not eliminate the carriers or their connections/index work.
Original-byte file writes are 692,662/692,615 bytes; generated native identities
account for slight byte variation. Successful syscall counts/bytes are logical
process operations, not physical-device traffic or whole-runtime SQL statement
counts. The Source optimization avoids redundant reads/parses; SQLite preparation
and persisted page construction remain substantial targets for the next stage.

Original-byte `pread64` calls fall from 91,712 to 40,753 (55.6% fewer) in these
short-small traces. Disk-index `pread64` calls remain 1,000; disk-index open calls
remain 160. Those opens approximate connection activity and are not SQL counts.
The compact [evidence archive](evidence/metadata-stage.tar.xz) preserves raw
profiles and successful/failed samples, with a per-file SHA-256
[manifest](evidence/metadata-stage.tar.manifest.json). Original evidence is retained
in the cloud workspace.

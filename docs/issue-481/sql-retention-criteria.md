# Prospective SQL-retention confirmation rule

Defined after inspecting the original three removal pairs, before collecting
confirmation pairs 3–5. This is a practical selection rule, not a preregistered
statistical test or a confidence claim. No further matrices are planned for this
choice. Every original, slow and failed sample remains retained.

Compare the existing immutable compact Source executable WITH SQL reuse
(`37570149…f9cfd55`, before) against the executable WITHOUT SQL reuse
(`2edfdae2…82ed293`, after). Run three additional interleaved pairs of all six
workloads, starting at pair 3. Together with original pairs 0–2 this balances
execution order at six pairs. No rebuild, competing build, profiler or compression.

All request/result oracles must pass. Primary workloads are short-small and
short-large, each with 32 measured turns; single-turn CPU is too coarsely resolved
for the primary decision. Ratios are without/with reuse, so values above 1 favor
retaining reuse. Compute each case's median paired ratio first, then the geometric
mean of those two case medians (equal workload weight).

Retain SQL reuse only if ALL conditions hold:

1. Aggregate primary CPU ratio is at least 1.03 (a practical 3% threshold),
   both primary case medians are at least 1, and at least four of six pairs
   favor reuse on CPU in EACH primary case.
2. Aggregate primary wall ratio is at least 1; neither primary wall median is
   below 0.98 (no >2% observed wall penalty for reuse in either primary case).
3. For each primary case, reuse does not increase median paired sampled RSS or
   lifecycle process writes by more than 2%: without/with ratios at least 1/1.02.
4. No single-turn workload has a median paired wall ratio below 1/1.10
   (an observed >10% penalty for reuse). Report every singleton CPU result,
   including adverse quantized results, without treating it as precise evidence.

Otherwise retain the simpler implementation without the extra statement-cache
operations, report the result as insufficient/inconclusive evidence for reuse,
and state any adverse removal measurements plainly. Do not select an isolated
best run, discard outliers, average unrelated medians, or claim that absence of
threshold evidence proves zero effect. The diagnostic allocation profiles and
implementation complexity remain supporting context, not substitutes for the
paired decision criteria. Final retention remains subject to independent review.

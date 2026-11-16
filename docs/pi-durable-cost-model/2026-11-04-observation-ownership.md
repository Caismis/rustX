# Share observation bodies and compact retained storage

This is the bounded ownership phase requested after the session-foundations round. It shares repeated native and normalized transcript rows within their respective representations, shares descriptor and checkpoint metadata owners, and releases unused capacity in frozen replay vectors. No Pi or DeepSeek product/build/benchmark was run. SQLite's 30 s /120 s idle policy, active cache capacities, replay count/byte window and persistence boundaries remain.

Production: e7289ac18f16d3bdde6c822e4260c0496791f28a. The row-sharing layer is f881e0be384efa8ddb00cadd532ea2d60e7fb472; the final vector-storage layer is measured against saved 57c7082ea (report 659023375), not an older pre-schema/TLS baseline.

| Ordinary process endpoint / metric | Before | After | Median paired change |
| --- | --- | --- | --- |
| 128 empty subscribed, anonymous PSS MiB | 169.51 | 168.41 | -0.65% |
| 128 × 8, anonymous PSS MiB | 584.89 | 480.75 | -17.75% |
| 128 × 8, RSS MiB | 648.70 | 544.94 | -15.93% |
| Eight-turn increase per session, anonymous PSS MiB | 3.25 | 2.44 | -24.73% |
| Fixed width 4 CPU ms/turn | 10.86 | 10.78 | -2.89% |
| Rotating width 4 CPU ms/turn | 11.12 | 11.07 | -0.96% |

[All values and adverse cells](2026-11-04-observation-ownership-tables.md) · [Raw evidence and offline replay](evidence/observation-ownership-20261104/README.md) · [Previous foundations round](2026-11-04-session-foundations.md).

## What the concrete owners showed

The private native census traverses actual retained events, current native rows and the current prepared row head. It counts unique objects by address and records reference-role totals, inline DTO sizes and JSON representation lengths. The private code performs address deduplication; the archived census contains its counts, not individual pointer sets. It is not a complete allocation inventory or a PSS partition. The root-O0/default census binary is intrusive and contributes no ordinary performance or PSS sample. Actual closed session/turn bindings select 126 conversations which remain at exactly 8 turns after the functional scenario; two conversations with additional turns and empty reloads are excluded.

Every selected baseline conversation retained **8 replacement checkpoints**, with roughly 100–107 total replay events. It did not retain one full checkpoint per event. The baseline transcript median was about 232 unique row objects per conversation; across 126 this accounted for 33.38 MiB of inline row storage alone. Current pages, prepared display rows and historical checkpoint copies were not one owner end to end.

| Representation | Before refs / unique median | After refs / unique median | Before / after inline total MiB |
| --- | --- | --- | --- |
| transcript_rows | 239.5 / 231.5 | 236.0 / 42.0 | 33.38 / 6.17 |
| descriptor_rows | 172.0 / 64.0 | 172.0 / 16.0 | 7.81 / 1.95 |
| binding_rows | 1444.0 / 373.5 | 1439.0 / 368.5 | 8.55 / 8.47 |
| frontier_rows | 3615.0 / 800.0 | 3613.0 / 797.5 | 6.14 / 6.13 |

The material owner-count reductions are in transcript and descriptors. Binding/frontier unique counts remain close; sharing their representation removes conversion work without demonstrating a large reduction of those unique bodies. No large metadata-memory saving is inferred from that code change alone.

A second, small native diagnostic measured the retained vector allocations themselves. For six conversations at exactly 8 turns, median unused frontier-vector space was 394,136 bytes/session, and event-vector space 131,600 bytes/session—about 0.5 MiB of unused capacity per conversation, not ordinary PSS. The two sources of slack were different: spare event slots after mutable aggregation, and Rust's in-place collection retaining a much wider allocation when 64-byte frontier values became 8-byte owner references. A preserved-library reproduction shows 36 references requiring 288 bytes but retaining 2,304 bytes. Source/code/data and the actual native capacity records are archived. These small diagnostic results are descriptive, not an additional paired capacity cohort.

| Median unused capacity bytes/session | Before diagnostic, six sessions | After diagnostic, 126 sessions |
| --- | --- | --- |
| events | 131,600 | 0 |
| metadata_frontiers | 394,136 | 0 |
| metadata_bindings | 176 | 0 |

The ordinary 8-turn census has no retained Trace rows. The prepared Trace copy path therefore was not treated as a large contributor to this workload. No new Trace caching or lifetime mode was introduced.

## One immutable row through the read and checkpoint boundaries

Transcript page arrays now contain immutable row references. Native response/body proof owners, installed pages and historical read-domain events share the exact validated native row. The reader still rechecks current descriptor/Tool/response bytes and dependencies before selecting that owner. A legal changed header or response creates a new row; a corrupt read still fails. Source proof health is never replaced by pointer identity.

The native body-preparation owner borrows that same row's three body fields for normalization instead of retaining an additional owned body copy. Current response decoration remains separate from immutable body identity. Bounded normalized capture retains one materialized display-row owner for its exact prepared result; repeated checkpoints and extended heads clone the owner rather than rematerializing the row. Native and normalized owners remain distinct where normalization or admission changes representation, including nullable/nonfinite and opaque-JSON cases. There is no global interning or digest-based reuse.

Unclipped descriptor rows likewise keep their original immutable owner into the final checkpoint. Tool clipping uses copy-on-write for only the affected row and keeps the existing content/frontier truncation rules. Checkpoint content/frontiers use the same existing metadata fragments as projection publication, removing the old conversion to fully owned rows and back. Inspector materialization always obtains a current store descriptor before sharing a resolved binding.

The new rows and heads have no links to an older head. A head keeps its own order/cut and references the rows it actually needs. Tests cover repeated and extended heads, changed decoration, clipped/mutated rows, older serialized bytes, source proof/rollback/corruption isolation and incremental/checkpoint equivalence. Rust container types intentionally change; there is no compatibility path. Serialized v52 schema/fixtures remain byte-identical.

[Transcript owners](../../src/runtime_client/checkpoint/prepared_transcript.rs) · [Borrowed normalization and materialization](../../src/runtime_client/checkpoint/prepared_transcript/body.rs) · [Current proof boundary](../../src/durable/sqlite/observation_checkpoint.rs) · [Checkpoint metadata](../../src/runtime_client/checkpoint.rs) · [Inspector binding reads](../../src/runtime_client/host.rs).

## Freeze the required storage rather than a mutable builder's capacity

Frontier preparation explicitly allocates the finite pointer array. It no longer lets an owned-value map retain the input's larger byte allocation. Canonical transcript conversion also builds its pointer array directly, eliminating the intermediate wide vector. At the single replay-retention boundary, owned event/metadata vectors release unused slots recursively. Shared bodies are not modified. Encoded values, ordering, cursor/cut, admission charge and the 4096-event/16 MiB replay service window do not change.

Regressions verify the frontier output's capacity bound and byte-identical event encoding before/after vector compaction. This is storage compaction, not expiry or budget reduction. The sampler's allocation-origin groups can move when a reallocation happens in a different function; total live allocations and concrete object/capacity observations are the stronger evidence, rather than interpreting every stack-group change as deletion of an entire subsystem.

[Frontier storage](../../src/runtime_client/projection/prepared_capture.rs) · [Replay storage boundary](../../src/runtime_client/projection.rs).

## Ordinary memory, deep history and operational cost

The separate deeper-history cohort uses eight subscribed sessions and two balanced pairs, with memory sampled at 8/16/32/64 turns. Anonymous PSS at 8 turns is 57.79→51.97 MiB, and at 64 turns 177.67→122.52 MiB. Exact histories, cuts, cancellation and unload/reload checks pass in each product. The new row/head objects hold no parent-version link; the existing bounded replay ring still owns the versions it serves. This finite experiment does not establish a constant PSS bound for arbitrary history, or pool eight-session results into the 128-session cohort.

| Activity / metric | Before | After | Median paired change |
| --- | --- | --- | --- |
| Fixed width 1 settled ms | 9.84 | 10.34 | +1.89% |
| Fixed width 1 CPU ms/turn | 10.94 | 11.25 | +2.94% |
| Fixed width 1 process writes KiB/turn | 640.62 | 640.88 | +0.06% |
| Fixed width 4 settled ms | 28.17 | 28.23 | -1.72% |
| Fixed width 4 CPU ms/turn | 10.86 | 10.78 | -2.89% |
| Fixed width 4 process writes KiB/turn | 667.38 | 667.34 | +0.02% |
| Fixed width 8 settled ms | 52.22 | 53.55 | +2.34% |
| Fixed width 8 CPU ms/turn | 10.90 | 10.86 | +0.36% |
| Fixed width 8 process writes KiB/turn | 693.62 | 693.12 | -0.09% |
| Rotating width 4 settled ms | 28.48 | 27.58 | -0.37% |
| Rotating width 4 CPU ms/turn | 11.12 | 11.07 | -0.96% |
| Rotating width 4 process writes KiB/turn | 628.44 | 628.52 | +0.02% |

Four balanced pairs per fixed/rotating family; all paired values, P95/P99 and adverse cells are preserved in the complete tables. The first deliberately held round is excluded from latency and included in PID CPU/write windows. Changes above are medians of paired ratios, not ratios of displayed product medians. CPU improvement is not assigned to one code change; no every-run speedup or production-tail guarantee is implied. Fixed width 1 has +1.89% settled latency and +2.94% CPU paired changes; width 8 settled latency is +2.34%, and its P99 is +10.20%. One rotating pair increases CPU by 3.28%. These costs remain visible. The earlier large rotating-CPU adverse cell is not stably reproduced; this round does not establish its precise retrospective cause. PID CPU endpoints have 10 ms granularity.

The unchanged 30 s /120 s SQLite policy is exercised by one separate ordinary cache pair. Anonymous PSS at 32 s is 571.77→466.17 MiB. Cold eligible-session median is 12.62→12.83 ms; warm median 9.01→10.04 ms. Inspector/history/two identical Source reads take 15.70→14.43 ms. This is functional and diagnostic coverage, not a timing confidence cohort. The 32 s point is exactly 128 ×8, whereas the mixed 130 s point has four sessions ×14 plus 124 ×8. Actual kept-warm gaps and 98 s mixed calls/idle/maintenance CPU remain in the tables; the latter is not CPU per turn.

The separate replacement-allocator experiment has twelve complete 64 KiB Poisson heap exports per side. Class-charged live allocation at seed 8 changes 616.94→507.31 MiB. Estimated requested bytes change 558.71→461.00 MiB. The largest candidate priority stack groups are `sqlite` 229.45 MiB, `other_observation` 90.87 MiB, `checkpoint_capture` 45.18 MiB. These groups classify allocation origins, not semantic owners or ordinary PSS. All raw header sums and independent Poisson corrections are checked; no PSS ratio is computed from this allocator experiment. Immediately after explicit unloading, ordinary anonymous PSS remains 651.83→523.93 MiB, whereas the separate replacement-allocator live endpoint is 23.81→23.92 MiB. The functional loaded count is zero. Object release and GNU physical-page return remain different observations; this round introduces no new allocator-return policy.

Process write counters are reported directly and are not device traffic. The change adds no durable row/index/commit or new recovery representation; smaller memory owners do not imply fewer SQLite pages written.

The authenticated older Pi sample is reused unchanged: 310.46 MiB anonymous PSS and 373.75 MiB RSS. Immediate Rust anonymous-PSS ratios are 1.88×→1.55×. Empty-session anonymous PSS is 1.60× the prior Pi reference. This is descriptive reuse of one earlier sample with different service/tool/inspection surfaces; it does not identify all residual memory as removable. No Pi or DeepSeek process, build or performance benchmark ran.

## Acceptance and PR closing conditions

All **22 command groups pass**, including formatting and release/all-target/all-feature Clippy with warnings denied: **4,384 unique tests pass /nine existing ignored**. All 974 source/test/Cargo inputs and two unchanged capacity helpers match the frozen checks and fresh O3/default build. Correctness uses separate root-O0/all-features artifacts. The serialized v52 schema/fixtures are unchanged and their conformance tests pass. Native canonical/full-wire history, independent proof failures, exact frozen/live/reopened requests, stream/large/tool/cancel, slow-reader, Inspector/Source and unload/reload checks pass. Development build/lint/test/audit failures remain archived; focused pretests are not counted again.

Successful offline replay authenticates **10,244 members**, independently reparses **28 native products /2,984 consistent SQLite images**, eight non-chat functional products, 11 exact full HTTP-body pairs, three native owner/capacity cohorts and 24 complete heap profiles, then reproduces all tables byte-for-byte. The native products comprise 20 primary ordinary processes (eight fixed, eight rotating, four deeper-history), two extended ordinary/cache processes, two replacement-allocator heap processes and four private census/development products. They are not 28 repetitions of one performance test. The initial development census had an empty owner selection and contributes no ownership result. Replay invokes only archived Python audits, no product/compiler/git/symbolizer/network. Stored prearchive proofs cover omitted executable/library/helper bytes. Full structured provider bodies, closed consistent images, private patches/modules, failures, raw profiles and frozen sources remain; mutable workspaces, secrets and actual binaries/dependency trees are excluded.

This phase meets the agreed conditions: the same 128×8 memory and chat-stage increment improve, the removed owners/capacities are identified, the new objects have no parent-version chain, and deeper history, fixed/rotating activity, cold resume and Inspector/Source have recorded cost and correctness coverage. It does not require 300 MiB, a new Pi multiplier or unlimited follow-on caching work. Remaining native versus normalized representations are not all labeled redundant.

PR #485 remains draft. A read-only `merge-tree` assessment pins main to `b0aa75fe110ca53a1dfc0637280f2f08508c8b7e`: main is 485 commits beyond the shared ancestor, with 383 conflict records /387 unmerged paths, and the wire protocol has advanced from v52 to v62. No merge was applied. The user explicitly chose to finish this ownership phase first and handle that protocol/integration migration independently. Main reconciliation, full integration CI and browser E2E remain explicit PR merge gates; the local checks above do not replace them. This ownership phase is complete at its measured source, without claiming that the whole PR is merge-ready.

Shared-host observations include compiler/load information but do not certify exclusive execution. Formal medians, profiler experiments and functional products have different purposes and are never pooled. Single adverse cells and development failures remain in the evidence. No every-run speedup or production-tail guarantee is claimed.

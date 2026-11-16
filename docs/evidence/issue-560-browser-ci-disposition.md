# Issue #560 browser fixture follow-up

## Actual hosted failures

[FULL run 43732239528](https://github.com/Caismis/rustX/actions/runs/43732239528)
tested candidate `54dce1566b90ce9a39acabf121e3ed77b9cb8610`, base
`33234e5b19f9074924a5fba1315c23dcf7210c32`, and merge
`a920d809e9d010ebf0035cbc080362c16e3a9fde`. It exposed two distinct fixture
failures. [Shard 1](https://github.com/Caismis/rustX/actions/runs/43732239528/job/132869429327)
reported 308 passed, 2 intentional skips and 1 failed;
[shard 3](https://github.com/Caismis/rustX/actions/runs/43732239528/job/132869429326)
reported 241 passed and 1 failed. Browser coverage correctly rejected the failed
shards. Their successful cases do not establish current-head full acceptance.

The local raw log SHA-256 values are respectively
`21208ce40a99c87c9c5e44a6ec22a4dcbdb3f6d7e4a99da9bb52c1e9f373905a`
and `13c95138523d3c98d9c2f7079c213ac8c185d2346138345cab2419ff28cffe8d`.
The convergence trace was downloaded from
[artifact 14169619220](https://github.com/Caismis/rustX/actions/runs/43732239528/artifacts/14169619220):
12448107 bytes, SHA-256
`20d5861209f92a57236dacf724296bd539af4b4066f331d486b47fb09e1150db`.
Local execution logs below are separate workstation receipts, not uploaded artifacts.

## Convergence: native settlement before rendering

The old test released only the real `thread/start` acknowledgement, then charged
attachment, admission, four provider generations and three native Tools to the
five-second heading assertion. In the actual trace, that assertion started at
327520.079 ms; final native text was forwarded at 332530.865 ms, **10.786 ms after
its deadline**. Attachment, admission and Write/Bash/Read succeeded. The later
failure screenshot already showed the heading. This identifies an incorrect
fixture boundary; it does not establish a production execution-speed regression.

The existing wire probe now supports a passive notification/retirement observer.
It forwards frames unchanged before observation and performs no additional RPC,
attachment, admission or runtime work. After creation, the test resolves the
real browser target, observes its unique `attempt_started`, and requires that
same target and Attempt's `attempt_settled` with completed/stop outcome. Live
observation and catch-up run in one synchronous turn. Target closure and
transport retirement reject the wait. Failed/cancelled outcomes fail explicitly.

The existing five-second heading assertion, 120-second case deadline, retained
composer identities, single creation/admission, exact execution settings and
Tool/process/accessibility assertions remain. The native witness establishes
completion before applying a rendering assertion; no timing retry or larger
budget is added.

## Reading: remove unused fixture Tool preparation

The unchanged reading fixture exhausted its 240-second preparation deadline
with 273 completed Attempts and Attempt 273 still running. The latest eight
settlement observations were approximately 718–948 ms. The actual diagnostic
records continued native progress, not a lost terminal cursor. The reading
fixture, follow helper and provider scenario were unchanged from the base.
Earlier [default-disk evidence](../issue-467-validation.md) also records this
preparation exhausting the same deadline; that older fsync/throttling diagnosis
is not assigned to this new failure without a matching native stack.

A local ordinary-disk diagnostic completed all 300 Attempts in 105.836 seconds.
Its 2934 event folds took 136.36 ms in total, so replacing the TypeScript fold
would not remove the material cost in that local sample. No native publication
cache or production optimization is introduced on the basis of that observation.

The exact `web_reading_surface` scenario contains 300 Text/Finish steps and one
later gated Text/Finish step, with no Tool calls or Tool-schema expectations.
Its fixture no longer prepares the 12 unused builtin Tools. Every other scenario,
including the historical `web_reading_scale_1000` measurement, retains its original
Tool list. The ordinary runtime, provider, SQLite, workspace and filesystem remain.
`reading.spec.ts` is byte-identical: 300 fresh real native settlements, transcript
distance greater than 512, exact original source/fork cuts, detached/latest gates,
bounded reads, both locales and all presentation assertions are preserved.
Neither the 240-second fixture nor 120-second browser deadline changes.

Three fresh-fixture diagnostic samples per configuration ran serially in order
original/minimal/minimal/original/original/minimal:

| Sample | Original preparation (s) | Reading-only Tool selection (s) | Original / minimal native CPU ticks | Real settlements in each |
| --- | ---: | ---: | ---: | ---: |
| 1 | 105.836497 | 52.330504 | 34202 / 17349 | 300 / 300 |
| 2 | 107.278911 | 52.832667 | 34594 / 17489 | 300 / 300 |
| 3 | 106.343637 | 64.742417 | 34390 / 21624 | 300 / 300 |

Every sample also passed the unchanged browser case. The local host is not
isolated; ordinary disk and OS caches are retained, and host Node is 24.21.0
(CI uses 24.22.0). Playwright and its pristine browser image remain pinned to
1.63.0. These observations support removal of unused preparation in this exact
fixture. They do not compare equal capability workloads, prove a compiler/runtime
speedup, quantify the hosted gain or establish physical-media crash durability.
The diagnostic `measure: true` and success-only profile printing are restored
exactly; neither is added to the final test. The portable companion retains
actual sample inputs, commands, statuses, source hashes and local log hashes.

## Validation and acceptance scope

Browser plugin is unavailable; validation uses the repository's existing pinned
Playwright container and scripts. The flow remains New Conversation through real
native completion, then process/Models interaction; the reading flow retains its
300-Attempt native history and gated distant/live navigation. Desktop and mobile
checks, accessibility and page-error assertions remain in their existing owners.
The final three-file fixture change passes TypeScript checking and a fresh
combined run of both rendered cases against the real App Server and Product Host
(2 passed). These local checks do not establish hosted acceptance.

The earlier [macOS correction](issue-560-macos-ci-disposition.md) and its corrected
Rust source identity remain separate. Earlier B0/B1/B2 measurements keep their
actual measured sources, Tool inputs and artifact identities. This follow-up
changes browser fixture inputs and claims only the preparation observations above.

Final acceptance requires fresh functional checks, independent review, and FULL,
every selected job and Merge verification at the PR's final current head. Exact
commit/run links and final results are recorded in PR metadata rather than
restamping historical receipts.

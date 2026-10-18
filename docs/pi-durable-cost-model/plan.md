# Pi Durable cost model: authorized implementation plan

The user explicitly authorized all six changes on 2026-10-18, including replacing
stronger independent witness/publication guarantees with behavior comparable to
actual Pi Durable SQLite. This supersedes those preservation requirements for
this dependent change; it does not permit ignoring I/O errors, losing committed
final results, unbounded lazy reads or claiming exactly-once external tools.

## Baseline and ownership

- Preserve reviewed `0c3f692abeaab590215cc953b66b28d0cf64f99e`, its measurements and
  withdrawn/repaired evidence unchanged in history.
- Depend on #484 at `7d659c06f692277d4711c54b579f9fe0f8d8792d`. That adds only a
  deterministic real-Unix fixture cleanup fix and operation-level diagnostics.
  Its new FULL run is 39117994005; the prior macOS failure remains documented.
- Work in branch `codex/pi-durable-cost-model`, separate worktree
  `/workspace/rustX-pi-durable`, and a separate draft PR based on #484's branch.
  The original issue explicitly preserved guarantees this work intentionally
  changes; separate review and dependency make that distinction concrete.
- Pi reference is `packages/durable` SQLite at
  `90d863f7c1183b965f099855715d6e88c7b8a52c`, not coding-agent JSONL.
  Both stores already use WAL/NORMAL. No real inference APIs or credentials.

## Six reviewable changes and acceptance boundaries

| User item / implementation order | Code owners and change | Acceptance evidence |
|---|---|---|
| 1 / first: repeated template work | `durable/sqlite/observation_request/composed/{nodes,fragments}.rs`: derive bounded scalar template metrics once per new leaf/template; reuse the base hole width and path bounds. No persistent cache authority. | Single-template, multi-template and reused-leaf fixtures; exact legacy widths, root/page bytes, large-u64 classes, same-buffer tamper rejection. Count actual loads/hole encodes and measure Source-only short cases. |
| 3 / second: small Source bytes | `runtime/read_layout.rs`: bounded resident byte carrier alongside resident navigation, with one-way lossless spill at the bound. Preserve independent reader/pin/clone/Fork lifetimes. | Threshold±1, split writes, lexical bytes/u64/order, failed spill, cloned readers, independent closure, large cold physically bounded reads; temp files/connections, anonymous memory and write measurements. |
| 2 / third: lazy observation | `durable/sqlite/observation_{request,state,trace}*`, `runtime_client/{checkpoint,projection,host}` and App Server resource reads: retain minimal committed descriptors; generate expensive previews/navigation only on demand using bounded owner-scoped work and truthful readiness/errors. | No eager preview/index work for unread observations; bounded first-read budget and pagination; cancellation/close joins work; unavailable/pending/ready states are consistent at the requested history cut; real Web/TUI/App Server reads. |
| 6 / fourth: simpler inspection identity | Composed Source authentication, request capture witnesses and inspection persistence/readers: remove independent original-byte/field authentication machinery that exceeds the revised contract. Native committed conversation/history remains authoritative; inspection describes committed content, not a cryptographic original-buffer witness. | Inventory every removed guarantee and API field; advance development gates/readers/docs together, reject unsupported layouts; malformed/truncated/corrupt committed records fail explicitly; meaningful history and independent archive/Fork remain coherent. No dual writes or legacy fallback. |
| 4 / fifth: atomic final settlement | `publication/*`, `durable/sqlite/projection_publication.rs`, native canonical/event/staging commit owners and recovery consumers: combine final answer acceptance, terminal state and required metadata into one native transaction; remove independent P/U/C cuts where they no longer express a retained contract. | Deterministic crash/I/O gates before, during and after commit: final result is wholly absent or wholly committed; commit-before-durable-ack; reopen preserves every acknowledged final; tool result/state consistent; no fabricated successful commit. |
| 5 / sixth: throttled partials | Stream publication/coalescing and live-generation persistence: live display can lead durable partial state; bounded trailing partial snapshots, one persistence operation in flight, finalization drains/fences it. Start from Pi's 100 ms default, validated by controlled clocks/gates. | Fast short answers avoid partial commits; paced streams demonstrate bounded cadence; final atomic result replaces partial; abort/crash reveals only last committed partial with explicit interrupted status; no stale partial can land after final; write errors surfaced; real Web streaming/cancel/tool workflows. |

Items 2 and 6 share inspection interfaces; item 2 first establishes minimal
truthful descriptors/readiness, and item 6 removes the superseded authentication
contract and its persisted machinery. Items 4 and 5 share settlement ownership;
final atomic settlement lands before throttled partials depend on it. Avoid an
unrelated generic cache or incremental persistence framework.

## Revised recovery contract to implement

Pi's `harness/generation.ts` commits partials at most every configured interval
(default 100 ms) with one commit in flight, stops the timer and waits that commit
before outcome classification, then classifies a terminal message and clears its
partial atomically. Its scheduler marks `abandonOnRestart` tasks as restart-aborted;
it does not blindly rerun those tasks. Its context cache retains immutable ranges
and scans new suffix entries when the head marker is unchanged.

For rustX, preserve committed user input, accepted final assistant/tool results,
meaningful ordered trace/history and terminal status. Live streaming deltas may
be visible but not yet durable. Recovery exposes the last committed partial as
interrupted rather than claiming completion or replaying every visible delta.
Final commit failure is observable and does not acknowledge durable success.

External tool side effects remain outside SQLite atomicity. A crash between an
external effect and result commit can leave an unknown outcome. Do not silently
rerun non-replayable effects; retain explicit unknown/replay-policy handling and
identify any intentional changes relative to Pi's abandoned-task behavior.
Exactly-once is not claimed. Pi records execution intent and reruns only when
both the stored and current tool policy say `safe`; unsafe/abandoned calls report
that they may have partially run. Tool completion and its committed result must agree.

Inspection no longer promises independent original wire-byte/field witnesses
where those are removed. It must still identify the committed historical content
it represents, distinguish not-ready from unavailable/error, obey bounded reads,
and avoid turning an inspection failure into false execution success/failure.
Client interface/schema changes must be reflected in generated protocol, Web/TUI,
docs and tests together, not by deleting assertions around unchanged APIs.

## Measurement and delivery

Keep each architecture stage reviewable and measure it before broadening. Build
release binaries through the root-clean identity-verified builder; retain source
SHA/tree, five fixture hashes, compiler/runtime and executable SHA. Never reuse a
shared-target artifact based solely on checkout identity. Compare against both
the fixed #484 production binary and pinned Pi using the same HTTP/SSE fake
provider, full ordered oracles and three interleaved fresh-process pairs.

Report short-small/short-large, stream, tool and cancel, plus growing-history
marginal appends/context work. Include CPU scope/resolution, sampled anonymous
memory/RSS/PSS, DB/WAL/temp logical and allocated bytes and process writes;
physical-device I/O and allocation counts remain unavailable unless measured.
Provider and product remain controller-owned siblings through cleanup. Profiles,
SQL/operation counters and oracle evidence output stay outside headline timings
or are explicitly attributable. Retain failures, adverse samples and startup,
checkpoint/close/reopen/cleanup costs. No assumed parity or multiplied stage gains.

Run targeted FAST checks per stage, then the full retained recovery, platform,
protocol, Web/TUI and real-process suites. Final FULL and Merge must pass on the
exact final head with its latest integrated dependency/base. Both draft PRs remain
unmerged for independent comprehensive review, fixes and re-review.

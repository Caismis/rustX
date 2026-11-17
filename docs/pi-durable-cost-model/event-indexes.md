# Scoped event indexes: consumer audit and change

Measured baseline: 8,171 event-index WAL frames of 20,002 total, 32 steady turns.
This is write-frame attribution from endpoint dbstat snapshots, not CPU share.

Presentation scopes in sqlite.rs::presentation_statements always compare their
identity with '='. The tool call scope permits null attempt/turn with IS, but
requires a nonnull tool_call_id equality; only call ID can safely be its partial
predicate. ContextReading uses the exact qualifying expression '= 1'.

Other explicit readers:
- attempt_kind: observation_members, observation/reading, statistics, trace event,
  observation_turn. **Keep this index full:** the multiline adoption-prompt UNION
  in reading::adoption_prompt_sql intentionally searches null attempt owners as
  well as a named Attempt. The initial audit missed that second arm; full durable
  tests exposed `turn prompt: no query solution`, and the predicate was removed.
- step_kind: trace turn/request start; attempt+turn both equality.
- request_id_kind: native request resolution, trace links, observation_execution;
  request_id equality.
- message_id_kind: transcript attempts and member lookup; message_id equality.
- execution/subagent/interaction/tool call: trace link resolution, equality of
  the appropriate identity. Tool attempt/turn can be NULL and must be retained.
- workflow_coordinates: trace workflow start, equality of conversation, attempt,
  invocation; choose conversation identity IS NOT NULL as partial predicate.
- observation_tool: statistics tool start lookup, call_id equality.
- events_attempt_idx: reading attempt facts, attempt_id equality.
- events_sequence_idx: no explicit consumers in src; sequence is INTEGER PRIMARY
  KEY, so removing this duplicate leaves the table's rowid range/order access.
- events_kind_idx remains full: unscoped family reads need all event rows.
- events_attempt_kind_idx remains full for the null-owner adoption prompt arm.

The implementation retains every trace/UI event. Partial indexes omit only rows
that cannot match their consumers. Explicit INDEXED BY and the existing query-plan
and physical VM-step contracts remain. SQLite development schema 72 requires the
new predicates; reopening old layouts is rejected without migration. New tests
cover tool occurrences with null owners, ordering at a historical cut, predicate
shape validation and refusal of schema 71. The prompt-observation index still
includes null attempt owners because its reader intentionally searches them.

A host-SQLite preflight found indexed searches without scans or temporary sorts
for all 12 presentation scopes, but did not cover the multiline null-owner prompt
reader. Production durable tests correctly caught that missing consumer; their
20 failures are retained (380 passed). Existing prompt/history bounds remain
unchanged. The corrected index passed all 400 durable tests (252.72 s), all 26 Source
tests (1.04 s), and all-target/all-feature Clippy with warnings denied, together
with the independent 16 KiB byte-bound refinement. The changes are committed
separately for independent measurements. Initial simultaneous test/lint compilers were killed by
the 16 GiB environment memory limit (two OOM kills); logs are retained and checks
were rerun sequentially. No memory/security setting or test timeout was changed.
Isolated performance measurements are recorded below; final combined validation remains pending.


## Isolated release measurements

Compared resident-16KiB `c34de111f5da991e5b92db40a9391d60842b8c49`
(binary `725d8d5dde5aba95a38a2e2fdf304cad4f62fb72247552bd5e9297e0af992668`)
with indexes `d054be08981dcc98c53c5ba83641b387f26442d4`
(binary `475c6ef0081a2ddf6646dd47e29f5d02d7b3ac2d3435506d47de00fffbc11eb6`).
The root-clean optimized builder verified both root artifacts as fresh and all
five fixture hashes as equal. Three interleaved fresh-process pairs per case:
36 samples, 846 full ordered requests, 18 paired request comparisons, all passed.
No compilation or profiling overlapped formal measurements.

Median within-pair after/before ratios (not ratios of independent medians):

| Case | Turn wall | Turn CPU | Lifecycle writes | Sampled anonymous PSS |
| --- | ---: | ---: | ---: | ---: |
| short-small | 1.0093 | 1.0068 | 0.8259 | 1.0052 |
| short-large | 0.9847 | 0.9940 | 0.8115 | 0.9985 |
| stream | 0.9954 | 0.9130 | 0.9266 | 0.9971 |
| large | 1.0016 | 1.0000 | 0.9650 | 0.9893 |
| tool | 0.9828 | 1.0000 | 0.8713 | 1.0028 |
| cancel | 0.9343 | 0.8333 | 0.9305 | 1.0010 |

Writes improve in every case; CPU/latency and memory do not establish a general
gain. Short-small wall/CPU regress slightly; tool lifecycle CPU/wall ratios are
1.0690/1.0166 and large-response lifecycle CPU/wall are 1.0270/1.0094. All adverse
samples are retained. CPU includes the runtime and driver, with 10 ms self-CPU
resolution; single-turn CPU ratios are coarse. Lifecycle includes initialization,
verification, checkpoint/close/reopen and cleanup. Provider startup is explicit
in the retained controller records. Evidence output follows counter capture.
Memory is sampled RSS/PSS, not a peak or allocation count. Writes are attributed
to the process, not physical-device writes. No unrelated medians are subtracted.

The separate 32-steady-turn WAL diagnostic uses the same sibling provider and
full ordered oracle. Both runs passed. Write commits stay at 672 and new event
rows at 256. WAL frames fall from 20,005 to 16,535 (82,420,600 to 68,124,200 frame
bytes). Endpoint-dbstat event-index frames fall from 7824 to 4337.
This attributes reduced index work, not fewer semantic commits. Page owners are
sampled at phase endpoints: intermediate reuse is not excluded, and mixed/unmapped
owners remain explicit. The parser rejects another steady-state WAL path. These
instrumented runs include tracing and snapshot overhead and are not headline
timing samples or exclusive CPU percentages.

Reproduction from the frozen index worktree:

```sh
python3 benchmarks/runtime/build-revision.py --repo /workspace/rustX-pi-index --target /workspace/issue-481-target --output /workspace/issue-481-evidence/pi-index-build
python3 benchmarks/runtime/compare-revisions.py --before /workspace/issue-481-evidence/pi-resident16-build/conformance --after /workspace/issue-481-evidence/pi-index-build/conformance --output /workspace/issue-481-evidence/pi-index-matrix --pairs 3
python3 benchmarks/runtime/summarize-comparisons.py /workspace/issue-481-evidence/pi-index-matrix
python3 /workspace/issue-481-evidence/pi-index-profile/run.py
python3 /workspace/issue-481-evidence/pi-index-profile/analyze.py
```

Each output directory must be new. Build environment and hardware are the same
as [the resident-bound comparison](resident-bound.md). The evidence archive in
`evidence/event-index-measurements/` retains scripts, phase snapshots, complete
straces, build identity/logs, controller/oracle records and a SHA-256 manifest.
Original files remain in the cloud workspace. No combined comparison with Pi or
claim about the pending lazy-preview/authentication/final-settlement changes is
made by this stage.

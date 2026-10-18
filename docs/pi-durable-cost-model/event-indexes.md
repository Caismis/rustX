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
Performance measurements and final validation remain pending.

import type { TraceDetail, TraceRecord } from '../../protocol/app-server/v39';

/** One bounded summary record, as the server pages them. */
export const traceRecord = (n: number, overrides: Partial<TraceRecord> = {}): TraceRecord => ({
  id: `trace:${n}`,
  position: `trace:${n}`,
  location: { attempt_id: 'attempt-a', step_id: '1' },
  kind: 'request',
  state: 'completed',
  timing: { started_at: '2026-09-15T00:00:00Z', ended_at: '2026-09-15T00:00:01Z', duration_ms: '1000' },
  preview: { text: `historical-model-${n}`, truncated: false },
  request: {
    request_id: `request-${n}`,
    assistant_message_id: `message-${n}`,
    retry_number: n,
    model: 'historical-model',
    previous_failure_kind: null,
    failure_kind: null,
    usage: null,
    generation: null,
    system_prompt: { state: 'unchanged', preview: null },
    predecessor: { availability: 'available', request_id: 'previous-request' },
    tool_catalog: 'unchanged',
    context_additions: [],
    context_truncated: false,
  },
  tool: null,
  calls: [],
  native_id: null,
  originating_tool_call_id: null,
  message_id: null,
  attachments: [],
  has_detail: true,
  truncated: false,
  ...overrides,
});

/** One started Tool record whose canonical result has settled. */
export const traceTool = (n: number, overrides: Partial<TraceRecord> = {}): TraceRecord =>
  traceRecord(n, {
    kind: 'tool',
    preview: { text: 'ls -la', truncated: false },
    request: null,
    tool: {
      call_id: `call-${n}`,
      tool_id: 'tool-bash',
      name: 'bash',
      arguments: { text: '{"command":"ls -la"}', truncated: false },
      started: true,
      outcome: 'success',
      detail: null,
    },
    ...overrides,
  });

/** Heavy detail for one request record. */
export const requestDetail = (n: number, overrides: Partial<TraceDetail> = {}): TraceDetail => ({
  id: `trace:${n}`,
  kind: 'request',
  request: {
    contributions: [],
    request_id: `request-${n}`,
    attempt_id: 'attempt-a',
    step_id: '1',
    retry_number: n,
    assistant_message_id: `message-${n}`,
    model: 'historical-model',
    protocol: 'openai_chat_completions',
    max_output_tokens: 128,
    context_window_tokens: '4096',
    reasoning_enabled: false,
    reasoning_profile: null,
    options: [{ name: 'temperature', value: { value: 0.25, truncated: false } }],
    omitted_option_count: 2,
    predecessor: { availability: 'available', request_id: 'previous-request' },
    previous_system_prompt: { text: 'Previous prompt.', truncated: false },
    effective_system_prompt: { text: 'You are the historical agent.', truncated: false },
    messages: [
      {
        role: 'user',
        message_id: 'user-1',
        source: 'human',
        blocks: [{ type: 'text', text: { text: 'Inspect the trajectory.', truncated: false } }],
        truncated: false,
      },
    ],
    messages_truncated: false,
    tools: [
      {
        tool_id: 'tool-bash',
        name: 'bash',
        description: { text: 'Run one command.', truncated: false },
        input_schema: { value: { type: 'object' }, truncated: false },
      },
    ],
    tools_truncated: false,
    usage: null,
    failure: null,
    generation: null,
  },
  tool: null,
  messages: [],
  truncated: false,
  ...overrides,
});

/** Heavy detail for one Tool record, including its native source view. */
export const toolDetail = (n: number, overrides: Partial<TraceDetail> = {}): TraceDetail => ({
  id: `trace:${n}`,
  kind: 'tool',
  request: null,
  messages: [],
  tool: {
    call_id: `call-${n}`,
    tool_id: 'tool-bash',
    name: 'bash',
    lifecycle: 'settled',
    arguments: { value: { command: 'ls -la' }, truncated: false },
    source: { field: 'command', text: { text: 'ls -la', truncated: false }, language: 'shell' },
    definition: {
      tool_id: 'tool-bash',
      name: 'bash',
      description: { text: 'Run one command.', truncated: false },
      input_schema: { value: { type: 'object' }, truncated: false },
    },
    result: {
      outcome: 'success',
      detail: null,
      blocks: [{ type: 'text', text: { text: 'total 8', truncated: false } }],
      blocks_truncated: false,
      duration_ms: '1234',
      exit_code: 0,
      attachments: [],
      truncation: null,
      managed_output: null,
    },
  },
  truncated: false,
  ...overrides,
});

/** Interleaved exact membership; labels alone uniquely match Step 2 / Turn 2. */
export const structuralSearchRecords = (): TraceRecord[] => [
  traceRecord(0, { kind: 'user', request: null, location: {} }),
  traceRecord(1, { kind: 'attempt', request: null, location: { attempt_id: 'attempt-a' } }),
  traceRecord(2, { kind: 'user', request: null, location: { attempt_id: 'attempt-a' } }),
  traceRecord(3, { location: { attempt_id: 'attempt-a', step_id: 'alpha' } }),
  traceRecord(4, { kind: 'step', request: null, location: { attempt_id: 'attempt-a', step_id: 'beta' } }),
  traceRecord(5, { location: { attempt_id: 'attempt-a', step_id: 'beta' } }),
  traceRecord(6, { kind: 'attempt', request: null, location: { attempt_id: 'attempt-b' } }),
  traceRecord(7, { location: { attempt_id: 'attempt-b', step_id: 'gamma' } }),
  traceRecord(8, { location: { attempt_id: 'attempt-a', step_id: 'beta' } }),
];

/** Fixed #421 acceptance data. Every group and retry is explicitly native-owned. */
export function semanticLedgerRecords(): TraceRecord[] {
  const a = { attempt_id: 'ledger-turn-a', step_id: 'ledger-step-a' };
  const b = { attempt_id: 'ledger-turn-b', step_id: 'ledger-step-b' };
  const prompt = traceRecord(102, { location: a });
  prompt.request!.system_prompt = { state: 'initial', preview: { text: 'You are the historical agent. Preserve exact native authority.', truncated: false } };
  prompt.request!.retry_number = 0;
  prompt.request!.context_additions = ['workspace', 'environment'].map((name, n) => ({ message_id: `ledger-context-${name}`, context_kind: n ? 'native_environment' : 'extension_environment', producer: n ? { Native: 'workspace_instructions' } : { CertifiedExtension: 'vendor.workspace' }, source: n ? { type: 'runtime' } : { type: 'certified_extension', contributor: 'vendor.workspace' }, preview: { text: n ? 'Runtime environment: Linux · Rust workspace' : 'Workspace instructions: preserve native ownership and bounded reads', truncated: false }, attachments: [], truncated: false }));
  const updated = traceRecord(112, { location: b });
  updated.request!.system_prompt = { state: 'changed', preview: { text: 'Inspect the final diff and report validation evidence.', truncated: false } };
  updated.request!.retry_number = 0;
  const tool = traceTool(104, { location: a, preview: { text: '3 files changed, 28 insertions', truncated: false } });
  tool.tool!.arguments = { text: '{"command":"git diff --stat"}', truncated: false };
  const failed = traceTool(105, { location: a, state: 'failed', preview: { text: 'error: missing field `name` in source record', truncated: false } });
  failed.tool!.arguments = { text: '{"command":"cargo test --lib"}', truncated: false }; failed.tool!.outcome = 'failed';
  const finalTool = traceTool(114, { location: b, preview: { text: 'All focused tests passed. Additional output omitted…', truncated: true } });
  finalTool.tool!.arguments = { text: '{"command":"cargo test --lib runtime_client::trace::', truncated: true };
  const retry = traceRecord(107, { location: { ...a, step_id: 'ledger-step-recovery' }, state: 'failed' });
  retry.request!.retry_number = 0;
  const recovery = traceRecord(108, { location: retry.location }); recovery.request!.retry_number = 1;
  return [
    traceRecord(100, { kind: 'attempt', request: null, location: { attempt_id: a.attempt_id } }),
    traceRecord(101, { kind: 'user', request: null, location: { attempt_id: a.attempt_id }, preview: { text: 'Review the workspace and run the focused checks.', truncated: false } }),
    traceRecord(120, { kind: 'step', request: null, location: a }), prompt,
    traceRecord(103, { kind: 'assistant', request: null, location: a, preview: { text: 'I will inspect the diff and verify the native contracts.', truncated: false }, calls: [tool, failed].map(record => ({ call_id: record.tool!.call_id, tool_id: record.tool!.tool_id, name: 'bash' })) }),
    tool, failed, traceRecord(121, { kind: 'step', request: null, location: retry.location }), retry, recovery,
    traceRecord(109, { kind: 'assistant', request: null, location: retry.location, preview: { text: 'The missing field is corrected. The retry passed.', truncated: false } }),
    traceRecord(110, { kind: 'attempt', request: null, location: { attempt_id: b.attempt_id } }),
    traceRecord(111, { kind: 'user', request: null, location: { attempt_id: b.attempt_id }, preview: { text: 'Verify the final implementation and its bounds.', truncated: false } }),
    traceRecord(122, { kind: 'step', request: null, location: b }), updated,
    traceRecord(113, { kind: 'assistant', request: null, location: b, preview: { text: 'Running the native Trace regression suite.', truncated: false } }), finalTool,
  ];
}

/** Deliberately non-sorted identities and equal timestamps: group order alone owns presentation. */
export function orderedStepRecords(initial = true, emptySteps = 2, withContext = false): TraceRecord[] {
  const location = (step_id: string) => ({ attempt_id: 'ordered-turn', step_id });
  const first = traceRecord(702, { location: location('z-first') });
  first.request!.system_prompt = { state: 'initial', preview: { text: 'order-match initial prompt', truncated: false } };
  if (withContext) first.request!.context_additions = [{ message_id: 'hidden-context', context_kind: 'native_environment', producer: { Native: 'workspace_instructions' }, source: { type: 'runtime' }, preview: { text: 'ordinary context', truncated: false }, attachments: [], truncated: false }];
  return [
    traceRecord(700, { kind: 'attempt', request: null, location: { attempt_id: 'ordered-turn' } }),
    traceRecord(701, { kind: 'step', request: null, location: location('z-first') }),
    initial ? first : traceRecord(702, { kind: 'assistant', request: null, location: location('z-first') }),
    ...['a-empty', 'm-empty'].slice(0, emptySteps).map((id, index) => traceRecord(703 + index, { kind: 'step', request: null, location: location(id) })),
    traceRecord(705, { kind: 'step', request: null, location: location('b-last') }),
    traceRecord(706, { kind: 'assistant', request: null, location: location('b-last'), preview: { text: 'order-match later semantic content', truncated: false } }),
  ];
}

/** Fifty exact native groups, with the same first/last identities as the small ordering fixture. */
export function manyStepRecords(): TraceRecord[] {
  const records = orderedStepRecords();
  records.splice(5, 0, ...Array.from({ length: 46 }, (_, index) => traceRecord(1000 + index, {
    kind: 'step', request: null, location: { attempt_id: 'ordered-turn', step_id: `empty-${46 - index}` },
  })));
  return records;
}

/** Native wire absence is explicit null, including adopted Attempt-owned input. */
export function stepLessRecords(): TraceRecord[] {
  return JSON.parse(JSON.stringify([
    traceRecord(910, { kind: 'user', request: null, message_id: 'adopted-first',
      location: { attempt_id: 'adopted-attempt', step_id: null },
      preview: { text: 'adopted first input', truncated: false } }),
    traceRecord(911, { kind: 'user', request: null, message_id: 'adopted-second',
      location: { attempt_id: 'adopted-attempt', step_id: null },
      preview: { text: 'adopted second input', truncated: false } }),
    traceRecord(912, { kind: 'assistant', request: null,
      location: { attempt_id: 'adopted-attempt', step_id: 'native-step' },
      preview: { text: 'Step-owned answer', truncated: false } }),
  ])) as TraceRecord[];
}

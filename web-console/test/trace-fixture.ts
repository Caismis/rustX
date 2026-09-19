import type { TraceDetail, TraceRecord } from '../../protocol/app-server/v12';

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
  },
  tool: null,
  calls: [],
  native_id: null,
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

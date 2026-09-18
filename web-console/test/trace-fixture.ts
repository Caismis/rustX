import type { TraceEntry } from '../../protocol/app-server/v8';
export const traceEntry = (n: number, overrides: Partial<TraceEntry> = {}): TraceEntry => ({
  id: `trace:${n}`, position: `trace:${n}`, location: { attempt_id: 'attempt-a', step_id: '1' }, kind: 'request', state: 'completed',
  timing: { started_at: '2026-09-15T00:00:00Z', ended_at: '2026-09-15T00:00:01Z', duration_ms: '1000' },
  request: { request_id: `request-${n}`, assistant_message_id: `message-${n}`, retry_number: n, model: { text: 'historical-model', redacted: false, truncated: false },
    max_output_tokens: 128, reasoning_enabled: false, effective_system_prompt: { text: '', redacted: true, truncated: false },
    context_input: { text: '', redacted: true, truncated: false }, tool_schema: { text: '', redacted: true, truncated: false } },
  calls: [], output: [], reasoning: [], artifacts: [], truncated: false, ...overrides,
});

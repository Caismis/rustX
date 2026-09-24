import { describe, expect, it } from 'vitest';
import { trajectoryTimeline } from '../src/app/trajectory/timeline';
import { traceRecord } from './trace-fixture';

const start = Date.parse('2026-09-15T00:00:00Z');
function request(withBridge: boolean, wallDuration = 2000) {
  const record = traceRecord(0);
  record.timing.duration_ms = String(wallDuration);
  record.timing.ended_at = new Date(start + wallDuration).toISOString();
  record.request!.generation = {
    ttft_ms: '320', generation_ms: '1280', terminal_ms: '1600', output_tokens_per_second: 93.75,
    timeline: withBridge ? {
      dispatch_ms: '400', first_output_ms: '720', last_output_ms: '1920', terminal_ms: '2000',
    } : null,
  };
  return record;
}
const model = (bridge: boolean, wallDuration?: number) =>
  trajectoryTimeline([request(bridge, wallDuration)], 'duration', () => undefined)!.spans[0]!;

describe('authoritative request phase positions', () => {
  it('includes preparation in first-output position but excludes it from TTFT', () => {
    const span = model(true);
    expect(span.dispatchAt).toBe(start + 400);
    expect(span.firstOutputAt).toBe(start + 720);
    expect(span.lastOutputAt).toBe(start + 1920);
    expect(span.providerTerminalAt).toBe(start + 2000);
    expect(span.end).toBe(start + 2000);
    expect(span.ttftMs).toBe(320);
    expect(span.generationMs).toBe(1280);
    expect(span.firstOutputAt).not.toBe(start + 320 / 1600 * 2000);
  });
  it('does not stretch phases to fit Journal wall duration or settlement delay', () => {
    const span = model(true, 9000);
    expect(span.firstOutputAt).toBe(start + 720);
    expect(span.end).toBe(start + 2000);
    expect(span.durationMs).toBe(9000);
  });
  it('Journal terminal at 9000ms cannot supply a Model endpoint without the native bridge', () => {
    const record = request(false, 9000);
    const span = trajectoryTimeline([record], 'duration', () => undefined)!.spans[0]!;
    expect(record.timing.ended_at).toBe(new Date(start + 9000).toISOString());
    expect(span.durationMs).toBe(9000);
    expect(span.start).toBe(start);
    expect(span.ttftMs).toBe(320);
    expect(span.firstOutputAt).toBeUndefined();
    expect(span.dispatchAt).toBeUndefined();
    expect(span.providerTerminalAt).toBeUndefined();
    expect(span.end).toBe(start);
  });
  it('does not paint duration boundaries onto equal-width sequence units', () => {
    const span = trajectoryTimeline([request(true)], 'sequence', () => undefined)!.spans[0]!;
    expect(span.dispatchAt).toBeUndefined();
    expect(span.firstOutputAt).toBeUndefined();
  });
  it('renders dispatch without inventing first output for a silent request', () => {
    const record = request(true);
    record.request!.generation!.timeline!.first_output_ms = null;
    record.request!.generation!.timeline!.last_output_ms = null;
    const span = trajectoryTimeline([record], 'duration', () => undefined)!.spans[0]!;
    expect(span.dispatchAt).toBe(start + 400);
    expect(span.end).toBe(start + 2000);
    expect(span.providerTerminalAt).toBe(start + 2000);
    expect(span.firstOutputAt).toBeUndefined();
    expect(span.lastOutputAt).toBeUndefined();
  });
});

it('preserves measured zero separately from absent and running timing', () => {
  const zero = traceRecord(0, { timing: { started_at: '2026-09-15T00:00:00Z', duration_ms: '0' } });
  zero.request!.generation = { ttft_ms: '0', generation_ms: '0', terminal_ms: '0', output_tokens_per_second: null, timeline: { dispatch_ms: '0', first_output_ms: '0', last_output_ms: '0', terminal_ms: '0' } };
  const running = traceRecord(1, { state: 'running', timing: { started_at: '2026-09-15T00:00:00Z' } });
  const [measured, missing] = trajectoryTimeline([zero, running], 'duration', () => undefined)!.spans;
  expect(measured!.start).toBe(start);
  expect(measured!.end).toBe(start);
  expect(measured!.providerTerminalAt).toBe(start);
  expect(measured!.durationMs).toBe(0);
  expect(measured!.ttftMs).toBe(0);
  expect(measured!.firstOutputAt).toBe(start);
  expect(missing!.durationMs).toBeUndefined();
  expect(missing!.ttftMs).toBeUndefined();
  expect(missing!.end).toBe(missing!.start);
});

it('T1-12 epoch zero, missing instant, parallel domains and canonical acceptance do not invent or duplicate spans', () => {
  const records = ['tool', 'background', 'subagent', 'workflow'].map((kind, n) => traceRecord(n, {
    kind: kind as 'tool', request: null,
    timing: { started_at: new Date(0).toISOString(), ended_at: new Date(1000).toISOString(), duration_ms: '1000' },
  }));
  records.push(traceRecord(4, { kind: 'assistant', request: null }), traceRecord(5, { kind: 'attempt', request: null }), traceRecord(6, { kind: 'step', request: null }));
  records.push(traceRecord(7, { timing: { started_at: 'unavailable' } }));
  const spans = trajectoryTimeline(records, 'duration', () => undefined)!.spans;
  expect(spans.map(span => span.id)).toEqual(['trace:0', 'trace:1', 'trace:2', 'trace:3']);
  expect(spans.map(span => [span.lane, span.start, span.end])).toEqual(Array.from({ length: 4 }, () => [2, 0, 1000]));
  expect(trajectoryTimeline(records, 'sequence', () => undefined)!.spans.map(span => span.id)).toEqual(['trace:0', 'trace:1', 'trace:2', 'trace:3', 'trace:7']);
});

it('T1-12 four modes keep native time distinct from a shared idle-compression transform', () => {
  const records = [0, 100, 2000].map((ms, n) => traceRecord(n, { kind: n === 1 ? 'background' : 'tool', request: null,
    timing: { started_at: new Date(ms).toISOString(), ended_at: new Date(ms + 1000).toISOString(), duration_ms: '1000' } }));
  const spans = (mode: 'sequence' | 'duration' | 'time' | 'actual') => trajectoryTimeline(records, mode, () => undefined)!.spans.map(s => [s.start, s.end]);
  expect(spans('sequence')).toEqual([[0, 1], [1, 2], [2, 3]]);
  expect(spans('duration')).toEqual([[0, 1000], [100, 1100], [1100, 2100]]);
  expect(spans('time')).toEqual([[0, 0], [100, 100], [2000, 2000]]);
  expect(spans('actual')).toEqual([[0, 1000], [100, 1100], [2000, 3000]]);
});

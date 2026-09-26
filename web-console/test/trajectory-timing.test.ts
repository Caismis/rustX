import { translator } from '../src/locale/translation';
import { projectTrajectory } from '../src/app/trajectory/layout';
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
  trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), [request(bridge, wallDuration)]), 'duration')!.spans[0]!;

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
    const span = trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), [record]), 'duration')!.spans[0]!;
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
    const span = trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), [request(true)]), 'sequence')!.spans[0]!;
    expect(span.dispatchAt).toBeUndefined();
    expect(span.firstOutputAt).toBeUndefined();
  });
  it('renders dispatch without inventing first output for a silent request', () => {
    const record = request(true);
    record.request!.generation!.timeline!.first_output_ms = null;
    record.request!.generation!.timeline!.last_output_ms = null;
    const span = trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), [record]), 'duration')!.spans[0]!;
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
  const [measured, missing] = trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), [zero, running]), 'duration')!.spans;
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
  const spans = trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), records), 'duration')!.spans;
  expect(spans.map(span => span.id)).toEqual(['trace:0', 'trace:1', 'trace:2', 'trace:3']);
  expect(spans.map(span => [span.lane, span.start, span.end])).toEqual(Array.from({ length: 4 }, () => [2, 0, 1000]));
  expect(trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), records), 'sequence')!.spans.map(span => span.id)).toEqual(['trace:0', 'trace:1', 'trace:2', 'trace:3', 'trace:7']);
});

it('T1-12 four modes keep native time distinct from a shared idle-compression transform', () => {
  const records = [0, 100, 2000].map((ms, n) => traceRecord(n, { kind: n === 1 ? 'background' : 'tool', request: null,
    timing: { started_at: new Date(ms).toISOString(), ended_at: new Date(ms + 1000).toISOString(), duration_ms: '1000' } }));
  const spans = (mode: 'sequence' | 'duration' | 'time' | 'actual') => trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), records), mode)!.spans.map(s => [s.start, s.end]);
  expect(spans('sequence')).toEqual([[0, 1], [1, 2], [2, 3]]);
  expect(spans('duration')).toEqual([[0, 1000], [100, 1100], [1100, 2100]]);
  expect(spans('time')).toEqual([[0, 0], [100, 100], [2000, 2000]]);
  expect(spans('actual')).toEqual([[0, 1000], [100, 1100], [2000, 3000]]);
});

it.each(['sequence', 'duration', 'time', 'actual'] as const)('%s Turn boundaries belong to their own projected visible activity', mode => {
  const at = (ms: number) => ({ started_at: new Date(ms).toISOString() });
  const records = [
    traceRecord(0, { kind: 'attempt', request: null, location: { attempt_id: 'a' }, timing: at(0) }),
    traceRecord(1, { kind: 'step', request: null, location: { attempt_id: 'a', step_id: 's' }, timing: at(10) }),
    traceRecord(2, { location: { attempt_id: 'a', step_id: 's' }, timing: at(100) }),
    traceRecord(3, { kind: 'tool', request: null, location: { attempt_id: 'a', step_id: 's' }, timing: { ...at(200), duration_ms: '100' } }),
    traceRecord(4, { kind: 'attempt', request: null, location: { attempt_id: 'empty' }, timing: at(400) }),
    traceRecord(5, { kind: 'step', request: null, location: { attempt_id: 'empty', step_id: 's' }, timing: at(410) }),
    traceRecord(6, { kind: 'attempt', request: null, location: { attempt_id: 'b' }, timing: at(500) }),
    // Deliberately out of timestamp order: first input is not earliest activity.
    traceRecord(7, { kind: 'tool', request: null, location: { attempt_id: 'b', step_id: 's' }, timing: { ...at(1100), duration_ms: '100' } }),
    traceRecord(8, { location: { attempt_id: 'b', step_id: 's' }, timing: at(1000) }),
    traceRecord(9, { location: { attempt_id: 'untimed', step_id: 's' }, timing: { started_at: 'unavailable' } }),
  ];
  const projection = projectTrajectory(translator('en'), records);
  const model = trajectoryTimeline(translator('en'), projection, mode)!;
  expect(model.boundaries.map(b => b.nativeAttemptId)).toEqual(mode === 'sequence' ? ['a', 'b', 'untimed'] : ['a', 'b']);
  for (const boundary of model.boundaries) {
    const turn = projection.sections.find(section => section.kind === 'turn' && section.nativeAttemptId === boundary.nativeAttemptId)!;
    if (turn.kind !== 'turn') throw new Error('Expected Turn');
    const ids = new Set(turn.records.map(record => record.id));
    const spans = model.spans.filter(span => ids.has(span.id));
    expect(boundary.at).toBe(Math.min(...spans.map(span => span.start)));
    expect(boundary.at).toBeGreaterThanOrEqual(model.start);
    expect(boundary.at).toBeLessThanOrEqual(model.end);
  }
  // Compression removes both intra-Turn gaps and the gap between Turns.
  expect(model.boundaries.map(b => b.at)).toEqual(mode === 'sequence' ? [0, 2, 4] : mode === 'duration' ? [100, 200] : [100, 1000]);
  expect(trajectoryTimeline(translator('en'), projectTrajectory(translator('en'), records.slice(4, 6)), mode)).toBeNull();
});

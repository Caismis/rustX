import { describe, expect, it } from 'vitest';
import { trajectoryTimeline } from '../src/app/trajectory/timeline';
import { traceRecord } from './trace-fixture';

const start = Date.parse('2026-09-15T00:00:00Z');
function request(withBridge: boolean, wallDuration = 2000) {
  const record = traceRecord(0);
  record.timing.duration_ms = String(wallDuration);
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
  it('keeps numeric metrics but no phase positions without the native bridge', () => {
    const span = model(false);
    expect(span.ttftMs).toBe(320);
    expect(span.firstOutputAt).toBeUndefined();
    expect(span.dispatchAt).toBeUndefined();
    expect(span.providerTerminalAt).toBeUndefined();
    expect(span.end).toBe(start + 2000);
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
    expect(span.firstOutputAt).toBeUndefined();
  });
});

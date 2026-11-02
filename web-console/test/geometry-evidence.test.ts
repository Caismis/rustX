// @vitest-environment node
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
it('publishes only finite nonnegative geometry elapsed measurements', () => {
  const timings = JSON.parse(readFileSync(new URL('../../docs/issue-516/format-display-timings.json', import.meta.url), 'utf8')) as { displayMs: number }[];
  const envelope = JSON.parse(readFileSync(new URL('../../docs/issue-516/stl-envelope.json', import.meta.url), 'utf8')) as { displayMs: number; observedFrameGapMs: number };
  for (const value of [...timings.map(row => row.displayMs), envelope.displayMs, envelope.observedFrameGapMs]) { expect(Number.isFinite(value)).toBe(true); expect(value).toBeGreaterThanOrEqual(0); }
});

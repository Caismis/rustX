// @vitest-environment node
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
it('publishes only finite nonnegative geometry elapsed measurements', () => {
  const timings = JSON.parse(readFileSync(new URL('../../docs/issue-516/format-display-timings.json', import.meta.url), 'utf8')) as { displayMs: number; importMs: number }[];
  const envelope = JSON.parse(readFileSync(new URL('../../docs/issue-516/stl-envelope.json', import.meta.url), 'utf8')) as { displayMs: number; observedFrameGapMs: number };
  const native = JSON.parse(readFileSync(new URL('../../docs/issue-516/native-display-timings.json', import.meta.url), 'utf8')) as { displayMs: number }[];
  const workbench = JSON.parse(readFileSync(new URL('../../docs/issue-516/native-workbench-display.json', import.meta.url), 'utf8')) as { displayMs: number };
  for (const value of [...timings.flatMap(row => [row.displayMs, row.importMs]), ...native.map(row => row.displayMs), workbench.displayMs, envelope.displayMs, envelope.observedFrameGapMs]) { expect(Number.isFinite(value)).toBe(true); expect(value).toBeGreaterThanOrEqual(0); }
});
it('publishes valid repeated samples, nearest-rank percentiles and retired resource counters', () => {
  const followup = JSON.parse(readFileSync(new URL('../../docs/issue-516/performance-followup-samples.json', import.meta.url), 'utf8'));
  const reports = ['stl-735000-vertices.json', 'cad-96-parts.json'].map(name => JSON.parse(readFileSync(new URL(`../../docs/issue-516/${name}`, import.meta.url), 'utf8')));
  for (const phase of ['before', 'after']) reports.push(...Object.values(followup[phase]));
  for (const report of reports) {
    for (const value of [report.firstDisplayMs, report.twoModelDisplayMs]) { expect(Number.isFinite(value)).toBe(true); expect(value).toBeGreaterThanOrEqual(0); }
    for (const key of ['remount', 'orbit', 'pan', 'picking']) for (const kind of ['elapsedMs', 'frameMs']) {
      const series = report[key][kind], sorted = [...series.samples].sort((a: number, b: number) => a - b);
      expect(sorted.length).toBeGreaterThan(0);
      for (const value of sorted) { expect(Number.isFinite(value)).toBe(true); expect(value).toBeGreaterThanOrEqual(0); }
      expect(series.p50).toBe(sorted[Math.ceil(sorted.length * .5) - 1]);
      expect(series.p95).toBe(sorted[Math.ceil(sorted.length * .95) - 1]);
    }
    expect(report.afterRetirement).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
    expect(report.twoModelResources.contexts).toBe(2);
  }
});

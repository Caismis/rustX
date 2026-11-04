import { test, expect, type Page } from '@playwright/test';
import { writeFileSync } from 'node:fs';
import { instrument, resources } from './geometry-observation';
import { FILE_PREVIEW_LIMITS } from '../../shared/file-preview';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: '<loopback>' } });
const painted = (page: Page) => page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
const duration = (start: number) => Math.round((performance.now() - start) * 10) / 10;
function distribution(samples: number[]) {
  expect(samples.length).toBeGreaterThan(0);
  for (const value of samples) { expect(Number.isFinite(value)).toBe(true); expect(value).toBeGreaterThanOrEqual(0); }
  const sorted = [...samples].sort((a, b) => a - b);
  return { samples: samples.map(value => Math.round(value * 10) / 10), p50: Math.round(sorted[Math.ceil(sorted.length * .5) - 1] * 10) / 10, p95: Math.round(sorted[Math.ceil(sorted.length * .95) - 1] * 10) / 10 };
}
async function sample(page: Page, action: () => Promise<void>) {
  await page.evaluate(() => {
    const state: { frames: number[]; start: number; last?: number; frame: number } = { frames: [], start: performance.now(), frame: 0 };
    // The first callback establishes the rAF time origin; registration can run
    // after a frame's timestamp was fixed, so it is not a full frame interval.
    const tick = (now: number) => { if (state.last !== undefined) state.frames.push(now - state.last); state.last = now; if (state.frames.length < 1024) state.frame = requestAnimationFrame(tick); };
    state.frame = requestAnimationFrame(tick);
    Object.assign(window, { geometrySampling: { state, activity: { ...(window as any).geometryContract.activity } } });
  });
  const times: number[] = [];
  for (let count = 0; count < 5; count++) { const start = performance.now(); await action(); await painted(page); times.push(duration(start)); }
  const observation = await page.evaluate(() => {
    const capture = (window as any).geometrySampling;
    cancelAnimationFrame(capture.state.frame);
    const activity = (window as any).geometryContract.activity;
    return { frames: capture.state.frames as number[], longTasks: ((window as any).geometryLongTasks as { start: number; duration: number }[]).filter(task => task.start >= capture.state.start).map(task => task.duration),
      publications: activity.publications - capture.activity.publications, commits: activity.commits - capture.activity.commits };
  });
  return { elapsedMs: distribution(times), frameMs: distribution(observation.frames), longTasksMs: observation.longTasks, publications: observation.publications, reactCommits: observation.commits };
}

for (const workload of [
  { id: 'stl-735000-vertices', name: 'cube.stl', dimensions: '350 × 350 × 0' },
  { id: 'cad-96-parts', name: 'cube.step', dimensions: '570 × 72 × 190' },
]) test(`repeated geometry performance and retirement: ${workload.id}`, async ({ page }, info) => {
  await instrument(page);
  await page.addInitScript(() => {
    const tasks: { start: number; duration: number }[] = [];
    new PerformanceObserver(list => { for (const entry of list.getEntries()) if (tasks.length < 2048) tasks.push({ start: entry.startTime, duration: entry.duration }); }).observe({ type: 'longtask' });
    Object.assign(window, { geometryLongTasks: tasks });
  });
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  if (workload.id.startsWith('stl')) await page.evaluate(() => (window as any).geometryContract.grid(350));
  else await page.evaluate(async () => {
    const response = await fetch('/test/fixtures/geometry/bracket-array.step');
    (window as any).geometryContract.replace('cube.step', [...new Uint8Array(await response.arrayBuffer())]);
  });
  const ready = async () => { await expect(page.locator('[data-geometry-format]')).toContainText(workload.dimensions, { timeout: 20_000 }); await painted(page); };
  const start = performance.now(); await page.getByRole('button', { name: `Preview ${workload.name} in sidebar`, exact: true }).click(); await ready();
  const firstDisplayMs = duration(start);
  const remount = await sample(page, async () => {
    await page.getByRole('button', { name: 'Hide delivery', exact: true }).click();
    await expect.poll(async () => (await resources(page)).contexts).toBe(0);
    await page.getByRole('button', { name: 'Show delivery', exact: true }).click(); await ready();
  });
  const canvas = page.locator('[data-geometry-format] canvas'), box = (await canvas.boundingBox())!;
  const contentReads = () => page.evaluate(name => (window as any).geometryContract.reads.filter((read: any) => read.name === name && !read.probe).length, workload.name);
  const expectedReads = workload.id.startsWith('stl') ? Math.ceil(12_250_084 / FILE_PREVIEW_LIMITS.pageBytes) : 1;
  expect(await contentReads()).toBe(expectedReads);
  const activity = async (button: 'left' | 'right') => {
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down({ button }); await page.mouse.move(box.x + box.width / 2 + 48, box.y + box.height / 2 + 16, { steps: 8 }); await page.mouse.up({ button });
  };
  const orbit = await sample(page, () => activity('left')), pan = await sample(page, () => activity('right'));
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Isometric', exact: true }).click(); await painted(page);
  let point: { x: number; y: number } | undefined;
  for (const y of [.5, .35, .65]) {
    for (const x of [.5, .35, .65]) {
      const candidate = { x: box.x + box.width * x, y: box.y + box.height * y };
      await page.mouse.click(candidate.x, candidate.y);
      if (await page.evaluate(() => (window as any).geometryContract.snapshot().some((tab: any) => tab.state?.selected))) { point = candidate; break; }
    }
    if (point) break;
  }
  expect(point).toBeDefined();
  const picking = await sample(page, async () => { await page.mouse.click(point!.x, point!.y); });
  expect(await contentReads()).toBe(expectedReads);
  const beforeSplit = performance.now(); await page.getByRole('button', { name: 'Preview cube.glb in sidebar', exact: true }).click();
  await expect(page.locator('[data-geometry-format]')).toContainText('10 × 10 × 10');
  await page.getByRole('button', { name: 'Split preview', exact: true }).click();
  await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(2);
  await expect.poll(async () => (await resources(page)).contexts).toBe(2); await painted(page);
  const twoModelDisplayMs = duration(beforeSplit), twoModelResources = await resources(page);
  await page.getByRole('button', { name: `Close preview ${workload.name}`, exact: true }).click();
  await page.getByRole('button', { name: 'Close preview cube.glb', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  const result = { workload, samplesPerAction: 5, firstDisplayMs, remount, orbit, pan, picking, twoModelDisplayMs, twoModelResources, afterRetirement: await resources(page) };
  writeFileSync(info.outputPath(`${workload.id}.json`), JSON.stringify(result, null, 2));
  await info.attach(workload.id, { body: JSON.stringify(result, null, 2), contentType: 'application/json' });
});

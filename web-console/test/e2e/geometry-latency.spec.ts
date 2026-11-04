import { test, expect } from '@playwright/test';
import { writeFileSync } from 'node:fs';
import { instrument, resources, observeGeometryLatency } from './geometry-observation';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: process.env.RUSTX_GEOMETRY_DIRECT === '1' ? undefined : '<loopback>' } });
function distribution(values: number[]) {
  const samples = values.map(value => Math.round(value * 10) / 10), sorted = [...samples].sort((a, b) => a - b);
  expect(samples.length).toBeGreaterThanOrEqual(80);
  for (const value of samples) { expect(Number.isFinite(value)).toBe(true); expect(value).toBeGreaterThanOrEqual(0); }
  return { samples, p50: sorted[Math.ceil(sorted.length * .5) - 1], p95: sorted[Math.ceil(sorted.length * .95) - 1] };
}

for (const workload of ['stl-735000-vertices', 'cad-96-parts']) test(`browser input and GPU-complete frame opportunity: ${workload}`, async ({ page }, info) => {
  await instrument(page);
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  if (workload.startsWith('stl')) await page.evaluate(() => (window as any).geometryContract.grid(350));
  else await page.evaluate(async () => {
    const response = await fetch('/test/fixtures/geometry/bracket-array.step');
    (window as any).geometryContract.replace('cube.step', [...new Uint8Array(await response.arrayBuffer())]);
  });
  await observeGeometryLatency(page);
  const name = workload.startsWith('stl') ? 'cube.stl' : 'cube.step';
  await page.getByRole('button', { name: `Preview ${name} in sidebar`, exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).geometryLatency.first), { timeout: 20_000 }).toBeGreaterThan(0);
  const canvas = page.locator('[data-geometry-format] canvas'), box = (await canvas.boundingBox())!;
  const browser = await canvas.evaluate((canvas: HTMLCanvasElement) => {
    const gl = canvas.getContext('webgl2')!, debug = gl.getExtension('WEBGL_debug_renderer_info');
    return { renderer: gl.getParameter(debug?.UNMASKED_RENDERER_WEBGL ?? gl.RENDERER), antialias: gl.getContextAttributes()!.antialias, userAgent: navigator.userAgent, dpr: devicePixelRatio };
  });
  const capture = async (button: 'left' | 'right') => {
    await page.evaluate(() => { (window as any).geometryLatency.samples = []; (window as any).geometryLatency.gpuSamples = []; (window as any).geometryLatency.stages = []; });
    for (let action = 0; action < 10; action++) {
      const x = box.x + box.width / 2, y = box.y + box.height / 2;
      await page.mouse.move(x, y); await page.mouse.down({ button });
      await page.mouse.move(x + 32, y + 12, { steps: 10 }); await page.mouse.up({ button });
    }
    await expect.poll(() => page.evaluate(() => (window as any).geometryLatency.pending)).toBe(0);
    const stages = await page.evaluate(() => (window as any).geometryLatency.stages as Record<string, number>[]);
    return { ...distribution(await page.evaluate(() => (window as any).geometryLatency.samples)), gpuComplete: distribution(await page.evaluate(() => (window as any).geometryLatency.gpuSamples)), stages: Object.fromEntries(['dispatch', 'submission', 'gpu', 'renderOpportunity'].map(key => [key, distribution(stages.map(stage => stage[key]))])) };
  };
  const orbit = await capture('left'), pan = await capture('right');
  expect(await page.evaluate(() => (window as any).geometryLatency.cancelled)).toBe(0);
  const result = { workload, browser, transport: process.env.RUSTX_GEOMETRY_DIRECT === '1' ? 'native-loopback' : 'Playwright-forwarded-loopback', firstFrameReadyMs: await page.evaluate(() => (window as any).geometryLatency.first), orbit, pan,
    firstGpuReadyMs: await page.evaluate(() => (window as any).geometryLatency.firstGpuReady),
    imports: await page.evaluate(() => ({ models: (window as any).geometryImports, timings: (window as any).geometryImportTimings })),
    mainPhases: await page.evaluate(() => performance.getEntriesByType('measure').filter(entry => entry.name.startsWith('geometry:')).map(entry => ({ name: entry.name, ms: entry.duration }))),
    drawCommands: await page.evaluate(() => ({ ...(window as any).geometryDrawCalls })),
    targets: { firstDisplayMs: 200, interactionP95Ms: 50 }, method: 'Proxy: trusted browser pointer timestamp to the actual model-render microtask, GPU fence settlement, and next rAF rendering opportunity. This does not observe compositor/physical display presentation. Source modules cold in each context. Original whole-drag display harness is retained separately.' };
  await page.getByRole('button', { name: `Close preview ${name}`, exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  writeFileSync(info.outputPath(`${workload}-latency.json`), JSON.stringify(result, null, 2));
  await info.attach(workload, { body: JSON.stringify(result, null, 2), contentType: 'application/json' });
  if (process.env.RUSTX_GEOMETRY_BUDGET === '1') { expect(result.firstFrameReadyMs).toBeLessThanOrEqual(200); expect(orbit.p95).toBeLessThanOrEqual(50); expect(pan.p95).toBeLessThanOrEqual(50); }
});

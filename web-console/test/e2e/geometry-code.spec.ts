import { test, expect, type Page } from '@playwright/test';
import { instrument, resources } from './geometry-observation';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: '<loopback>' } });
const retired = { workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 };
async function observeCode(page: Page) {
  await instrument(page);
  await page.addInitScript(() => {
    const Native = window.Worker, code = { workers: 0, modules: 0 };
    window.Worker = class extends Native {
      constructor(url: string | URL, options?: WorkerOptions) { super(url, options); if (options?.name === 'rustx-cad') code.workers++; }
      postMessage(message: any, options?: Transferable[] | StructuredSerializeOptions) {
        if (message?.module instanceof WebAssembly.Module) code.modules++;
        if (Array.isArray(options)) super.postMessage(message, options); else super.postMessage(message, options);
      }
    };
    Object.assign(window, { geometryCode: code });
  });
}
const counts = (page: Page) => page.evaluate(() => ({ ...(window as any).geometryCode }));
const ready = (page: Page, count = 1) => expect(page.locator('[data-geometry-format] canvas')).toHaveCount(count, { timeout: 20_000 });
const settledOutlines = (page: Page) => expect.poll(async () => page.getByRole('button', { name: 'Model outlines', exact: true }).evaluateAll(buttons => buttons.every(button => button.getAttribute('aria-pressed') === 'true' && button.getAttribute('aria-busy') === 'false'))).toBe(true);

test('cancelled CAD code wait retires its idle Worker and the retained copy reuses only compiled code', async ({ page }, info) => {
  await observeCode(page);
  let release!: () => void, requested!: () => void, fetches = 0;
  const held = new Promise<void>(resolve => { release = resolve; }), started = new Promise<void>(resolve => { requested = resolve; });
  await page.route(/occt-import-js.*\.wasm(?:\?|$)/, async route => { fetches++; requested(); await held; await route.continue(); });
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  expect(fetches).toBe(0);
  try {
    await page.getByRole('button', { name: 'Preview cube.step in sidebar', exact: true }).click();
    await started;
    await expect.poll(() => counts(page)).toEqual({ workers: 1, modules: 0 });
    await expect.poll(async () => (await resources(page)).workers).toBe(1);
    await page.getByRole('button', { name: 'Hide delivery', exact: true }).click();
    await expect.poll(() => resources(page)).toEqual(retired);
    release();
    await expect.poll(() => page.evaluate(() => performance.getEntriesByName('geometry:cad-code-compile').length), { timeout: 20_000 }).toBe(1);
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    expect(await counts(page)).toEqual({ workers: 1, modules: 0 });
    await ready(page, 0);
    const start = performance.now();
    await page.getByRole('button', { name: 'Show delivery', exact: true }).click(); await ready(page); await settledOutlines(page);
    const reopenedMs = performance.now() - start;
    expect(await counts(page)).toEqual({ workers: 2, modules: 1 }); expect(fetches).toBe(1);
    const phases = await page.evaluate(() => performance.getEntriesByType('measure').filter(entry => entry.name.startsWith('geometry:')).map(entry => ({ name: entry.name, ms: entry.duration })));
    await page.getByRole('button', { name: 'Close preview cube.step', exact: true }).click();
    await expect.poll(() => resources(page)).toEqual(retired);
    await info.attach('cancel-and-code-only-reopen', { body: JSON.stringify({ fetches, reopenedMs, phases, retirement: await resources(page), method: 'Lifecycle observation; reopen includes outline completion and uses the original elapsed action harness, not the cold acceptance endpoint.' }, null, 2), contentType: 'application/json' });
  } finally { release(); }
});

test('two actual split CAD owners share code and allocate fresh import Workers after retirement', async ({ page }, info) => {
  await observeCode(page); let fetches = 0;
  page.on('request', request => { if (/occt-import-js.*\.wasm(?:\?|$)/.test(request.url())) fetches++; });
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.getByRole('button', { name: 'Show all 8 delivered files', exact: true }).click();
  await page.evaluate(async () => {
    const bytes = [...new Uint8Array(await (await fetch('/test/fixtures/geometry/bracket-array.step')).arrayBuffer())];
    (window as any).geometryContract.replace('cube.step', bytes); (window as any).geometryContract.replace('cube-inch.step', bytes);
  });
  for (const name of ['cube.step', 'cube-inch.step']) {
    await page.getByRole('button', { name: `Preview ${name} in sidebar`, exact: true }).click(); await ready(page); await settledOutlines(page);
  }
  await page.getByRole('button', { name: 'Split preview', exact: true }).click(); await ready(page, 2); await settledOutlines(page);
  await expect.poll(async () => (await resources(page)).contexts).toBe(2);
  await page.getByRole('button', { name: 'Hide delivery', exact: true }).click(); await expect.poll(() => resources(page)).toEqual(retired);
  const before = await counts(page), start = performance.now();
  await page.getByRole('button', { name: 'Show delivery', exact: true }).click(); await ready(page, 2); await settledOutlines(page);
  const reopenedMs = performance.now() - start, after = await counts(page);
  expect(after.workers - before.workers).toBe(2); expect(after.modules - before.modules).toBe(2); expect(fetches).toBe(1);
  for (const name of ['cube.step', 'cube-inch.step']) expect(await page.evaluate(name => (window as any).geometryContract.reads.filter((read: any) => read.name === name && !read.probe).length, name)).toBe(1);
  const phases = await page.evaluate(() => performance.getEntriesByType('measure').filter(entry => entry.name.startsWith('geometry:')).map(entry => ({ name: entry.name, ms: entry.duration })));
  for (const name of ['cube.step', 'cube-inch.step']) await page.getByRole('button', { name: `Close preview ${name}`, exact: true }).click();
  await expect.poll(() => resources(page)).toEqual(retired);
  await info.attach('two-owner-code-only-reopen', { body: JSON.stringify({ fetches, before, after, reopenedMs, phases, retirement: await resources(page), method: 'Two genuine split CAD bodies; fresh Workers and instances on every reveal. Only compiled immutable code is shared. Reopen elapsed includes outline completion and is not cold acceptance latency.' }, null, 2), contentType: 'application/json' });
});

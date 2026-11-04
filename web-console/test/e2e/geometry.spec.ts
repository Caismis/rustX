import { test, expect, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { writeFileSync } from 'node:fs';
import { instrument, resources } from './geometry-observation';
import { denseBoxStl, planeAndTinySolidStl } from '../geometry-fixtures';
import { isCadRuntimeRequest, isGeometryRuntimeRequest } from './geometry-requests';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
const names = ['cube.glb', 'cube.stl', 'cube-ascii.stl', 'cube.step', 'cube-inch.step', 'cube.iges', 'cube.brep', 'cube-textured.glb'];
// Playwright's native forwarding keeps fixture URLs on the test runner even
// when the pinned browser runs behind Docker Desktop's separate network.
test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: '<loopback>' } });

const ready = (page: Page) => expect(page.locator('[data-geometry-format]')).toContainText('10 × 10 × 10', { timeout: 20_000 });
async function openDelivery(page: Page, name: string) { await page.getByRole('button', { name: `Preview ${name} in sidebar`, exact: true }).click(); await ready(page); }
function measuredDuration(value: number) {
  expect(Number.isFinite(value)).toBe(true); expect(value).toBeGreaterThanOrEqual(0);
  return Math.round(value * 10) / 10;
}
const elapsed = (start: number) => measuredDuration(performance.now() - start);

for (const fixture of [
  { name: 'near-limit dense closed STL', bytes: denseBoxStl, triangles: 330672, dimensions: '1 × 1 × 1' },
  { name: 'plane with disconnected tiny zero-normal solid', bytes: planeAndTinySolidStl, triangles: 20012, dimensions: '1 × 1 × 2' },
]) test(`actual Worker retains ${fixture.name} under its original deadline`, async ({ page }) => {
  await instrument(page); const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const bytes = fixture.bytes();
  await page.route('**/test/fixtures/geometry/cube.stl', route => route.fulfill({ body: Buffer.from(bytes), contentType: 'application/octet-stream' }));
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.getByRole('button', { name: 'Preview cube.stl in sidebar', exact: true }).click();
  await expect(page.locator('[data-geometry-format]')).toContainText(fixture.dimensions, { timeout: 20_000 });
  const imports = await page.evaluate(() => (window as any).geometryImports as { triangles: number }[][]);
  expect(imports).toHaveLength(1); expect(imports[0].reduce((sum, mesh) => sum + mesh.triangles, 0)).toBe(fixture.triangles);
  await page.getByRole('button', { name: 'Close preview cube.stl', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  expect(errors).toEqual([]);
});

test('all geometry formats render through actual delivery and Workspace entry points', async ({ page }, info) => {
  await instrument(page); const errors: string[] = [], timings: { entry: string; name: string; displayMs: number; importMs: number }[] = [];
  let navigations = 0; page.on('framenavigated', frame => { if (frame === page.mainFrame()) navigations++; });
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.getByRole('button', { name: 'Show all 8 delivered files', exact: true }).click();
  for (const entry of ['delivery', 'workspace']) for (const name of names) {
    const start = performance.now();
    if (entry === 'delivery') await openDelivery(page, name);
    else { await page.getByRole('button', { name: `Workspace ${name}`, exact: true }).click(); await ready(page); }
    const displayMs = elapsed(start), importMs = measuredDuration(await page.evaluate(name => (window as any).geometryContract.measureImport(name), name));
    timings.push({ entry, name, displayMs, importMs });
    if (entry === 'delivery') expect(await page.evaluate(name => (window as any).geometryContract.snapshot().find((item: any) => item.name === name).state.camera.up, name)).toEqual([0, 1, 0]);
    await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(1);
    await expect.poll(async () => (await resources(page)).workers).toBe(0);
    await expect.poll(async () => (await resources(page)).contexts).toBe(1);
  }
  await page.locator('[data-geometry-format]').screenshot({ path: info.outputPath('cad-preview.png') });
  writeFileSync(info.outputPath('format-display-timings.json'), JSON.stringify(timings, null, 2));
  await info.attach('format-display-timings', { body: JSON.stringify(timings, null, 2), contentType: 'application/json' });
  expect(errors).toEqual([]); expect(navigations).toBe(1);
});

test('production startup defers geometry and the STL envelope remains responsive', async ({ page }, info) => {
  await instrument(page); const runtime: string[] = [];
  page.on('request', request => { if (isGeometryRuntimeRequest(new URL(request.url()))) runtime.push(request.url()); });
  await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_PREVIEW_PORT ?? 5173}/`);
  await expect(page.locator('#root')).not.toBeEmpty(); await page.waitForLoadState('networkidle'); expect(runtime).toEqual([]);
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.evaluate(() => {
    (window as any).geometryContract.grid(350);
    const delays: number[] = []; let last = performance.now();
    const sample = (now: number) => { delays.push(now - last); last = now; if (delays.length < 600) requestAnimationFrame(sample); };
    requestAnimationFrame(sample); Object.assign(window, { geometryFrameDelays: delays });
  });
  const start = performance.now(); await page.getByRole('button', { name: 'Preview cube.stl in sidebar', exact: true }).click();
  await expect(page.locator('[data-geometry-format]')).toContainText('350 × 350 × 0', { timeout: 20_000 });
  const measurement = { triangles: 245000, vertices: 735000, sourceBytes: 12250084, displayMs: elapsed(start),
    observedFrameGapMs: measuredDuration(await page.evaluate(() => Math.max(...(window as any).geometryFrameDelays))), resources: await resources(page),
    browser: await page.locator('[data-geometry-format] canvas').evaluate((canvas: HTMLCanvasElement) => {
      const context = canvas.getContext('webgl2')!, debug = context.getExtension('WEBGL_debug_renderer_info');
      return { userAgent: navigator.userAgent, renderer: context.getParameter(debug?.UNMASKED_RENDERER_WEBGL ?? context.RENDERER), vendor: context.getParameter(debug?.UNMASKED_VENDOR_WEBGL ?? context.VENDOR), version: context.getParameter(context.VERSION), devicePixelRatio };
    }),
    idleDrawCalls: 0 };
  // Shaded dimensions can appear while default-on outlines are still preparing.
  // Complete that owned work before settling frames and sampling an idle view.
  const outlines = page.getByRole('button', { name: 'Model outlines', exact: true });
  await expect(outlines).toHaveAttribute('aria-pressed', 'true');
  await expect(outlines).toHaveAttribute('aria-busy', 'false');
  await expect.poll(async () => (await resources(page)).workers).toBe(0);
  await expect(page.locator('[data-geometry-format] canvas')).toHaveAttribute('data-geometry-quality', 'complete');
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  measurement.idleDrawCalls = await page.evaluate(async () => {
    const counter = (window as any).geometryDrawCalls, start = counter.calls;
    // A defined one-second sampling window after preparation measures demand rendering.
    await new Promise(resolve => setTimeout(resolve, 1000)); return counter.calls - start;
  });
  expect(measurement.idleDrawCalls).toBe(0);
  expect(runtime.filter(url => isCadRuntimeRequest(new URL(url)))).toEqual([]);
  await page.getByRole('button', { name: 'Fit view', exact: true }).click();
  await page.getByRole('button', { name: 'Close preview cube.stl', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  writeFileSync(info.outputPath('stl-envelope.json'), JSON.stringify(measurement, null, 2));
  await info.attach('stl-envelope', { body: JSON.stringify(measurement, null, 2), contentType: 'application/json' });
});

test('fixed copies survive source changes and hide/show; Refresh and original Download reauthorize', async ({ page }, info) => {
  await instrument(page); await page.goto(`${origin}/test/fixtures/geometry.html`); await openDelivery(page, 'cube.stl');
  await page.evaluate(() => (window as any).geometryContract.mutate()); await ready(page);
  await page.getByRole('button', { name: 'Hide delivery', exact: true }).click();
  await expect.poll(async () => (await resources(page)).contexts).toBe(0);
  await page.getByRole('button', { name: 'Show delivery', exact: true }).click(); await ready(page);
  expect(await page.evaluate(() => (window as any).geometryContract.reads.filter((read: any) => read.name === 'cube.stl' && !read.probe).length)).toBe(1);
  const downloaded = page.waitForEvent('download'); await page.getByRole('button', { name: 'Download cube.stl', exact: true }).click();
  const download = await downloaded, path = info.outputPath('current-source.stl'); await download.saveAs(path);
  expect([...readFileSync(path)]).toEqual(await page.evaluate(() => (window as any).geometryContract.source('cube.stl')));
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(page.locator('[data-geometry-format]')).toContainText('20 × 20 × 20');
  await page.getByRole('button', { name: 'Close preview cube.stl', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
});

test('selection, visibility, keyboard views, split/fullscreen and narrow localized layouts preserve ownership', async ({ page }, info) => {
  await instrument(page); await page.goto(`${origin}/test/fixtures/geometry.html`); await openDelivery(page, 'cube.glb');
  await page.getByRole('button', { name: /^Parts/ }).click();
  const part = page.getByRole('list', { name: 'Parts', exact: true }).getByRole('button').last(); await part.click(); await expect(part).toHaveAttribute('aria-pressed', 'true');
  await page.getByRole('button', { name: 'View angle', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Front', exact: true }).focus(); await page.keyboard.press('Enter');
  const visibility = page.getByRole('checkbox').last(); await visibility.uncheck(); await expect(visibility).not.toBeChecked(); await visibility.check();
  await openDelivery(page, 'cube.stl'); await page.getByRole('button', { name: 'Split preview', exact: true }).click();
  await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(2);
  await page.getByRole('button', { name: 'Fullscreen preview', exact: true }).click(); await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(2);
  await page.getByRole('button', { name: 'Restore preview', exact: true }).click();
  await page.setViewportSize({ width: 390, height: 844 }); await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(1);
  await page.getByRole('button', { name: 'Chinese', exact: true }).click(); await expect(page.getByRole('button', { name: '适合窗口', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Dark theme', exact: true }).click(); await expect(page.locator('body')).toHaveAttribute('data-ds-dark-theme', '');
  await page.locator('[data-geometry-format]').screenshot({ path: info.outputPath('geometry-narrow-zh-dark.png') });
  await page.getByRole('button', { name: 'Revoke authority', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
});

test('GLB external resources and lost WebGL contexts fail without requests or retained graphics', async ({ page }) => {
  await instrument(page); const directed: string[] = [];
  page.on('request', request => { if (request.url().includes('forbidden-model-resource')) directed.push(request.url()); });
  await page.goto(`${origin}/test/fixtures/geometry.html`); await openDelivery(page, 'cube.glb');
  await page.evaluate(() => {
    const api = (window as any).geometryContract, bytes = new Uint8Array(api.source('cube.glb')), view = new DataView(bytes.buffer), length = view.getUint32(12, true);
    const json = JSON.parse(new TextDecoder().decode(bytes.subarray(20, 20 + length))); json.buffers[0].uri = 'https://example.com/forbidden-model-resource.bin';
    const text = new TextEncoder().encode(JSON.stringify(json)), padded = Math.ceil(text.length / 4) * 4, bin = bytes.subarray(20 + length), next = new Uint8Array(20 + padded + bin.length), header = new DataView(next.buffer);
    next.set(bytes.subarray(0, 12)); header.setUint32(8, next.length, true); header.setUint32(12, padded, true); header.setUint32(16, 0x4e4f534a, true); next.fill(32, 20, 20 + padded); next.set(text, 20); next.set(bin, 20 + padded);
    api.replace('cube.glb', [...next]);
  });
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('external files'); expect(directed).toEqual([]);
  await expect.poll(async () => (await resources(page)).contexts).toBe(0);
  await openDelivery(page, 'cube.stl');
  await page.locator('[data-geometry-format] canvas').evaluate((canvas: HTMLCanvasElement) => canvas.getContext('webgl2')!.getExtension('WEBGL_lose_context')!.loseContext());
  await expect(page.getByRole('alert')).toContainText('3D preview is unavailable');
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
});

test('a decoded texture arriving after close is released without mounting a stale scene', async ({ page }) => {
  await instrument(page);
  await page.addInitScript(() => {
    const create = window.createImageBitmap, held: (() => void)[] = [], state = { open: 0, closed: 0, held: 0 };
    Object.defineProperty(window, 'createImageBitmap', { value: (...args: unknown[]) => Reflect.apply(create, window, args).then((bitmap: ImageBitmap) => {
      state.open++; const close = bitmap.close.bind(bitmap); let live = true;
      Object.defineProperty(bitmap, 'close', { value: () => { if (!live) return; live = false; state.open--; state.closed++; close(); } });
      return new Promise<ImageBitmap>(resolve => { held.push(() => resolve(bitmap)); state.held++; });
    }) });
    Object.assign(window, { geometryBitmaps: state, releaseGeometryBitmaps: () => { held.splice(0).forEach(release => release()); } });
  });
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.getByRole('button', { name: 'Show all 8 delivered files', exact: true }).click();
  await page.getByRole('button', { name: 'Preview cube-textured.glb in sidebar', exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).geometryBitmaps.held)).toBe(1);
  await page.getByRole('button', { name: 'Close preview cube-textured.glb', exact: true }).click();
  await page.getByRole('button', { name: 'Preview cube-textured.glb in sidebar', exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).geometryBitmaps.held)).toBe(2);
  await page.getByRole('button', { name: 'Close preview cube-textured.glb', exact: true }).click();
  await page.getByRole('button', { name: 'Preview cube-textured.glb in sidebar', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Preview is busy');
  await page.evaluate(() => (window as any).releaseGeometryBitmaps());
  await expect.poll(() => page.evaluate(() => (window as any).geometryBitmaps.open)).toBe(0);
  await page.getByRole('button', { name: 'Close preview cube-textured.glb', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  await expect(page.locator('[data-geometry-format]')).toHaveCount(0);
});

test('failed embedded-image decodes release generated URLs and physical import slots', async ({ page }) => {
  await instrument(page);
  await page.addInitScript(() => { Object.defineProperty(window, 'createImageBitmap', { value: () => Promise.reject(new Error('Controlled decoder rejection')) }); });
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.evaluate(async () => {
    const response = await fetch('/test/fixtures/geometry/cube-embedded.glb');
    (window as any).geometryContract.replace('cube.glb', [...new Uint8Array(await response.arrayBuffer())]);
  });
  await page.getByRole('button', { name: 'Preview cube.glb in sidebar', exact: true }).click();
  for (let attempt = 0; attempt < 3; attempt++) {
    await expect(page.getByRole('alert')).toContainText('Could not preview this model');
    await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
    if (attempt < 2) await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  }
});

for (const view of ['Top', 'Bottom']) test(`${view} horizontal orbit uses the selected up axis before and after restoration`, async ({ page }) => {
  await page.goto(`${origin}/test/fixtures/geometry.html`); await openDelivery(page, 'cube.stl');
  const pose = () => page.evaluate(() => (window as any).geometryContract.snapshot().find((item: any) => item.name === 'cube.stl').state.camera as { position: number[]; target: number[]; up: number[] });
  const drag = async () => {
    const before = await pose(), box = (await page.locator('[data-geometry-format] canvas').boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2); await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 80, box.y + box.height / 2, { steps: 8 }); await page.mouse.up();
    const after = await pose(); expect(Math.hypot(...after.position.map((value, index) => value - before.position[index]))).toBeGreaterThan(1);
    const axisDistance = (value: typeof before) => value.position.reduce((sum, component, index) => sum + (component - value.target[index]) * before.up[index], 0);
    expect(axisDistance(after)).toBeCloseTo(axisDistance(before), 5);
  };
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: view, exact: true }).click();
  await drag();
  await page.getByRole('button', { name: 'Hide delivery', exact: true }).click(); await page.getByRole('button', { name: 'Show delivery', exact: true }).click(); await ready(page);
  await drag();
});

test('a burst of camera changes is coalesced and immediate hide saves its final pose', async ({ page }) => {
  await page.goto(`${origin}/test/fixtures/geometry.html`); await openDelivery(page, 'cube.stl');
  const canvas = page.locator('[data-geometry-format] canvas'), box = (await canvas.boundingBox())!;
  await canvas.evaluate(element => element.addEventListener('pointerdown', event => {
    (window as any).geometryPointerId = (event as PointerEvent).pointerId;
  }, { once: true }));
  const start = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  await page.mouse.move(start.x, start.y); await page.mouse.down();
  const capture = await page.evaluate(start => {
    const api = (window as any).geometryContract, canvas = document.querySelector('[data-geometry-format] canvas')!;
    const pose = () => api.snapshot().find((tab: any) => tab.name === 'cube.stl').state.camera;
    const publications = api.activity.publications;
    for (let step = 1; step <= 20; step++) canvas.dispatchEvent(new PointerEvent('pointermove', {
      pointerId: (window as any).geometryPointerId, pointerType: 'mouse', buttons: 1,
      clientX: start.x + step * 2, clientY: start.y + step, bubbles: true,
    }));
    const during = pose();
    [...document.querySelectorAll('button')].find(button => button.textContent === 'Hide delivery')!.click();
    return { during, saved: pose(), publications: api.activity.publications - publications };
  }, start);
  await page.mouse.up();
  expect(capture.publications).toBeLessThanOrEqual(3);
  expect(capture.saved.position).not.toEqual(capture.during.position);
  await page.getByRole('button', { name: 'Show delivery', exact: true }).click(); await ready(page);
  const restored = await page.evaluate(() => (window as any).geometryContract.snapshot().find((tab: any) => tab.name === 'cube.stl').state.camera);
  for (const key of ['position', 'target', 'up'] as const) capture.saved[key].forEach((value: number, index: number) => expect(restored[key][index]).toBeCloseTo(value, 7));
  expect(await page.evaluate(() => (window as any).geometryContract.reads.filter((read: any) => read.name === 'cube.stl' && !read.probe).length)).toBe(1);
});

import { test, expect, type Page } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { BoxGeometry, SphereGeometry, Triangle, Vector3 } from 'three';
import { PNG } from 'pngjs';
import { instrument, resources } from './geometry-observation';
import { denseBoxStl } from '../geometry-fixtures';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: '<loopback>' } });
const ready = (page: Page) => expect(page.locator('[data-geometry-format] canvas')).toBeVisible();
const pose = (page: Page) => page.evaluate(() => (window as any).geometryContract.snapshot().find((tab: any) => tab.name === 'cube.glb').state);
const reads = (page: Page) => page.evaluate(() => (window as any).geometryContract.reads.filter((read: any) => !read.probe).length);
const painted = (page: Page) => page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));

function assemblyGlb(nodes: object[] = [{ name: 'Assembly', translation: [100,200,300], scale: [2,3,4], children: [1,2] }, { name: 'A', mesh: 0 }, { name: 'B', mesh: 0, translation: [20,0,0] }]) {
  const original = new Uint8Array(readFileSync(new URL('../fixtures/geometry/cube.glb', import.meta.url))), header = new DataView(original.buffer), length = header.getUint32(12, true);
  const json = JSON.parse(new TextDecoder().decode(original.subarray(20, 20 + length)));
  json.nodes = nodes;
  json.scenes = [{ nodes: [0] }]; json.scene = 0;
  const text = new TextEncoder().encode(JSON.stringify(json)), padded = Math.ceil(text.length / 4) * 4, bin = original.subarray(20 + length);
  const bytes = new Uint8Array(20 + padded + bin.length), view = new DataView(bytes.buffer);
  bytes.set(original.subarray(0,12)); view.setUint32(8, bytes.length, true); view.setUint32(12, padded, true); view.setUint32(16, 0x4e4f534a, true);
  bytes.fill(32,20,20+padded); bytes.set(text,20); bytes.set(bin,20+padded); return bytes;
}
async function load(page: Page, entry = 'delivery', bytes = assemblyGlb()) {
  await instrument(page); await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.evaluate(bytes => (window as any).geometryContract.replace('cube.glb', bytes), [...bytes]);
  await page.getByRole('button', { name: entry === 'delivery' ? 'Preview cube.glb in sidebar' : 'Workspace cube.glb', exact: true }).click(); await ready(page);
}
async function parts(page: Page) {
  await page.getByRole('button', { name: 'Parts', exact: true }).click();
  await page.getByRole('button', { name: 'Expand: Assembly', exact: true }).click();
  return page.getByRole('list', { name: 'Parts', exact: true });
}
for (const entry of ['delivery', 'workspace']) test(`roadmap structure, focus and source-coordinate properties: ${entry}`, async ({ page }) => {
  await load(page, entry); const tree = await parts(page), initialReads = await reads(page);
  const a = tree.getByRole('button', { name: 'A', exact: true }), b = tree.getByRole('button', { name: 'B', exact: true });
  await tree.getByRole('checkbox', { name: 'Visible: B', exact: true }).uncheck();
  await a.click(); const properties = page.getByRole('region', { name: 'Properties', exact: true });
  await expect(properties).toContainText('20 × 30 × 40'); await expect(properties).toContainText('100, 200, 300');
  await expect(properties).toContainText('120, 230, 340'); await expect(properties).toContainText('Tessellated triangle mesh');
  await tree.getByRole('checkbox', { name: 'Visible: Assembly', exact: true }).uncheck();
  await expect(tree.getByRole('checkbox', { name: 'Visible: A', exact: true })).toHaveAttribute('aria-description', 'Hidden by an ancestor');
  await expect(page.getByRole('button', { name: 'Focus selected', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Isolate', exact: true }).click(); await expect(page.getByRole('button', { name: 'Exit isolation', exact: true })).toBeVisible();
  await b.click(); await expect(page.getByRole('button', { name: 'Focus selected', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Isolate', exact: true }).click();
  await expect(tree.getByRole('checkbox', { name: 'Visible: B', exact: true })).toBeChecked();
  await page.getByRole('button', { name: 'Focus selected', exact: true }).click();
  await page.getByRole('button', { name: 'Exit isolation', exact: true }).click();
  await expect(tree.getByRole('checkbox', { name: 'Visible: B', exact: true })).not.toBeChecked();
  await page.getByRole('button', { name: 'Show all', exact: true }).click(); await expect(tree.getByRole('checkbox', { name: 'Visible: B', exact: true })).toBeChecked();
  await a.click(); await page.getByRole('button', { name: 'Isolate', exact: true }).click();
  await page.getByRole('button', { name: entry === 'delivery' ? 'Hide delivery' : 'Toggle workspace body', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  await page.getByRole('button', { name: entry === 'delivery' ? 'Show delivery' : 'Toggle workspace body', exact: true }).click(); await ready(page);
  await page.getByRole('button', { name: 'Parts', exact: true }).click(); await expect(page.getByRole('button', { name: 'Exit isolation', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Show all', exact: true }).click();
  expect(await reads(page)).toBe(initialReads);
});

test('all six axis endpoints match menu directions, stay separate and never pick the model', async ({ page }, info) => {
  await load(page); const tree = await parts(page); await tree.getByRole('button', { name: 'A', exact: true }).click();
  await page.getByRole('button', { name: 'Close parts', exact: true }).click();
  await expect(page.locator('[data-geometry-axis-indicator] button')).toHaveCount(0);
  for (const [view, label] of [['right','Right'], ['left','Left'], ['top','Top'], ['bottom','Bottom'], ['front','Front'], ['back','Back']]) {
    await page.getByRole('button', { name: 'View angle', exact: true }).click();
    await expect(page.getByRole('menuitem', { name: label, exact: true }).locator('[data-view-icon]')).toHaveAttribute('data-view-icon', view);
    await page.getByRole('menuitem', { name: label, exact: true }).click(); const menuPose = await pose(page); await painted(page);
    await page.locator(`[data-geometry-axis="${view}"] text`).click(); const axisPose = await pose(page);
    for (const key of ['position', 'target', 'up']) axisPose.camera[key].forEach((value: number, index: number) => expect(value).toBeCloseTo(menuPose.camera[key][index], 8));
    expect(axisPose.selected).toBe(menuPose.selected);
    const boxes = await page.locator('[data-geometry-axis] text').evaluateAll(labels => labels.map(label => { const b = label.getBoundingClientRect(); return { x: b.x, y: b.y, width: b.width, height: b.height }; }));
    for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) {
      const a = boxes[i], b = boxes[j]; expect(Math.max(0, Math.min(a.x+a.width,b.x+b.width)-Math.max(a.x,b.x)) * Math.max(0, Math.min(a.y+a.height,b.y+b.height)-Math.max(a.y,b.y))).toBeLessThan(1);
    }
    if (view === 'top' || view === 'bottom') {
      const box = (await page.locator('[data-geometry-format] canvas').boundingBox())!;
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2); await page.mouse.down(); await page.mouse.move(box.x + box.width / 2 + 60, box.y + box.height / 2, { steps: 8 }); await page.mouse.up();
      const after = await pose(page); expect(after.camera.position).not.toEqual(axisPose.camera.position); expect(after.camera.up).toEqual(axisPose.camera.up);
    }
  }
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Isometric', exact: true }).click(); await painted(page);
  await page.locator('[data-geometry-axis="right"] path').click();
  const linePose = await pose(page); expect(linePose.camera.position[0]).toBeGreaterThan(0); expect(Math.abs(linePose.camera.position[2])).toBeLessThan(1e-6); expect(linePose.selected).toBeDefined();
  await page.setViewportSize({ width: 700, height: 500 }); await painted(page);
  await page.locator('[data-geometry-axis="right"]').focus(); await page.keyboard.press('Enter'); expect((await pose(page)).selected).toBeDefined();
  await page.getByRole('button', { name: 'Chinese', exact: true }).click(); await page.getByRole('button', { name: 'Dark theme', exact: true }).click();
  await expect(page.getByRole('button', { name: '+X: 右视图', exact: true })).toBeVisible();
  await page.locator('[data-geometry-format]').screenshot({ path: info.outputPath('geometry-axis-narrow-zh-dark.png') });
});

test('projection switches preserve framing and resources, persist orthographic wheel zoom, and reset in the chosen projection', async ({ page }, info) => {
  await load(page); const tree = await parts(page);
  await tree.getByRole('checkbox', { name: 'Visible: B', exact: true }).uncheck(); await tree.getByRole('button', { name: 'A', exact: true }).click();
  await page.getByRole('button', { name: 'Focus selected', exact: true }).click(); await page.getByRole('button', { name: 'Close parts', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Model outlines', exact: true })).toHaveAttribute('aria-pressed', 'true'); await painted(page);
  const initial = await pose(page), initialReads = await reads(page), before = await resources(page);
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Orthographic', exact: true }).click(); await painted(page);
  const orthographic = await pose(page); expect(orthographic.camera.projection).toBe('orthographic');
  for (const key of ['position', 'target', 'up']) orthographic.camera[key].forEach((value: number, i: number) => expect(value).toBeCloseTo(initial.camera[key][i], 8));
  const distance = Math.hypot(...initial.camera.position.map((value: number, i: number) => value - initial.camera.target[i]));
  expect(orthographic.camera.verticalSpan).toBeCloseTo(2 * distance * Math.tan(Math.PI / 8), 8); expect(await resources(page)).toEqual(before);
  const canvas = page.locator('[data-geometry-format] canvas'), box = (await canvas.boundingBox())!; await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2); await page.mouse.wheel(0, -100);
  await expect.poll(async () => (await pose(page)).camera.verticalSpan).toBeLessThan(orthographic.camera.verticalSpan); const zoomed = await pose(page);
  zoomed.camera.position.forEach((value: number, i: number) => expect(value).toBeCloseTo(orthographic.camera.position[i], 8));
  await page.getByRole('button', { name: 'Hide delivery', exact: true }).click(); await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  await page.getByRole('button', { name: 'Show delivery', exact: true }).click(); await ready(page);
  const restored = await pose(page); expect(restored.camera.projection).toBe('orthographic'); expect(restored.camera.verticalSpan).toBeCloseTo(zoomed.camera.verticalSpan, 8); expect(restored.hidden).toEqual(initial.hidden); expect(restored.selected).toBe(initial.selected); expect(await reads(page)).toBe(initialReads);
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Reset view', exact: true }).click();
  const reset = await pose(page); expect(reset.camera.projection).toBe('orthographic'); expect(reset.hidden).toEqual(initial.hidden); expect(reset.camera.position[0]).toBeCloseTo(reset.camera.position[1], 8); expect(reset.camera.position[1]).toBeCloseTo(reset.camera.position[2], 8);
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await expect(page.getByRole('menuitem', { name: 'Orthographic', exact: true })).toHaveAttribute('aria-current', 'true');
  await page.locator('[data-geometry-format]').screenshot({ path: info.outputPath('projection-orthographic-menu.png') });
  await page.getByRole('menuitem', { name: 'Perspective', exact: true }).click();
  expect((await pose(page)).camera.projection).toBe('perspective'); expect((await pose(page)).camera).not.toHaveProperty('verticalSpan'); expect((await resources(page)).contexts).toBe(1);
});

test('mesh outlines start enabled, can be disabled, restore the option and add no idle loop', async ({ page }, info) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await load(page); const initialReads = await reads(page);
  const button = page.getByRole('button', { name: 'Model outlines', exact: true });
  await expect(button).toHaveAttribute('aria-pressed', 'true');
  await button.click(); await expect(button).toHaveAttribute('aria-pressed', 'false'); await painted(page);
  const before = await resources(page);
  for (let i = 0; i < 2; i++) {
    await button.click(); await expect(button).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(async () => (await resources(page)).workers).toBe(0); await painted(page);
    expect((await resources(page)).buffers).toBeGreaterThan(before.buffers);
    const calls = await page.evaluate(() => (window as any).geometryDrawCalls.calls); await page.waitForTimeout(250);
    expect(await page.evaluate(() => (window as any).geometryDrawCalls.calls)).toBe(calls);
    await page.locator('[data-geometry-format]').screenshot({ path: info.outputPath(`geometry-outlines-${i}.png`) });
    await button.click(); await expect(button).toHaveAttribute('aria-pressed', 'false'); await painted(page);
    expect((await resources(page)).buffers).toBe(before.buffers);
  }
  await button.click(); await expect(button).toHaveAttribute('aria-pressed', 'true');
  await page.getByRole('button', { name: 'Hide delivery', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  await page.getByRole('button', { name: 'Show delivery', exact: true }).click(); await ready(page); await expect(button).toHaveAttribute('aria-pressed', 'true');
  expect(await reads(page)).toBe(initialReads); expect(errors).toEqual([]);
  await page.getByRole('button', { name: 'Close preview cube.glb', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
});

for (const projection of ['Perspective', 'Orthographic']) for (const scale of [1e-6, 1, 1e6]) test(`${projection} thin occluded outlines remain hidden at model scale ${scale}`, async ({ page }) => {
  // The source cube spans [0, 10]. Keep its opaque authored material and form
  // a 2x2x.02 occluder at the origin and a 1x1x.02 box at z=-.04.
  await load(page, 'delivery', assemblyGlb([
    { name: 'Assembly', scale: [scale, scale, scale], children: [1, 2] },
    { name: 'A', mesh: 0, translation: [-1, -1, -.01], scale: [.2, .2, .002] },
    { name: 'B', mesh: 0, translation: [-.5, -.5, -.05], scale: [.1, .1, .002] },
  ]));
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: projection, exact: true }).click();
  await page.getByRole('button', { name: 'View angle', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Front', exact: true }).click();
  const tree = await parts(page), canvas = page.locator('[data-geometry-format] canvas');
  const settle = async () => { await canvas.focus(); await page.mouse.move(0, 0); await expect(page.getByRole('tooltip')).toHaveCount(0); await painted(page); };
  await page.getByRole('button', { name: 'Close parts', exact: true }).click(); await settle();
  await expect.poll(async () => (await resources(page)).workers).toBe(0); await painted(page);
  const both = PNG.sync.read(await canvas.screenshot());
  await page.getByRole('button', { name: 'Parts', exact: true }).click();
  await tree.getByRole('checkbox', { name: 'Visible: B', exact: true }).uncheck();
  await page.getByRole('button', { name: 'Close parts', exact: true }).click(); await settle();
  const occluder = PNG.sync.read(await canvas.screenshot());
  expect(both.data).toEqual(occluder.data);
  await page.getByRole('button', { name: 'Model outlines', exact: true }).click(); await settle();
  const shaded = PNG.sync.read(await canvas.screenshot());
  // Also establish that the visible outline survives the anti-fighting change.
  expect(occluder.data.equals(shaded.data)).toBe(false);
});

function thinAssemblyStl(scale: number) {
  const text: string[] = [];
  for (const [name, width, z] of [['A', 2, 0], ['B', 1, -.04]] as const) {
    const box = new BoxGeometry(width, width, .02), geometry = box.toNonIndexed(); box.dispose(); geometry.translate(0, 0, z).scale(scale, scale, scale);
    const positions = geometry.getAttribute('position'), normals = geometry.getAttribute('normal'); text.push(`solid ${name}`);
    for (let vertex = 0; vertex < positions.count; vertex += 3) {
      text.push(`facet normal ${normals.getX(vertex)} ${normals.getY(vertex)} ${normals.getZ(vertex)}`, 'outer loop');
      for (let i = 0; i < 3; i++) text.push(`vertex ${positions.getX(vertex + i)} ${positions.getY(vertex + i)} ${positions.getZ(vertex + i)}`);
      text.push('endloop', 'endfacet');
    }
    text.push(`endsolid ${name}`); geometry.dispose();
  }
  return [...new TextEncoder().encode(text.join('\n'))];
}
for (const projection of ['Perspective', 'Orthographic']) for (const scale of [1e-6, 1, 1e6]) test(`${projection} generated thin occluded outlines remain hidden at model scale ${scale}`, async ({ page }) => {
  await instrument(page); await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.evaluate(bytes => (window as any).geometryContract.replace('cube.stl', bytes), thinAssemblyStl(scale));
  await page.getByRole('button', { name: 'Preview cube.stl in sidebar', exact: true }).click(); await ready(page);
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: projection, exact: true }).click();
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Front', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Model outlines', exact: true })).toHaveAttribute('aria-pressed', 'true');
  const canvas = page.locator('[data-geometry-format] canvas'), tree = page.getByRole('list', { name: 'Parts', exact: true });
  const settle = async () => { await canvas.focus(); await page.mouse.move(0, 0); await expect(page.getByRole('tooltip')).toHaveCount(0); await painted(page); };
  await expect.poll(async () => (await resources(page)).workers).toBe(0); await settle();
  const both = PNG.sync.read(await canvas.screenshot());
  await page.getByRole('button', { name: 'Parts', exact: true }).click();
  await tree.getByRole('checkbox', { name: 'Visible: B', exact: true }).uncheck();
  await page.getByRole('button', { name: 'Close parts', exact: true }).click(); await settle();
  const occluder = PNG.sync.read(await canvas.screenshot()); expect(both.data).toEqual(occluder.data);
  await page.getByRole('button', { name: 'Model outlines', exact: true }).click(); await settle();
  const shaded = PNG.sync.read(await canvas.screenshot()); expect(occluder.data.equals(shaded.data)).toBe(false);
});

test('outline gate preserves a near-limit shaded STL and keeps original Download', async ({ page }) => {
  await instrument(page); await page.route('**/test/fixtures/geometry/cube.stl', route => route.fulfill({ body: Buffer.from(denseBoxStl()), contentType: 'application/octet-stream' }));
  await page.goto(`${origin}/test/fixtures/geometry.html`); await page.getByRole('button', { name: 'Preview cube.stl in sidebar', exact: true }).click(); await ready(page);
  await expect(page.getByRole('status')).toContainText('Outlines are unavailable');
  await expect(page.locator('[data-geometry-format] canvas')).toBeVisible(); await expect(page.getByRole('button', { name: 'Download artifact', exact: true })).toBeEnabled();
  expect(await reads(page)).toBe(1); expect((await resources(page)).workers).toBe(0);
  await page.getByRole('button', { name: 'Close preview cube.stl', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
});

test('curved outline capture contains silhouettes rather than a triangle wireframe', async ({ page }, info) => {
  const geometry = new SphereGeometry(1, 48, 32).toNonIndexed(), positions = geometry.getAttribute('position');
  const bytes = new Uint8Array(84 + positions.count / 3 * 50), data = new DataView(bytes.buffer); data.setUint32(80, positions.count / 3, true);
  for (let i = 0; i < positions.count; i += 3) {
    const normal = new Triangle(...[0, 1, 2].map(offset => new Vector3().fromBufferAttribute(positions, i + offset)) as [Vector3, Vector3, Vector3]).getNormal(new Vector3());
    normal.toArray().forEach((value, axis) => data.setFloat32(84 + i / 3 * 50 + axis * 4, value, true));
    for (let vertex = 0; vertex < 3; vertex++) for (let axis = 0; axis < 3; axis++) data.setFloat32(84 + i / 3 * 50 + 12 + vertex * 12 + axis * 4, positions.getComponent(i + vertex, axis), true);
  }
  await instrument(page); await page.route('**/test/fixtures/geometry/cube.stl', route => route.fulfill({ body: Buffer.from(bytes), contentType: 'application/octet-stream' }));
  await page.goto(`${origin}/test/fixtures/geometry.html`); await page.getByRole('button', { name: 'Preview cube.stl in sidebar', exact: true }).click(); await ready(page);
  await expect(page.getByRole('button', { name: 'Model outlines', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await page.locator('[data-geometry-format]').screenshot({ path: info.outputPath('geometry-smooth-silhouette.png') });
  writeFileSync(info.outputPath('outline-workload.json'), JSON.stringify({ triangles: positions.count / 3, resources: await resources(page), reads: await reads(page) }, null, 2));
});

for (const workload of ['planar-outline-envelope', 'cad-96-parts']) test(`two-pane outline resource and interaction capture: ${workload}`, async ({ page }, info) => {
  await instrument(page); await page.goto(`${origin}/test/fixtures/geometry.html`);
  // 96,800 triangles exercise 96.8% of the optional outline input ceiling,
  // independently of whether the cooperative planar optimizer completes.
  if (workload === 'planar-outline-envelope') await page.evaluate(() => (window as any).geometryContract.grid(220));
  else await page.evaluate(async () => (window as any).geometryContract.replace('cube.step', [...new Uint8Array(await (await fetch('/test/fixtures/geometry/bracket-array.step')).arrayBuffer())]));
  const name = workload === 'planar-outline-envelope' ? 'cube.stl' : 'cube.step';
  await page.getByRole('button', { name: `Preview ${name} in sidebar`, exact: true }).click(); await ready(page);
  await page.getByRole('button', { name: 'Preview cube.glb in sidebar', exact: true }).click(); await ready(page);
  await page.getByRole('button', { name: 'Split preview', exact: true }).click();
  await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(2); await expect.poll(async () => (await resources(page)).workers).toBe(0); await painted(page);
  for (const panel of await page.locator('[data-geometry-format]').all()) {
    const button = panel.getByRole('button', { name: 'Model outlines', exact: true });
    await expect(button).toHaveAttribute('aria-pressed', 'true'); await button.click(); await expect(button).toHaveAttribute('aria-pressed', 'false');
  }
  await page.evaluate(() => {
    const before = window.Worker, outputs: { segments: number; bytes: number }[] = [];
    window.Worker = class extends before {
      constructor(url: string | URL, options?: WorkerOptions) { super(url, options); if (options?.name === 'rustx-outlines') this.addEventListener('message', event => {
        if (event.data?.ok && Array.isArray(event.data.data)) outputs.push({ segments: event.data.data.reduce((sum: number, array: Float32Array) => sum + array.length / 20, 0), bytes: event.data.data.reduce((sum: number, array: Float32Array) => sum + array.byteLength, 0) });
      }); }
    }; Object.assign(window, { roadmapOutlineOutputs: outputs });
  });
  const format = workload === 'planar-outline-envelope' ? 'stl' : 'step', canvas = page.locator(`[data-geometry-format="${format}"] canvas`), box = (await canvas.boundingBox())!;
  const capture = async () => {
    await page.locator(`[data-geometry-format="${format}"]`).getByRole('button', { name: 'View angle', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Isometric', exact: true }).click(); await painted(page);
    const samples: number[] = [];
    for (let i = 0; i < 5; i++) {
      const start = performance.now(); await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2); await page.mouse.down();
      await page.mouse.move(box.x + box.width / 2 + 32, box.y + box.height / 2 + 12, { steps: 8 }); await page.mouse.up(); await painted(page); samples.push(performance.now() - start);
    }
    const sorted = [...samples].sort((a, b) => a - b); return { samples, p50: sorted[2], p95: sorted[4] };
  };
  const off = await capture(), beforeResources = await resources(page), preparation: number[] = [];
  for (const panel of await page.locator('[data-geometry-format]').all()) {
    const start = performance.now(), button = panel.getByRole('button', { name: 'Model outlines', exact: true });
    await button.click(); await expect(button).toHaveAttribute('aria-pressed', 'true'); preparation.push(performance.now() - start);
  }
  await painted(page); const on = await capture(), withOutlines = await resources(page);
  const browser = await canvas.evaluate((canvas: HTMLCanvasElement) => {
    const gl = canvas.getContext('webgl2')!, debug = gl.getExtension('WEBGL_debug_renderer_info');
    return { userAgent: navigator.userAgent, renderer: gl.getParameter(debug?.UNMASKED_RENDERER_WEBGL ?? gl.RENDERER), dpr: devicePixelRatio, viewport: [innerWidth, innerHeight], canvasCss: [canvas.clientWidth, canvas.clientHeight], canvasPixels: [canvas.width, canvas.height], antialias: gl.getContextAttributes()!.antialias };
  });
  const outputs = await page.evaluate(() => (window as any).roadmapOutlineOutputs as { segments: number; bytes: number }[]);
  expect(outputs).toHaveLength(2); for (const output of outputs) { expect(output.segments).toBeLessThanOrEqual(65536); expect(output.bytes).toBeLessThanOrEqual(65536 * 80); }
  expect(withOutlines.contexts).toBe(2); expect(withOutlines.workers).toBe(0);
  expect(browser.antialias).toBe(true); expect(await reads(page)).toBe(2);
  const draws = await page.evaluate(() => (window as any).geometryDrawCalls.calls); await page.waitForTimeout(1000); expect(await page.evaluate(() => (window as any).geometryDrawCalls.calls)).toBe(draws);
  await page.getByRole('button', { name: `Close preview ${name}`, exact: true }).click(); await page.getByRole('button', { name: 'Close preview cube.glb', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  const result = { workload, source: workload === 'planar-outline-envelope' ? { vertices: 290400, triangles: 96800 } : { parts: 96 }, browser, method: 'Five whole eight-step drags plus two rAFs; Playwright forwarding; optional outlines off/on in the same two-pane page, resetting to Isometric before each phase. Preparation ends at the logical ready state, not GPU completion. This is not per-event presentation latency.', off, on, preparation, outputs, beforeResources, withOutlines, afterRetirement: await resources(page) };
  writeFileSync(info.outputPath(`${workload}-outlines.json`), JSON.stringify(result, null, 2)); await info.attach(workload, { body: JSON.stringify(result, null, 2), contentType: 'application/json' });
});

test.describe('scaled coordinate navigation', () => {
  test.use({ deviceScaleFactor: 2 });
  test('uses CSS hit areas after DPR/resize and preserves selection', async ({ page }) => {
    await load(page); await page.setViewportSize({ width: 700, height: 500 }); await painted(page);
    await page.getByRole('button', { name: '+X: Right', exact: true }).locator('text').click();
    const state = await pose(page); expect(state.camera.position[0]).toBeGreaterThan(0);
    expect(Math.abs(state.camera.position[1])).toBeLessThan(1e-6); expect(Math.abs(state.camera.position[2])).toBeLessThan(1e-6);
    const scale = await page.locator('[data-geometry-format] canvas').evaluate(canvas => (canvas as HTMLCanvasElement).width / canvas.clientWidth); expect(scale).toBeCloseTo(2, 2);
    await page.getByRole('button', { name: '−Z: Back', exact: true }).focus(); await page.keyboard.press('Enter'); expect((await pose(page)).camera.position[2]).toBeLessThan(0);
    expect(await reads(page)).toBe(1);
    await page.getByRole('button', { name: 'Close preview cube.glb', exact: true }).click();
    await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  });
});

import { test, expect, type Page } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { BoxGeometry, OrthographicCamera, PerspectiveCamera, Plane, Raycaster, SphereGeometry, Triangle, Vector2, Vector3 } from 'three';
import { PNG } from 'pngjs';
import { instrument, observeGeometryLatency, resources } from './geometry-observation';
import { denseBoxStl } from '../geometry-fixtures';
import { FILE_PREVIEW_LIMITS } from '../../shared/file-preview';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: '<loopback>' } });
const ready = (page: Page) => expect(page.locator('[data-geometry-format]:visible canvas[data-geometry-completeness="complete"]')).toBeVisible();
const pose = (page: Page) => page.evaluate(() => (window as any).geometryContract.snapshot().find((tab: any) => tab.name === 'cube.glb').state);
const reads = (page: Page) => page.evaluate(() => (window as any).geometryContract.reads.filter((read: any) => !read.probe).length);
const painted = (page: Page) => page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
// Pixel/resource/idle assertions require the final picture. Latency clocks keep
// their original first-model and input endpoints and do not use this helper.
const refined = async (page: Page) => { await expect.poll(() => page.locator('[data-geometry-format]:visible canvas').evaluateAll(canvases => canvases.length > 0 && canvases.every(canvas => canvas.getAttribute('data-geometry-quality') === 'complete'))).toBe(true); await painted(page); };

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

for (const entry of ['delivery', 'workspace']) test(`section clips transformed surfaces in ${entry} and preserves the original copy`, async ({ page }, info) => {
  const shaderErrors: string[] = [];
  page.on('console', message => { if (message.type() === 'error' && /WebGLProgram|Shader Error/.test(message.text())) shaderErrors.push(message.text()); });
  const model = assemblyGlb([
    { name: 'Assembly', translation: [100, 200, 300], scale: [2, 3, 4], children: [1, 2] },
    { name: 'A', mesh: 0 }, { name: 'B', mesh: 0, translation: [30, 0, 0], scale: [-1, 1, 1] },
  ]);
  await load(page, entry, model); await refined(page);
  const canvas = page.locator('[data-geometry-format]:visible canvas');
  await expect(page.getByRole('button', { name: 'Model outlines', exact: true })).toHaveAttribute('aria-pressed', 'true');
  const viewport = (await canvas.boundingBox())!;
  const span = Math.hypot(60, 30, 40) * 1.1 / Math.min(1, viewport.width / viewport.height);
  await canvas.press('Numpad1'); await refined(page);
  const startingReads = await reads(page), startingResources = await resources(page);
  const handles = await canvas.elementHandle();
  const samples = async () => {
    await painted(page);
    const image = PNG.sync.read(await canvas.screenshot({ scale: 'css' }));
    const center = image.width / 2, displacement = 20 * image.height / span;
    const sample = (x: number, y: number) => {
      const at = (Math.round(y) * image.width + Math.round(x)) * 4;
      return image.data.subarray(at, at + 3);
    };
    const background = sample(image.width * .92, image.height / 2);
    return [center - displacement, center + displacement].map(x => {
      const color = sample(x, image.height / 2);
      return Math.hypot(...Array.from(color, (value, index) => value - background[index])) > 35;
    });
  };
  await expect.poll(samples).toEqual([true, true]);
  const toggle = page.getByRole('button', { name: 'Section', exact: true });
  await observeGeometryLatency(page, '[data-geometry-format] button[aria-label="Section"]', 'click', true);
  await toggle.click();
  const axis = page.getByRole('combobox', { name: 'Axis' }), slider = page.getByRole('slider', { name: 'Position' });
  await expect(axis).toHaveValue('z');
  await expect.poll(() => page.evaluate(() => (window as any).geometryLatency.first)).toBeGreaterThan(0);
  await axis.selectOption('x');
  await expect.poll(samples).toEqual([false, true]);
  await page.getByRole('button', { name: 'Reverse side', exact: true }).click();
  await expect.poll(samples).toEqual([true, false]);
  await slider.press('Home');
  await expect.poll(samples).toEqual([false, false]);
  await slider.press('End');
  await expect.poll(samples).toEqual([true, true]);
  await expect.poll(() => page.evaluate(() => (window as any).geometryLatency.samples.length)).toBe(2);
  await expect(slider).toBeFocused();
  const track = (await slider.boundingBox())!;
  const trackY = track.y + track.height / 2;
  await page.mouse.move(track.x + track.width * .8, trackY);
  await page.mouse.down();
  await page.mouse.move(track.x + track.width * .08, trackY, { steps: 8 });
  await page.mouse.up();
  await expect.poll(() => page.evaluate(() => (window as any).geometryLatency.samples.length)).toBeGreaterThan(4);
  await expect.poll(samples).toEqual([false, false]);
  await slider.press('End');
  await toggle.click(); await toggle.click();
  await expect(axis).toHaveValue('x'); await expect(slider).toHaveValue('1000');
  await axis.selectOption('y');
  await expect(axis).toHaveValue('y');
  await page.getByRole('button', { name: 'Reset section', exact: true }).click();
  await expect(axis).toHaveValue('z'); await expect(slider).toHaveValue('500');
  await expect(page.getByRole('button', { name: 'Reverse side', exact: true })).toHaveAttribute('aria-pressed', 'false');
  await toggle.click(); await refined(page);
  await expect.poll(samples).toEqual([true, true]);
  expect(await reads(page)).toBe(startingReads);
  expect(await resources(page)).toEqual(startingResources);
  expect(await canvas.evaluate((element, before) => element === before, handles)).toBe(true);
  expect(shaderErrors).toEqual([]);
  const timing = await page.evaluate(() => {
    const { first, samples, gpuSamples, stages } = (window as any).geometryLatency;
    return { activationMs: first, samples, gpuSamples, stages };
  });
  const timingPath = info.outputPath('section-timings.json');
  writeFileSync(timingPath, JSON.stringify(timing, null, 2));
  await info.attach('section-timings', { path: timingPath, contentType: 'application/json' });
});

test('section settings follow the retained copy and reset after refresh', async ({ page }) => {
  await page.setViewportSize({ width: 600, height: 900 });
  await load(page);
  const section = page.getByRole('button', { name: 'Section', exact: true });
  await section.click();
  await page.getByRole('combobox', { name: 'Axis' }).selectOption('y');
  await page.getByRole('slider', { name: 'Position' }).press('End');
  await page.getByRole('button', { name: 'Reverse side', exact: true }).click();
  const original = (await pose(page)).section, originalReads = await reads(page);
  expect(original).toMatchObject({ enabled: true, axis: 'y', offset: 230, reversed: true });
  await page.getByRole('button', { name: 'Hide delivery', exact: true }).click();
  await expect.poll(async () => (await resources(page)).contexts).toBe(0);
  await page.getByRole('button', { name: 'Dark theme', exact: true }).click();
  await page.getByRole('button', { name: 'Chinese', exact: true }).click();
  await page.getByRole('button', { name: 'Show delivery', exact: true }).click();
  await ready(page);
  await expect(page.getByRole('button', { name: '剖切', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByRole('combobox', { name: '坐标轴' })).toHaveValue('y');
  await expect(page.getByRole('slider', { name: '位置' })).toHaveAttribute('aria-valuetext', '230 米');
  await expect(page.getByRole('button', { name: '反转保留侧', exact: true })).toHaveAttribute('aria-pressed', 'true');
  expect(await reads(page)).toBe(originalReads);
  const refresh = page.getByRole('button', { name: '刷新', exact: true });
  await refresh.click();
  await ready(page);
  await expect(page.getByRole('button', { name: '剖切', exact: true })).toHaveAttribute('aria-pressed', 'false');
  expect((await pose(page)).section).toMatchObject({ enabled: false, axis: 'z', reversed: false });
});

test('single Pan toggle preserves real drag semantics, pressed styles and geometry ownership', async ({ page }) => {
  await page.addInitScript(() => {
    const seen = new WeakSet<object>(), activity = { contextsCreated: 0, displayPreparations: 0 };
    const getContext = HTMLCanvasElement.prototype.getContext, measure = performance.measure;
    Object.defineProperty(HTMLCanvasElement.prototype, 'getContext', { value: function (this: HTMLCanvasElement, ...args: unknown[]) {
      const context = Reflect.apply(getContext, this, args);
      if (args[0] === 'webgl2' && context && !seen.has(context)) { seen.add(context); activity.contextsCreated++; }
      return context;
    } });
    Object.defineProperty(performance, 'measure', { value: function (...args: unknown[]) {
      if (args[0] === 'geometry:display-prepare:glb') activity.displayPreparations++;
      return Reflect.apply(measure, performance, args);
    } });
    Object.assign(window, { geometryPanActivity: activity });
  });
  await load(page); await refined(page);
  const pan = page.getByRole('button', { name: 'Pan', exact: true }), outlines = page.getByRole('button', { name: 'Model outlines', exact: true });
  await expect(pan).toHaveCount(1); await expect(pan).toHaveAttribute('aria-pressed', 'false');
  await expect(page.getByRole('button', { name: 'Orbit', exact: true })).toHaveCount(0);
  const canvas = page.locator('[data-geometry-format] canvas'), originalCanvas = await canvas.elementHandle();
  const initialReads = await reads(page), activity = () => page.evaluate(() => ({ ...(window as any).geometryPanActivity }));
  const initialActivity = await activity(); expect(initialActivity).toEqual({ contextsCreated: 1, displayPreparations: 1 });
  const inactiveStyle = await pan.evaluate(button => { const style = getComputedStyle(button); return { backgroundColor: style.backgroundColor, color: style.color, boxShadow: style.boxShadow }; });
  await pan.click(); await expect(pan).toHaveAttribute('aria-pressed', 'true');
  const tree = await parts(page); await tree.getByRole('button', { name: 'A', exact: true }).click();
  const isolate = page.getByRole('button', { name: 'Isolate', exact: true });
  await isolate.click(); await expect(isolate).toHaveAttribute('aria-pressed', 'true');
  await expect(outlines).toHaveAttribute('aria-pressed', 'true');
  await page.mouse.move(0, 0); await canvas.focus(); await painted(page);
  const pressedStyle = await pan.evaluate(button => { const style = getComputedStyle(button); return { backgroundColor: style.backgroundColor, color: style.color, boxShadow: style.boxShadow }; });
  expect(pressedStyle.backgroundColor).not.toBe(inactiveStyle.backgroundColor);
  for (const button of [outlines, isolate]) expect(await button.evaluate(button => { const style = getComputedStyle(button); return { backgroundColor: style.backgroundColor, color: style.color, boxShadow: style.boxShadow }; })).toEqual(pressedStyle);
  await expect(page.getByRole('button', { name: /Exit isolation|Restore isolation/ })).toHaveCount(0);
  await isolate.click(); await expect(isolate).toHaveAttribute('aria-pressed', 'false');
  await expect(tree.getByRole('checkbox', { name: 'Visible: B', exact: true })).toBeChecked();
  await page.getByRole('button', { name: 'Close parts', exact: true }).click();
  const axis = page.locator('[data-axis="Y"]'); await axis.locator('circle').last().click(); await painted(page);
  expect(await axis.evaluate(element => getComputedStyle(element).outlineStyle)).toBe('none');
  await expect(pan).toHaveAttribute('aria-pressed', 'true'); expect((await pose(page)).navigationMode).toBe('pan');
  const beforePan = (await pose(page)).camera, position = new Vector3(...beforePan.position), target = new Vector3(...beforePan.target), direction = position.clone().sub(target);
  const drag = async () => {
    const box = (await canvas.boundingBox())!, x = box.x + box.width / 2, y = box.y + box.height / 2;
    await page.mouse.move(x, y); await page.mouse.down(); await page.mouse.move(x + 72, y - 32, { steps: 8 }); await page.mouse.up(); await painted(page);
  };
  await drag(); const afterPan = (await pose(page)).camera;
  const positionShift = new Vector3(...afterPan.position).sub(position), targetShift = new Vector3(...afterPan.target).sub(target);
  expect(positionShift.length()).toBeGreaterThan(.01); expect(targetShift.length()).toBeGreaterThan(.01);
  expect(positionShift.distanceTo(targetShift)).toBeLessThan(1e-7);
  expect(new Vector3(...afterPan.position).sub(new Vector3(...afterPan.target)).distanceTo(direction)).toBeLessThan(1e-7);
  expect(afterPan.verticalSpan).toBeCloseTo(beforePan.verticalSpan, 8);
  await pan.click(); await expect(pan).toHaveAttribute('aria-pressed', 'false');
  await drag(); const afterOrbit = await pose(page);
  expect(new Vector3(...afterOrbit.camera.target).distanceTo(new Vector3(...afterPan.target))).toBeLessThan(1e-8);
  expect(new Vector3(...afterOrbit.camera.position).sub(new Vector3(...afterOrbit.camera.target)).distanceTo(direction)).toBeGreaterThan(.01);
  expect(afterOrbit.navigationMode).toBeUndefined();
  await refined(page); expect(await activity()).toEqual(initialActivity); expect(await reads(page)).toBe(initialReads);
  expect(await canvas.evaluate((element, original) => element === original, originalCanvas)).toBe(true);
  expect((await resources(page)).contexts).toBe(1);
});

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
  const isolate = page.getByRole('button', { name: 'Isolate', exact: true });
  await isolate.click(); await expect(isolate).toHaveAttribute('aria-pressed', 'true');
  await b.click(); await expect(page.getByRole('button', { name: 'Focus selected', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Isolate', exact: true }).click();
  await expect(tree.getByRole('checkbox', { name: 'Visible: B', exact: true })).toBeChecked();
  await page.getByRole('button', { name: 'Focus selected', exact: true }).click();
  await isolate.click(); await expect(isolate).toHaveAttribute('aria-pressed', 'false');
  await expect(tree.getByRole('checkbox', { name: 'Visible: B', exact: true })).not.toBeChecked();
  await page.getByRole('button', { name: 'Show all', exact: true }).click(); await expect(tree.getByRole('checkbox', { name: 'Visible: B', exact: true })).toBeChecked();
  await a.click(); await page.getByRole('button', { name: 'Isolate', exact: true }).click();
  await page.getByRole('button', { name: entry === 'delivery' ? 'Hide delivery' : 'Toggle workspace body', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  await page.getByRole('button', { name: entry === 'delivery' ? 'Show delivery' : 'Toggle workspace body', exact: true }).click(); await ready(page);
  await page.getByRole('button', { name: 'Parts', exact: true }).click(); await expect(isolate).toHaveAttribute('aria-pressed', 'true');
  await page.getByRole('button', { name: 'Show all', exact: true }).click(); await expect(isolate).toHaveAttribute('aria-pressed', 'false');
  expect(await reads(page)).toBe(initialReads);
});

test('three arrow controls toggle all six views, stay separate at poles and never pick the model', async ({ page }, info) => {
  await load(page); const tree = await parts(page); await tree.getByRole('button', { name: 'A', exact: true }).click();
  await page.getByRole('button', { name: 'Close parts', exact: true }).click();
  await expect(page.locator('[data-geometry-axis-indicator] [role="button"]')).toHaveCount(3);
  await expect(page.locator('[data-geometry-axis-indicator] text')).toHaveCount(0);
  const expected = new Map<string, any>();
  for (const [view, label] of [['right','Right'], ['left','Left'], ['top','Top'], ['bottom','Bottom'], ['front','Front'], ['back','Back']]) {
    await page.getByRole('button', { name: 'View angle', exact: true }).click();
    await expect(page.getByRole('menuitem', { name: label, exact: true }).locator('[data-view-icon]')).toHaveAttribute('data-view-icon', view);
    await page.getByRole('menuitem', { name: label, exact: true }).click(); expected.set(view, await pose(page));
  }
  for (const [axis, positive, negative] of [['X', 'right', 'left'], ['Y', 'top', 'bottom'], ['Z', 'front', 'back']]) {
    await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Isometric', exact: true }).click(); await painted(page);
    for (const view of [positive, negative]) {
      await page.locator(`[data-axis="${axis}"] circle`).last().click(); const axisPose = await pose(page), menuPose = expected.get(view); await painted(page);
      for (const key of ['position', 'target', 'up']) axisPose.camera[key].forEach((value: number, index: number) => expect(value).toBeCloseTo(menuPose.camera[key][index], 8));
      expect(axisPose.camera.projection).toBe('orthographic'); expect(axisPose.selected).toBe(menuPose.selected);
      const targets = await page.locator('[data-geometry-axis] circle:last-child').evaluateAll(circles => circles.map(circle => { const b = circle.getBoundingClientRect(); return { x: b.x + b.width / 2, y: b.y + b.height / 2, width: b.width }; }));
      for (const target of targets) expect(target.width).toBeGreaterThanOrEqual(24);
      for (let i = 0; i < targets.length; i++) for (let j = i + 1; j < targets.length; j++) expect(Math.hypot(targets[i].x - targets[j].x, targets[i].y - targets[j].y)).toBeGreaterThanOrEqual(24);
    }
  }
  const beforeOrbit = await pose(page), box = (await page.locator('[data-geometry-format] canvas').boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2); await page.mouse.down(); await page.mouse.move(box.x + box.width / 2 + 60, box.y + box.height / 2, { steps: 8 }); await page.mouse.up();
  expect((await pose(page)).camera.position).not.toEqual(beforeOrbit.camera.position);
  await page.locator('[data-axis="Z"] circle').last().click(); expect((await pose(page)).camera.position[2]).toBeGreaterThan(0);
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Isometric', exact: true }).click(); await painted(page);
  await page.locator('[data-axis="X"] circle').last().click();
  const linePose = await pose(page); expect(linePose.camera.position[0]).toBeGreaterThan(0); expect(Math.abs(linePose.camera.position[2])).toBeLessThan(1e-6); expect(linePose.selected).toBeDefined();
  await page.setViewportSize({ width: 700, height: 500 }); await painted(page);
  await page.locator('[data-axis="X"]').focus(); await page.keyboard.press('Enter'); expect((await pose(page)).camera.position[0]).toBeLessThan(0); expect((await pose(page)).selected).toBeDefined();
  await page.getByRole('button', { name: 'Chinese', exact: true }).click(); await page.getByRole('button', { name: 'Dark theme', exact: true }).click();
  await expect(page.getByRole('button', { name: '右视图', exact: true })).toBeVisible();
  await page.locator('[data-geometry-format]').screenshot({ path: info.outputPath('geometry-axis-narrow-zh-dark.png') });
});

test('projection switches preserve framing and resources, persist wheel zoom, and reset to the initial projection', async ({ page }, info) => {
  await load(page); expect((await pose(page)).camera.projection).toBe('orthographic');
  await page.getByRole('button', { name: 'View angle', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Orthographic', exact: true })).toHaveAttribute('aria-current', 'true');
  await page.getByRole('menuitem', { name: 'Perspective', exact: true }).click();
  const tree = await parts(page);
  await tree.getByRole('checkbox', { name: 'Visible: B', exact: true }).uncheck(); await tree.getByRole('button', { name: 'A', exact: true }).click();
  await page.getByRole('button', { name: 'Focus selected', exact: true }).click(); await page.getByRole('button', { name: 'Close parts', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Model outlines', exact: true })).toHaveAttribute('aria-pressed', 'true'); await refined(page);
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
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Perspective', exact: true }).click();
  expect((await pose(page)).camera.projection).toBe('perspective');
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Reset view', exact: true }).click();
  const reset = await pose(page); expect(reset.camera.projection).toBe('orthographic'); expect(reset.hidden).toEqual(initial.hidden); expect(reset.camera.position[0]).toBeCloseTo(reset.camera.position[1], 8); expect(reset.camera.position[1]).toBeCloseTo(reset.camera.position[2], 8);
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await expect(page.getByRole('menuitem', { name: 'Orthographic', exact: true })).toHaveAttribute('aria-current', 'true');
  await page.locator('[data-geometry-format]').screenshot({ path: info.outputPath('projection-orthographic-menu.png') });
  await page.getByRole('menuitem', { name: 'Perspective', exact: true }).click();
  expect((await pose(page)).camera.projection).toBe('perspective'); expect((await pose(page)).camera).not.toHaveProperty('verticalSpan'); expect((await resources(page)).contexts).toBe(1);
  const perspective = await pose(page);
  await page.getByRole('button', { name: 'Hide delivery', exact: true }).click();
  await page.getByRole('button', { name: 'Show delivery', exact: true }).click(); await ready(page);
  const perspectiveRestored = (await pose(page)).camera; expect(perspectiveRestored.projection).toBe('perspective');
  for (const key of ['position', 'target', 'up']) perspectiveRestored[key].forEach((value: number, index: number) => expect(value).toBeCloseTo(perspective.camera[key][index], 8));
  await page.getByRole('button', { name: 'View angle', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Perspective', exact: true })).toHaveAttribute('aria-current', 'true');
});

test('Blender middle-button gestures preserve Pan mode and never pick in either projection', async ({ page }) => {
  await load(page); const tree = await parts(page);
  const part = tree.getByRole('button', { name: 'A', exact: true });
  await part.click();
  const selected = (await pose(page)).selected;
  await page.getByRole('button', { name: 'Close parts', exact: true }).click();

  const canvas = page.locator('[data-geometry-format] canvas');
  const pan = page.getByRole('button', { name: 'Pan', exact: true });
  await pan.click(); await expect(pan).toHaveAttribute('aria-pressed', 'true');
  await refined(page); await expect.poll(async () => (await resources(page)).workers).toBe(0);
  const readCount = await reads(page), activity = await page.evaluate(() => ({ ...(window as any).geometryActivity }));
  const offset = (camera: { position: number[]; target: number[] }) => new Vector3().fromArray(camera.position).sub(new Vector3().fromArray(camera.target));
  const span = (camera: { projection: string; verticalSpan?: number; position: number[]; target: number[] }) =>
    camera.projection === 'orthographic' ? camera.verticalSpan! : 2 * offset(camera).length() * Math.tan(Math.PI / 8);
  const drag = async (button: 'middle' | 'left' | 'right', modifier?: 'Control' | 'Shift') => {
    const box = (await canvas.boundingBox())!;
    const x = box.x + box.width * .42, y = box.y + box.height * .44;
    await page.mouse.move(x, y);
    if (modifier) await page.keyboard.down(modifier);
    await page.mouse.down({ button }); await page.mouse.move(x + 40, y - 34, { steps: 6 }); await page.mouse.up({ button });
    if (modifier) await page.keyboard.up(modifier);
  };
  for (const projection of ['orthographic', 'perspective']) {
    await canvas.focus();
    if ((await pose(page)).camera.projection !== projection) await canvas.press('Numpad5');
    const before = (await pose(page)).camera;
    await drag('middle');
    const orbited = (await pose(page)).camera;
    expect(orbited.projection).toBe(projection);
    expect(orbited.target).toEqual(before.target);
    expect(offset(orbited).distanceTo(offset(before))).toBeGreaterThan(1);
    await drag('middle', 'Shift');
    const panned = (await pose(page)).camera;
    expect(panned.target).not.toEqual(orbited.target);
    expect(offset(panned).distanceTo(offset(orbited))).toBeLessThan(1e-6);
    await drag('middle', 'Control');
    const zoomed = (await pose(page)).camera;
    expect(span(zoomed)).toBeLessThan(span(panned));
    await drag('left'); const left = (await pose(page)).camera;
    expect(left.target).not.toEqual(zoomed.target); expect(offset(left).distanceTo(offset(zoomed))).toBeLessThan(1e-6);
    await drag('right'); const right = (await pose(page)).camera;
    expect(right.target).not.toEqual(left.target); expect(offset(right).distanceTo(offset(left))).toBeLessThan(1e-6);
    expect((await pose(page)).selected).toBe(selected);
    await expect(pan).toHaveAttribute('aria-pressed', 'true');
  }
  await refined(page);
  expect(await reads(page)).toBe(readCount);
  expect(await page.evaluate(() => (window as any).geometryActivity)).toEqual(activity);
  const draws = await page.evaluate(() => (window as any).geometryDrawCalls.calls);
  await page.waitForTimeout(220);
  expect(await page.evaluate(() => (window as any).geometryDrawCalls.calls)).toBe(draws);
  await page.getByRole('button', { name: 'Close preview cube.glb', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
});

test('the first trusted middle drag from either pole orbits at the same scale', async ({ page }) => {
  await load(page);
  const canvas = page.locator('[data-geometry-format] canvas');
  const box = (await canvas.boundingBox())!;
  const x = box.x + box.width * .45, y = box.y + box.height * .45;
  for (const key of ['Numpad7', 'Control+Numpad7']) {
    await canvas.press(key);
    const before = (await pose(page)).camera;
    expect(before.up[1]).toBe(0);
    await page.mouse.move(x, y);
    await page.mouse.down({ button: 'middle' });
    await page.mouse.move(x + 54, y + (key === 'Numpad7' ? -35 : 35), { steps: 5 });
    await page.mouse.up({ button: 'middle' });
    const after = (await pose(page)).camera;
    expect(after.up).toEqual([0, 1, 0]);
    expect(after.target).toEqual(before.target);
    expect(after.verticalSpan).toBeCloseTo(before.verticalSpan, 8);
    expect(new Vector3().fromArray(after.position).distanceTo(new Vector3().fromArray(before.position))).toBeGreaterThan(1);
  }
});

test('trusted perspective wheel and Ctrl-middle dolly stop before the original front surface', async ({ page }) => {
  await load(page, 'delivery', assemblyGlb([{ name: 'Solid', mesh: 0 }]));
  const canvas = page.locator('[data-geometry-format] canvas');
  await canvas.press('Numpad1');
  await canvas.press('Numpad5');
  const box = (await canvas.boundingBox())!;
  const x = box.x + box.width / 2, y = box.y + box.height / 2;
  const eyeZ = async () => ((await pose(page)).camera.position as number[])[2];
  const front = 5;
  await page.mouse.move(x, y);
  await page.mouse.wheel(0, -12_000);
  await expect.poll(eyeZ).toBeLessThan(front + .02);
  expect(await eyeZ()).toBeGreaterThan(front);

  await page.keyboard.down('Control');
  await page.mouse.down({ button: 'middle' });
  await page.mouse.move(x, y - 100, { steps: 4 });
  await page.mouse.up({ button: 'middle' });
  await page.keyboard.up('Control');
  expect(await eyeZ()).toBeGreaterThan(front);

  await page.mouse.move(x, y);
  await page.mouse.wheel(0, 1200);
  await expect.poll(eyeZ).toBeGreaterThan(front + .1);
});

test('trusted inward wheel steps reuse the original surface hit on a dense mesh', async ({ page }, info) => {
  await instrument(page);
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  const { triangles } = await page.evaluate(() => (window as any).geometryContract.denseBox(240));
  await page.getByRole('button', { name: 'Preview cube.stl in sidebar', exact: true }).click();
  await ready(page);
  const canvas = page.locator('[data-geometry-format] canvas');
  await canvas.press('Numpad1');
  await canvas.press('Numpad5');
  await refined(page);
  await page.evaluate(() => (window as any).geometryContract.observeRaycasts());
  const box = (await canvas.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  for (let i = 0; i < 8; i++) {
    await page.mouse.wheel(0, -30);
    await painted(page);
  }
  const samples = await page.evaluate(() => (window as any).geometryContract.raycastTimes() as number[]);
  const timingPath = info.outputPath('zoom-raycasts.json');
  writeFileSync(timingPath, JSON.stringify({ triangles, steps: 8, samples }));
  await info.attach('zoom-raycasts', { path: timingPath, contentType: 'application/json' });
  expect(samples).toHaveLength(1);
});

test('tracked pointers clear on cancellation and capture loss, including a drag returning to its origin', async ({ page }) => {
  await load(page, 'delivery', assemblyGlb([
    { name: 'Assembly', children: [1, 2] },
    { name: 'Center', mesh: 0 }, { name: 'Offset', mesh: 0, translation: [20, 0, 0] },
  ]));
  const canvas = page.locator('[data-geometry-format] canvas');
  await canvas.focus(); await canvas.press('Numpad1');
  const tree = await parts(page); await tree.getByRole('button', { name: 'Center', exact: true }).click();
  await page.getByRole('button', { name: 'Focus selected', exact: true }).click();
  await page.getByRole('button', { name: 'Close parts', exact: true }).click();
  const selected = (await pose(page)).selected, box = (await canvas.boundingBox())!;
  const x = box.x + box.width / 2, y = box.y + box.height / 2;
  // Scope the empty-space click to the canvas, outside the focused cube and rail.
  await canvas.click({ position: { x: box.width * .11, y: box.height * .18 } });
  await expect.poll(async () => (await pose(page)).selected).toBeUndefined();
  await page.mouse.click(x, y); await expect.poll(async () => (await pose(page)).selected).toBe(selected);
  await page.getByRole('button', { name: 'Parts', exact: true }).click();
  await tree.getByRole('button', { name: 'Offset', exact: true }).click();
  await page.getByRole('button', { name: 'Close parts', exact: true }).click();
  const selectionToPreserve = (await pose(page)).selected;
  await page.mouse.move(x, y); await page.mouse.down();
  await page.mouse.move(x + 31, y + 19, { steps: 4 }); await page.mouse.move(x, y, { steps: 4 }); await page.mouse.up();
  expect((await pose(page)).selected).toBe(selectionToPreserve);

  await canvas.evaluate(element => element.addEventListener('pointerdown', event => {
    element.dataset.testPointer = String((event as PointerEvent).pointerId);
  }));
  for (const loss of ['cancel', 'capture'] as const) {
    await page.mouse.move(x, y); await page.mouse.down({ button: 'middle' });
    await page.mouse.move(x + 28, y + 17, { steps: 4 });
    await canvas.evaluate((element: HTMLCanvasElement, loss) => {
      const pointerId = Number(element.dataset.testPointer);
      if (loss === 'capture') element.releasePointerCapture(pointerId);
      else element.dispatchEvent(new PointerEvent('pointercancel', { pointerId, pointerType: 'mouse', bubbles: true }));
    }, loss);
    // Pointer capture changes are delivered with the next real pointer event.
    await page.mouse.move(x + 29, y + 18);
    const stopped = (await pose(page)).camera;
    await page.mouse.move(x + 66, y + 40, { steps: 4 }); await page.mouse.up({ button: 'middle' });
    expect((await pose(page)).camera).toEqual(stopped);
    expect((await pose(page)).selected).toBe(selectionToPreserve);
  }
  const before = (await pose(page)).camera;
  await page.mouse.move(x, y); await page.mouse.down({ button: 'middle' }); await page.mouse.move(x + 30, y + 22, { steps: 4 }); await page.mouse.up({ button: 'middle' });
  expect((await pose(page)).camera).not.toEqual(before);
});

test('viewport and gizmo dispatch every physical keypad shortcut through the existing controls', async ({ page }) => {
  await load(page); const tree = await parts(page);
  const canvas = page.locator('[data-geometry-format] canvas');
  const selected = tree.getByRole('button', { name: 'B', exact: true });
  await selected.click();
  const hiddenA = tree.getByRole('checkbox', { name: 'Visible: A', exact: true });
  await hiddenA.uncheck();
  const preIsolation = (await pose(page)).hidden;
  await canvas.focus(); await canvas.press('NumpadDecimal');
  const focused = (await pose(page)).camera;
  expect(focused.target[0]).toBeGreaterThan(0);

  const directions = [
    ['Numpad1', [0,0,1]], ['Control+Numpad1', [0,0,-1]],
    ['Numpad3', [1,0,0]], ['Control+Numpad3', [-1,0,0]],
    ['Numpad7', [0,1,0]], ['Control+Numpad7', [0,-1,0]],
  ] as const;
  for (const [key, direction] of directions) {
    await canvas.press('Numpad5'); expect((await pose(page)).camera.projection).toBe('perspective');
    await canvas.press(key);
    const state = (await pose(page)).camera;
    expect(state.projection).toBe('orthographic');
    expect(new Vector3().fromArray(state.position).sub(new Vector3().fromArray(state.target)).normalize().distanceTo(new Vector3(...direction))).toBeLessThan(1e-6);
  }
  await canvas.press('Numpad1'); await canvas.press('Numpad5');
  const front = (await pose(page)).camera, target = new Vector3().fromArray(front.target);
  for (const [key, opposite] of [['Numpad4', 'Numpad6'], ['Numpad8', 'Numpad2']]) {
    await canvas.press(key);
    const offset = new Vector3().fromArray(front.position).sub(target);
    await expect.poll(async () => new Vector3().fromArray((await pose(page)).camera.position).sub(target).angleTo(offset)).toBeCloseTo(Math.PI / 12, 6);
    const camera = (await pose(page)).camera;
    expect(camera.projection).toBe('perspective'); expect(camera.target).toEqual(front.target);
    await canvas.press(opposite);
    await expect.poll(async () => new Vector3().fromArray((await pose(page)).camera.position).distanceTo(new Vector3().fromArray(front.position))).toBeLessThan(1e-6);
  }

  const isolate = page.getByRole('button', { name: 'Isolate', exact: true });
  await canvas.press('NumpadDivide'); await expect(isolate).toHaveAttribute('aria-pressed', 'true');
  await canvas.press('NumpadDivide'); await expect(isolate).toHaveAttribute('aria-pressed', 'false');
  expect((await pose(page)).hidden).toEqual(preIsolation);
  await isolate.click(); await expect(isolate).toHaveAttribute('aria-pressed', 'true');
  await canvas.focus(); await canvas.press('NumpadDivide');
  expect((await pose(page)).hidden).toEqual(preIsolation);
  await hiddenA.check(); await canvas.focus(); await canvas.press('Home');
  expect((await pose(page)).camera.target).toEqual([0, 0, 0]);

  const search = page.getByRole('searchbox', { name: 'Search parts', exact: true });
  await search.focus(); const beforeTyping = await pose(page);
  await page.keyboard.press('Digit1'); await page.keyboard.press('Slash'); await page.keyboard.press('Numpad5');
  await expect(search).toBeFocused(); expect((await pose(page)).camera).toEqual(beforeTyping.camera);
  await page.getByRole('button', { name: 'Close parts', exact: true }).click();
  const axis = page.locator('[data-geometry-axis-indicator] [data-axis="Z"]');
  await axis.focus(); await page.keyboard.press('Numpad5');
  expect((await pose(page)).camera.projection).toBe('orthographic'); await expect(axis).toBeFocused();
});

for (const projection of ['orthographic', 'perspective'] as const) test(`${projection} pointer zoom anchors a tiny off-origin feature and empty background`, async ({ page }) => {
  const scale = 1e-7;
  await load(page, 'delivery', assemblyGlb([
    { name: 'Assembly', children: [1, 2, 3] },
    { name: 'A', mesh: 0, translation: [-.5, -1000.5, -.5], scale: [.1, .1, .1] },
    { name: 'B', mesh: 0, translation: [-.5, 999.5, -.5], scale: [.1, .1, .1] },
    { name: 'Small', mesh: 0, translation: [-5 * scale, 500 - 5 * scale, -5 * scale], scale: [scale, scale, scale] },
  ]));
  const canvas = page.locator('[data-geometry-format] canvas');
  await canvas.focus(); await canvas.press('Numpad1');
  if (projection === 'perspective') await canvas.press('Numpad5');
  const tree = await parts(page);
  await tree.getByRole('button', { name: 'Small', exact: true }).click();
  await page.getByRole('button', { name: 'Focus selected', exact: true }).click();
  await page.getByRole('button', { name: 'Close parts', exact: true }).click();
  const box = (await canvas.boundingBox())!, aspect = box.width / box.height;
  type CameraPose = { projection: string; verticalSpan?: number; position: number[]; target: number[]; up: number[] };
  const height = (camera: CameraPose) => camera.projection === 'orthographic' ? camera.verticalSpan!
    : 2 * new Vector3().fromArray(camera.position).distanceTo(new Vector3().fromArray(camera.target)) * Math.tan(Math.PI / 8);
  function fromPose(pose: CameraPose) {
    const half = height(pose) / 2;
    const camera = pose.projection === 'orthographic'
      ? new OrthographicCamera(-half * aspect, half * aspect, half, -half, 1e-12, 1e5)
      : new PerspectiveCamera(45, aspect, 1e-12, 1e5);
    camera.position.fromArray(pose.position); camera.up.fromArray(pose.up); camera.lookAt(new Vector3().fromArray(pose.target)); camera.updateMatrixWorld();
    return camera;
  }
  for (const location of ['surface', 'background'] as const) {
    const before = (await pose(page)).camera as CameraPose;
    const x = Math.round(box.x + box.width * (location === 'surface' ? .54 : .82)), y = Math.round(box.y + box.height * .47);
    const ndc = new Vector2((x - box.x) / box.width * 2 - 1, 1 - (y - box.y) / box.height * 2);
    const eye = new Vector3().fromArray(before.position);
    const target = new Vector3().fromArray(before.target).sub(eye);
    // Build the test ray relative to the eye to avoid subtracting large world
    // coordinates while reconstructing a ray near a submicron feature.
    const camera = fromPose({ ...before, position: [0, 0, 0], target: target.toArray() });
    const raycaster = new Raycaster(); raycaster.setFromCamera(ndc, camera); raycaster.ray.origin.add(eye);
    const plane = new Plane(new Vector3(0, 0, 1), -(before.target[2] + (location === 'surface' ? 5 * scale : 0)));
    const point = raycaster.ray.intersectPlane(plane, new Vector3())!;
    const beforeProjection = point.clone().project(fromPose(before));
    expect(Math.hypot((beforeProjection.x - ndc.x) * box.width / 2, (beforeProjection.y - ndc.y) * box.height / 2)).toBeLessThan(.1);
    if (location === 'surface') {
      expect(Math.abs(point.x - before.target[0])).toBeLessThan(5 * scale);
      expect(Math.abs(point.y - before.target[1])).toBeLessThan(5 * scale);
    }
    await page.mouse.move(x, y); await page.mouse.wheel(0, -140);
    await expect.poll(async () => height((await pose(page)).camera)).toBeLessThan(height(before));
    const afterPose = (await pose(page)).camera;
    const after = fromPose(afterPose);
    const projected = point.clone().project(after);
    const error = Math.hypot((projected.x - ndc.x) * box.width / 2, (projected.y - ndc.y) * box.height / 2);
    expect(error, JSON.stringify({location, before, afterPose, point: point.toArray(), ndc: ndc.toArray(), projected: projected.toArray()})).toBeLessThan(.75);
  }
});

test('pointer zoom preserves an original surface point during simplified motion rendering', async ({ page }, info) => {
  await instrument(page);
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  // Keep the original above the streamed motion-preview threshold.
  const { triangles } = await page.evaluate(() => (window as any).geometryContract.denseBox(240));
  await page.getByRole('button', { name: 'Preview cube.stl in sidebar', exact: true }).click();
  await ready(page);
  await page.getByRole('button', { name: 'View angle', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Front', exact: true }).click();
  await expect.poll(async () => (await resources(page)).workers).toBe(0);
  await refined(page);
  const canvas = page.locator('[data-geometry-format] canvas'), box = (await canvas.boundingBox())!;
  const camera = () => page.evaluate(() => (window as any).geometryContract.snapshot().find((tab: any) => tab.name === 'cube.stl').state.camera);
  const before = await camera();
  // The dense unit box is centered, facing +Z. Pick CSS offsets whose
  // unprojection falls on the interior of its original front face.
  const x = Math.round(box.x + box.width * .56), y = Math.round(box.y + box.height * .45);
  const dx = x - box.x - box.width / 2, dy = box.y + box.height / 2 - y;
  const anchorX = before.target[0] + dx * before.verticalSpan / box.height;
  const anchorY = before.target[1] + dy * before.verticalSpan / box.height;
  expect(Math.abs(anchorX)).toBeLessThan(.5); expect(Math.abs(anchorY)).toBeLessThan(.5);
  const frameCount = await page.evaluate(() => (window as any).geometryModelFrames.length);
  await page.mouse.move(x, y);
  for (let tick = 0; tick < 7; tick++) {
    await page.mouse.wheel(0, -20);
    await painted(page);
  }
  await expect.poll(async () => (await camera()).verticalSpan).toBeLessThan(before.verticalSpan);
  const after = await camera();
  expect((anchorX - after.target[0]) * box.height / after.verticalSpan).toBeCloseTo(dx, 5);
  expect((anchorY - after.target[1]) * box.height / after.verticalSpan).toBeCloseTo(dy, 5);
  try {
    await expect.poll(() => page.evaluate(({ frameCount, triangles }) => (window as any).geometryModelFrames.slice(frameCount)
      .some((frame: { quality: string; triangles: number }) => frame.quality === 'interactive' && frame.triangles < triangles), { frameCount, triangles })).toBe(true);
  } catch (error) {
    const diagnostics = await page.evaluate(frameCount => {
      const observed = window as any;
      return { previews: observed.geometryPreviewTimings, frames: observed.geometryModelFrames.slice(frameCount) };
    }, frameCount);
    await info.attach('motion-display', { body: JSON.stringify(diagnostics, null, 2), contentType: 'application/json' });
    throw error;
  }
  await refined(page);
  await expect.poll(() => page.evaluate(() => (window as any).geometryDrawCalls.lastModelTriangles)).toBe(triangles);
});

test('projection roundtrip preserves a tiny focused part far from the orbit origin', async ({ page }) => {
  await load(page, 'delivery', assemblyGlb([
    { name: 'Assembly', children: [1, 2, 3] },
    { name: 'A', mesh: 0, translation: [-.5, -1000.5, -.5], scale: [.1, .1, .1] },
    { name: 'B', mesh: 0, translation: [-.5, 999.5, -.5], scale: [.1, .1, .1] },
    { name: 'Small', mesh: 0, translation: [-5e-7, 500 - 5e-7, -5e-7], scale: [1e-7, 1e-7, 1e-7] },
  ]));
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Front', exact: true }).click();
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Perspective', exact: true }).click();
  const tree = await parts(page); await tree.getByRole('button', { name: 'Small', exact: true }).click();
  await page.getByRole('button', { name: 'Focus selected', exact: true }).click(); await painted(page);
  await expect.poll(async () => (await resources(page)).workers).toBe(0); await painted(page);
  const initial = await pose(page), before = await resources(page);
  const distance = Math.hypot(...initial.camera.position.map((value: number, i: number) => value - initial.camera.target[i]));
  expect(distance).toBeGreaterThan(1e-6); expect(distance).toBeLessThan(1e-5);
  for (const projection of ['Orthographic', 'Perspective']) {
    await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: projection, exact: true }).click(); await painted(page);
    const after = await pose(page);
    expect(after.camera.projection).toBe(projection.toLowerCase()); expect(after.camera.target).toEqual(initial.camera.target); expect(after.camera.up).toEqual(initial.camera.up);
    expect(Math.hypot(...after.camera.position.map((value: number, i: number) => value - initial.camera.position[i]))).toBeLessThan(distance * 1e-6);
    if (projection === 'Orthographic') expect(after.camera.verticalSpan / (2 * distance * Math.tan(Math.PI / 8))).toBeCloseTo(1, 6);
    await refined(page); expect(await resources(page)).toEqual(before);
  }
  await refined(page); const calls = await page.evaluate(() => (window as any).geometryDrawCalls.calls); await page.waitForTimeout(250);
  expect(await page.evaluate(() => (window as any).geometryDrawCalls.calls)).toBe(calls);
});

test('mesh outlines start enabled, can be disabled, restore the option and add no idle loop', async ({ page }, info) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await load(page); const initialReads = await reads(page);
  const button = page.getByRole('button', { name: 'Model outlines', exact: true });
  await expect(button).toHaveAttribute('aria-pressed', 'true');
  await button.click(); await expect(button).toHaveAttribute('aria-pressed', 'false'); await refined(page);
  const before = await resources(page);
  for (let i = 0; i < 2; i++) {
    await button.click(); await expect(button).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(async () => (await resources(page)).workers).toBe(0); await refined(page);
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
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Front', exact: true }).click();
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: projection, exact: true }).click();
  const tree = await parts(page), canvas = page.locator('[data-geometry-format] canvas');
  const settle = async () => { await canvas.focus(); await page.mouse.move(0, 0); await expect(page.getByRole('tooltip')).toHaveCount(0); await refined(page); };
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
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Front', exact: true }).click();
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: projection, exact: true }).click();
  await expect(page.getByRole('button', { name: 'Model outlines', exact: true })).toHaveAttribute('aria-pressed', 'true');
  const canvas = page.locator('[data-geometry-format] canvas'), tree = page.getByRole('list', { name: 'Parts', exact: true });
  const settle = async () => { await canvas.focus(); await page.mouse.move(0, 0); await expect(page.getByRole('tooltip')).toHaveCount(0); await refined(page); };
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
  const source = denseBoxStl();
  await instrument(page); await page.route('**/test/fixtures/geometry/cube.stl', route => route.fulfill({ body: Buffer.from(source), contentType: 'application/octet-stream' }));
  await page.goto(`${origin}/test/fixtures/geometry.html`); await page.getByRole('button', { name: 'Preview cube.stl in sidebar', exact: true }).click(); await ready(page);
  await expect(page.getByRole('status')).toContainText('Outlines are unavailable');
  await expect(page.locator('[data-geometry-format] canvas')).toBeVisible(); await expect(page.getByRole('button', { name: 'Download artifact', exact: true })).toBeEnabled();
  expect(await reads(page)).toBe(Math.ceil(source.length / FILE_PREVIEW_LIMITS.pageBytes)); expect((await resources(page)).workers).toBe(0);
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
  await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(2);
  await expect(page.locator('[data-geometry-format]:visible canvas')).toHaveCount(2);
  await expect.poll(async () => (await resources(page)).workers).toBe(0); await painted(page);
  for (const panel of await page.locator('[data-geometry-format]:visible').all()) {
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
  for (const panel of await page.locator('[data-geometry-format]:visible').all()) {
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
  // The small CAD source and GLB each need one complete content read.
  const expectedReads = workload === 'planar-outline-envelope' ? Math.ceil((84 + 96_800 * 50) / FILE_PREVIEW_LIMITS.pageBytes) + 1 : 2;
  expect(browser.antialias).toBe(true); expect(await reads(page)).toBe(expectedReads);
  await refined(page);
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
    await page.locator('[data-axis="X"] circle').last().click();
    const state = await pose(page); expect(state.camera.position[0]).toBeGreaterThan(0);
    expect(Math.abs(state.camera.position[1])).toBeLessThan(1e-6); expect(Math.abs(state.camera.position[2])).toBeLessThan(1e-6);
    await refined(page); const scale = await page.locator('[data-geometry-format] canvas').evaluate(canvas => (canvas as HTMLCanvasElement).width / canvas.clientWidth); expect(scale).toBeCloseTo(2, 2);
    await page.locator('[data-axis="Z"]').focus(); await page.keyboard.press('Enter'); expect((await pose(page)).camera.position[2]).toBeGreaterThan(0);
    await page.keyboard.press('Space'); expect((await pose(page)).camera.position[2]).toBeLessThan(0);
    expect(await reads(page)).toBe(1);
    await page.getByRole('button', { name: 'Close preview cube.glb', exact: true }).click();
    await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  });
});

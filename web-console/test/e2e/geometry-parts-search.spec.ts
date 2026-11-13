import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { instrument, resources } from './geometry-observation';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: '<loopback>' } });
function namedAssembly() {
  const source = new Uint8Array(readFileSync(new URL('../fixtures/geometry/cube.glb', import.meta.url))), length = new DataView(source.buffer).getUint32(12, true);
  const json = JSON.parse(new TextDecoder().decode(source.subarray(20, 20 + length)));
  // These sibling paths use child ordinals1 and10, exercising whole segments.
  const nodes: object[] = [{ name: 'Assembly', children: [...Array.from({ length: 12 }, (_, i) => i + 1), ...Array.from({ length: 48 }, (_, i) => i + 15)] }];
  for (let i = 1; i <= 12; i++) nodes.push({ name: `Filler ${i}`, mesh: 0, translation: [i * 15, 0, 0] });
  nodes[2] = { name: 'Branch one', children: [13] }; nodes[11] = { name: 'Branch ten', children: [14] };
  nodes.push({ name: 'Needle one', mesh: 0 }, { name: 'Needle ten 涡轮', mesh: 0, translation: [20, 0, 0] });
  for (let i = 15; i < 63; i++) nodes.push({ name: `Filler ${i}`, mesh: 0, translation: [(i % 12) * 15, 0, Math.floor(i / 12) * 15] });
  json.nodes = nodes; json.scenes = [{ nodes: [0] }]; json.scene = 0;
  const text = new TextEncoder().encode(JSON.stringify(json)), padded = Math.ceil(text.length / 4) * 4, bin = source.subarray(20 + length);
  const result = new Uint8Array(20 + padded + bin.length), header = new DataView(result.buffer);
  result.set(source.subarray(0, 12)); header.setUint32(8, result.length, true); header.setUint32(12, padded, true); header.setUint32(16, 0x4e4f534a, true);
  result.fill(32, 20, 20 + padded); result.set(text, 20); result.set(bin, 20 + padded); return result;
}

test('Parts search reveals only matching admitted paths, preserves view state and expansion, and stays independently scrollable in Chinese', async ({ page }, info) => {
  await instrument(page); await page.setViewportSize({ width: 700, height: 540 });
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.evaluate(bytes => (window as any).geometryContract.replace('cube.glb', bytes), [...namedAssembly()]);
  await page.getByRole('button', { name: 'Preview cube.glb in sidebar', exact: true }).click();
  const canvas = page.locator('[data-geometry-format] canvas'); await expect(canvas).toBeVisible();
  await expect(page.getByRole('button', { name: 'Model outlines', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await page.getByRole('button', { name: 'View angle', exact: true }).click(); await page.getByRole('menuitem', { name: 'Orthographic', exact: true }).click();
  await page.getByRole('button', { name: 'Parts', exact: true }).click();
  const tree = page.getByRole('list', { name: 'Parts', exact: true }), search = page.getByRole('searchbox', { name: 'Search parts', exact: true });
  await page.getByRole('button', { name: 'Expand: Assembly', exact: true }).click();
  await tree.getByRole('button', { name: 'Expand: Branch one', exact: true }).click();
  await tree.getByRole('checkbox', { name: 'Visible: Needle one', exact: true }).uncheck(); await tree.getByRole('button', { name: 'Needle one', exact: true }).click();
  await canvas.evaluate(node => { node.dataset.searchOwner = 'retained'; });
  const observation = () => page.evaluate(() => ({ state: (window as any).geometryContract.snapshot().find((tab: any) => tab.name === 'cube.glb').state,
    reads: (window as any).geometryContract.reads.filter((read: any) => !read.probe).length, imports: (window as any).geometryImportTimings.length }));
  const initial = await observation(), owned = await resources(page);
  await search.fill('NEEDLE TEN');
  await expect(tree.getByRole('button', { name: 'Assembly', exact: true })).toBeVisible();
  await expect(tree.getByRole('button', { name: 'Branch ten', exact: true })).toBeVisible();
  await expect(tree.getByRole('button', { name: 'Needle ten 涡轮', exact: true })).toBeVisible();
  await expect(tree.getByRole('button', { name: 'Branch one', exact: true })).toHaveCount(0);
  await expect(tree.getByRole('button', { name: 'Collapse: Assembly', exact: true })).toBeDisabled();
  await page.locator('[data-geometry-format]').screenshot({ path: info.outputPath('parts-search-en-light.png') });
  await search.fill('涡轮'); await expect(tree.getByRole('button', { name: 'Needle ten 涡轮', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Clear parts search', exact: true }).click(); await expect(search).toBeFocused();
  await expect(tree.getByRole('checkbox', { name: 'Visible: Needle one', exact: true })).not.toBeChecked();
  await expect(tree.getByRole('button', { name: 'Needle one', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect(tree.getByRole('button', { name: 'Expand: Branch ten', exact: true })).toBeVisible(); await expect(tree.getByRole('button', { name: 'Needle ten 涡轮', exact: true })).toHaveCount(0);
  const pageScroll = await page.evaluate(() => scrollY), scroll = tree.locator('..');
  expect(await scroll.evaluate(node => node.scrollHeight > node.clientHeight && getComputedStyle(node).overflowY === 'auto')).toBe(true);
  await scroll.evaluate(node => { node.scrollTop = node.scrollHeight; }); expect(await scroll.evaluate(node => node.scrollTop)).toBeGreaterThan(0);
  await expect(search).toBeInViewport(); expect(await page.evaluate(() => scrollY)).toBe(pageScroll);
  await page.getByRole('button', { name: 'Chinese', exact: true }).click(); await page.getByRole('button', { name: 'Dark theme', exact: true }).click();
  const chineseSearch = page.getByRole('searchbox', { name: '搜索部件', exact: true });
  await chineseSearch.fill('不存在的部件'); await expect(page.getByRole('status')).toHaveText('没有匹配的部件');
  await chineseSearch.fill('涡轮'); await expect(page.getByRole('list', { name: '部件', exact: true }).getByRole('button', { name: 'Needle ten 涡轮', exact: true })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 700 });
  await expect(chineseSearch).toBeInViewport(); await expect(page.getByRole('button', { name: '清除部件搜索', exact: true })).toBeInViewport();
  await page.locator('[data-geometry-format]').screenshot({ path: info.outputPath('parts-search-zh-dark.png') });
  expect(await observation()).toEqual(initial); expect(await resources(page)).toEqual(owned); await expect(canvas).toHaveAttribute('data-search-owner', 'retained');
  await chineseSearch.press('Escape'); const toggle = page.getByRole('button', { name: '部件', exact: true }); await expect(toggle).toBeFocused(); await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await toggle.click(); await expect(chineseSearch).toHaveValue(''); await expect(page.getByRole('button', { name: '展开: Branch ten', exact: true })).toBeVisible();
  expect(await observation()).toEqual(initial); expect(errors).toEqual([]);
  await page.getByRole('button', { name: 'Hide delivery', exact: true }).click();
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
});

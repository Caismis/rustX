import { test, expect, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { instrument, resources } from './geometry-observation';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: '<loopback>' } });
type Variant = 'empty' | 'degenerate' | 'unequal-depth' | 'numeric-sibling';
async function source(page: Page, variant: Variant) {
  await page.goto(`${origin}/test/fixtures/geometry.html`);
  await page.evaluate(async variant => {
    const original = new Uint8Array(await (await fetch('/test/fixtures/geometry/cube.glb')).arrayBuffer());
    const view = new DataView(original.buffer), length = view.getUint32(12, true);
    const json = JSON.parse(new TextDecoder().decode(original.subarray(20, 20 + length))); json.scene = 0;
    if (variant === 'empty') json.scenes = [{ nodes: [] }, json.scenes[0]];
    else if (variant === 'degenerate') { json.nodes = [{ mesh: 0, scale: [0,0,0] }, { mesh: 0 }]; json.scenes = [{ nodes: [0] }, { nodes: [1] }]; }
    else if (variant === 'unequal-depth') {
      json.nodes = [{ children: [1] }, { name: 'A', mesh: 0 }, { children: [3] }, { children: [4] }, { name: 'B', mesh: 0, translation: [20,0,0] }];
      json.scenes = [{ nodes: [0,2] }];
    } else {
      json.nodes = Array.from({ length: 12 }, () => ({})); json.nodes[1] = { name: 'A', mesh: 0 };
      json.nodes[10] = { children: [11] }; json.nodes[11] = { name: 'B', mesh: 0, translation: [20,0,0] };
      json.scenes = [{ nodes: Array.from({ length: 11 }, (_, index) => index) }];
    }
    const text = new TextEncoder().encode(JSON.stringify(json)), padded = Math.ceil(text.length / 4) * 4;
    const bin = original.subarray(20 + length), bytes = new Uint8Array(20 + padded + bin.length), header = new DataView(bytes.buffer);
    bytes.set(original.subarray(0,12)); header.setUint32(8, bytes.length, true); header.setUint32(12, padded, true); header.setUint32(16, 0x4e4f534a, true);
    bytes.fill(32,20,20+padded); bytes.set(text,20); bytes.set(bin,20+padded);
    (window as any).geometryContract.replace('cube.glb', [...bytes]);
  }, variant);
  await page.getByRole('button', { name: 'Preview cube.glb in sidebar', exact: true }).click();
}

for (const variant of ['empty', 'degenerate'] as const) test(`${variant} selected GLB scene fails before renderer admission and retains original Download`, async ({ page }, info) => {
  await instrument(page); await source(page, variant);
  await expect(page.getByRole('alert')).toContainText('This file has no model to display.');
  await expect(page.locator('[data-geometry-format] canvas')).toHaveCount(0);
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
  const control = page.getByRole('button', { name: 'Download artifact', exact: true }); await expect(control).toBeEnabled();
  const waiting = page.waitForEvent('download'); await control.click(); const download = await waiting;
  const path = info.outputPath(`${variant}-original.glb`); await download.saveAs(path);
  expect([...readFileSync(path)]).toEqual(await page.evaluate(() => (window as any).geometryContract.source('cube.glb')));
  await expect.poll(() => resources(page)).toEqual({ workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 });
});

for (const variant of ['unequal-depth', 'numeric-sibling'] as const) test(`parts from ${variant} unnamed branches remain independent siblings`, async ({ page }) => {
  await source(page, variant); await expect(page.locator('[data-geometry-format]')).toContainText('30 × 10 × 10');
  await page.getByRole('button', { name: 'Parts', exact: true }).click();
  const tree = page.getByRole('list', { name: 'Parts', exact: true }), a = tree.getByRole('button', { name: 'A', exact: true }), b = tree.getByRole('button', { name: 'B', exact: true });
  await expect(a).toBeVisible(); await expect(b).toBeVisible();
  await expect(tree.getByRole('button', { name: 'Expand: A', exact: true })).toHaveCount(0);
  await a.click(); await expect(a).toHaveAttribute('aria-pressed', 'true');
  const visibleA = tree.getByRole('checkbox', { name: 'Visible: A', exact: true }), visibleB = tree.getByRole('checkbox', { name: 'Visible: B', exact: true });
  await visibleA.uncheck(); await expect(visibleB).toBeChecked(); await expect(b).toBeVisible();
  await b.click(); await expect(b).toHaveAttribute('aria-pressed', 'true'); await expect(visibleA).not.toBeChecked();
});

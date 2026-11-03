import { test, expect, type Page } from '@playwright/test';
import { copyFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { choose, connectRemote, openEmptySession, expectSettled } from './shell-actions';
import { wireProbe } from './wire-probe';

test.use({ connectOptions: { wsEndpoint: process.env.RUSTX_BROWSER_WS_ENDPOINT ?? '', exposeNetwork: '<loopback>' } });

function displayDuration(start: number) {
  const value = Math.round((performance.now() - start) * 10) / 10;
  expect(Number.isFinite(value)).toBe(true); expect(value).toBeGreaterThanOrEqual(0); return value;
}
const painted = (page: Page) => page.locator('[data-geometry-format] canvas').evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));

async function deliveredModel(page: Page) {
  const fixture = await startDogfood('web_geometry_previews');
  try {
    for (const name of ['mounting-bracket.step', 'mounting-bracket.glb']) copyFileSync(new URL(`../fixtures/geometry/${name}`, import.meta.url), join(fixture.workspaceA, name));
    writeFileSync(join(fixture.workspaceA, 'notes.md'), '# Model review\n\nInspect the shape and its parts in the sidebar.');
    const wire = await wireProbe(page);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Inspect the CAD model');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByText('The model files are ready to inspect.', { exact: true })).toBeVisible();
    await expectSettled(page, wire);
    return fixture;
  } catch (error) { await fixture.stop(false); throw error; }
}

test('delivery CAD occupies the existing document sidebar without clipping on a short window', async ({ page }, info) => {
  const fixture = await deliveredModel(page); let passed = false;
  const timings: { entry: string; name: string; phase: string; displayMs: number }[] = [];
  try {
    let start = performance.now();
    await page.getByRole('button', { name: 'Preview mounting-bracket.step in sidebar', exact: true }).click();
    const sidebar = page.getByRole('complementary', { name: 'Previews', exact: true });
    await expect(sidebar).toBeVisible();
    await expect(sidebar).toHaveAttribute('data-sidebar-right-panel', 'normal');
    await expect(sidebar.getByRole('tab', { name: 'mounting-bracket.step', exact: true })).toBeVisible();
    await expect(sidebar.locator('[data-geometry-format] canvas')).toBeVisible();
    await expect(sidebar.locator('[data-geometry-format]')).toContainText('120 × 72 × 80');
    await painted(page); timings.push({ entry: 'delivery', name: 'mounting-bracket.step', phase: 'first-open', displayMs: displayDuration(start) });
    const controls = sidebar.getByRole('toolbar', { name: 'Model controls', exact: true });
    await expect(controls).toHaveAttribute('aria-orientation', 'vertical');
    const toolsBox = (await controls.boundingBox())!, canvasBox = (await sidebar.locator('[data-geometry-format] canvas').boundingBox())!;
    expect(toolsBox.width).toBeLessThanOrEqual(40); expect(toolsBox.x - canvasBox.x).toBeLessThanOrEqual(20);
    await expect(sidebar.getByRole('button', { name: 'Refresh', exact: true })).toBeVisible();
    await expect(sidebar.getByRole('button', { name: 'Download artifact', exact: true })).toBeVisible();
    await expect(sidebar.getByRole('note')).toHaveCount(0);
    const parts = sidebar.getByRole('button', { name: 'Parts', exact: true });
    await expect(parts).toHaveAttribute('aria-expanded', 'false');
    await expect(sidebar.getByRole('list', { name: 'Parts', exact: true })).toHaveCount(0);
    await page.screenshot({ path: info.outputPath('geometry-sidebar-light.png'), animations: 'disabled' });
    const canvas = sidebar.locator('[data-geometry-format] canvas'), beforeOrbit = await canvas.screenshot();
    const orbit = (await canvas.boundingBox())!;
    await page.mouse.move(orbit.x + orbit.width / 2, orbit.y + orbit.height / 2); await page.mouse.down();
    await page.mouse.move(orbit.x + orbit.width / 2 + 40, orbit.y + orbit.height / 2 + 18, { steps: 8 }); await page.mouse.up();
    expect((await canvas.screenshot()).equals(beforeOrbit)).toBe(false);
    await sidebar.getByRole('button', { name: 'Fit view', exact: true }).click();
    await parts.click();
    const expand = sidebar.getByRole('button', { name: 'Expand: Mounting bracket', exact: true });
    await expect(expand).toHaveAttribute('aria-expanded', 'false');
    await expand.click();
    await sidebar.getByRole('button', { name: 'Base plate', exact: true }).click();
    await expect(sidebar.getByRole('button', { name: 'Base plate', exact: true })).toHaveAttribute('aria-pressed', 'true');
    const base = sidebar.getByRole('checkbox', { name: 'Visible: Base plate', exact: true });
    await base.uncheck(); await expect(base).not.toBeChecked(); await base.check();
    await page.screenshot({ path: info.outputPath('geometry-sidebar-parts.png'), animations: 'disabled' });
    await sidebar.getByRole('button', { name: 'Close parts', exact: true }).click();
    await expect(parts).toBeFocused();
    await page.setViewportSize({ width: 1440, height: 480 });
    await page.screenshot({ path: info.outputPath('geometry-sidebar-short.png'), animations: 'disabled' });
    await expect.poll(() => sidebar.locator('[data-geometry-format]').evaluate(element => {
      const panel = element.closest('[data-preview-pane]')!.getBoundingClientRect();
      const model = element.getBoundingClientRect();
      return Math.max(model.bottom - panel.bottom, element.scrollHeight - element.clientHeight);
    })).toBeLessThanOrEqual(1);
    await parts.click();
    await expect.poll(() => sidebar.getByRole('list', { name: 'Parts', exact: true }).evaluate(element => {
      const stage = element.closest('[data-geometry-format]')!.querySelector('canvas')!.getBoundingClientRect();
      const tree = element.parentElement!.getBoundingClientRect();
      return Math.max(stage.top - tree.top, tree.bottom - stage.bottom, stage.left - tree.left, tree.right - stage.right);
    })).toBeLessThanOrEqual(0);
    await page.keyboard.press('Escape'); await expect(parts).toHaveAttribute('aria-expanded', 'false'); await expect(sidebar).toBeVisible();
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.getByRole('button', { name: 'Preview notes.md in sidebar', exact: true }).click();
    await expect(sidebar.getByRole('heading', { name: 'Model review', exact: true })).toBeVisible();
    await expect(sidebar.locator('[data-geometry-format]')).toHaveCount(0);
    start = performance.now(); await sidebar.getByRole('tab', { name: 'mounting-bracket.step', exact: true }).click();
    await expect(sidebar.locator('[data-geometry-format] canvas')).toBeVisible();
    await painted(page); timings.push({ entry: 'delivery', name: 'mounting-bracket.step', phase: 'retained-reopen', displayMs: displayDuration(start) });
    await expect(parts).toHaveAttribute('aria-expanded', 'false');
    start = performance.now(); await page.getByRole('button', { name: 'Preview mounting-bracket.glb in sidebar', exact: true }).click();
    await expect(sidebar.locator('[data-geometry-format]')).toContainText('0.12 × 0.072 × 0.08');
    await painted(page); timings.push({ entry: 'delivery', name: 'mounting-bracket.glb', phase: 'first-open', displayMs: displayDuration(start) });
    await expect(parts).toHaveAttribute('aria-expanded', 'false');
    await page.screenshot({ path: info.outputPath('geometry-sidebar-glb.png'), animations: 'disabled' });
    writeFileSync(info.outputPath('native-display-timings.json'), JSON.stringify(timings, null, 2));
    passed = true;
  } finally { await page.close(); await fixture.workspaceHost.stop(); await fixture.stop(passed); }
});

test('Workbench CAD uses the shared document body and defaults to revealing the original file', async ({ page }, info) => {
  const fixture = await deliveredModel(page); let passed = false;
  try {
    await page.locator('[data-preview-toggle]').click();
    const workbench = page.locator('[data-workbench]');
    await workbench.getByRole('button', { name: /Workspace files/ }).click();
    const start = performance.now(); await workbench.getByRole('button', { name: 'mounting-bracket.step', exact: true }).click();
    const file = workbench.locator('[data-workbench-file="mounting-bracket.step"]');
    await expect(file.locator('[data-geometry-format] canvas')).toBeVisible();
    await painted(page);
    writeFileSync(info.outputPath('native-workbench-display.json'), JSON.stringify({ entry: 'workbench', name: 'mounting-bracket.step', phase: 'first-open', displayMs: displayDuration(start) }, null, 2));
    await page.screenshot({ path: info.outputPath('geometry-workbench.png'), animations: 'disabled' });
    await expect(file.getByRole('button', { name: 'Open file location', exact: true })).toBeVisible();
    await expect(file.getByRole('button', { name: 'Choose application', exact: true })).toBeVisible();
    await expect(file.getByRole('note')).toHaveCount(0);
    await expect(file.getByRole('button', { name: 'Refresh', exact: true })).toBeVisible();
    await expect(file.getByRole('button', { name: 'Parts', exact: true })).toHaveAttribute('aria-expanded', 'false');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByRole('group', { name: 'Appearance', exact: true }).getByRole('button', { name: 'Dark', exact: true }).click();
    await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
    await page.getByRole('button', { name: '关闭设置', exact: true }).click();
    await page.screenshot({ path: info.outputPath('geometry-workbench-zh-dark.png'), animations: 'disabled' });
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(file.getByRole('button', { name: '打开文件位置', exact: true })).toBeVisible();
    await expect(file.getByRole('button', { name: '选择应用', exact: true })).toBeVisible();
    await expect(file.locator('[data-geometry-format] canvas')).toBeVisible();
    await expect(file.getByRole('note')).toHaveCount(0);
    await page.screenshot({ path: info.outputPath('geometry-sidebar-zh-dark-narrow.png'), animations: 'disabled' });
    await file.getByRole('button', { name: '部件', exact: true }).click();
    await expect(file.getByRole('button', { name: '展开: Mounting bracket', exact: true })).toHaveAttribute('aria-expanded', 'false');
    await page.screenshot({ path: info.outputPath('geometry-sidebar-parts-zh-dark-narrow.png'), animations: 'disabled' });
    await file.getByRole('button', { name: '收起部件', exact: true }).click();
    await expect(file.getByRole('button', { name: '部件', exact: true })).toBeFocused();
    passed = true;
  } finally { await page.close(); await fixture.workspaceHost.stop(); await fixture.stop(passed); }
});

import { expect, test } from '@playwright/test';
import { instrument, resources } from './geometry-observation';
import { isCadRuntimeRequest } from './geometry-requests';

test('Linux native STEP and STP open as full-detail GLB in Workbench', async ({ page }) => {
  if (process.env.RUSTX_REQUIRE_NATIVE_STEP === '1' && !process.env.RUSTX_NATIVE_STEP_EXECUTABLE)
    throw new Error('The required real OCCT STEP helper is unavailable');
  test.skip(!process.env.RUSTX_NATIVE_STEP_EXECUTABLE, 'Set RUSTX_NATIVE_STEP_EXECUTABLE to the pinned Linux helper.');
  const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
  const wasm: string[] = [];
  page.on('request', request => { if (isCadRuntimeRequest(new URL(request.url()))) wasm.push(request.url()); });
  await instrument(page);
  await page.goto(`${origin}/test/fixtures/workbench.html?draft`);
  await page.getByRole('button', { name: 'Select Workspace Workspace A', exact: true }).click();
  await page.locator('[data-preview-toggle]').click();
  const workbench = page.locator('[data-workbench]');
  await workbench.getByRole('button', { name: /Workspace files/ }).click();
  for (const name of ['cube.step', 'inch.stp']) {
    await workbench.getByRole('button', { name, exact: true }).click();
    const geometry = page.locator(`[data-workbench-file="${name}"] [data-geometry-format="step"]`);
    await expect(geometry).toHaveAttribute('data-geometry-preview-format', 'glb');
    await expect(geometry.locator('canvas')).toHaveAttribute('data-geometry-lod', 'full');
    await expect(geometry.locator('footer')).toContainText('0.01 × 0.01 × 0.01');
    await expect(geometry.locator('footer')).toContainText('STEP');
    await expect.poll(() => page.evaluate(() => performance.getEntriesByName('geometry:native-step', 'mark').length)).toBe(1);
    await workbench.getByRole('tab', { name, exact: false }).hover();
    await workbench.getByRole('tab', { name, exact: false }).locator('[data-dockkit-tab-close]').click();
    await expect.poll(async () => (await resources(page)).contexts).toBe(0);
    await workbench.getByRole('tab', { name: 'Files', exact: false }).click();
  }
  for (const name of ['malformed.step', 'partial-model.step', 'empty-wire.step']) {
    await workbench.getByRole('button', { name, exact: true }).click();
    const file = page.locator(`[data-workbench-file="${name}"]`);
    await expect(file.getByRole('alert')).toContainText('Document parser failed');
    await expect(file.locator('canvas')).toHaveCount(0);
    await workbench.getByRole('tab', { name: 'Files', exact: false }).click();
  }
  expect(wasm).toEqual([]);
});

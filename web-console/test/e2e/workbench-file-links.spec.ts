import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
test('conversation file references reveal rendered documents, reuse tabs and navigate source lines', async ({ page }, info) => {
  await page.goto(`${origin}/test/fixtures/workbench.html?file-links`);
  await page.getByRole('button', { name: 'モルガン 解説', exact: true }).click();
  const panel = page.locator('[data-workbench]');
  await expect(panel.getByRole('heading', { name: 'モルガン 解説' })).toBeVisible();
  await page.screenshot({ path: info.outputPath('file-reference-preview.png') });
  await page.getByRole('button', { name: 'Same file', exact: true }).click();
  await expect(panel.getByRole('tab')).toHaveCount(1);
  await panel.getByRole('button', { name: 'Toggle workspace panel', exact: true }).click();
  await page.getByRole('button', { name: 'モルガン 解説', exact: true }).click();
  await expect(panel.getByRole('heading', { name: 'モルガン 解説' })).toBeVisible();
  await page.getByRole('button', { name: 'Source line', exact: true }).click();
  await expect(panel.locator('[data-workbench-file="lines.py"]')).toBeVisible();
  await expect.poll(() => panel.locator('[data-code-block-content]').evaluate(el => el.scrollTop)).toBeGreaterThan(1000);
  await page.getByRole('button', { name: 'Missing', exact: true }).click();
  await expect(panel.getByRole('alert')).toBeVisible();
  await expect(panel.getByRole('tab')).toHaveCount(2);
});

test('a held file reference cannot supersede a newer selection or survive leaving the Session', async ({ page }) => {
  await page.goto(`${origin}/test/fixtures/workbench.html?file-links`);
  let release!: () => void, admitted!: () => void;
  let held = new Promise<void>(resolve => { release = resolve; });
  let started = new Promise<void>(resolve => { admitted = resolve; });
  await page.route('**/product-host/workbench-fixture/workbench', async route => {
    const body = route.request().postDataJSON();
    if (body.call?.request.kind === 'resolve' && body.call.request.path === 'docs/モルガン_解説.md') {
      await route.fetch(); admitted(); await held;
      await route.fulfill({ json: { path: 'docs/モルガン_解説.md' } });
    } else await route.continue();
  });
  await page.getByRole('button', { name: 'モルガン 解説', exact: true }).click(); await started;
  await page.getByRole('button', { name: 'Source line', exact: true }).click();
  const panel = page.locator('[data-workbench]');
  await expect(panel.locator('[data-workbench-file="lines.py"]')).toBeVisible();
  release();
  await expect(panel.getByRole('tab')).toHaveCount(1);
  held = new Promise<void>(resolve => { release = resolve; });
  started = new Promise<void>(resolve => { admitted = resolve; });
  await page.getByRole('button', { name: 'モルガン 解説', exact: true }).click(); await started;
  await page.getByRole('button', { name: 'New Conversation', exact: true }).first().click();
  release();
  await expect(page.getByRole('button', { name: 'モルガン 解説', exact: true })).toHaveCount(0);
  await expect(panel.locator('[data-workbench-file]')).toHaveCount(0);
});

test('trajectory preview uses the same file reference delegate', async ({ page }) => {
  await page.goto(`${origin}/test/fixtures/workbench.html?file-links`);
  await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
  await page.locator('[data-trace-id="trace:1"]').click();
  const inspector = page.getByRole('complementary', { name: 'Trace record inspector' });
  await inspector.getByRole('tab', { name: 'Summary', exact: true }).click();
  await inspector.getByRole('button', { name: 'モルガン 解説', exact: true }).click();
  await expect(page.locator('[data-workbench]').getByRole('heading', { name: 'モルガン 解説' })).toBeVisible();
});

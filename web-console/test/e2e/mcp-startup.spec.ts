import { expect, test } from '@playwright/test';
import { openSettingsPage } from './shell-actions';

for (const width of [1440, 390]) {
  test(`MCP startup presents cached metadata without claiming a ready connection at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1000 });
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.addInitScript(() => localStorage.setItem('rustx-locale-v1', 'en'));
    await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/settings.html?scenario=mcp-startup`);
    await expect(page).toHaveTitle('rustX native Settings reference');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.setViewportSize({ width, height: 1000 });
    await openSettingsPage(page, 'MCP servers');
    for (const [name, label] of [['cold', 'Connecting'], ['warm', 'Connecting · cached catalog']]) {
      const row = page.getByRole('listitem', { name, exact: true });
      await expect(row.getByText(label, { exact: true })).toBeVisible();
      await expect(row.getByRole('button', { name: `MCP ${name}`, exact: true })).toHaveAccessibleDescription(label);
      await expect(page.getByText('Prepared', { exact: true })).toHaveCount(0);
      await expect(page.getByText('Preparation unavailable', { exact: true })).toHaveCount(0);
      await page.screenshot({ path: test.info().outputPath(`${name}-connecting.png`) });
      await row.getByRole('button', { name: `MCP ${name}`, exact: true }).click();
      await expect(page.getByRole('heading', { name: 'Edit MCP server', exact: true })).toBeVisible();
      await page.getByRole('button', { name: '← MCP servers', exact: true }).click();
      await expect(row.getByText(label, { exact: true })).toBeVisible();
    }
    expect(errors).toEqual([]);
  });
}

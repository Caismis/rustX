import { expect, test } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';

test('native Settings source save, reset, catalog edit and responsive projection', async ({ page }) => {
  const fixture = await startDogfood();
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await page.getByLabel('WebSocket endpoint').fill(`${fixture.endpoint}/`);
    await page.getByLabel('Transport token').fill(fixture.token);
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(page.locator('.status strong')).toHaveText('connected');
    await page.getByLabel('Choose Workspace').selectOption({ label: 'Workspace A' });
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(page.locator('.session-toolbar small')).toContainText('attached');
    await page.getByRole('tab', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('region', { name: 'Settings', exact: true });
    await expect(settings.getByRole('heading', { name: 'Settings · Provider / Models' })).toBeVisible();
    await expect(settings.getByLabel('User model', { exact: true })).toHaveValue('fixture/console-model');
    await settings.getByLabel('Workspace model', { exact: true }).selectOption('fixture/second-model');
    await settings.getByRole('button', { name: 'Save Workspace', exact: true }).click();
    await expect(settings.getByRole('status')).toContainText('Source committed');
    await expect(settings.locator('dd').first()).toHaveText('fixture/second-model');
    await settings.getByRole('button', { name: 'Reset Workspace', exact: true }).click();
    await settings.getByRole('button', { name: 'Save Workspace', exact: true }).click();
    await expect(settings.locator('dd').first()).toHaveText('fixture/console-model');
    await settings.getByLabel('Context window').first().fill('256000');
    await settings.getByRole('button', { name: 'Save User catalog', exact: true }).click();
    await expect(settings.getByRole('status')).toContainText('Source committed');
    await settings.getByRole('button', { name: 'Reload / discard draft' }).click();
    await expect(settings.getByLabel('Context window').first()).toHaveValue('256000');
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    await page.screenshot({ path: 'test-results/settings-desktop.png', fullPage: true });
    await settings.getByText('User Provider / model catalog', { exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: 'test-results/settings-catalog.png', fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    await settings.scrollIntoViewIfNeeded();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: 'test-results/settings-mobile.png', fullPage: true });
    expect(errors).toEqual([]);
  } finally { const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

import { expect, test, type Locator } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';

for (const width of [390, 820, 1280, 1600]) test(`one product keyboard and editor reachability at ${width}px`, async ({ page }) => {
  const fixture = await startDogfood();
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width, height: 900 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  // Traverse real Tab order. A finite traversal detects focus traps.
  const tabTo = async (target: Locator) => {
    for (let i = 0; i < 180; i++) {
      if (await target.evaluate(el => el === document.activeElement)) return;
      await page.keyboard.press('Tab');
    }
    throw new Error('Keyboard could not reach the requested control');
  };
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/'); await expect(page).toHaveTitle(/rustX/);
    await tabTo(page.getByLabel('WebSocket endpoint')); await page.keyboard.press('ControlOrMeta+A'); await page.keyboard.type(fixture.endpoint);
    await tabTo(page.getByLabel('Transport token')); await page.keyboard.type(fixture.token);
    await tabTo(page.getByRole('button', { name: 'Connect', exact: true })); await page.keyboard.press('Enter');
    await expect(page.locator('.status strong')).toHaveText('connected');
    const workspace = page.getByLabel('Choose Workspace');
    await tabTo(workspace); await page.keyboard.press('ArrowDown'); await page.keyboard.press('Enter');
    await tabTo(page.getByRole('button', { name: 'Create Session', exact: true })); await page.keyboard.press('Enter');
    const message = page.getByLabel('Message', { exact: true }); await expect(message).toBeEnabled();
    await tabTo(message); await page.keyboard.type('/mdl'); await page.keyboard.press('Enter');
    const dialog = page.getByRole('dialog', { name: '/model', exact: true }); await expect(dialog).toBeVisible();
    await page.keyboard.press('Escape'); await expect(dialog).toHaveCount(0); await expect(message).toBeFocused();
    expect(await message.evaluate(el => getComputedStyle(el).outlineStyle)).not.toBe('none');
    await page.keyboard.press('ControlOrMeta+A'); await page.keyboard.press('Backspace');
    const chat = page.getByRole('tab', { name: 'Chat', exact: true }); await tabTo(chat);
    await page.keyboard.press('ArrowRight');
    await expect(page.getByRole('tab', { name: 'Trajectory', exact: true })).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('region', { name: 'Trajectory', exact: true })).toBeVisible();
    await page.keyboard.press('End'); await page.keyboard.press('Enter');
    await expect(page.getByRole('tab', { name: 'Settings', exact: true })).toBeFocused();
    const settings = page.getByRole('region', { name: 'Settings', exact: true });
    const endpoint = settings.getByLabel('Endpoint · fixture');
    await tabTo(endpoint); await expect(endpoint).toBeFocused();
    await page.keyboard.press('End'); await page.keyboard.type('/draft-only');
    await tabTo(settings.getByRole('button', { name: 'Integrations', exact: true })); await page.keyboard.press('Enter');
    const integrations = page.getByRole('region', { name: 'Integrations', exact: true });
    await tabTo(integrations.getByRole('button', { name: 'Add User MCP server' })); await page.keyboard.press('Enter');
    const identity = integrations.getByLabel('Server identity'); await tabTo(identity); await page.keyboard.type('keyboard-draft');
    await tabTo(integrations.getByLabel('Command', { exact: true })); await page.keyboard.type('disabled-draft');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    expect(await page.locator('main').evaluate(el => getComputedStyle(el.parentElement!).transitionDuration)).toBe('0s');
    await page.screenshot({ path: `test-results/acceptance-keyboard-${width}.png`, fullPage: true });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  } finally { await page.close(); const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

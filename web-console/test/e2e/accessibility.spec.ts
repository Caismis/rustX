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
    await tabTo(page.getByRole('button', { name: 'Settings', exact: true })); await page.keyboard.press('Enter');
    await expect(page.getByRole('button', { name: 'Overview', exact: true })).toHaveAttribute('aria-current', 'page');
    await tabTo(page.getByRole('button', { name: 'Connection', exact: true })); await page.keyboard.press('Enter');
    await tabTo(page.getByLabel('Connection mode')); await page.keyboard.press('ArrowDown'); await page.keyboard.press('Enter');
    await tabTo(page.getByLabel('WebSocket endpoint')); await page.keyboard.press('ControlOrMeta+A'); await page.keyboard.type(fixture.endpoint);
    await tabTo(page.getByLabel('Transport token')); await page.keyboard.type(fixture.token);
    await tabTo(page.getByRole('button', { name: 'Connect', exact: true })); await page.keyboard.press('Enter');
    await expect(page.locator('.connection-status')).toHaveText('Connected');
    await tabTo(page.getByRole('button', { name: 'Close Settings', exact: true })); await page.keyboard.press('Enter');
    await tabTo(page.getByRole('button', { name: 'New Session', exact: true }).first()); await page.keyboard.press('Enter');
    const workspace = page.getByLabel('Choose Workspace');
    await tabTo(workspace); await page.keyboard.press('ArrowDown'); await page.keyboard.press('Enter');
    await tabTo(page.getByRole('button', { name: 'Create Session', exact: true })); await page.keyboard.press('Enter');
    const message = page.getByLabel('Message', { exact: true }); await expect(message).toBeEnabled();
    const expandSidebar = page.getByRole('button', { name: 'Expand Sidebar', exact: true });
    const wasCollapsed = await expandSidebar.isVisible();
    if (wasCollapsed) { await tabTo(expandSidebar); await page.keyboard.press('Enter'); }
    await tabTo(page.locator('button[data-session-id]').first()); await page.keyboard.press('Enter');
    const rowActions = page.locator('button[data-session-actions]').first();
    await tabTo(rowActions); await page.keyboard.press('Enter'); await page.keyboard.press('ArrowDown');
    await expect(page.getByRole('menuitem', { name: 'Close New session view', exact: true })).toBeFocused();
    await page.keyboard.press('Escape'); await expect(rowActions).toBeFocused();
    if (wasCollapsed) { await tabTo(page.getByRole('button', { name: 'Collapse Sidebar', exact: true })); await page.keyboard.press('Enter'); }
    await tabTo(message); await page.keyboard.type('/mdl'); await page.keyboard.press('Enter');
    const dialog = page.getByRole('dialog', { name: '/model', exact: true }); await expect(dialog).toBeVisible();
    await page.keyboard.press('Escape'); await expect(dialog).toHaveCount(0); await expect(message).toBeFocused();
    // The composer paints keyboard focus on its rounded card, not a second
    // rectangular outline around the native text scrollport.
    expect(await message.evaluate(el => {
      const card = getComputedStyle(el.closest('[data-composer-card]')!);
      return el.matches(':focus-visible') && card.boxShadow !== 'none'
        && card.getPropertyValue('--dsw-elevation-stroke-color').trim() === card.getPropertyValue('--dsw-alias-state-business-primary').trim();
    })).toBe(true);
    await page.keyboard.press('ControlOrMeta+A'); await page.keyboard.press('Backspace');
    const chat = page.getByRole('tab', { name: 'Chat', exact: true }); await tabTo(chat);
    await page.keyboard.press('ArrowRight');
    await expect(page.getByRole('tab', { name: 'Trajectory', exact: true })).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('region', { name: 'Trajectory', exact: true })).toBeVisible();
    await tabTo(page.getByRole('button', { name: 'Settings', exact: true })); await page.keyboard.press('Enter');
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    await expect(settings.getByRole('button', { name: 'Overview', exact: true })).toHaveAttribute('aria-current', 'page');
    await tabTo(settings.getByLabel('Configuration owner')); await expect(settings.getByLabel('Configuration owner')).toBeFocused();
    await tabTo(settings.getByRole('button', { name: 'Providers & Models', exact: true })); await page.keyboard.press('Enter');
    await tabTo(settings.getByRole('button', { name: 'Edit Provider fixture', exact: true })); await page.keyboard.press('Enter');
    const endpoint = settings.getByLabel('Endpoint', { exact: true });
    await tabTo(endpoint); await expect(endpoint).toBeFocused();
    await page.keyboard.press('End'); await page.keyboard.type('/draft-only');
    await tabTo(settings.getByRole('button', { name: 'MCP', exact: true })); await page.keyboard.press('Enter');
    await tabTo(settings.getByLabel('New MCP identity')); await page.keyboard.type('keyboard-draft');
    await tabTo(settings.getByRole('button', { name: 'Add MCP', exact: true })); await page.keyboard.press('Enter');
    await tabTo(settings.getByLabel('MCP command', { exact: true })); await page.keyboard.type('inert-draft');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    expect(await page.locator('main').evaluate(el => getComputedStyle(el.parentElement!).transitionDuration)).toBe('0s');
    await page.screenshot({ path: `test-results/acceptance-keyboard-${width}.png`, fullPage: true });
    await page.keyboard.press('Escape');
    await tabTo(page.getByRole('button', { name: 'Toggle Inspector', exact: true })); await page.keyboard.press('Enter');
    await expect(page.getByRole('complementary', { name: 'Developer inspector' })).toBeVisible();
    await tabTo(page.getByRole('button', { name: 'Close Inspector', exact: true })); await page.keyboard.press('Enter');
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  } finally { await page.close(); const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

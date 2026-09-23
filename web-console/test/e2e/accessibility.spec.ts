import { expect, test, type Locator } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { selectedSettingsPage, settingsSectionMenu } from './shell-actions';

const settingsPageOrder = ['General', 'Models', 'Agent', 'Tools & Permissions', 'Extensions', 'Advanced'];

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
  const pageTab = (name: string) => page.getByRole('tablist', { name: 'Settings pages' }).getByRole('tab', { name, exact: true });
  /** Select a Settings page from the keyboard alone, through whichever page
   * selector the panel shows: the rail is one roving tab stop moved by
   * ArrowUp/ArrowDown; the narrow section menu opens with Enter on its first
   * row, walks with ArrowDown and settles with Enter, handing focus back to
   * its trigger. */
  const keyboardPage = async (name: string) => {
    const menu = settingsSectionMenu(page);
    if (await menu.isVisible()) {
      await tabTo(menu); await page.keyboard.press('Enter');
      const rows = page.getByRole('menu').getByRole('menuitem');
      await expect(rows.first()).toBeFocused();
      const target = page.getByRole('menu').getByRole('menuitem', { name, exact: true });
      for (let step = 0; step < settingsPageOrder.length && !await target.evaluate(el => el === document.activeElement); step++) await page.keyboard.press('ArrowDown');
      await page.keyboard.press('Enter');
      await expect(page.getByRole('menu')).toHaveCount(0);
      await expect(menu).toHaveAccessibleName(`Settings page: ${name}`); await expect(menu).toBeFocused();
      return;
    }
    const current = await selectedSettingsPage(page);
    await tabTo(pageTab(current));
    const steps = settingsPageOrder.indexOf(name) - settingsPageOrder.indexOf(current);
    for (let step = 0; step < Math.abs(steps); step++) await page.keyboard.press(steps > 0 ? 'ArrowDown' : 'ArrowUp');
    await expect(pageTab(name)).toHaveAttribute('aria-selected', 'true');
  };
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/'); await expect(page).toHaveTitle(/rustX/);
    await tabTo(page.getByRole('button', { name: 'Settings', exact: true })); await page.keyboard.press('Enter');
    // Global Settings opens at General. At 390 the panel is narrow and the
    // section menu replaces the rail; every other width shows the rail.
    await expect(settingsSectionMenu(page)).toBeVisible({ visible: width === 390 });
    await expect(pageTab('General')).toBeVisible({ visible: width !== 390 });
    expect(await selectedSettingsPage(page)).toBe('General');
    await keyboardPage('Advanced');
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
    expect(await selectedSettingsPage(page)).toBe('General');
    await keyboardPage('Models');
    // A resource list is one grid: the row opens its detail with Enter.
    await tabTo(settings.getByRole('row', { name: 'fixture', exact: true })); await page.keyboard.press('Enter');
    const endpoint = settings.getByLabel('Endpoint', { exact: true });
    await tabTo(endpoint); await expect(endpoint).toBeFocused();
    await page.keyboard.press('End'); await page.keyboard.type('/draft-only');
    // A select opens and closes from the keyboard without changing the value.
    const credential = settings.getByRole('button', { name: /Credential source$/ });
    await tabTo(credential); await page.keyboard.press('Enter');
    await expect(page.getByRole('listbox')).toBeVisible();
    await page.keyboard.press('Escape'); await expect(page.getByRole('listbox')).toHaveCount(0);
    await expect(credential).toBeFocused();
    // A disclosure toggles from the keyboard.
    const revision = settings.getByRole('button', { name: 'Source revision & replacement', exact: true });
    await tabTo(revision); await page.keyboard.press('Enter');
    await expect(revision).toHaveAttribute('aria-expanded', 'true');
    // A destructive action opens a modal confirmation that holds focus, is
    // dismissed with Escape without writing, and returns focus to its trigger.
    const remove = settings.getByRole('button', { name: 'Remove Provider fixture', exact: true });
    await tabTo(remove); await page.keyboard.press('Enter');
    const confirmation = page.getByRole('alertdialog');
    await expect(confirmation).toBeVisible();
    await expect(confirmation.getByRole('button', { name: 'Cancel', exact: true })).toBeFocused();
    await page.keyboard.press('Tab'); await page.keyboard.press('Tab');
    expect(await confirmation.evaluate(el => el.contains(document.activeElement))).toBe(true);
    await page.keyboard.press('Escape');
    await expect(confirmation).toHaveCount(0); await expect(remove).toBeFocused();
    await expect(endpoint).toHaveValue(/\/draft-only$/);
    // Across pages to Extensions, then the extension-kind filter, all from
    // the keyboard.
    await keyboardPage('Extensions');
    const filterTab = (name: string) => settings.getByRole('tablist', { name: 'Extension kinds' }).getByRole('tab', { name, exact: true });
    await tabTo(filterTab('All')); await page.keyboard.press('ArrowRight');
    await expect(filterTab('MCP')).toHaveAttribute('aria-selected', 'true');
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

import { expect, type Locator, type Page } from '@playwright/test';
export async function closeSessionView(page: Page, id: string) {
  await page.locator(`button[data-session-id="${id}"]`).hover();
  await page.locator(`button[data-session-actions="${id}"]`).click();
  await page.getByRole('menuitem', { name: /^Close .+ view$/ }).click();
}
export async function closeSettings(page: Page) {
  const close = page.getByRole('button', { name: 'Close Settings', exact: true });
  if (await close.isVisible()) await close.click();
}
export async function chooseWorkspace(page: Page, label: string) {
  await closeSettings(page);
  await page.getByRole('button', { name: 'New Session', exact: true }).first().click();
  await page.getByLabel('Choose Workspace').selectOption({ label });
}
/** Enter Workspace Settings from the exact Workspace object action. */
export async function openWorkspaceSettings(page: Page, label: string) {
  await closeSettings(page);
  await page.getByRole('button', { name: `Select Workspace ${label}`, exact: true }).hover();
  await page.getByRole('button', { name: `Workspace actions for ${label}`, exact: true }).click();
  await page.getByRole('menuitem', { name: 'Workspace settings', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toBeVisible();
}
export async function connectionAction(page: Page, action: 'Disconnect' | 'Reconnect') {
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  const previous = await settings.isVisible() ? await selectedSettingsPage(page) : undefined;
  await openConnectionSettings(page);
  await page.getByRole('region', { name: 'Connection Settings', exact: true }).getByRole('button', { name: action, exact: true }).click();
  if (action === 'Reconnect') await expect(page.locator('.connection-status')).toHaveText('Connected');
  if (previous) {
    await settings.getByRole('button', { name: 'Back to Advanced', exact: true }).click();
    await openSettingsPage(page, previous);
  } else await closeSettings(page);
}
/** Connection is the Advanced sub-surface of the global client's Settings. */
export async function openConnectionSettings(page: Page) {
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  if (!await settings.isVisible()) await page.getByRole('button', { name: 'Settings', exact: true }).click();
  if (!await settings.getByRole('region', { name: 'Connection Settings', exact: true }).isVisible()) {
    await openSettingsPage(page, 'Advanced');
    await settings.getByRole('button', { name: 'Connection', exact: true }).click();
  }
}
/** The Settings page selector the panel currently shows. A wide panel shows
 * the vertical page rail; a narrow panel hides it and shows the section menu
 * trigger instead. Which one is decided by the panel's own width in CSS, so a
 * test asks the rendered page rather than assuming it from the viewport. */
export function settingsSectionMenu(page: Page) {
  return page.getByRole('dialog', { name: 'Settings', exact: true }).getByRole('button', { name: /^Settings page: / });
}
/** The primary Settings page currently selected. */
export async function selectedSettingsPage(page: Page): Promise<string> {
  const menu = settingsSectionMenu(page);
  if (await menu.isVisible()) return (await menu.getAttribute('aria-label'))!.replace(/^Settings page: /, '');
  return page.getByRole('tablist', { name: 'Settings pages', exact: true }).getByRole('tab', { selected: true }).innerText();
}
/** Select one of the six primary Settings pages the way a user can on the
 * rendered layout: its rail tab on a wide panel, or its row in the section
 * menu on a narrow one. Either way the selection is the navigation owner's,
 * and the menu hands focus back to its trigger. */
export async function openSettingsPage(page: Page, name: string) {
  const menu = settingsSectionMenu(page);
  if (await menu.isVisible()) {
    await menu.click();
    await page.getByRole('menu').getByRole('menuitem', { name, exact: true }).click();
    await expect(menu).toHaveAccessibleName(`Settings page: ${name}`);
    await expect(page.getByRole('menu')).toHaveCount(0);
    return;
  }
  const tab = page.getByRole('tablist', { name: 'Settings pages', exact: true }).getByRole('tab', { name, exact: true });
  await tab.click();
  await expect(tab).toHaveAttribute('aria-selected', 'true');
}
/** Pick a value from a React Aria Select. Its trigger is named by its current
 * value followed by its label, and its listbox opens in a popover. */
export async function choose(scope: Locator, label: string, option: string) {
  await scope.getByRole('button', { name: new RegExp(`${label.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`) }).click();
  await scope.page().getByRole('listbox').getByRole('option', { name: option, exact: true }).click();
}
/** Complete a removal through its confirmation dialog. Restoring inheritance
 * (Use global default) is an ordinary dialog; a real deletion is an alert
 * dialog. Both are titled by the question they ask. */
export async function confirmSettingsAction(page: Page, label: string) {
  await page.getByRole('dialog', { name: 'Settings', exact: true }).getByRole('button', { name: label, exact: true }).click();
  const dialog = page.getByRole(label.startsWith('Use global default') ? 'dialog' : 'alertdialog', { name: /\?$/ });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: label, exact: true }).click();
  await expect(dialog).toHaveCount(0);
}
export async function connectRemote(page: Page, endpoint: string, token: string) {
  await openConnectionSettings(page);
  await page.getByLabel('Connection mode').selectOption('remote');
  await page.getByLabel('WebSocket endpoint').fill(endpoint);
  await page.getByLabel('Transport token').fill(token);
  await page.getByRole('button', { name: 'Connect', exact: true }).click();
  await expect(page.locator('.connection-status')).toHaveText('Connected');
  await closeSettings(page);
}
export async function showInspector(page: Page) {
  const panel = page.getByRole('complementary', { name: 'Developer inspector' });
  if (!await panel.isVisible()) await page.getByRole('button', { name: 'Toggle Inspector' }).click();
  await expect(panel).toBeVisible();
  const details = panel.getByText('Complete native runtime facts', { exact: true });
  if (!await panel.getByLabel('Native diagnostic JSON').isVisible()) await details.click();
}

export async function sessionTree(page: Page) {
  await page.getByRole('button', { name: 'Session actions', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Session tree', exact: true }).click();
}

export async function expectSettled(page: Page) {
  await showInspector(page);
  await expect.poll(async () => JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).attempt?.phase.type).toBe('settled');
}

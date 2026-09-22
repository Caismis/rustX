import { expect, type Page } from '@playwright/test';
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
  const previous = await settings.isVisible() ? await settings.locator('[aria-current="page"]').innerText() : undefined;
  await openConnectionSettings(page);
  await page.getByRole('region', { name: 'Connection Settings', exact: true }).getByRole('button', { name: action, exact: true }).click();
  if (action === 'Reconnect') await expect(page.locator('.connection-status')).toHaveText('Connected');
  if (previous) await settings.getByRole('button', { name: previous, exact: true }).click();
  else await closeSettings(page);
}
export async function openConnectionSettings(page: Page) {
  if (!await page.getByRole('dialog', { name: 'Settings', exact: true }).isVisible()) await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Connection', exact: true }).click();
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

import { expect, type Page } from '@playwright/test';
export async function closeSettings(page: Page) {
  const close = page.getByRole('button', { name: 'Close Settings', exact: true });
  if (await close.isVisible()) await close.click();
}
export async function chooseWorkspace(page: Page, label: string) {
  await closeSettings(page);
  await page.getByRole('button', { name: 'New Session', exact: true }).first().click();
  await page.getByLabel('Choose Workspace').selectOption({ label });
}
export async function connectionAction(page: Page, action: 'Disconnect' | 'Reconnect') {
  if (!await page.getByRole('dialog', { name: 'Connection', exact: true }).isVisible()) {
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    await (await settings.isVisible() ? settings : page).getByRole('button', { name: 'Connection', exact: true }).click();
  }
  await page.getByRole('dialog', { name: 'Connection', exact: true }).getByRole('button', { name: action, exact: true }).click();
  if (action === 'Disconnect') await page.getByRole('dialog', { name: 'Connection', exact: true }).getByRole('button', { name: 'Close dialog', exact: true }).click();
  else await expect(page.getByRole('dialog', { name: 'Connection', exact: true })).toHaveCount(0);
}
export async function showInspector(page: Page) {
  const panel = page.getByRole('complementary', { name: 'Developer inspector' });
  if (!await panel.isVisible()) await page.getByRole('button', { name: 'Toggle Inspector' }).click();
  await expect(panel).toBeVisible();
  const details = panel.getByText('Complete native runtime facts', { exact: true });
  if (!await panel.getByLabel('Native diagnostic JSON').isVisible()) await details.click();
}

export async function unloadSession(page: Page) {
  await page.getByRole('button', { name: 'Session actions', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Advanced Session controls' }).click();
  await page.getByRole('dialog', { name: 'Advanced Session controls' }).getByRole('button', { name: 'Unload runtime' }).click();
  await expect(page.getByLabel('Session status').getByRole('button', { name: 'Open Session', exact: true })).toBeVisible();
}

export async function sessionTree(page: Page) {
  await page.getByRole('button', { name: 'Session actions', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Session tree', exact: true }).click();
}

export async function expectSettled(page: Page) {
  await showInspector(page);
  await expect.poll(async () => JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).attempt?.phase.type).toBe('settled');
}

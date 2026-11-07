import { expect, type Locator, type Page } from '@playwright/test';
import type { AttachmentTarget } from '../../../protocol/app-server/v74';
import type { wireProbe } from './wire-probe';
export async function closeSessionView(page: Page, id: string) {
  await page.getByRole('button', { name: 'View options', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Close view', exact: true }).hover();
  await page.locator(`[role="menuitem"][data-menu-item-id="close:${id}"]`).click();
}
/** Leave a primary page through the authoritative Home route. */
export async function returnHome(page: Page) {
  const home = page.getByRole('button', { name: 'Home', exact: true });
  if (await home.getAttribute('aria-current') !== 'page') {
    await home.click();
    await expect(home).toHaveAttribute('aria-current', 'page');
  }
}
export async function chooseWorkspace(page: Page, label: string) {
  await returnHome(page);
  // The sidebar action owns creation; the provisional history row can have
  // the same accessible name while an unsent draft is selected.
  const create = page.locator('#home-sidebar button[aria-label="New Conversation"]');
  if (!await create.isVisible()) await page.getByRole('button', { name: 'Home', exact: true }).click();
  await create.click();
  await page.getByRole('button', { name: 'Choose Workspace', exact: true }).click();
  await page.getByRole('menuitem', { name: label, exact: true }).click();
}
/** Enter Workspace Settings from the exact Workspace object action. */
export async function openWorkspaceSettings(page: Page, label: string) {
  await returnHome(page);
  if (!await page.getByRole('button', { name: `Select Workspace ${label}`, exact: true }).isVisible()) await page.getByRole('button', { name: 'Home', exact: true }).click();
  await page.getByRole('button', { name: `Select Workspace ${label}`, exact: true }).hover();
  await page.getByRole('button', { name: `Workspace actions for ${label}`, exact: true }).click();
  await page.getByRole('menuitem', { name: 'Workspace settings', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Settings', exact: true })).toBeVisible();
}
export async function connectionAction(page: Page, action: 'Disconnect' | 'Reconnect') {
  const settings = page.getByRole('region', { name: 'Settings', exact: true });
  const previous = await settings.isVisible() ? await selectedSettingsPage(page) : undefined;
  await openConnectionSettings(page);
  await page.getByRole('region', { name: 'Connection Settings', exact: true }).getByRole('button', { name: action, exact: true }).click();
  if (action === 'Reconnect') await expect(page.locator('.connection-status')).toHaveText('Connected');
  if (previous) {
    await settings.getByRole('button', { name: 'Back to Advanced', exact: true }).click();
    await openSettingsPage(page, previous);
  } else await returnHome(page);
}
/** Connection is the Advanced sub-surface of the global client's Settings. */
export async function openConnectionSettings(page: Page) {
  const settings = page.getByRole('region', { name: 'Settings', exact: true });
  if (!await settings.isVisible()) await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(settings).toBeVisible();
  const user = settings.getByRole('button', { name: 'Open User Settings', exact: true });
  if (await user.isVisible()) await user.click();
  if (!await settings.getByRole('region', { name: 'Connection Settings', exact: true }).isVisible()) {
    await openSettingsPage(page, 'Advanced');
    // Section selection resumes its retained detail. A prior Connection visit
    // may already be displayed after the section transition settles.
    if (!await settings.getByRole('region', { name: 'Connection Settings', exact: true }).isVisible()) await settings.getByRole('button', { name: 'Connection', exact: true }).click();
  }
  await expect(settings.getByRole('region', { name: 'Connection Settings', exact: true })).toBeVisible();
}
/** The Settings page selector the panel currently shows. A wide panel shows
 * the vertical page rail; a narrow panel hides it and shows the section menu
 * trigger instead. Which one is decided by the panel's own width in CSS, so a
 * test asks the rendered page rather than assuming it from the viewport. */
export function settingsSectionMenu(page: Page) {
  return page.getByRole('region', { name: 'Settings', exact: true }).getByRole('button', { name: /^Settings page: / });
}
/** The primary Settings page currently selected. */
export async function selectedSettingsPage(page: Page): Promise<string> {
  const menu = settingsSectionMenu(page);
  if (await menu.isVisible()) return (await menu.getAttribute('aria-label'))!.replace(/^Settings page: /, '');
  return page.getByRole('tablist', { name: 'Settings pages', exact: true }).getByRole('tab', { selected: true }).innerText();
}
/** Select an authorized Settings section the way a user can on the
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
  await page.getByRole('button', { name: label, exact: true }).click();
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
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  // Fixture setup leaves both the section and its retained detail at known
  // roots. Later primary-page navigation is free to resume its exact detail.
  const back = page.getByRole('button', { name: 'Back to Advanced', exact: true });
  if (await back.isVisible()) await back.click();
  await openSettingsPage(page, 'General');
  await returnHome(page);
}
export async function selectedSessionId(page: Page) {
  return (await page.locator('button[data-session-id][aria-current="page"]').getAttribute('data-session-id'))!;
}

export async function selectedTarget(page: Page, wire: Awaited<ReturnType<typeof wireProbe>>): Promise<AttachmentTarget> {
  const id = await selectedSessionId(page);
  return wire.attachedTarget(id);
}

export async function selectedCheckpoint(page: Page, wire: Awaited<ReturnType<typeof wireProbe>>) {
  return (await wire.read('session/checkpoint', await selectedTarget(page, wire))).checkpoint;
}

export async function sessionTree(page: Page) {
  await page.getByRole('button', { name: 'Session actions', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Session tree', exact: true }).click();
}

export async function expectSettled(page: Page, wire: Awaited<ReturnType<typeof wireProbe>>) {
  await expect.poll(async () => (await selectedCheckpoint(page, wire)).snapshot.attempt?.phase.type).toBe('settled');
}

/** Native fixture setup for scenarios that begin with an existing empty Session.
 * Creation UX is exercised separately by convergence.spec.ts. This performs the
 * real native create and normal catalog refresh and attachment, without a browser-only ID. */
export async function openEmptySession(page: Page, fixture: { endpoint: string; token: string; automaticTitles?: boolean; workspaceHost: { host: import('../../src/workspaces/host').ProductHostWorkspaces } }, label: string) {
  const { AppServerHost } = await import('../../../tui/src/app-server/host');
  const catalog = await fixture.workspaceHost.host.listWorkspaces();
  const workspace = catalog.workspaces.find(row => row.displayName === label);
  if (!workspace) throw new Error(`Fixture Workspace unavailable: ${label}`);
  const remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
  let id: string;
  try {
    id = (await remote.createSession(await fixture.workspaceHost.host.resolveWorkspace(workspace.id, fixture.endpoint))).session.id;
    // These scenarios test primary execution; a saved native label suppresses
    // independent automatic Title work without hiding any provider request.
    if (!fixture.automaticTitles) await remote.renameSession(id, 'New session');
  }
  finally { await remote.shutdown(); }
  await returnHome(page);
  const expand = page.getByRole('button', { name: 'Home', exact: true, expanded: false });
  const wasCollapsed = await expand.isVisible();
  if (wasCollapsed) await expand.click();
  await page.getByRole('button', { name: 'View options', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Refresh list', exact: true }).click();
  const overlay = await page.locator('[data-harness-frame]').getAttribute('data-sidebar-overlay') === 'true';
  await page.locator(`button[data-session-id="${id}"]`).click();
  if (overlay) {
    await expect(page.locator('[data-harness-frame]')).not.toHaveAttribute('data-sidebar-overlay');
    await expect(page.locator('main')).not.toHaveAttribute('inert', '');
    await expand.click(); // Inspect the selected row after the navigation dismissed its overlay.
  }
  // Native fixture selection includes reading the Workspace's authored files.
  // Await its completion within the test deadline; a 16 MiB Workflow does
  // not impose the presentation assertions' default five-second load budget.
  await page.locator('#session-view[data-shell-route="session"]').waitFor({ state: 'visible' });
  await expect(page.locator(`button[data-session-id="${id}"]`)).toHaveAttribute('aria-current', 'page');
  await expect(page.getByLabel('Message', { exact: true })).toBeEnabled();
  await expect(page.getByLabel('Session location', { exact: true })).toHaveCount(0);
  if (wasCollapsed) await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
  return id;
}

/** Reach advanced model authoring through the landing page disclosures. */
export async function expandModelAuthoring(page: Page) {
  for (const name of ['Default model for new Sessions']) {
    const toggle = page.getByRole('button', { name, exact: true, expanded: false });
    if (await toggle.isVisible()) await toggle.click();
  }
}

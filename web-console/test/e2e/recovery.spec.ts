import { expandModelAuthoring } from './shell-actions';
import { openEmptySession } from './shell-actions';
import { connectionAction, connectRemote, openSettingsPage, selectedSettingsPage, settingsSectionMenu } from './shell-actions';
import { expect, test } from '@playwright/test';
import { readFileSync, appendFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { wireProbe } from './wire-probe';

test('CFG3 committed write and reconciliation response loss reconstructs native state without replay', { tag: '@smoke' }, async ({ page }) => {
  const fixture = await startDogfood(); const wire = await wireProbe(page);
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await openEmptySession(page, fixture, 'Workspace A');
    await expect(page.getByLabel('Message', { exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    const model = page.getByRole('dialog', { name: 'Model fixture/console-model', exact: true });
    expect(await selectedSettingsPage(page)).toBe('General');
    const writes = () => wire.requests.filter(request => request.method === 'configuration/sourceWrite');
    const reconnect = async () => {
      await expect(page.locator('button[data-phase=disconnected]')).toBeVisible();
      if (await model.isVisible()) await model.getByRole('button', { name: 'Close', exact: true }).click();
      // The retained model focus can reopen a modal after reconnect. Stay on
      // Advanced until the authoritative reread is complete, rather than
      // asking connectionAction to inspect a tab behind that modal.
      await openSettingsPage(page, 'Advanced');
      await connectionAction(page, 'Reconnect');
      await expect(page.getByLabel('Transport token')).toHaveCount(0);
      await settings.getByRole('button', { name: 'Reload configuration', exact: true }).click();
    };
    const openModel = async () => {
      const menu = settingsSectionMenu(page);
      if (await menu.isVisible()) {
        await menu.click();
        await page.getByRole('menu').getByRole('menuitem', { name: 'Models', exact: true }).click();
      } else await page.getByRole('tablist', { name: 'Settings pages', exact: true }).getByRole('tab', { name: 'Models', exact: true }).click();
      // Returning to Models restores its retained editor. The background
      // tabpanel remains rendered but is aria-hidden while the modal is open.
      await expect(page.getByRole('tabpanel', { name: 'Models', exact: true, includeHidden: true })).toBeVisible();
      if (await model.isVisible()) {
        await expect(model.getByRole('heading', { name: 'Edit Model', exact: true })).toBeVisible();
        return;
      }
      await expandModelAuthoring(page);
      if (await settings.getByRole('button', { name: '← Models', exact: true }).isVisible()) await settings.getByRole('button', { name: '← Models', exact: true }).click();
      await settings.getByRole('button', { name: /^All Models/ }).click();
      await settings.getByRole('grid', { name: 'All Models', exact: true }).getByRole('row', { name: 'fixture/console-model', exact: true }).click();
      await expect(model.getByRole('heading', { name: 'Edit Model', exact: true })).toBeVisible();
    };
    const openMcp = async () => {
      if (await model.isVisible()) await model.getByRole('button', { name: 'Close', exact: true }).click();
      await openSettingsPage(page, 'MCP servers');
      const back = settings.getByRole('button', { name: '← MCP servers', exact: true });
      if (await back.isVisible()) await back.click();
      await openSettingsPage(page, 'MCP servers');
    };
    await openModel();
    await model.getByLabel('Context window').fill('250000');
    appendFileSync(fixture.settings, '\n# external revision before save\n');
    await model.getByRole('button', { name: 'Save Model fixture/console-model', exact: true }).click();
    await expect(model.getByRole('alert')).toContainText('was not saved: the source changed');
    await expect(model.getByLabel('Context window')).toHaveValue('250000');
    expect(readFileSync(fixture.settings, 'utf8')).not.toContain('250000');
    expect(writes()).toHaveLength(1);
    await model.getByRole('button', { name: 'Use reviewed revision', exact: true }).click();
    wire.loseNext('configuration/sourceWrite');
    await model.getByRole('button', { name: 'Save Model fixture/console-model', exact: true }).click();
    await expect.poll(wire.lost).toBe(1);
    expect(readFileSync(fixture.settings, 'utf8')).toContain('250000');
    await reconnect();
    // Reopen the independent Model dialog after the authoritative native reread.
    await openModel();
    await expect(model.getByLabel('Context window')).toHaveValue('250000');
    expect(writes()).toHaveLength(2);
    await openMcp();
    await settings.getByRole('button', { name: '＋ New', exact: true }).click();
    await settings.getByLabel('Name', { exact: true }).fill('loss-fixture');
    await settings.getByLabel('Command', { exact: true }).fill('inert-fixture');
    wire.loseNext('configuration/sourceWrite');
    await settings.getByRole('button', { name: 'Save', exact: true }).click();
    await expect.poll(wire.lost).toBe(2);
    expect(readFileSync(join(fixture.directory, 'home/rustx/.agents/mcp.toml'), 'utf8')).toContain('inert-fixture');
    await reconnect();
    await openMcp();
    await settings.getByRole('button', { name: 'MCP loss-fixture', exact: true }).click();
    await expect(settings.getByLabel('Command', { exact: true })).toHaveValue('inert-fixture');
    expect(writes()).toHaveLength(3);
    wire.loseNext('configuration/reconcile');
    await openSettingsPage(page, 'Advanced');
    await settings.getByRole('button', { name: 'Rescan configuration files', exact: true }).click();
    await expect.poll(wire.lost).toBe(3); await reconnect();
    await expect(settings.getByText(/Preparing configuration/)).toHaveCount(0);
    expect(wire.requests.filter(request => request.method === 'configuration/reconcile')).toHaveLength(1);
    expect(writes()).toHaveLength(3);
    // A User draft remains User-owned across Session focus changes.
    await openMcp();
    await settings.getByRole('button', { name: 'MCP loss-fixture', exact: true }).click();
    await settings.getByLabel('Command', { exact: true }).fill('unsaved-draft');
    await openEmptySession(page, fixture, 'Workspace B');
    await expect(page.getByLabel('Session location', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    expect(await selectedSettingsPage(page)).toBe('General');
    await openMcp();
    await settings.getByRole('button', { name: 'MCP loss-fixture', exact: true }).click();
    await expect(settings.getByLabel('Command', { exact: true })).toHaveValue('unsaved-draft');
    expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toMatch(/250000|loss-fixture|unsaved-draft/);
  } finally { await page.close(); const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

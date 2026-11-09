import {openResourceModule} from './shell-actions';
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
    await expect(page.getByLabel('Access token')).toBeHidden();
    await openEmptySession(page, fixture, 'Workspace A');
    await expect(page.getByLabel('Message', { exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('region', { name: 'Settings', exact: true });
    const mcp=page.getByRole('region',{name:'MCP servers',exact:true});
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
      await expect(page.getByLabel('Access token')).toBeHidden();
      await settings.getByRole('button', { name: 'Reload configuration', exact: true }).click();
    };
    const openModel = async (mode: 'open' | 'restore') => {
      const menu = settingsSectionMenu(page);
      if (await menu.isVisible()) {
        await menu.click();
        await page.getByRole('menu').getByRole('menuitem', { name: 'Models', exact: true }).click();
      } else await page.getByRole('tablist', { name: 'Settings pages', exact: true }).getByRole('tab', { name: 'Models', exact: true }).click();
      // Returning to Models restores its retained editor. The background
      // tabpanel remains rendered but is aria-hidden while the modal is open.
      await expect(page.getByRole('tabpanel', { name: 'Models', exact: true, includeHidden: true })).toBeVisible();
      if (mode === 'restore') {
        await expect(model.getByRole('heading', { name: 'Edit Model', exact: true })).toBeVisible();
        return;
      }
      await expandModelAuthoring(page);
      await settings.getByRole('row', { name: 'fixture/console-model', exact: true }).getByRole('button', { name: 'Edit Model', exact: true }).click();
      await expect(model.getByRole('heading', { name: 'Edit Model', exact: true })).toBeVisible();
    };
    const openMcp = async () => {
      if (await model.isVisible()) await model.getByRole('button', { name: 'Close', exact: true }).click();
      // A retained editor hides the scope tabs. Return to the catalog before
      // selecting its scope, preserving the editor's transaction on the way.
      await openResourceModule(page,'mcp');
      await expect(mcp.locator('[data-mcp-page]')).toBeVisible();
      const back = mcp.getByRole('button', { name: '← MCP servers', exact: true });
      const create = mcp.getByRole('button', { name: '＋ New MCP server', exact: true });
      // Reconnecting can restore the retained editor after the tabpanel mounts.
      // Wait for its actual surface before deciding whether to return to the list.
      await expect(back.or(create).first()).toBeVisible();
      if (await back.isVisible()) { await back.click(); await expect(back).toHaveCount(0); }
      await mcp.getByRole('tab', { name: 'Global', exact: true }).click();
      await expect(create).toBeVisible();
    };
    await openModel('open');
    await model.getByLabel('Context window').fill('250000');
    appendFileSync(fixture.settings, '\n# external revision before save\n');
    await model.getByRole('button', { name: 'Save Model fixture/console-model', exact: true }).click();
    await expect(page.locator('[data-toast][data-foreground]').filter({ hasText: 'was not saved because it changed elsewhere' })).toBeVisible();
    await expect(model.getByLabel('Context window')).toHaveValue('250000');
    expect(readFileSync(fixture.settings, 'utf8')).not.toContain('250000');
    expect(writes()).toHaveLength(1);
    await model.getByRole('button', { name: 'Use latest setting', exact: true }).click();
    wire.loseNext('configuration/sourceWrite');
    await model.getByRole('button', { name: 'Save Model fixture/console-model', exact: true }).click();
    await expect.poll(wire.lost).toBe(1);
    expect(readFileSync(fixture.settings, 'utf8')).toContain('250000');
    await reconnect();
    // Reopen the independent Model dialog after the authoritative native reread.
    await openModel('restore');
    await expect(model.getByLabel('Context window')).toHaveValue('250000');
    expect(writes()).toHaveLength(2);
    await openMcp();
    await mcp.getByRole('button', { name: '＋ New MCP server', exact: true }).click();
    await mcp.getByLabel('Name', { exact: true }).fill('loss-fixture');
    await mcp.getByLabel('Command', { exact: true }).fill('inert-fixture');
    wire.loseNext('configuration/sourceWrite');
    await mcp.getByRole('button', { name: 'Save', exact: true }).click();
    await expect.poll(wire.lost).toBe(2);
    expect(readFileSync(join(fixture.directory, 'home/rustx/.agents/mcp.toml'), 'utf8')).toContain('inert-fixture');
    await reconnect();
    await openMcp();
    await mcp.getByRole('button', { name: 'MCP loss-fixture', exact: true }).click();
    await expect(mcp.getByLabel('Command', { exact: true })).toHaveValue('inert-fixture');
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
    await mcp.getByRole('button', { name: 'MCP loss-fixture', exact: true }).click();
    await mcp.getByLabel('Command', { exact: true }).fill('unsaved-draft');
    await openEmptySession(page, fixture, 'Workspace B');
    await expect(page.getByLabel('Session location', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    expect(await selectedSettingsPage(page)).toBe('Advanced');
    await openMcp();
    await mcp.getByRole('button', { name: 'MCP loss-fixture', exact: true }).click();
    await expect(mcp.getByLabel('Command', { exact: true })).toHaveValue('unsaved-draft');
    expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toMatch(/250000|loss-fixture|unsaved-draft/);
  } finally { await page.close(); const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

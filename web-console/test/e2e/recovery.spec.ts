import { chooseWorkspace, connectionAction, connectRemote, openSettingsPage, selectedSettingsPage } from './shell-actions';
import { expect, test } from '@playwright/test';
import { readFileSync, appendFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { wireProbe } from './wire-probe';

test('CFG3 committed write and reconciliation response loss reconstructs native state without replay', async ({ page }) => {
  const fixture = await startDogfood(); const wire = await wireProbe(page);
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await chooseWorkspace(page, 'Workspace A');
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(page.getByLabel('Message', { exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    expect(await selectedSettingsPage(page)).toBe('General');
    const writes = () => wire.requests.filter(request => request.method === 'configuration/sourceWrite');
    const reconnect = async () => {
      await expect(page.getByLabel('Session status')).toContainText(/Connection interrupted|Needs verification/);
      await connectionAction(page, 'Reconnect');
      await expect(page.getByLabel('Transport token')).toHaveCount(0);
      await settings.getByRole('button', { name: 'Reload configuration', exact: true }).click();
    };
    const openModel = async () => {
      await openSettingsPage(page, 'Models');
      if (await settings.getByRole('button', { name: '← Models', exact: true }).isVisible()) await settings.getByRole('button', { name: '← Models', exact: true }).click();
      await settings.getByRole('button', { name: /^All Models/ }).click();
      await settings.getByRole('row', { name: 'fixture/console-model', exact: true }).click();
    };
    const openMcp = async () => {
      await openSettingsPage(page, 'Extensions');
      if (await settings.getByRole('button', { name: '← Extensions', exact: true }).isVisible()) await settings.getByRole('button', { name: '← Extensions', exact: true }).click();
      await settings.getByRole('tab', { name: 'MCP', exact: true }).click();
    };
    await openModel();
    await settings.getByLabel('Context window').fill('250000');
    appendFileSync(fixture.settings, '\n# external revision before save\n');
    await settings.getByRole('button', { name: 'Save Model fixture/console-model', exact: true }).click();
    await expect(settings.getByRole('alert')).toContainText('was not saved: the source changed');
    await expect(settings.getByLabel('Context window')).toHaveValue('250000');
    expect(readFileSync(fixture.settings, 'utf8')).not.toContain('250000');
    expect(writes()).toHaveLength(1);
    await settings.getByRole('button', { name: 'Use reviewed revision', exact: true }).click();
    wire.loseNext('configuration/sourceWrite');
    await settings.getByRole('button', { name: 'Save Model fixture/console-model', exact: true }).click();
    await expect.poll(wire.lost).toBe(1);
    expect(readFileSync(fixture.settings, 'utf8')).toContain('250000');
    await reconnect();
    // Reconnecting returned to the Model detail the Models page kept focused.
    await expect(settings.getByRole('heading', { name: 'Model fixture/console-model', exact: true })).toBeVisible();
    await expect(settings.getByLabel('Context window')).toHaveValue('250000');
    expect(writes()).toHaveLength(2);
    await openMcp();
    await settings.getByLabel('New MCP identity').fill('loss-fixture');
    await settings.getByRole('button', { name: 'Add MCP', exact: true }).click();
    await settings.getByLabel('MCP command', { exact: true }).fill('inert-fixture');
    wire.loseNext('configuration/sourceWrite');
    await settings.getByRole('button', { name: 'Save MCP loss-fixture', exact: true }).click();
    await expect.poll(wire.lost).toBe(2);
    expect(readFileSync(join(fixture.directory, 'home/rustx/.agents/mcp.toml'), 'utf8')).toContain('inert-fixture');
    await reconnect();
    await openMcp();
    await settings.getByRole('row', { name: 'loss-fixture', exact: true }).click();
    await expect(settings.getByLabel('MCP command', { exact: true })).toHaveValue('inert-fixture');
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
    await settings.getByRole('row', { name: 'loss-fixture', exact: true }).click();
    await settings.getByLabel('MCP command', { exact: true }).fill('unsaved-draft');
    await chooseWorkspace(page, 'Workspace B');
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(page.getByLabel('Session location', { exact: true })).toContainText(fixture.workspaceB);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    expect(await selectedSettingsPage(page)).toBe('General');
    await openMcp();
    await settings.getByRole('row', { name: 'loss-fixture', exact: true }).click();
    await expect(settings.getByLabel('MCP command', { exact: true })).toHaveValue('unsaved-draft');
    expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toMatch(/250000|loss-fixture|unsaved-draft/);
  } finally { await page.close(); const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

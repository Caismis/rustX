import { expect, test } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, enableWorkspaceCustomization, openSettingsPage, openWorkspaceSettings, settledSetting } from './shell-actions';

// Model/provider creation and shared API format have their own real-native
// coverage in model-authoring.spec.ts. This flow covers the other settings.
test('Workspace settings save permissions and features, and keep configuration repair actionable without diagnostics dumps', async ({ page }) => {
  const fixture = await startDogfood();
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openWorkspaceSettings(page, 'Workspace A');
    const settings = page.getByRole('region', { name: 'Settings', exact: true });
    await openSettingsPage(page, 'Agent');
    await enableWorkspaceCustomization(page);
    await expect(settings.getByRole('form', { name: 'Global instructions', exact: true })).toHaveCount(0);
    const guidance = settings.getByRole('form', { name: 'Project guidance', exact: true });
    await guidance.getByRole('switch', { name: 'Automatically load project guidance' }).uncheck();
    await settledSetting(guidance);
    expect(readFileSync(`${fixture.workspaceA}/rustx.toml`, 'utf8')).toContain('inherit = false');
    await expect(settings.getByRole('form', { name: 'Instructions', exact: true })).toHaveCount(0);

    await openSettingsPage(page, 'Tools & Permissions');
    await enableWorkspaceCustomization(page);
    const tools = settings.getByRole('form', { name: 'Built-in tools', exact: true });
    await tools.getByLabel('read', { exact: true }).uncheck();
    await settledSetting(tools);
    await expect(settings.getByRole('form',{name:'Available skills',exact:true})).toHaveCount(0);
    await expect(settings.getByRole('note', { name: 'Agent controls' })).toHaveCount(0);

    await openSettingsPage(page, 'Extensions');
    await settings.getByRole('tab', { name: 'Built-in', exact: true }).click();
    await enableWorkspaceCustomization(page);
    const todo = settings.getByRole('switch', { name: 'Enable Todo', exact: true });
    await todo.locator('xpath=ancestor::label').click();
    await settledSetting(todo.locator('xpath=ancestor::form'));
    await expect(todo).toBeEnabled();
    await expect(todo).toBeChecked();

    await openSettingsPage(page, 'Advanced');
    await expect(settings.getByRole('button', { name: 'Configuration details', exact: true })).toHaveCount(0);
    await expect(settings.getByRole('form', { name: 'App Server policy', exact: true })).toHaveCount(0);
    await expect(settings.getByLabel('Source and application projection')).toHaveCount(0);
    await expect(settings.getByText(/Revision:|Draft base revision:/)).toHaveCount(0);
    const file = `${fixture.workspaceA}/rustx.toml`;
    const authored = readFileSync(file, 'utf8');
    writeFileSync(file, 'invalid = [');
    await settings.getByRole('button', { name: 'Rescan configuration files', exact: true }).click();
    await expect(settings.getByRole('form', { name: 'Repair invalid source', exact: true })).toBeVisible();
    await expect(page.locator('[data-toast][data-foreground]').filter({ hasText: 'malformed TOML' }).first()).toBeVisible();
    await settings.screenshot({ path: test.info().outputPath('settings-repair-needed.png') });
    writeFileSync(file, authored);
    await settings.getByRole('button', { name: 'Rescan configuration files', exact: true }).click();
    // The rescan's reread must land before customizing: its adoption re-derives
    // the follow-global state and would drop a customization staged mid-flight.
    await expect(settings.locator('[data-lifecycle="ready"]')).toBeVisible();
    await enableWorkspaceCustomization(page);
    await expect(settings.getByRole('form', { name: 'Context', exact: true })).toBeVisible();
    await expect(settings.getByRole('form', { name: 'Repair invalid source', exact: true })).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally {
    const report = await stopDogfoodPage(page, fixture, false);
    expect(report.requestCount).toBe(0);
  }
});

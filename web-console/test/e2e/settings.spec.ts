import { expect, test } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openSettingsPage, openWorkspaceSettings } from './shell-actions';

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
    const saved = async (unit: string) => expect(settings.getByText(`${unit} saved.`, { exact: true })).toBeVisible();
    await openSettingsPage(page, 'Agent');
    const instructions = settings.getByRole('form', { name: 'Instructions', exact: true });
    await instructions.getByLabel('Instructions', { exact: true }).fill('Follow the project conventions.');
    await instructions.getByRole('button', { name: 'Save Instructions', exact: true }).click();
    await saved('Instructions');
    expect(readFileSync(`${fixture.workspaceA}/rustx.toml`, 'utf8')).toContain('Follow the project conventions.');

    await openSettingsPage(page, 'Tools & Permissions');
    const tools = settings.getByRole('form', { name: 'Built-in tools', exact: true });
    await tools.getByLabel('read', { exact: true }).uncheck();
    await tools.getByRole('button', { name: 'Save Built-in tools', exact: true }).click();
    await saved('Built-in tools');
    await expect(settings.getByRole('form',{name:'Available skills',exact:true})).toHaveCount(0);
    await expect(settings.getByRole('note', { name: 'Agent controls' })).toHaveCount(0);

    await openSettingsPage(page, 'Extensions');
    await settings.getByRole('tab', { name: 'Built-in', exact: true }).click();
    const todo = settings.getByRole('switch', { name: 'Enable Todo', exact: true });
    await todo.locator('xpath=ancestor::label').click();
    await settings.getByRole('button', { name: 'Save Todo extension', exact: true }).click();
    await saved('Todo extension');
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
    await expect(settings.getByRole('alert').filter({ hasText: 'invalid rustx.toml' }).first()).toBeVisible();
    await settings.screenshot({ path: test.info().outputPath('settings-repair-needed.png') });
    writeFileSync(file, authored);
    await settings.getByRole('button', { name: 'Rescan configuration files', exact: true }).click();
    await expect(settings.getByRole('form', { name: 'Context', exact: true })).toBeVisible();
    await expect(settings.getByRole('form', { name: 'Repair invalid source', exact: true })).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally {
    const report = await fixture.stop(false);
    expect(report.requestCount).toBe(0);
  }
});

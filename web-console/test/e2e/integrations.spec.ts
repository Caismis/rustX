import { expandModelAuthoring } from './shell-actions';
import { openEmptySession } from './shell-actions';
import {
  choose, closeSettings, connectRemote, openSettingsPage, openWorkspaceSettings, selectedSettingsPage,
} from './shell-actions';
import { expect, test } from '@playwright/test';
import { appendFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';

test('CFG3 structured source authoring, inert definitions, CAS and automatic no-op application', async ({ page }) => {
  const fixture = await startDogfood(); const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await openEmptySession(page, fixture, 'Workspace A');
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    expect(await selectedSettingsPage(page)).toBe('General');
    // The source path is a diagnostic: it is on Advanced.
    await openSettingsPage(page, 'Advanced');
    await expect(settings).toContainText(fixture.settings);
    await openSettingsPage(page, 'Extensions');
    await settings.getByRole('tab', { name: 'MCP', exact: true }).click();
    await settings.getByLabel('New MCP identity').fill('local-fixture');
    await settings.getByRole('button', { name: 'Add MCP', exact: true }).click();
    await settings.getByLabel('MCP command').fill('python3');
    await settings.getByRole('button', { name: 'Add Arguments', exact: true }).click();
    await settings.getByLabel('Arguments 1', { exact: true }).fill(fileURLToPath(new URL('./web09-mcp.py', import.meta.url)));
    await settings.getByRole('button', { name: 'Add Arguments', exact: true }).click();
    await settings.getByLabel('Arguments 2', { exact: true }).fill(join(fixture.directory, 'mcp-started'));
    await settings.getByRole('button', { name: 'Save MCP local-fixture', exact: true }).click();
    await expect(settings.getByText('MCP local-fixture saved. Native coordination owns application.')).toBeVisible();
    expect(existsSync(join(fixture.directory, 'mcp-started'))).toBe(false);
    await openSettingsPage(page, 'Advanced');
    await expect(settings.getByText(/Revision:/)).toBeVisible();
    expect(existsSync(join(fixture.directory, 'mcp-started'))).toBe(false);
    await closeSettings(page);
    await openWorkspaceSettings(page, 'Workspace A'); await expandModelAuthoring(page);
    await openSettingsPage(page, 'Extensions');
    await settings.getByRole('tab', { name: 'MCP', exact: true }).click();
    // The User definition is the native effective one for this identity, so the
    // Workspace inventory lists it as inherited instead of hiding it; it is
    // reachable without retyping the identity, and it offers no removal.
    const inherited = settings.getByRole('row', { name: 'local-fixture', exact: true });
    await expect(inherited).toContainText('Inherited from User');
    await inherited.click();
    // Viewing the inherited User definition is not Workspace authoring: its
    // safe native facts are shown read-only, nothing can be saved or removed,
    // and nothing is written.
    const definition = settings.getByRole('form', { name: 'MCP local-fixture', exact: true });
    await expect(definition).toHaveAttribute('data-definition', 'inherited');
    await expect(settings.getByLabel('MCP command')).toHaveValue('python3');
    await expect(settings.getByLabel('MCP command')).toBeDisabled();
    await expect(settings.getByRole('button', { name: 'Save MCP local-fixture', exact: true })).toBeDisabled();
    await expect(settings.getByRole('button', { name: 'Use global default MCP local-fixture', exact: true })).toHaveCount(0);
    // Only the explicit override begins a Workspace definition. It replaces the
    // whole User one when saved, against the real native Workspace document.
    await settings.getByRole('button', { name: 'Override MCP local-fixture in this Workspace', exact: true }).click();
    await expect(definition).toHaveAttribute('data-definition', 'overriding');
    await settings.getByLabel('MCP command').fill('unused-workspace-command');
    await settings.getByRole('button', { name: 'Save MCP local-fixture', exact: true }).click();
    await expect(settings.getByText('MCP local-fixture saved. Native coordination owns application.')).toBeVisible();
    await expect(definition).toHaveAttribute('data-definition', 'authored');
    await settings.getByLabel('MCP command').fill('preserved-draft');
    appendFileSync(join(fixture.workspaceA, '.agents/mcp.toml'), '\n# external edit invalidates the draft revision\n');
    await settings.getByRole('button', { name: 'Save MCP local-fixture', exact: true }).click();
    await expect(settings.getByRole('alert')).toContainText('was not saved: the source changed');
    await expect(settings.getByLabel('MCP command')).toHaveValue('preserved-draft');
    await settings.screenshot({ path: test.info().outputPath('cfg3-cas-conflict.png') });
    await settings.getByRole('button', { name: 'Use reviewed revision', exact: true }).click();
    await settings.getByRole('button', { name: 'Save MCP local-fixture', exact: true }).click();
    await expect(settings.getByText('MCP local-fixture saved. Native coordination owns application.')).toBeVisible();
    await settings.getByRole('button', { name: '← Extensions', exact: true }).click();
    await settings.getByRole('tab', { name: 'Agents', exact: true }).click();
    await settings.getByLabel('New Agent identity').fill('reviewer');
    await settings.getByRole('button', { name: 'Add Agent', exact: true }).click();
    await settings.getByLabel('Description', { exact: true }).fill('Independent review profile');
    await settings.getByRole('button', { name: 'Extensions, worktree and guidance', exact: true }).click();
    await expect(settings.getByLabel('todo', { exact: true })).not.toBeChecked();
    await settings.getByLabel('Instructions', { exact: true }).fill('Review the requested change and report concrete findings.');
    await settings.getByLabel('read', { exact: true }).check();
    await settings.getByRole('button', { name: 'Save Agent reviewer', exact: true }).click();
    await expect(settings.getByText('Agent reviewer saved. Native coordination owns application.')).toBeVisible();
    await settings.getByRole('button', { name: '← Extensions', exact: true }).click();
    await expect(settings.getByRole('row', { name: 'reviewer', exact: true })).toContainText('Valid definition');
    await settings.screenshot({ path: test.info().outputPath('cfg3-named-agent.png') });
    await openSettingsPage(page, 'Advanced');
    await expect(settings.getByText(/Revision:/)).toBeVisible();
    await expect(settings.getByRole('button', { name: 'Adopt prepared context', exact: true })).toHaveCount(0);
    await page.setViewportSize({ width: 390, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: test.info().outputPath('cfg3-settings-mobile.png'), fullPage: true });
    // A Workspace surface has no General page: appearance is set globally, from
    // the desktop layout where the Workspace object action is reachable.
    await page.setViewportSize({ width: 1440, height: 1000 });
    await closeSettings(page);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await choose(settings, 'Theme', 'Dark');
    await closeSettings(page);
    await openWorkspaceSettings(page, 'Workspace A'); await expandModelAuthoring(page);
    await openSettingsPage(page, 'Extensions');
    await settings.getByRole('tab', { name: 'Agents', exact: true }).click();
    await settings.getByRole('row', { name: 'reviewer', exact: true }).click();
    await page.setViewportSize({ width: 390, height: 844 });
    await settings.getByRole('form', { name: 'Agent reviewer', exact: true }).evaluate(el => el.scrollIntoView({ block: 'start' }));
    await page.screenshot({ path: test.info().outputPath('cfg3-agent-mobile-dark.png') });
    expect(errors).toEqual([]);
    expect(existsSync(join(fixture.directory, 'mcp-started'))).toBe(false);
  } finally { const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

import { formatArguments } from '../../src/app/resources/mcp/form-values';
import {openResourceModule,newSubagent} from './shell-actions';
import { expandModelAuthoring } from './shell-actions';
import { openEmptySession } from './shell-actions';
import {
  returnHome, connectRemote, openSettingsPage, openWorkspaceSettings, selectedSettingsPage,
} from './shell-actions';
import { expect, test } from '@playwright/test';
import { appendFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';

test('CFG3 structured source authoring, inert definitions, CAS and automatic no-op application', async ({ page }) => {
  const fixture = await startDogfood(); const errors: string[] = [];
  let invalidateNextWrite = false;
  page.on('pageerror', error => errors.push(error.message));
  try {
    await routeWorkspaceHost(page, fixture, {
      before: async () => {
        if (!invalidateNextWrite) return;
        invalidateNextWrite = false;
        appendFileSync(join(fixture.workspaceA, '.agents/mcp.toml'), '\n# external edit invalidates the draft revision\n');
      },
      observed: async () => {},
    }); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByLabel('Access token')).toBeHidden();
    await openEmptySession(page, fixture, 'Workspace A');
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    let settings = page.getByRole('region', { name: 'Settings', exact: true });
    expect(await selectedSettingsPage(page)).toBe('General');
    // Ordinary settings keep internal source paths out of the page.
    await openSettingsPage(page, 'Advanced');
    await expect(settings).not.toContainText(fixture.settings);
    settings=await openResourceModule(page,'mcp','Global');
    await settings.getByRole('button', { name: '＋ New', exact: true }).click();
    await settings.getByLabel('Name', { exact: true }).fill('local-fixture');
    await settings.getByLabel('Command', { exact: true }).fill('python3');
    await settings.getByLabel('Arguments (space-separated)',{exact:true}).fill(formatArguments([fileURLToPath(new URL('./web09-mcp.py', import.meta.url)), join(fixture.directory, 'mcp-started')]));
    await settings.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(settings.getByRole('listitem', { name: 'local-fixture', exact: true })).toBeVisible();
    expect(existsSync(join(fixture.directory, 'mcp-started'))).toBe(false);
    await openSettingsPage(page, 'Advanced');
    settings=page.getByRole('region',{name:'Settings',exact:true});
    await expect(settings.locator('[data-lifecycle=ready]')).toBeVisible();
    expect(existsSync(join(fixture.directory, 'mcp-started'))).toBe(false);
    await returnHome(page);
    await openWorkspaceSettings(page, 'Workspace A'); await expandModelAuthoring(page);
    settings=await openResourceModule(page,'mcp','Workspace A');
    await settings.getByRole('button',{name:'MCP local-fixture',exact:true}).click();
    // Viewing the inherited User definition is not Workspace authoring: its
    // safe native facts are shown read-only, nothing can be saved or removed,
    // and nothing is written.
    await expect(settings.getByLabel('Command', { exact: true })).toHaveValue('python3');
    await expect(settings.getByLabel('Command', { exact: true })).toBeDisabled();
    await expect(settings.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
    await expect(settings.getByRole('button', { name: 'Use global default MCP local-fixture', exact: true })).toHaveCount(0);
    // Only the explicit override begins a Workspace definition. It replaces the
    // whole User one when saved, against the real native Workspace document.
    await settings.getByRole('button', { name: 'Override', exact: true }).click();
    await settings.getByLabel('Command', { exact: true }).fill('unused-workspace-command');
    await settings.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(settings.getByRole('listitem', { name: 'local-fixture', exact: true })).toBeVisible();
    await settings.getByRole('button', { name: 'MCP local-fixture', exact: true }).click();
    await settings.getByLabel('Command', { exact: true }).fill('preserved-draft');
    // The reviewed draft must be submitted before the external edit. An edit
    // before clicking can instead invalidate a publication-owned read and
    // correctly prevent submission, which never exercises the native write CAS.
    invalidateNextWrite = true;
    await settings.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(settings.getByRole('alert')).toContainText('was not saved because it changed elsewhere');
    expect(invalidateNextWrite).toBe(false);
    await expect(settings.getByLabel('Command', { exact: true })).toHaveValue('preserved-draft');
    await settings.screenshot({ path: test.info().outputPath('cfg3-cas-conflict.png') });
    await settings.getByRole('button', { name: 'Use latest setting', exact: true }).click();
    await settings.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(settings.getByRole('listitem', { name: 'local-fixture', exact: true })).toBeVisible();
    settings=await openResourceModule(page,'subagents','Workspace A');
    await newSubagent(page,settings,'reviewer');
    await settings.getByLabel('Description', { exact: true }).fill('Independent review profile');
    await settings.getByRole('button', { name: 'Extensions and guidance', exact: true }).click();
    await expect(settings.getByLabel('todo', { exact: true })).not.toBeChecked();
    await settings.getByLabel('Instructions', { exact: true }).fill('Review the requested change and report concrete findings.');
    await settings.getByLabel('read', { exact: true }).check();
    await settings.getByRole('button', { name: 'Save Agent reviewer', exact: true }).click();
    await expect(settings.getByText('Agent reviewer saved.')).toBeVisible();
    await settings.getByRole('button', { name: 'Back to list', exact: true }).click();
    await settings.getByRole('listitem',{name:'reviewer',exact:true}).getByRole('button',{name:'reviewer',exact:true}).click();
    await expect(settings.getByLabel('Description',{exact:true})).toHaveValue('Independent review profile');
    await settings.getByRole('button',{name:'Back to list',exact:true}).click();
    await settings.screenshot({ path: test.info().outputPath('cfg3-named-agent.png') });
    await openSettingsPage(page, 'Advanced');
    settings=page.getByRole('region',{name:'Settings',exact:true});
    await expect(settings.locator('[data-lifecycle=ready]')).toBeVisible();
    await expect(settings.getByRole('button', { name: 'Adopt prepared context', exact: true })).toHaveCount(0);
    await page.setViewportSize({ width: 390, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: test.info().outputPath('cfg3-settings-mobile.png'), fullPage: true });
    // A Workspace surface has no General page: appearance is set globally, from
    // the desktop layout where the Workspace object action is reachable.
    await page.setViewportSize({ width: 1440, height: 1000 });
    await returnHome(page);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    settings=page.getByRole('region',{name:'Settings',exact:true});
    await settings.getByRole('button', { name: 'Open User Settings', exact: true }).click();
    await settings.getByRole('group', { name: 'Appearance', exact: true }).getByRole('button', { name: 'Dark', exact: true }).click();
    await returnHome(page);
    await openWorkspaceSettings(page, 'Workspace A'); await expandModelAuthoring(page);
    settings=await openResourceModule(page,'subagents','Workspace A');
    await settings.getByRole('listitem',{name:'reviewer',exact:true}).getByRole('button',{name:'reviewer',exact:true}).click();
    await page.setViewportSize({ width: 390, height: 844 });
    await settings.getByRole('form', { name: 'Agent reviewer', exact: true }).evaluate(el => el.scrollIntoView({ block: 'start' }));
    await page.screenshot({ path: test.info().outputPath('cfg3-agent-mobile-dark.png') });
    expect(errors).toEqual([]);
    expect(existsSync(join(fixture.directory, 'mcp-started'))).toBe(false);
  } finally { const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

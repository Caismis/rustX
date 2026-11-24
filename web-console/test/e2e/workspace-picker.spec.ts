import { expect, test } from '@playwright/test';
import { mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { connectRemote } from './shell-actions';
import { captureEvidence } from './screenshot';
import { routeWorkspaceHost, startWorkspaceHost } from './workspace-host';

test('Add Workspace registers a Client-verified folder from both entry points', async ({page}) => {
  const fixture = await startDogfood('web_console_dogfood', undefined, true);
  const path = join(fixture.workspaceA, 'Added project');
  mkdirSync(path);
  const secondPath = join(fixture.workspaceA, 'Composer project');
  mkdirSync(secondPath);
  const metadataFile = join(fixture.directory, 'folder-picker-workspaces.json');
  const service = await startWorkspaceHost({ ...JSON.parse(readFileSync(fixture.hostConfigFile, 'utf8')), metadataFile,
    browsePicker: true });
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    expect((await service.host.listWorkspaces()).picker.kind).toBe('browser');
    await routeWorkspaceHost(page, {workspaceHostUrl: service.url});
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page).toHaveTitle(/rustX/);
    const sidebar = page.getByRole('region', {name: 'Workspaces and Sessions'});
    const add = sidebar.getByRole('button', {name: 'Add Workspace', exact: true});
    const dialog = page.getByRole('dialog', {name: 'Add Workspace', exact: true});
    const addFromBrowser = async (name: string) => {
      await dialog.getByRole('button', {name: 'Workspace A', exact: true}).click();
      await dialog.getByRole('button', {name, exact: true}).click();
      await expect(dialog.getByRole('status')).toHaveText('No subfolders');
      await expect(dialog.getByRole('button', {name: 'Up one level'})).toBeVisible();
      if (name === 'Added project') {
        await dialog.getByRole('button', {name: 'Up one level'}).click();
        await expect(dialog.getByRole('button', {name, exact: true})).toBeVisible();
        await captureEvidence(dialog, 'workspace-folder-browser');
        await dialog.getByRole('button', {name, exact: true}).click();
      } else await captureEvidence(dialog, 'workspace-folder-composer');
      await dialog.getByRole('button', {name: 'Add folder', exact: true}).click();
      await expect(dialog).toHaveCount(0);
    };
    const draft = page.getByRole('textbox', {name: 'Message'});
    await draft.fill('Draft before adding a workspace');
    await add.click();
    await addFromBrowser('Added project');
    const addedWorkspace = sidebar.getByRole('button', {name: 'Select Workspace Added project'});
    const chooser = page.getByRole('button', {name: 'Choose Workspace', exact: true});
    await addedWorkspace.click();
    await expect(chooser).toContainText('Added project');
    await expect(draft).toHaveValue('Draft before adding a workspace');
    await expect(page.getByRole('button', {name: 'Send', exact: true})).toBeEnabled();
    const added = (await service.host.listWorkspaces()).workspaces.find(row => row.displayName === 'Added project')!;
    expect(await service.host.resolveWorkspace(added.id, fixture.endpoint)).toEqual({cwd: path});
    expect(readFileSync(metadataFile, 'utf8')).toContain(path);

    await page.setViewportSize({width: 390, height: 844});
    await chooser.click();
    await page.getByRole('menuitem', {name: '+ Add Workspace', exact: true}).click();
    await addFromBrowser('Composer project');
    await expect(chooser).toContainText('Composer project');
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally {
    try { await page.close(); }
    finally { await service.stop(); await fixture.stop(false); }
  }
});

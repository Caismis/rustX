import { expect, test } from '@playwright/test';
import { mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { DesktopAdapter } from '../../host/desktop';
import { startDogfood } from './dogfood-server';
import { connectRemote } from './shell-actions';
import { captureEvidence } from './screenshot';
import { routeWorkspaceHost, startWorkspaceHost } from './workspace-host';

for (const mode of ['native', 'browser'] as const) test(`Add Workspace registers a folder with the ${mode} picker`, async ({page}) => {
  const fixture = await startDogfood();
  const path = join(mode === 'native' ? fixture.directory : fixture.workspaceA, 'Added project');
  mkdirSync(path);
  const secondPath = join(mode === 'native' ? fixture.directory : fixture.workspaceA, 'Composer project');
  mkdirSync(secondPath);
  const answers = [null, path, secondPath];
  const desktop = new DesktopAdapter({
    platform: 'linux', env: { PATH: '/usr/bin', ...(mode === 'native' ? { DISPLAY: ':fixture' } : {}) },
    executable: value => value === '/usr/bin/zenity' ? value : undefined,
    launch: async () => { throw new Error('Unexpected launcher request'); },
    pickFolder: async () => { if (!answers.length) throw new Error('Unexpected chooser'); return answers.shift()!; },
  });
  const metadataFile = join(fixture.directory, 'folder-picker-workspaces.json');
  const service = await startWorkspaceHost({ ...JSON.parse(readFileSync(fixture.hostConfigFile, 'utf8')), metadataFile,
    nativePicker: true, browsePicker: true }, desktop);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    expect((await service.host.listWorkspaces()).picker.kind).toBe(mode);
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
    if (mode === 'native') {
      const canceled = page.waitForResponse(response => response.url().endsWith('/product-host/pick-workspace'));
      await add.click();
      expect(await (await canceled).json()).toBeNull();
      await expect(add).toBeEnabled();
      await expect(sidebar.getByRole('button', {name: 'Select Workspace Added project'})).toHaveCount(0);
    }
    await add.click();
    if (mode === 'browser') await addFromBrowser('Added project');
    else await expect(dialog).toHaveCount(0);
    await expect(sidebar.getByRole('button', {name: 'Select Workspace Added project'})).toBeVisible();
    const added = (await service.host.listWorkspaces()).workspaces.find(row => row.displayName === 'Added project')!;
    expect(await service.host.resolveWorkspace(added.id, fixture.endpoint)).toEqual({cwd: path});
    expect(readFileSync(metadataFile, 'utf8')).toContain(path);

    if (mode === 'browser') await page.setViewportSize({width: 390, height: 844});
    const chooser = page.getByRole('button', {name: 'Choose Workspace', exact: true});
    await chooser.click();
    await page.getByRole('menuitem', {name: '+ Add Workspace', exact: true}).click();
    if (mode === 'browser') await addFromBrowser('Composer project');
    await expect(chooser).toContainText('Composer project');
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally {
    try { await page.close(); }
    finally { await service.stop(); await fixture.stop(false); }
  }
});

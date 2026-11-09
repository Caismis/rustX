import { settledSave } from './shell-actions';
import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { chooseWorkspace, connectRemote, openEmptySession, openSettingsPage, returnHome } from './shell-actions';

for (const width of [1440, 390]) test(`provider single-click navigation and workspace model catalogs stay current at ${width}px`, async ({page}) => {
  const fixture = await startDogfood();
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', event => { if (['error', 'warning'].includes(event.type())) errors.push(event.text()); });
  let workspaceReads = 0;
  try {
    await page.setViewportSize({width, height:1000});
    await routeWorkspaceHost(page, fixture, undefined, request => {
      if (request.url().endsWith('/configuration') && request.postDataJSON()?.operation?.kind === 'read') workspaceReads++;
    });
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    // Establish a native published catalog and a resident composer before editing.
    await openEmptySession(page, fixture, 'Workspace A');
    await chooseWorkspace(page, 'Workspace A');
    const trigger = page.locator('[data-model-select]');
    await expect(trigger).toContainText('fixture/console-model');
    const draft = page.getByRole('textbox', {name:'Message',exact:true});
    await draft.fill('Keep this draft when visiting Settings');
    await page.getByRole('button', {name:'Settings',exact:true}).click();
    await openSettingsPage(page, 'Models');
    const settings = page.getByRole('region', {name:'Settings',exact:true});
    await settings.getByRole('button', {name:'+ Add Provider',exact:true}).click();
    const naming = page.getByRole('dialog', {name:'Add Provider',exact:true});
    await naming.getByLabel('New Provider identity', {exact:true}).fill('second');
    await naming.getByRole('button', {name:'Continue',exact:true}).click();
    const second = settings.getByRole('form', {name:'Provider second',exact:true});
    await second.getByLabel('Base URL', {exact:true}).fill(`${fixture.providerUrl}/v1`);
    await second.getByLabel('API Key', {exact:true}).fill('fixture-only');
    await second.getByRole('button', {name:'Save Provider second',exact:true}).click();
    await expect(settledSave(second, 'Provider second')).toBeVisible();
    await expect(second.getByRole('button', {name:'Save Provider second',exact:true})).toBeDisabled();
    // Every transition below is one mouse click, including returning to the first provider.
    await settings.getByRole('row', {name:'fixture',exact:true}).click();
    await expect(settings.getByRole('form', {name:'Provider fixture',exact:true})).toBeVisible();
    await settings.getByRole('row', {name:'second',exact:true}).click();
    await expect(second).toBeVisible();
    await settings.getByRole('row', {name:'fixture',exact:true}).click();
    const addModel = async (id: string) => {
      await settings.getByRole('button', {name:'+ Add Model',exact:true}).click();
      const dialog = page.getByRole('dialog', {name:'Add Model',exact:true});
      await dialog.getByLabel('Model ID', {exact:true}).fill(id);
      await dialog.getByRole('button', {name:'Save Add Model',exact:true}).click();
      await expect(dialog).toHaveCount(0);
      await expect(settings.getByRole('row', {name:`fixture/${id}`,exact:true})).toBeVisible();
    };
    await addModel('fresh-model');
    const readsBeforeReturn = workspaceReads;
    await returnHome(page);
    await expect(draft).toHaveValue('Keep this draft when visiting Settings');
    await expect.poll(() => workspaceReads).toBeGreaterThan(readsBeforeReturn);
    await trigger.click(); await page.getByRole('menuitem', {name:'Model',exact:true}).click();
    await expect(page.getByRole('menuitemradio', {name:'fixture/fresh-model',exact:true})).toBeVisible();
    await page.keyboard.press('Escape'); await page.keyboard.press('Escape');

    // A second edit also reaches a new draft in the same Workspace without a reload.
    await page.getByRole('button', {name:'Settings',exact:true}).click();
    await addModel('newest-model');
    await chooseWorkspace(page, 'Workspace A');
    await expect(draft).toHaveValue('Keep this draft when visiting Settings');
    await trigger.click(); await page.getByRole('menuitem', {name:'Model',exact:true}).click();
    await expect(page.getByRole('menuitemradio', {name:'fixture/fresh-model',exact:true})).toBeVisible();
    await page.getByRole('menuitemradio', {name:'fixture/newest-model',exact:true}).click();
    await expect(trigger).toContainText('fixture/newest-model');
    expect(readFileSync(fixture.settings, 'utf8')).toContain('newest-model');
    expect(errors).toEqual([]);
  } catch (error) {
    await test.info().attach('native-diagnostics', {body:JSON.stringify(fixture.diagnostics()),contentType:'application/json'});
    throw error;
  } finally { await fixture.stop(false); }
});

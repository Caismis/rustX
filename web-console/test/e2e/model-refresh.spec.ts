import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { chooseWorkspace, confirmSettingsAction, connectRemote, openEmptySession, openSettingsPage, returnHome } from './shell-actions';
import { wireProbe } from './wire-probe';

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
    // Every transition below is one mouse click, including returning to the first provider.
    await settings.getByRole('row', {name:'fixture',exact:true}).click();
    await expect(settings.getByRole('form', {name:'Provider fixture',exact:true})).toBeVisible();
    await expect.poll(() => readFileSync(fixture.settings, 'utf8')).toContain('[providers.second]');
    await settings.getByRole('row', {name:'second',exact:true}).click();
    await expect(second).toBeVisible();
    await expect(second.getByLabel('API Key', {exact:true})).toHaveValue('');
    await settings.getByRole('row', {name:'fixture',exact:true}).click();
    const endpoint = settings.getByRole('form', {name:'Provider fixture',exact:true}).getByLabel('Base URL', {exact:true});
    await endpoint.fill(`${fixture.providerUrl}/v1/`);
    await expect.poll(() => readFileSync(fixture.settings, 'utf8')).toContain(`base_url = "${fixture.providerUrl}/v1/"`);
    await expect(endpoint).toBeFocused();
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

test('deleting the default and then the last model preserves old conversations and gates new sends', async ({page}) => {
  const fixture = await startDogfood('web_model_deletion');
  const wire = await wireProbe(page);
  const native = await AppServerHost.connectRemote({endpoint:fixture.endpoint, token:fixture.token});
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', event => { if (['error', 'warning'].includes(event.type())) errors.push(event.text()); });
  try {
    const previous = (await native.createSession({cwd:fixture.workspaceB})).session;
    const preview = await native.previewSessionDeletion(previous.id);
    expect(preview.status).toBe('preview');
    if (preview.status !== 'preview') throw new Error('Expected a Session deletion preview.');
    expect((await native.deleteSession(previous.id, preview.preview.target_revision)).status).toBe('deleted');
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await expect(page).toHaveTitle(/rustX/);
    await connectRemote(page, fixture.endpoint, fixture.token);
    const oldId = await openEmptySession(page, fixture, 'Workspace A');
    const oldRow = page.locator(`button[data-session-id="${oldId}"]`);
    const trigger = page.locator('[data-model-select]');
    const input = page.getByRole('textbox', {name:'Message', exact:true});
    const send = page.getByRole('button', {name:'Send', exact:true});
    const reply = (text: string) => page.locator('[data-chat-group-part="response"]').getByText(text, {exact:true});
    const chooseModel = async (model: string) => {
      await trigger.click();
      const drill = page.getByRole('menuitem', {name:'Model', exact:true});
      if (await drill.isVisible()) await drill.click();
      await page.getByRole('menuitemradio', {name:model, exact:true}).click();
      await expect(page.locator('[data-model-menu]')).toHaveCount(0);
    };
    // Also make the removed model the browser's most recent selection.
    await chooseModel('fixture/console-model');
    await input.fill('Before model deletion');
    await send.click();
    const history = reply('Existing conversation history.');
    await expect(history).toBeVisible();
    const remove = async (model: string) => {
      await openSettingsPage(page, 'Models');
      await confirmSettingsAction(page, `Remove Model ${model}`);
      await expect(page.getByRole('row', {name:model, exact:true})).toHaveCount(0);
    };
    await remove('fixture/console-model');
    const sessionless = (await native.createSession({cwd:fixture.workspaceB})).session;
    expect((await native.readSettings(sessionless.id)).settings.model?.model).toBe('fixture/second-model');
    await chooseWorkspace(page, 'Workspace A');
    await expect(trigger).toContainText('fixture/second-model');
    await input.fill('Start with the replacement');
    // Keep the provider fixture focused on the primary model, as in the
    // other real first-submission tests.
    wire.holdNext('thread/start');
    await send.click();
    await expect.poll(() => wire.held()).toBe(1);
    const started = wire.responses.find(response => response.method === 'thread/start')!.result.session;
    await native.renameSession(started.id, 'New session after model deletion');
    wire.releaseHeld();
    await fixture.gate('new-conversation-ready');
    await expect(page.getByRole('region', {name:'Conversation composer', exact:true})).toBeVisible();
    await fixture.release('new-conversation-ready');
    await expect(reply('New conversation uses the replacement.')).toBeVisible();
    expect(wire.requests.filter(request => request.method === 'thread/start').at(-1)?.params.settings.model).toBeUndefined();

    await oldRow.click();
    await expect(history).toBeVisible();
    await expect(trigger).toContainText('fixture/console-model');
    const unavailable = page.getByText('This conversation’s model is unavailable. Choose another model to continue.', {exact:true});
    await expect(unavailable).toBeVisible();
    await input.fill('Continue the old conversation');
    await expect(send).toBeDisabled();
    const sends = () => wire.requests.filter(request => request.method === 'inbound/queue').length;
    const before = sends();
    await input.press('Enter');
    expect(sends()).toBe(before);
    await chooseModel('fixture/second-model');
    await expect(unavailable).toHaveCount(0);
    await expect(input).toHaveValue('Continue the old conversation');
    await send.click();
    await expect(reply('Old conversation continues after reselecting.')).toBeVisible();
    await expect(history).toBeVisible();

    await remove('fixture/second-model');
    await chooseWorkspace(page, 'Workspace A');
    await input.fill('Keep the draft until a model is added');
    await expect(send).toBeDisabled();
    await expect(page.getByText(/No model is available\. Add or enable a model in Settings\./).first()).toBeVisible();
    await oldRow.click();
    await expect(history).toBeVisible();
    await expect(unavailable).toBeVisible();
    expect(errors).toEqual([]);
    await page.goto('about:blank');
    await fixture.restartApp();
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await oldRow.click();
    await expect(history).toBeVisible();
    await expect(unavailable).toBeVisible();
    await input.fill('A cold conversation still keeps the draft');
    await expect(send).toBeDisabled();
    expect(errors).toEqual([]);
  } catch (error) {
    await test.info().attach('native-diagnostics', {body:JSON.stringify(fixture.diagnostics()),contentType:'application/json'});
    throw error;
  } finally { await native.shutdown(); await fixture.stop(false); }
});

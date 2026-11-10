import { expect, test } from '@playwright/test';
import { appendFileSync, readFileSync } from 'node:fs';
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

test('provider rename saves the display name without changing its models', async ({page}) => {
  const fixture = await startDogfood();
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openSettingsPage(page, 'Models');
    const actions = page.getByRole('button', {name:'Provider actions', exact:true});
    const input = page.getByRole('textbox', {name:'Provider display name', exact:true});
    const rename = async () => {
      await actions.click();
      await page.getByRole('menuitem', {name:'Rename', exact:true}).click();
      await expect(input).toBeFocused();
    };
    const provider = (name: string) => page.getByRole('row', {name, exact:true});
    const config = () => readFileSync(fixture.settings, 'utf8');
    const model = provider('fixture/console-model');

    await rename();
    await input.fill('Discard this name');
    await input.press('Escape');
    await expect(input).toHaveCount(0);
    await expect(actions).toBeFocused();
    await expect(provider('fixture')).toBeVisible();
    expect(config()).not.toContain('display_name');

    await rename();
    await input.fill('  OpenAI 工作网关  ');
    await input.press('Enter');
    await expect(provider('OpenAI 工作网关')).toBeVisible();
    await expect(model).toBeVisible();
    await expect.poll(config).toContain('display_name = "OpenAI 工作网关"');
    expect(config()).toContain('[providers.fixture]');

    await rename();
    await input.fill('团队网关');
    const url = page.getByRole('textbox', {name:'Base URL', exact:true});
    await url.click();
    await expect(provider('团队网关')).toBeVisible();
    await url.fill(`${fixture.providerUrl}/v1/`);
    await expect.poll(config).toContain(`base_url = "${fixture.providerUrl}/v1/"`);
    expect(config()).toContain('display_name = "团队网关"');

    await page.getByRole('button', {name:'Reload configuration', exact:true}).click();
    const search = page.getByLabel('Find a Provider', {exact:true});
    await search.fill('团队');
    await expect(provider('团队网关')).toBeVisible();
    await search.fill('fixture');
    await expect(provider('团队网关')).toBeVisible();
    await expect(model).toBeVisible();
    await openEmptySession(page, fixture, 'Workspace A');
    await expect(page.locator('[data-model-select]')).toContainText('fixture/console-model');
  } finally { await fixture.stop(false); }
});

test('model connection tests identify an App Server without the probe method', async ({page}) => {
  const fixture = await startDogfood();
  try {
    await page.routeWebSocket(new URL(fixture.endpoint).href, socket => {
      const server = socket.connectToServer();
      socket.onMessage(message => {
        const request = JSON.parse(String(message));
        if (request.method === 'model/probe') {
          socket.send(JSON.stringify({jsonrpc:'2.0', id:request.id, error:{code:-32601, message:'Method not found'}}));
        } else server.send(message);
      });
    });
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openSettingsPage(page, 'Models');
    await page.getByRole('button', {name:'Test connection for console-model', exact:true}).click();
    await expect(page.locator('[data-toast]')).toHaveText('The connected App Server does not support model connection tests. Update or restart it and try again.');
  } finally { await fixture.stop(false); }
});

test('model probe feedback follows explicit tests across provider switches and model edits', async ({page}) => {
  const fixture = await startDogfood('web_model_probe');
  const wire = await wireProbe(page);
  try {
    appendFileSync(fixture.settings, `
[providers.other]
protocol = "openai_chat_completions"
base_url = "${fixture.providerUrl}/v1"
api_key = "$RUSTX_WEB_MODEL_PROBE_UNSET_KEY"

[models."other/unconfigured"]
provider = "other"
id = "unconfigured"
context_window = 128000
max_output_tokens = 4096
capabilities = { input_modalities = ["text"], output_modalities = ["text"] }
compat = { chat_reasoning_replay = "omit" }
`);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openSettingsPage(page, 'Models');
    const probe = page.getByRole('button', {name:'Test connection for console-model', exact:true});
    const toast = page.locator('[data-toast]');
    const outcomes = () => wire.responses.filter(response => response.method === 'model/probe').map(response => response.result?.outcome);
    await probe.click();
    await expect.poll(outcomes).toEqual(['connected']);
    await expect(toast).toHaveText('Connection successful');
    await probe.click();
    await expect(toast).toContainText('The model request failed.');
    await expect(toast).toHaveCount(0, { timeout: 8000 });

    const other = page.getByRole('row', {name:'other', exact:true});
    const primary = page.getByRole('row', {name:'fixture', exact:true});
    const unavailable = page.getByRole('button', {name:'Test connection for unconfigured', exact:true});
    await other.click();
    await unavailable.click();
    await expect(toast).toContainText('Could not use the saved model settings.');
    await primary.click();
    await expect(probe).toHaveAttribute('data-probe', 'rejected');
    await expect(toast).toHaveCount(0, { timeout: 8000 });
    await other.click();
    await expect(unavailable).toHaveAttribute('data-probe', 'unavailable');
    await expect(toast).toHaveCount(0);
    await primary.click();
    await expect(probe).toHaveAttribute('data-probe', 'rejected');
    await expect(toast).toHaveCount(0);
    const toggle = page.getByRole('switch', {name:'Show console-model in the model picker', exact:true});
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-checked', 'false');
    await expect(probe).not.toHaveAttribute('data-probe');
    await expect(toast).toHaveCount(0);
    await probe.click();
    await expect.poll(outcomes).toEqual(['connected', 'rejected', 'unavailable', 'connected']);
    await expect(toast).toHaveText('Connection successful');
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

for (const replaceLastProvider of [false, true]) test(replaceLastProvider
  ? 'replacing the last provider refreshes new and old conversation model pickers without a reload'
  : 'deleting a provider removes its models and keeps the workspace and old conversation usable', async ({page}) => {
  const fixture = await startDogfood('web_model_deletion');
  const wire = await wireProbe(page);
  const native = await AppServerHost.connectRemote({endpoint:fixture.endpoint, token:fixture.token});
  try {
    appendFileSync(fixture.settings, `
[agent.model.auxiliary]
mode = "explicit"
model = "fixture/console-model"
`);
    if (!replaceLastProvider) appendFileSync(fixture.settings, `
[providers.alternative]
protocol = "openai_chat_completions"
base_url = "${fixture.providerUrl}/v1"
api_key = "fixture-only"

[models."alternative/replacement"]
provider = "alternative"
id = "second-model"
context_window = 128000
max_output_tokens = 4096
capabilities = { input_modalities = ["text"], output_modalities = ["text"] }
compat = { chat_reasoning_replay = "omit" }
`);
    appendFileSync(`${fixture.workspaceA}/rustx.toml`, `
[models."workspace-alias"]
provider = "fixture"
id = "console-model"
context_window = 128000
max_output_tokens = 4096
capabilities = { input_modalities = ["text"], output_modalities = ["text"] }
compat = { chat_reasoning_replay = "omit" }
`);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    const oldId = await openEmptySession(page, fixture, 'Workspace A');
    const input = page.getByRole('textbox', {name:'Message', exact:true});
    const send = page.getByRole('button', {name:'Send', exact:true});
    const trigger = page.locator('[data-model-select]');
    const history = page.locator('[data-chat-group-part="response"]').getByText('Existing conversation history.', {exact:true});
    await input.fill('Before model deletion');
    await send.click();
    await expect(history).toBeVisible();
    await openSettingsPage(page, 'Models');
    const provider = page.getByRole('row', {name:'fixture', exact:true});
    await provider.click();
    await expect(page.getByRole('row', {name:'fixture/console-model', exact:true})).toBeVisible();
    await expect(page.getByRole('row', {name:'fixture/second-model', exact:true})).toBeVisible();
    await page.getByRole('button', {name:'Provider actions', exact:true}).click();
    await page.getByRole('menuitem', {name:'Delete', exact:true}).click();
    const confirm = page.getByRole('alertdialog', {name:'Remove Provider fixture from User configuration?', exact:true});
    await expect(confirm).toContainText('This also removes this provider’s models');
    await confirm.getByRole('button', {name:'Remove Provider fixture', exact:true}).click();
    await expect(provider).toHaveCount(0);
    const config = readFileSync(fixture.settings, 'utf8');
    expect(config).not.toContain('[providers.fixture]');
    expect(config).not.toContain('[models."fixture/');
    expect(readFileSync(`${fixture.workspaceA}/rustx.toml`, 'utf8')).toContain('workspace-alias');
    const oldRow = page.locator(`button[data-session-id="${oldId}"]`);
    const unavailable = page.getByText('This conversation’s model is unavailable. Choose another model to continue.', {exact:true});
    const replacementModel = replaceLastProvider ? 'alternative/second-model' : 'alternative/replacement';

    if (replaceLastProvider) {
      await chooseWorkspace(page, 'Workspace A');
      await expect(page.getByText(/No model is available\. Add or enable a model in Settings\./).first()).toBeVisible();
      await expect(send).toBeDisabled();
      await oldRow.click();
      await expect(history).toBeVisible();
      await expect(unavailable).toBeVisible();
      await openSettingsPage(page, 'Models');
      await page.getByRole('button', {name:'+ Add Provider', exact:true}).click();
      const naming = page.getByRole('dialog', {name:'Add Provider', exact:true});
      await naming.getByLabel('New Provider identity', {exact:true}).fill('alternative');
      await naming.getByRole('button', {name:'Continue', exact:true}).click();
      const connection = page.getByRole('form', {name:'Provider alternative', exact:true});
      await connection.getByLabel('Base URL', {exact:true}).fill(`${fixture.providerUrl}/v1`);
      await connection.getByLabel('API Key', {exact:true}).fill('fixture-only');
      await connection.getByLabel('API Key', {exact:true}).press('Enter');
      const addModel = page.getByRole('button', {name:'+ Add Model', exact:true});
      await expect(addModel).toBeEnabled();
      await addModel.click();
      const dialog = page.getByRole('dialog', {name:'Add Model', exact:true});
      await dialog.getByLabel('Model ID', {exact:true}).fill('second-model');
      await dialog.getByRole('button', {name:'Save Add Model', exact:true}).click();
      await expect(dialog).toHaveCount(0);
      await expect(page.getByRole('row', {name:replacementModel, exact:true})).toBeVisible();

      // The resident conversation must see the new catalog before another
      // Session can bootstrap publication for its Workspace.
      await returnHome(page);
      await oldRow.click();
      await expect(history).toBeVisible();
      await expect(unavailable).toBeVisible();
      await expect(trigger).toBeEnabled();
      await trigger.click();
      await expect(page.getByRole('menuitemradio', {name:replacementModel, exact:true})).toBeEnabled();
      await page.keyboard.press('Escape');
    } else {
      expect(config).toContain('[providers.alternative]');
    }

    await chooseWorkspace(page, 'Workspace A');
    await expect(trigger).toContainText(replacementModel);
    await input.fill('Start with the replacement');
    wire.holdNext('thread/start');
    await send.click();
    await expect.poll(() => wire.held()).toBe(1);
    const started = wire.responses.find(response => response.method === 'thread/start')!.result.session;
    await native.renameSession(started.id, 'New session after provider deletion');
    wire.releaseHeld();
    await fixture.gate('new-conversation-ready');
    await fixture.release('new-conversation-ready');
    await expect(page.getByText('New conversation uses the replacement.', {exact:true})).toBeVisible();

    await oldRow.click();
    await expect(history).toBeVisible();
    await expect(trigger).toContainText('fixture/console-model');
    await expect(unavailable).toBeVisible();
    await trigger.click();
    await expect(page.locator('[data-model-menu]')).toBeVisible();
    const drill = page.getByRole('menuitem', {name:'Model', exact:true});
    if (await drill.isVisible()) await drill.click();
    await expect(page.getByRole('menuitemradio', {name:'workspace-alias', exact:true})).toHaveCount(0);
    const replacement = page.getByRole('menuitemradio', {name:replacementModel, exact:true});
    await expect(replacement).toBeEnabled();
    await replacement.click();
    await expect(trigger).toContainText(replacementModel);
    await input.fill('Continue the old conversation');
    await send.click();
    await expect(page.getByText('Old conversation continues after reselecting.', {exact:true})).toBeVisible();
    const selection = (await native.readSettings(oldId)).settings.model;
    expect(selection).toMatchObject({auxiliary:{mode:'session'}});
    expect(wire.requests.filter(request => request.method === 'initialize')).toHaveLength(1);
  } catch (error) {
    await test.info().attach('native-diagnostics', {body:JSON.stringify(fixture.diagnostics()),contentType:'application/json'});
    throw error;
  } finally { await native.shutdown(); await fixture.stop(false); }
});

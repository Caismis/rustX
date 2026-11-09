import { expect, test } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { choose, connectRemote, openEmptySession, openSettingsPage, settledSave } from './shell-actions';
import { captureEvidence } from './screenshot';
import { wireProbe } from './wire-probe';

for (const width of [1440, 390]) test(`Responses profile API parameters save directly and preserve the independent output limit at ${width}px`, async ({page}) => {
  const fixture = await startDogfood();
  const wire = await wireProbe(page);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', event => { if (['error','warning'].includes(event.type())) errors.push(event.text()); });
  try {
    writeFileSync(fixture.settings, `[providers.OpenAI]
protocol = "openai_responses"
base_url = "${fixture.providerUrl}/v1"
api_key = "fixture-only"
[models."OpenAI/gpt-6-luna"]
provider = "OpenAI"
id = "gpt-6-luna"
context_window = 128000
max_output_tokens = 8192
capabilities = { input_modalities = ["text"], output_modalities = ["text"] }
default_profile = "none"
[models."OpenAI/gpt-6-luna".profiles.none]
request_params = '{}'
`);
    await page.setViewportSize({width, height:1000});
    await page.emulateMedia({colorScheme:'dark', reducedMotion:'reduce'});
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await page.getByRole('button', {name:'Settings',exact:true}).click();
    await openSettingsPage(page, 'Models');
    const settings = page.getByRole('region', {name:'Settings',exact:true});
    await settings.getByRole('row', {name:'OpenAI',exact:true}).click();
    await settings.getByRole('row', {name:'OpenAI/gpt-6-luna',exact:true}).click();
    const modal = page.getByRole('dialog', {name:'Model OpenAI/gpt-6-luna',exact:true});
    await modal.getByRole('button', {name:'Advanced configuration',exact:true}).click();
    const params = modal.getByRole('textbox', {name:'API parameters (JSON)',exact:true});
    const limit = modal.getByLabel('Profile output limit (optional)', {exact:true});
    const low = {reasoning:{effort:'low'}};
    await params.press('ControlOrMeta+a');
    await page.keyboard.insertText(JSON.stringify(low));
    await limit.fill('2048');
    const save = modal.getByRole('button', {name:'Save Model OpenAI/gpt-6-luna',exact:true});
    await save.click();
    await expect(settledSave(modal, 'Model OpenAI/gpt-6-luna')).toBeVisible();
    await expect(save).toBeDisabled();
    await expect(modal.getByRole('alert')).toHaveCount(0);
    const writes = () => wire.requests.filter(request => request.method === 'configuration/sourceWrite');
    expect(writes()).toHaveLength(1);
    expect(writes()[0].params.mutation.mutation.authored.profiles.none).toEqual({max_output_tokens:2048, request_params:low});
    expect(readFileSync(fixture.settings, 'utf8')).toContain('"reasoning":{"effort":"low"}');
    await captureEvidence(modal, `responses-profile-${width}`);
    await modal.getByRole('button', {name:'Cancel',exact:true}).click();
    await settings.getByRole('row', {name:'OpenAI/gpt-6-luna',exact:true}).click();
    await modal.getByRole('button', {name:'Advanced configuration',exact:true}).click();
    await expect(limit).toHaveValue('2048');
    await expect(params).toHaveText(JSON.stringify(low,null,2), {useInnerText:true});
    const high = {reasoning:{effort:'high'}};
    await params.press('ControlOrMeta+a');
    await page.keyboard.insertText(JSON.stringify(high));
    await limit.fill('');
    await save.click();
    await expect.poll(() => readFileSync(fixture.settings, 'utf8')).toContain('\"reasoning\":{\"effort\":\"high\"}');
    await expect(settledSave(modal, 'Model OpenAI/gpt-6-luna')).toBeVisible();
    await expect(save).toBeDisabled();
    await expect(modal.getByRole('alert')).toHaveCount(0);
    expect(writes()).toHaveLength(2);
    expect(writes()[1].params.mutation.mutation.authored.profiles.none).toEqual({request_params:high});
    expect(readFileSync(fixture.settings, 'utf8')).not.toContain('max_output_tokens = 2048');
    expect(errors).toEqual([]);
  } catch (error) {
    await test.info().attach('native-diagnostics', {body:JSON.stringify(fixture.diagnostics()),contentType:'application/json'});
    throw error;
  } finally { await fixture.stop(false); }
});

for (const width of [1440, 390]) test(`provider-only setup stays editable and adds its first native model at ${width}px`, async ({ page }) => {
  const fixture = await startDogfood();
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error' || message.type() === 'warning') errors.push(message.text()); });
  try {
    writeFileSync(fixture.settings, '# Model setup starts without a provider or model.\n');
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await openSettingsPage(page, 'Models');
    const settings = page.getByRole('region', { name: 'Settings', exact: true });
    await settings.getByRole('button', { name: '+ Add Provider', exact: true }).click();
    const name = page.getByRole('dialog', { name: 'Add Provider', exact: true });
    await name.getByLabel('New Provider identity').fill('custom');
    await name.getByRole('button', { name: 'Continue', exact: true }).click();
    const provider = settings.getByRole('form', { name: 'Provider custom' });
    await provider.getByLabel('Base URL', { exact: true }).fill(fixture.providerUrl + '/v1');
    await provider.getByLabel('API Key', { exact: true }).fill('fixture-only');
    await choose(provider, 'API format', 'OpenAI Responses (/v1/responses)');
    await provider.getByRole('button', { name: 'Provider actions', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Choose provider icon', exact: true }).click();
    const icons = page.getByRole('dialog', { name: 'Choose provider icon', exact: true });
    await icons.getByRole('searchbox', { name: 'Search icons' }).fill('deepseek');
    await icons.getByRole('row', { name: 'DeepSeek', exact: true }).click();
    await expect(icons).toHaveCount(0);
    await expect(settings.getByRole('button', { name: '+ Add Model', exact: true })).toBeDisabled();
    await provider.getByRole('button', { name: 'Save Provider custom', exact: true }).click();
    await expect(provider.getByRole('button', { name: 'Save Provider custom', exact: true })).toBeHidden();
    await expect(settings.getByRole('button', { name: '+ Add Model', exact: true })).toBeEnabled();
    await expect(settings.getByRole('alert')).toHaveCount(0);
    await expect(settings.getByText('No models configured yet. Add a model to use this provider in conversations.')).toBeVisible();
    await settings.getByRole('button', { name: 'Reload configuration', exact: true }).click();
    await expect(settings.getByRole('alert')).toHaveCount(0);
    expect(readFileSync(fixture.settings, 'utf8')).toContain('icon = "deepseek"');
    await expect(settings.getByRole('row', { name: 'custom', exact: true }).locator('[data-provider-icon="deepseek"] img')).toHaveJSProperty('complete', true);
    await captureEvidence(settings, `provider-only-${width}`);

    await settings.getByRole('button', { name: '+ Add Model', exact: true }).click();
    const model = page.getByRole('dialog', { name: 'Add Model', exact: true });
    await expect(model.getByRole('button', { name: /API format$/ })).toHaveCount(0);
    await model.getByLabel('Model ID', { exact: true }).fill('first-model');
    await expect(model.getByLabel('Model ID', { exact: true })).toBeFocused();
    await model.getByLabel('Context window', { exact: true }).fill('1000');
    await model.getByRole('button', { name: 'Save Add Model', exact: true }).click();
    await expect(page.locator('[data-toast][data-foreground]')).toContainText('Maximum output tokens must be smaller than the context window.');
    expect(readFileSync(fixture.settings, 'utf8')).not.toContain('[models.');
    await model.getByLabel('Context window', { exact: true }).fill('128000');
    await expect(page.locator('[data-toast][data-foreground]')).toHaveCount(0);
    await captureEvidence(model, `add-model-${width}`);
    await model.getByRole('button', { name: 'Save Add Model', exact: true }).click();
    await expect(model).toHaveCount(0);
    await expect(settings.getByRole('row', { name: 'custom/first-model', exact: true })).toBeVisible();
    await expect(settings.getByRole('alert')).toHaveCount(0);
    expect(readFileSync(fixture.settings, 'utf8')).toContain('first-model');
    expect(readFileSync(fixture.settings, 'utf8').split('[models.')[1]).not.toContain('protocol =');
    await expect(settings.getByRole('row', { name: 'custom/first-model', exact: true })).not.toContainText('openai_responses');
    await choose(provider, 'API format', 'OpenAI Chat Completions (/v1/chat/completions)');
    await provider.getByRole('button', { name: 'Save Provider custom', exact: true }).click();
    await expect(provider.getByRole('button', { name: 'Save Provider custom', exact: true })).toBeHidden();
    expect(readFileSync(fixture.settings, 'utf8').split('[models.')[0]).toContain('protocol = "openai_chat_completions"');
    await expect(provider.getByRole('button', { name: /API format$/ })).toContainText('OpenAI Chat Completions');


    await settings.getByRole('button', { name: '+ Add Model', exact: true }).click();
    await model.getByLabel('Model ID', { exact: true }).fill('first-model');
    await expect(page.locator('[data-toast][data-foreground]')).toContainText('This configuration name already exists.');
    await model.getByRole('button', { name: 'Cancel', exact: true }).click();
    await settings.getByRole('button', { name: '+ Add Model', exact: true }).click();
    await expect(model.getByLabel('Model ID', { exact: true })).toHaveValue('');
    await model.getByRole('button', { name: 'Cancel', exact: true }).click();
    await settings.getByRole('button', { name: 'Reload configuration', exact: true }).click();
    await expect(settings.getByRole('row', { name: 'custom/first-model', exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)).toBe(false);
    await settings.getByRole('button', { name: '+ Add Model', exact: true }).click();
    await model.getByLabel('Model ID', { exact: true }).fill('disposable-model');
    await model.getByRole('button', { name: 'Save Add Model', exact: true }).click();
    const disposable = settings.getByRole('row', { name: 'custom/disposable-model', exact: true });
    const remove = disposable.getByRole('button', { name: 'Remove Model custom/disposable-model', exact: true });
    await remove.click();
    const confirmation = page.getByRole('alertdialog');
    await confirmation.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(remove).toBeFocused();
    await remove.click();
    await confirmation.getByRole('button', { name: 'Remove Model custom/disposable-model', exact: true }).click();
    await expect(disposable).toHaveCount(0);
    await expect(settings.getByRole('row', { name: 'custom/first-model', exact: true })).toBeVisible();
    await settings.getByRole('button', { name: 'Default model for new Sessions', exact: true }).click();
    const defaults = settings.getByRole('form', { name: 'Default model', exact: true });
    await choose(defaults, 'Model', 'custom/first-model');
    await defaults.getByRole('button', { name: 'Save Default model', exact: true }).click();
    await expect(defaults.getByRole('button', { name: 'Save Default model', exact: true })).toBeDisabled();
    await expect.poll(() => readFileSync(fixture.settings, 'utf8')).toContain('model = "custom/first-model"');
    const sessionId = await openEmptySession(page, fixture, 'Workspace A');
    expect(sessionId).toBeTruthy();
    await expect(page.locator('[data-model-select]')).toContainText('custom/first-model');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    if (width === 1440) {
      await openSettingsPage(page, 'General');
      await choose(settings, 'Language', '中文');
      const chinese = page.getByRole('region', { name: '设置', exact: true });
      await chinese.getByRole('tab', { name: '模型', exact: true }).click();
      await captureEvidence(chinese, 'models-zh-desktop');
      await chinese.getByRole('button', { name: '+ 添加模型', exact: true }).click();
      const dialog = page.getByRole('dialog', { name: '添加模型', exact: true });
      await dialog.getByLabel('模型 ID', { exact: true }).fill('deepseek-flash');
      await dialog.getByRole('button', { name: '高级配置', exact: true }).click();
      await captureEvidence(dialog, 'add-model-zh-desktop');
      await dialog.getByRole('button', { name: '取消', exact: true }).click();
    }
    expect(errors).toEqual([]);
  } catch (error) {
    await test.info().attach('native-diagnostics', { body: JSON.stringify(fixture.diagnostics(), null, 2), contentType: 'application/json' });
    const details = page.getByRole('button', { name: 'Show details', exact: true });
    if (await details.isVisible()) {
      await details.click();
      await test.info().attach('connection-details', { body: await page.locator('body').innerText(), contentType: 'text/plain' });
    }
    throw error;
  } finally { await fixture.stop(false); }
});

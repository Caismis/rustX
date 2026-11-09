import {openResourceModule, settledSave } from './shell-actions';
import { expandModelAuthoring } from './shell-actions';
import { openEmptySession } from './shell-actions';
import { connectRemote } from './shell-actions';
import { expect, test } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { choose, returnHome, openSettingsPage, openWorkspaceSettings, selectedSettingsPage } from './shell-actions';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';

// Real native source fixtures: the browser only edits the generated projection.
for (const auxiliary of ['Summary', 'Title', 'Classification', 'BTW'] as const) test(`${auxiliary} selections round-trip and implicit MCP/optional Agent sources remain editable`, async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const fixture = await startDogfood();
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const summary = `${auxiliary.toLowerCase()}_model = { mode = "explicit", model = "summary-a", profile = "deep", max_output_tokens = { mode = "limit", tokens = 2048 }, request_params = \'{"temperature":0.2}\' }\n`;
  const model = '[model]\nmodel = "fixture/console-model"\n' + summary;
  try {
    mkdirSync(join(fixture.workspaceA, '.agents/agents'), { recursive: true });
    writeFileSync(join(fixture.workspaceA, 'rustx.toml'), ['summary-a', 'summary-b'].map(id => `
[models."${id}"]
provider = "fixture"
id = "console-model"
context_window = 128000
max_output_tokens = 4096
capabilities = { input_modalities = ["text"], output_modalities = ["text"] }
compat = { chat_reasoning_replay = "omit" }
default_profile = "deep"
profiles = { deep = { request_params = '{}' }, quick = { request_params = '{"temperature":0.1}' } }
`).join('') + '[agent.model]\nmodel = "fixture/console-model"\n' + summary);
    writeFileSync(join(fixture.workspaceA, '.agents/agents/optional.toml'), model);
    const mcpFile = join(fixture.workspaceA, '.agents/mcp.toml');
    writeFileSync(mcpFile, '[mcp_servers.implicit-http]\nurl = "https://example.invalid/mcp"\nheaders = { Authorization = "fixture-header-secret" }\n[mcp_servers.implicit-stdio]\ncommand = "fixture-not-executed"\nenv = { TOKEN = "fixture-env-secret" }\n');
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByLabel('Access token')).toBeHidden();
    await openEmptySession(page, fixture, 'Workspace A');
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('region', { name: 'Settings', exact: true });
    let surface=settings;
    expect(await selectedSettingsPage(page)).toBe('General');
    await returnHome(page);
    await openWorkspaceSettings(page, 'Workspace A'); await expandModelAuthoring(page);
    for (const owner of ['Root', 'Agent'] as const) {
      if (owner === 'Root') { await openSettingsPage(page, 'Models'); await expandModelAuthoring(page);surface=settings; }
      else {
        surface=await openResourceModule(page,'subagents','Workspace A');
        await surface.getByRole('listitem',{name:'optional',exact:true}).getByRole('button',{name:'optional',exact:true}).click();
      }
      const title = owner === 'Root' ? 'Default model' : 'Agent optional';
      const form = surface.getByRole('form', { name: title, exact: true });
      const save = async () => {
        await form.getByRole('button', { name: `Save ${title}`, exact: true }).click();
        await expect(settledSave(surface, title)).toBeVisible();
      };
      if (owner === 'Root') {
        await expect(form.getByLabel(`${auxiliary} model`, { exact: true })).toHaveCount(0);
        await choose(form, 'Model', 'summary-b');
        await choose(form, 'Profile', 'quick');
        await save();
        const root = readFileSync(join(fixture.workspaceA, 'rustx.toml'), 'utf8');
        expect(root).toContain('model = "summary-b"');
        expect(root).toContain('profile = "quick"');
        expect(root).toContain(`${auxiliary.toLowerCase()}_model`);
        expect(root).toMatch(/request_params = '\{"temperature":0\.2\}'/);
        continue;
      }
      await choose(form, `${auxiliary} model`, 'summary-b');
      await expect(form.getByLabel('Description', { exact: true })).toHaveValue('');
      await expect(form.getByLabel('Instructions', { exact: true })).toHaveValue('');
      await form.getByLabel('read', { exact: true }).check();
      await save();
      const sourceFile = join(fixture.workspaceA, '.agents/agents/optional.toml');
      const preserved = readFileSync(sourceFile, 'utf8');
      expect(preserved).not.toMatch(/^(description|instructions)\s*=/m);
      for (const fact of ['summary-b', 'deep', '2048', '0.2']) expect(preserved).toContain(fact);
      // Native writes keep provider-native parameters as JSON strings.
      expect(preserved).toMatch(/request_params = '\{"temperature":0\.2\}'/);
      // Acknowledge then reread: clean forms follow the native source, not old drafts.
      await surface.getByRole('button', { name: 'Refresh', exact: true }).click();
      const nested = form.getByRole('group', { name: `Explicit ${auxiliary} Model settings`, exact: true });
      await expect(nested.getByRole('button', { name: new RegExp(`Profile \\(${auxiliary}\\)$`) })).toContainText('deep');
      await expect(nested.getByLabel(`Output limit (${auxiliary})`)).toHaveValue('2048');
      await expect(nested.getByLabel(`Request parameter overrides (${auxiliary})`, {exact:true})).toHaveText('{\n  "temperature": 0.2\n}', {useInnerText:true});
      await choose(nested, `Profile (${auxiliary})`, 'quick');
      await nested.getByLabel(`Output limit (${auxiliary})`).fill('1024');
      await nested.getByLabel(`Request parameter overrides (${auxiliary})`, {exact:true}).fill('{"seed": 7, "stop": [null]}');
      await save();
      const authored = readFileSync(sourceFile, 'utf8');
      expect(authored).toContain('summary-b'); expect(authored).toContain('quick');
      expect(authored).toContain('1024');
      expect(authored).toMatch(/request_params = '\{"seed":7,"stop":\[null\]\}'/);
      await choose(form, `${auxiliary} model`, 'Follow selected model'); await save();
      await expect(nested).toHaveCount(0);
    }
    surface=await openResourceModule(page,'mcp','Workspace A');
    for (const transport of ['http', 'stdio'] as const) {
      await surface.getByRole('button', { name: `MCP implicit-${transport}`, exact: true }).click();
      await expect(surface).not.toContainText('fixture-header-secret');
      await expect(surface).not.toContainText('fixture-env-secret');
      await surface.getByRole('button', { name: 'JSON', exact: true }).click();
      const editor = surface.getByLabel('Complete configuration',{exact:true});
      const config = JSON.parse(await editor.innerText());
      const definition = config[`implicit-${transport}`];
      expect(config).not.toHaveProperty('retained_headers');
      expect(config).not.toHaveProperty('retained_env');
      if (transport === 'http') definition.url = 'https://example.invalid/edited';
      else definition.cwd = fixture.workspaceA;
      await editor.fill(JSON.stringify(config));
      await surface.getByRole('button', { name: 'Save', exact: true }).click();
      await expect(surface.getByRole('listitem', { name: `implicit-${transport}`, exact: true })).toBeVisible();
    }
    const mcp = readFileSync(mcpFile, 'utf8');
    expect(mcp).toContain('fixture-header-secret'); expect(mcp).toContain('fixture-env-secret');
    expect(mcp).not.toMatch(/^type\s*=/m);
    await openSettingsPage(page, 'Advanced');
    await expect(settings.locator('[data-lifecycle=ready]')).toBeVisible();
    expect(errors).toEqual([]);
  } finally { const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

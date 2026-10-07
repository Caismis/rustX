import { expandModelAuthoring } from './shell-actions';
import { openEmptySession } from './shell-actions';
import { connectRemote } from './shell-actions';
import { expect, test } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { choose, closeSettings, openSettingsPage, openWorkspaceSettings, selectedSettingsPage } from './shell-actions';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';

// Real native source fixtures: the browser only edits the generated projection.
test('Summary selections round-trip and implicit MCP/optional Agent sources remain editable', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const fixture = await startDogfood();
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const summary = 'summary_model = { mode = "explicit", model = "summary-a", reasoning_profile = { mode = "profile", name = "deep" }, max_output_tokens = { mode = "limit", tokens = 2048 }, request_params = { temperature = 0.2 } }\n';
  const model = '[model]\nmodel = "fixture/console-model"\n' + summary;
  try {
    mkdirSync(join(fixture.workspaceA, '.agents/agents'), { recursive: true });
    writeFileSync(join(fixture.workspaceA, 'rustx.toml'), ['summary-a', 'summary-b'].map(id => `
[models."${id}"]
provider = "fixture"
id = "console-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 4096
capabilities = { input_modalities = ["text"], output_modalities = ["text"], tool_calls = true, reasoning = true }
compat = { chat_reasoning_replay = "omit" }
reasoning = { default_profile = "deep", profiles = { deep = { enabled = true }, quick = { enabled = true } } }
`).join('') + '[agent.model]\nmodel = "fixture/console-model"\n' + summary);
    writeFileSync(join(fixture.workspaceA, '.agents/agents/optional.toml'), model);
    const mcpFile = join(fixture.workspaceA, '.agents/mcp.toml');
    writeFileSync(mcpFile, '[mcp_servers.implicit-http]\nurl = "https://example.invalid/mcp"\nheaders = { Authorization = "fixture-header-secret" }\n[mcp_servers.implicit-stdio]\ncommand = "fixture-not-executed"\nenv = { TOKEN = "fixture-env-secret" }\n');
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await openEmptySession(page, fixture, 'Workspace A');
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    expect(await selectedSettingsPage(page)).toBe('General');
    await closeSettings(page);
    await openWorkspaceSettings(page, 'Workspace A'); await expandModelAuthoring(page);
    for (const owner of ['Root', 'Agent'] as const) {
      if (owner === 'Root') { await openSettingsPage(page, 'Models'); await expandModelAuthoring(page); }
      else {
        await openSettingsPage(page, 'Extensions');
        await settings.getByRole('tab', { name: 'Agents', exact: true }).click();
        await settings.getByRole('row', { name: 'optional', exact: true }).click();
      }
      const title = owner === 'Root' ? 'Default model' : 'Agent optional';
      const form = settings.getByRole('form', { name: title, exact: true });
      const save = async () => {
        await form.getByRole('button', { name: `Save ${title}`, exact: true }).click();
        await expect(settings.getByText(`${title} saved. Native coordination owns application.`)).toBeVisible();
      };
      await choose(form, 'Summary model', 'summary-b');
      if (owner === 'Agent') {
        await expect(form.getByLabel('Description', { exact: true })).toHaveValue('');
        await expect(form.getByLabel('Instructions', { exact: true })).toHaveValue('');
        await form.getByLabel('read', { exact: true }).check();
      }
      await save();
      const sourceFile = owner === 'Root' ? join(fixture.workspaceA, 'rustx.toml') : join(fixture.workspaceA, '.agents/agents/optional.toml');
      const preserved = readFileSync(sourceFile, 'utf8');
      if (owner === 'Agent') {
        expect(preserved).not.toMatch(/^(description|instructions)\s*=/m);
      }
      for (const fact of ['summary-b', 'deep', '2048', '0.2']) expect(preserved).toContain(fact);
      // Acknowledge then reread: clean forms follow the native source, not old drafts.
      await settings.getByRole('button', { name: 'Reload configuration', exact: true }).click();
      const nested = form.getByRole('group', { name: 'Explicit Summary Model settings', exact: true });
      await expect(nested.getByLabel('Profile identity (Summary)')).toHaveValue('deep');
      await expect(nested.getByLabel('Output limit (Summary)')).toHaveValue('2048');
      await expect(nested.getByLabel('temperature', { exact: true })).toHaveValue('0.2');
      await nested.getByLabel('Profile identity (Summary)').fill('quick');
      await nested.getByLabel('Output limit (Summary)').fill('1024');
      await nested.getByLabel('temperature', { exact: true }).fill('0.4');
      await save();
      const authored = readFileSync(sourceFile, 'utf8');
      expect(authored).toContain('summary-b'); expect(authored).toContain('quick');
      expect(authored).toContain('1024'); expect(authored).toContain('0.4');
      if (owner === 'Root') {
        await nested.evaluate(el => el.scrollIntoView({ block: 'start' }));
        await page.screenshot({ path: test.info().outputPath('summary-model-complete.png') });
      }
      await choose(form, 'Summary model', 'Follow selected model'); await save();
      await expect(nested).toHaveCount(0);
    }
    await settings.getByRole('button', { name: '← Extensions', exact: true }).click();
    await settings.getByRole('tab', { name: 'MCP', exact: true }).click();
    for (const transport of ['http', 'stdio'] as const) {
      await settings.getByRole('row', { name: `implicit-${transport}`, exact: true }).click();
      const form = settings.getByRole('form', { name: `MCP implicit-${transport}`, exact: true });
      await expect(form.getByRole('button', { name: new RegExp(`^${transport === 'http' ? 'HTTP' : 'stdio'} Transport$`) })).toBeVisible();
      await expect(settings).not.toContainText('fixture-header-secret');
      await expect(settings).not.toContainText('fixture-env-secret');
      if (transport === 'http') {
        await expect(form.getByLabel('MCP command')).toHaveCount(0);
        await expect(form.getByLabel('Retain existing header keys 1', { exact: true })).toHaveValue('Authorization');
        await form.getByLabel('MCP URL').fill('https://example.invalid/edited');
      } else {
        await expect(form.getByLabel('MCP URL')).toHaveCount(0);
        await expect(form.getByLabel('Retain existing environment keys 1', { exact: true })).toHaveValue('TOKEN');
        await form.getByLabel('Working directory').fill(fixture.workspaceA);
      }
      await form.getByRole('button', { name: `Save MCP implicit-${transport}`, exact: true }).click();
      await expect(settings.getByText(`MCP implicit-${transport} saved. Native coordination owns application.`)).toBeVisible();
      await settings.getByRole('button', { name: '← Extensions', exact: true }).click();
    }
    const mcp = readFileSync(mcpFile, 'utf8');
    expect(mcp).toContain('fixture-header-secret'); expect(mcp).toContain('fixture-env-secret');
    expect(mcp).not.toMatch(/^type\s*=/m);
    await openSettingsPage(page, 'Advanced');
    await expect(settings.getByText(/Revision:/)).toBeVisible();
    expect(errors).toEqual([]);
  } finally { const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

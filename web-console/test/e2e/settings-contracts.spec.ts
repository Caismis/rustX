import { connectRemote } from './shell-actions';
import { expect, test } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { chooseWorkspace } from './shell-actions';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { wireProbe } from './wire-probe';

// Real native source fixtures: the browser only edits the generated projection.
test('Summary selections round-trip and implicit MCP/optional Agent sources remain editable', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const fixture = await startDogfood();
  const wire = await wireProbe(page);
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
    await chooseWorkspace(page, 'Workspace A');
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    await settings.getByRole('button', { name: 'Overview', exact: true }).click();
    await settings.getByRole('tab', { name: 'Workspace', exact: true }).click();
    for (const owner of ['Root', 'Agent'] as const) {
      await settings.getByRole('button', { name: owner === 'Root' ? 'Model' : 'Agents', exact: true }).click();
      if (owner === 'Agent') await settings.getByRole('button', { name: 'Edit Agent optional', exact: true }).click();
      const form = settings.getByRole('form', { name: owner === 'Root' ? 'Root model' : 'Agent optional', exact: true });
      const save = async () => {
        await form.getByRole('button', { name: owner === 'Root' ? 'Save Root model' : 'Save Agent optional', exact: true }).click();
        await expect(settings.getByText(/Source saved. Use Reload/)).toBeVisible();
        await expect(form.getByRole('status')).toHaveText('Source saved. Reload separately to publish configuration.');
      };
      await form.getByRole('combobox', { name: 'Summary model', exact: true }).selectOption('summary-b');
      if (owner === 'Agent') {
        await expect(form.getByLabel('Description', { exact: true })).toHaveValue('');
        await expect(form.getByLabel('Instructions', { exact: true })).toHaveValue('');
        await form.getByLabel('read', { exact: true }).check();
      }
      await save();
      const sourceFile = owner === 'Root' ? join(fixture.workspaceA, 'rustx.toml') : join(fixture.workspaceA, '.agents/agents/optional.toml');
      const preserved = readFileSync(sourceFile, 'utf8');
      if (owner === 'Agent') {
        const request = wire.requests.filter(request => request.method === 'configuration/sourceWrite').at(-1);
        if (request?.method !== 'configuration/sourceWrite' || request.params.mutation.kind !== 'agent') throw new Error('Expected native Agent write');
        expect(request.params.mutation.authored).not.toHaveProperty('description');
        expect(request.params.mutation.authored).not.toHaveProperty('instructions');
        expect(preserved).not.toMatch(/^(description|instructions)\s*=/m);
      }
      for (const fact of ['summary-b', 'deep', '2048', '0.2']) expect(preserved).toContain(fact);
      // Acknowledge then reread: clean forms follow the native source, not old drafts.
      await settings.getByRole('button', { name: 'Read current sources' }).click();
      const nested = form.getByRole('group', { name: 'Explicit Summary Model settings', exact: true });
      await expect(nested.getByLabel('Summary Profile identity')).toHaveValue('deep');
      await expect(nested.getByLabel('Summary Output limit')).toHaveValue('2048');
      await expect(nested.getByLabel('temperature', { exact: true })).toHaveValue('0.2');
      await nested.getByLabel('Summary Profile identity').fill('quick');
      await nested.getByLabel('Summary Output limit').fill('1024');
      await nested.getByLabel('temperature', { exact: true }).fill('0.4');
      await save();
      const authored = readFileSync(sourceFile, 'utf8');
      expect(authored).toContain('summary-b'); expect(authored).toContain('quick');
      expect(authored).toContain('1024'); expect(authored).toContain('0.4');
      if (owner === 'Root') {
        await nested.evaluate(el => el.scrollIntoView({ block: 'start' }));
        await page.screenshot({ path: test.info().outputPath('summary-model-complete.png') });
      }
      await form.getByRole('combobox', { name: 'Summary model', exact: true }).selectOption(''); await save();
      await expect(nested).toHaveCount(0);
    }
    await settings.getByRole('button', { name: 'MCP', exact: true }).click();
    for (const transport of ['http', 'stdio'] as const) {
      await settings.getByRole('button', { name: `Edit MCP implicit-${transport}`, exact: true }).click();
      const form = settings.getByRole('form', { name: `MCP implicit-${transport}`, exact: true });
      await expect(form.getByRole('combobox', { name: 'Transport', exact: true })).toHaveValue(transport);
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
      await expect(settings.getByText(/Source saved. Use Reload/)).toBeVisible();
      await expect(form.getByRole('status')).toHaveText('Source saved. Reload separately to publish configuration.');
    }
    const mcp = readFileSync(mcpFile, 'utf8');
    expect(mcp).toContain('fixture-header-secret'); expect(mcp).toContain('fixture-env-secret');
    expect(mcp).not.toMatch(/^type\s*=/m);
    await settings.getByRole('button', { name: 'Reload', exact: true }).click();
    await expect(settings.getByText(/Configuration published:/)).toBeVisible();
    expect(errors).toEqual([]);
  } finally { const report = await fixture.stop(false); expect(report.requestCount).toBe(0); }
});

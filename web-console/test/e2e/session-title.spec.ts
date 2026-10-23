import { test, expect } from '@playwright/test';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, selectedSessionId } from './shell-actions';

for (const width of [1440, 390]) test(`native automatic Title, explicit invocation and manual precedence (${width})`, async ({ page }, info) => {
  // Independent strict scripts make concurrent primary/Title request ordering
  // observable without weakening either provider's assertions.
  const primary = await startDogfood('web_session_title_primary');
  const auxiliary = await startDogfood('web_session_title_auxiliary');
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (['error', 'warning'].includes(message.type())) errors.push(message.text()); });
  let native: AppServerHost | undefined;
  let passed = false;
  try {
    writeFileSync(join(primary.workspaceA, 'rustx.toml'), `
[providers.titles]
base_url = "${auxiliary.providerUrl}/v1"
api_key = "$RUSTX_CONSOLE_FIXTURE_KEY"
[models."titles/short"]
provider = "titles"
id = "title-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 256
capabilities = { input_modalities = ["text"], output_modalities = ["text"], tool_calls = true, reasoning = false }
compat = { chat_reasoning_replay = "omit" }
[agent.model]
model = "fixture/console-model"
[agent.model.title_model]
mode = "explicit"
model = "titles/short"
request_params = '{"title_tag":"explicit-native"}'
max_output_tokens = { mode = "limit", tokens = 128 }
`);
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await routeWorkspaceHost(page, primary);
    await page.goto('/');
    await expect(page).toHaveTitle('rustX Developer Console');
    await connectRemote(page, primary.endpoint, primary.token);
    await openEmptySession(page, primary, 'Workspace A');
    const id = await selectedSessionId(page);
    native = await AppServerHost.connectRemote({ endpoint: primary.endpoint, token: primary.token });
    const settings = await native.readSettings(id);
    expect(settings.settings.model?.titleModel).toMatchObject({ mode: 'explicit', model: 'titles/short', max_output_tokens: 128 });
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Investigate the cache invalidation bug');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await auxiliary.gate('title-dispatched');
    await expect(page.getByText('Primary task completed.', { exact: true })).toBeVisible();
    expect((await native.readSession(id)).generated_title).toBeNull();
    await auxiliary.release('title-dispatched');
    await expect(page.getByLabel('Session title', { exact: true })).toHaveText('Cache invalidation investigation');
    const generated = await native.readSession(id);
    expect(generated.name).toBeNull();
    expect(generated.generated_title).toMatchObject({ text: 'Cache invalidation investigation', provenance: { type: 'local' } });
    expect((await native.readSettings(id)).revision).toBe(settings.revision);
    await page.screenshot({ path: info.outputPath(`automatic-title-${width}.png`) });
    const sidebar = page.getByRole('button', { name: 'Expand Sidebar', exact: true });
    if (await sidebar.isVisible()) await sidebar.click();
    await page.locator(`button[data-session-id="${id}"]`).hover();
    await page.getByRole('button', { name: 'Session actions for Cache invalidation investigation', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Rename', exact: true }).click();
    await page.getByLabel('Session name', { exact: true }).fill('Manual cache review');
    await page.getByRole('button', { name: 'Rename', exact: true }).click();
    await expect(page.getByLabel('Session title', { exact: true })).toHaveText('Manual cache review');
    expect((await native.readSession(id)).generated_title).toEqual(generated.generated_title);
    await page.reload();
    await connectRemote(page, primary.endpoint, primary.token);
    const expand = page.getByRole('button', { name: 'Expand Sidebar', exact: true });
    if (await expand.isVisible()) await expand.click();
    await page.locator(`button[data-session-id="${id}"]`).click();
    await expect(page.getByLabel('Session title', { exact: true })).toHaveText('Manual cache review');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
    passed = true;
  } finally {
    await native?.shutdown();
    await page.close();
    const [mainReport, titleReport] = await Promise.all([primary.stop(passed), auxiliary.stop(passed)]);
    if (passed) { expect(mainReport.requestCount).toBe(1); expect(titleReport.requestCount).toBe(1); }
  }
});

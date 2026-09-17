import { test, expect } from '@playwright/test';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Launcher } from '../../../dev/src/launcher.ts';
import { parseArguments } from '../../../dev/src/arguments.ts';
import { spawnOwned } from '../../../dev/src/process.ts';

/** Normal local composition: no dogfood helper, provider process, or Host proxy. */
test('development launcher serves the real Web carrier, native App Server and exact-root Host', async ({ page }) => {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-dev-browser-'));
  const root = fileURLToPath(new URL('../../../', import.meta.url));
  const workspace = join(directory, 'workspace with spaces'); mkdirSync(workspace);
  const nested = join(workspace, 'unauthorized descendant'); mkdirSync(nested);
  const settings = join(directory, 'rustx.toml'); writeFileSync(settings, `[agent.model]
model = "local/local-model"
[providers.local]
base_url = "http://127.0.0.1:1/v1"
api_key = "launcher-test-unused"
[models."local/local-model"]
provider = "local"
id = "local-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 4096
capabilities = { input_modalities = ["text"], output_modalities = ["text"], tool_calls = true, reasoning = false }
compat = { chat_reasoning_replay = "omit" }
`);
  const marker = join(workspace, 'user-owned'); writeFileSync(marker, 'keep');
  const pids: number[] = [];
  const launcher = new Launcher(root, (spec, exited, ownerShutdown) => {
    const child = spawnOwned(spec, exited, ownerShutdown); pids.push(child.pid!); return child;
  });
  let scratch: string | undefined;
  try {
    const ready = await launcher.start(parseArguments(['web', '--config', settings,
      '--runtime-root', join(directory, 'runtime'), '--workspace', workspace], root));
    expect(ready).toBeDefined();
    if (!ready) throw new Error(`Launcher failed: ${await launcher.done}`);
    scratch = dirname(ready.tokenFile);
    const token = readFileSync(ready.tokenFile, 'utf8');
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    await page.goto(ready.url);
    await page.getByLabel('WebSocket endpoint').fill(ready.endpoint);
    await page.getByLabel('Transport token').fill(token);
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(page.locator('.status strong')).toHaveText('connected');
    await expect(page.getByLabel('Choose Workspace')).toContainText('workspace with spaces');
    const catalog = await (await page.request.post(`${ready.url}product-host/list`, { data: {} })).json();
    expect(catalog.endpoint).toBe(ready.endpoint);
    expect(catalog.workspaces.map((row: { displayPath: string }) => row.displayPath)).toEqual([workspace]);
    const classified = await (await page.request.post(`${ready.url}product-host/classify`, { data: { endpoint: ready.endpoint, cwds: [workspace, nested, directory] } })).json();
    expect(classified.map((row: { authorized: boolean }) => row.authorized)).toEqual([true, false, false]);
    expect(await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } }))).not.toContain(token);
    await page.getByLabel('Choose Workspace').selectOption({ label: 'workspace with spaces' });
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(page.locator('.session-toolbar small')).toContainText('attached');
    expect(errors).toEqual([]);
    expect(pids).toHaveLength(2);
    expect(await launcher.settle(130)).toBe(130);
    for (const pid of pids) expect(() => process.kill(pid, 0)).toThrow();
    expect(existsSync(scratch)).toBe(false);
    expect(readFileSync(marker, 'utf8')).toBe('keep');
    expect(existsSync(join(directory, 'runtime'))).toBe(true);
  } finally {
    await launcher.settle(1);
    if (scratch) expect(existsSync(scratch)).toBe(false);
    rmSync(directory, { recursive: true, force: true });
  }
});

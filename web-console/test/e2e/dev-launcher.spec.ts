import { chooseWorkspace, openConnectionSettings, closeSettings } from './shell-actions';
import { test, expect } from '@playwright/test';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Launcher } from '../../../dev/src/launcher.ts';
import { parseArguments } from '../../../dev/src/arguments.ts';
import { spawnOwned } from '../../../dev/src/process.ts';
import { BROWSER_SESSION_HEADER, BROWSER_SESSION_STORAGE } from '../../browser-session';

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
    const ready = await launcher.start(parseArguments(['web', '--no-open', '--config', settings,
      '--runtime-root', join(directory, 'runtime'), '--workspace', workspace], root));
    expect(ready).toBeDefined();
    if (!ready) throw new Error(`Launcher failed: ${await launcher.done}`);
    scratch = dirname(ready.tokenFile);
    const token = readFileSync(ready.tokenFile, 'utf8');
    const launchToken = new URL(ready.url).searchParams.get('token')!;
    expect(launchToken).not.toBe(token);
    // Real native admission: browser credentials cannot enter the v9 transport.
    await new Promise<void>((resolve, reject) => {
      const socket = new WebSocket(ready.endpoint, ['rustx.app-server.v15', `rustx-token.${launchToken}`]);
      socket.onopen = () => { socket.close(); reject(new Error('Browser token admitted by native App Server')); };
      socket.onerror = () => resolve();
    });
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    await page.goto(ready.url);
    const clean = new URL('/', ready.url).href;
    await expect(page).toHaveURL(clean);
    await expect(page.getByLabel('WebSocket endpoint')).toHaveCount(0);
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
    await expect(page.getByRole('region', { name: 'Connection recovery' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Select Workspace workspace with spaces' })).toBeVisible();
    await page.screenshot({ path: test.info().outputPath('local-managed.png') });
    const proof = await page.evaluate(key => sessionStorage.getItem(key), BROWSER_SESSION_STORAGE);
    expect(proof).toMatch(/^[A-Za-z0-9_-]{43}$/); expect([token, launchToken]).not.toContain(proof);
    await new Promise<void>((resolve, reject) => {
      const socket = new WebSocket(ready.endpoint, ['rustx.app-server.v15', `rustx-token.${proof}`]);
      socket.onopen = () => { socket.close(); reject(new Error('Browser session proof admitted by native App Server')); };
      socket.onerror = () => resolve();
    });
    const headers = { [BROWSER_SESSION_HEADER]: proof! };
    const bootstrap = await page.request.get(`${clean}__rustx/bootstrap`, { headers });
    expect(bootstrap.headers()['cache-control']).toBe('no-store');
    expect(await bootstrap.json()).toEqual({ connectionMode: 'local', appServerEndpoint: ready.endpoint, appServerTransportToken: token });
    const catalog = await (await page.request.post(`${clean}product-host/list`, { headers, data: {} })).json();
    expect(catalog.endpoint).toBe(ready.endpoint);
    expect(catalog.workspaces.map((row: { displayPath: string }) => row.displayPath)).toEqual([workspace]);
    const classified = await (await page.request.post(`${clean}product-host/classify`, { headers, data: { endpoint: ready.endpoint, cwds: [workspace, nested, directory] } })).json();
    expect(classified.map((row: { authorized: boolean }) => row.authorized)).toEqual([true, false, false]);
    expect((await page.request.post(`${clean}product-host/adopt`, { headers, data: { location: nested } })).status()).toBe(400);
    expect(await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } }))).not.toContain(token);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Overview', exact: true })).toHaveAttribute('aria-current', 'page');
    await openConnectionSettings(page);
    await expect(page.getByLabel('Connection mode')).toHaveValue('local');
    await page.getByLabel('Connection mode').selectOption('remote');
    await expect(page.getByText('Active mode: Local managed connection', { exact: true })).toBeVisible();
    await expect(page.locator('.connection-status')).toHaveText('Connected');
    await page.getByLabel('WebSocket endpoint').fill('ws://user:password@127.0.0.1:1/');
    await page.getByLabel('Transport token').fill(token);
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Connection Settings' }).getByRole('alert')).toContainText('no credentials');
    await page.getByLabel('WebSocket endpoint').fill(ready.endpoint);
    await page.getByLabel('Transport token').fill(launchToken);
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Connection Settings' }).getByRole('alert')).toContainText('WebSocket');
    await expect(page.getByLabel('Connection mode')).toHaveValue('remote');
    await page.getByLabel('Transport token').fill(token);
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(page.locator('.connection-status')).toHaveText('Connected');
    await page.screenshot({ path: test.info().outputPath('remote-settings.png') });
    expect(await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } }))).not.toContain(token);
    await page.route('**/__rustx/bootstrap', route => route.fulfill({ status: 503, body: 'Unavailable' }));
    await page.getByLabel('Connection mode').selectOption('local');
    await expect(page.getByRole('region', { name: 'Connection Settings' }).getByRole('alert')).toContainText('No local managed connection');
    await expect(page.getByLabel('Connection mode')).toHaveValue('local');
    await expect(page.locator('.connection-status')).toHaveText('Connected');
    await expect(page.getByText('Active mode: Remote App Server', { exact: true })).toBeVisible();
    await page.unroute('**/__rustx/bootstrap');
    await page.route('**/__rustx/bootstrap', route => route.fulfill({ json: { connectionMode: 'local', appServerEndpoint: ready.endpoint, appServerTransportToken: launchToken } }));
    await page.getByLabel('Connection mode').selectOption('local');
    await expect(page.getByRole('region', { name: 'Connection Settings' }).getByRole('alert')).toContainText('WebSocket');
    await expect(page.getByLabel('Connection mode')).toHaveValue('local');
    await expect(page.getByText('Active mode: Local managed connection', { exact: true })).toBeVisible();
    await expect(page.locator('.connection-status')).toHaveText('Disconnected');
    await closeSettings(page);
    await page.getByRole('button', { name: 'Show details', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Connection', exact: true })).toHaveAttribute('aria-current', 'page');
    await page.unroute('**/__rustx/bootstrap');
    await page.getByRole('region', { name: 'Connection Settings' }).getByRole('button', { name: 'Reconnect', exact: true }).click();
    await expect(page.locator('.connection-status')).toHaveText('Connected');
    await closeSettings(page);
    expect(await page.evaluate(() => indexedDB.databases())).toEqual([]);
    expect(await page.evaluate(() => document.cookie)).not.toContain('rustx-browser');
    expect(await page.context().cookies(clean)).toEqual([]);
    expect(await page.evaluate(() => Object.keys(sessionStorage))).toEqual([BROWSER_SESSION_STORAGE]);
    await chooseWorkspace(page, 'workspace with spaces');
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    expect(await page.evaluate(key => sessionStorage.getItem(key), BROWSER_SESSION_STORAGE)).toBe(proof);
    await page.reload();
    await expect(page).toHaveURL(clean);
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    expect(await page.evaluate(key => sessionStorage.getItem(key), BROWSER_SESSION_STORAGE)).toBe(proof);
    expect(await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } }))).not.toContain(launchToken);
    expect(await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } }))).not.toContain(token);
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

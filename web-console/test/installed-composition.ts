/** The extracted Web bundle, with neither test routes nor a substitute Host. */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { chromium, expect } from '@playwright/test';
import { chooseWorkspace, returnHome, selectedSessionId } from './e2e/shell-actions.ts';

const [url, runtime, workspace, mode] = process.argv.slice(2);
if (!url || !runtime || !workspace || (mode !== 'granted' && mode !== 'exact-only' && mode !== 'resumed')) {
  throw new Error('Expected the startup URL, runtime path, Workspace and picker mode');
}
const rootLabel = 'workspace';
function clientProcesses(workspacePath: string): number[] {
  return execFileSync('ps', ['-axww', '-o', 'pid=,command='], { encoding: 'utf8' })
    .split('\n')
    .filter(row => row.includes('/rustx-execution-client ') && row.trimEnd().endsWith(` --workspace ${workspacePath}`))
    .map(row => Number(row.trimStart().split(/\s+/, 1)[0]));
}
function exited(pid: number): boolean {
  try { process.kill(pid, 0); return false; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ESRCH') return true;
    throw error;
  }
}
const browser = await chromium.launch({ executablePath: process.env.RUSTX_INSTALLED_BROWSER_EXECUTABLE });
try {
  const page = await browser.newPage({ locale: 'en-US', viewport: { width: 1440, height: 900 } });
  page.setDefaultTimeout(20_000);
  await page.addInitScript(() => localStorage.setItem('rustx-locale-v1', 'en'));
  const errors: string[] = [];
  const skillQueries: Array<{ target: { kind: string }; execution_session?: string }> = [];
  page.on('websocket', socket => socket.on('framesent', ({ payload }) => {
    if (typeof payload !== 'string') return;
    const request = JSON.parse(payload);
    if (request.method === 'skills/list' && request.params.target.kind === 'workspace') {
      skillQueries.push(request.params);
    }
  }));
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url);
  const sidebar = page.getByRole('region', { name: 'Workspaces and Sessions' });
  const add = sidebar.getByRole('button', { name: 'Add Workspace', exact: true });
  const dialog = page.getByRole('dialog', { name: 'Add Workspace', exact: true });
  const root = sidebar.getByRole('button', { name: 'Select Workspace workspace', exact: true });
  await expect(root).toBeVisible({ timeout: 20_000 });

  const state = join(dirname(runtime), 'browser-session.json');
  const pending = (session: string) => join(runtime, 'execution-pending', createHash('sha256').update(session).digest('hex') + '.json');
  const row = (id: string) => page.locator(`button[data-session-id="${id}"]`);
  function binding(id: string, cwd: string) {
    const lane = JSON.parse(readFileSync(pending(id), 'utf8'));
    expect(lane).toMatchObject({ session: id, workspace: cwd });
    const processes = clientProcesses(cwd);
    expect(processes.length).toBeGreaterThan(0);
    return { id, instance: lane.instance, environment: lane.environment, processes };
  }
  async function toolTurn(name: string, cwd: string) {
    await page.getByRole('textbox', { name: 'Message' }).fill(`exercise ${name}`);
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByText(`Installed tools complete for ${name}`, { exact: true })).toBeVisible({ timeout: 40_000 });
    await expect(page.getByRole('button', { name: 'Regenerate', exact: true }).last()).toBeEnabled();
    expect(readFileSync(join(cwd, 'written.txt'), 'utf8')).toBe(`write-${name}\n`);
    expect(readFileSync(join(cwd, 'bash-cwd.txt'), 'utf8').trim()).toBe(cwd);
  }
  async function remove(id: string, processes: readonly number[]) {
    await returnHome(page);
    await row(id).hover();
    await page.getByRole('treeitem').filter({has: row(id)}).getByRole('button', { name: 'Delete Session', exact: true }).click();
    await page.getByRole('dialog', { name: 'Delete this session?' }).getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(row(id)).toHaveCount(0);
    await expect.poll(() => existsSync(pending(id)), { timeout: 20_000 }).toBe(false);
    await expect.poll(() => processes.every(exited), { timeout: 20_000 }).toBe(true);
  }

  if (mode === 'exact-only') {
    await root.hover();
    await sidebar.getByRole('button', { name: 'Workspace actions for workspace' }).click();
    await page.getByRole('menuitem', { name: 'Unregister Workspace', exact: true }).click();
    await page.getByRole('dialog', { name: 'Unregister workspace?' }).getByRole('button', { name: 'Unregister', exact: true }).click();
    await expect(root).toHaveCount(0);
    await add.click();
    await dialog.getByRole('button', { name: rootLabel, exact: true }).click();
    await expect(dialog.getByText(/requires a configured environment root/i)).toBeVisible({ timeout: 20_000 });
    await expect(dialog.getByRole('button', { name: 'project', exact: true })).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Add folder', exact: true }).click();
    await expect(root).toBeVisible();
    await expect(dialog).toHaveCount(0);
  } else if (mode === 'resumed') {
    const original = JSON.parse(readFileSync(state, 'utf8'));
    expect(existsSync(pending(original.id))).toBe(false);
    expect(clientProcesses(workspace)).toEqual([]);
    await row(original.id).click();
    await expect(page.getByText('Installed tools complete for web-root', { exact: true })).toBeVisible();
    await toolTurn('web-resume', workspace);
    const resumed = binding(original.id, workspace);
    expect(await selectedSessionId(page)).toBe(original.id);
    expect(resumed.instance).not.toBe(original.instance);
    await remove(resumed.id, resumed.processes);
  } else {
    await add.click();
    await dialog.getByRole('button', { name: rootLabel, exact: true }).click();
    await dialog.getByRole('button', { name: 'project', exact: true }).click();
    const registration = page.waitForResponse(response =>
      new URL(response.url()).pathname === '/product-host/register-workspace-folder'
      && response.request().method() === 'POST');
    await dialog.getByRole('button', { name: 'Add folder', exact: true }).click();
    const receipt = await registration;
    expect(receipt.ok(), await receipt.text()).toBe(true);
    await expect(dialog).toHaveCount(0);
    await chooseWorkspace(page, rootLabel);
    await toolTurn('web-root', workspace);
    const first = binding(await selectedSessionId(page), workspace);

    // The Home catalog has a Workspace and a draft, with no Session identity.
    await chooseWorkspace(page, rootLabel);
    await expect(page.locator('button[data-session-id][aria-current="page"]')).toHaveCount(0);
    const earlierQueries = skillQueries.length;
    await page.getByRole('button', { name: 'Skills', exact: true }).click();
    const skills = page.getByRole('region', { name: 'Skills', exact: true });
    await skills.getByRole('tab', { name: 'Workspace', exact: true }).click();
    await expect(skills.getByRole('listitem', { name: 'installed-guide', exact: true })).toBeVisible({timeout: 20_000});
    await expect(skills).toHaveAttribute('data-catalog-authority', 'client');
    const queries = skillQueries.slice(earlierQueries);
    expect(queries.length).toBeGreaterThan(0);
    expect(queries.every(query => !query.execution_session)).toBe(true);
    await skills.getByRole('button', { name: 'installed-guide', exact: true }).click();
    const content = page.getByRole('dialog', { name: 'installed-guide', exact: true });
    await expect(content).toContainText('Private Client catalog marker.');
    await content.getByRole('button', { name: 'Close', exact: true }).click();
    await expect.poll(() => binding(first.id, workspace)).toEqual(first);

    await chooseWorkspace(page, 'project');
    const project = join(workspace, 'project');
    await toolTurn('web-child', project);
    const second = binding(await selectedSessionId(page), project);
    expect(first.id).not.toBe(second.id);
    expect(first.instance).not.toBe(second.instance);
    expect(first.environment).not.toBe(second.environment);

    await returnHome(page);
    await row(first.id).hover();
    await page.locator(`button[data-session-actions="${first.id}"]`).click();
    await page.getByRole('menuitem', { name: 'Fork session', exact: true }).click();
    const forkRow = page.locator(`button[data-session-id]:not([data-session-id="${first.id}"]):not([data-session-id="${second.id}"])`);
    await expect(forkRow).toBeVisible();
    const forkId = (await forkRow.getAttribute('data-session-id'))!;
    expect(existsSync(pending(forkId))).toBe(false);
    await forkRow.click();
    await toolTurn('web-fork', workspace);
    const fork = binding(forkId, workspace);
    expect(fork.instance).not.toBe(first.instance);
    expect(fork.environment).not.toBe(first.environment);
    const forkProcesses = fork.processes.filter(pid => !first.processes.includes(pid));
    expect(forkProcesses.length).toBeGreaterThan(0);
    await remove(fork.id, forkProcesses);
    await expect.poll(() => binding(first.id, workspace)).toEqual(first);
    await remove(second.id, second.processes);
    expect(readFileSync(join(project, 'written.txt'), 'utf8')).toBe('write-web-child\n');
    writeFileSync(state, JSON.stringify(first));
  }
  expect(errors).toEqual([]);
} finally {
  await browser.close();
}

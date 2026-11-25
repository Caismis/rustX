/** The extracted Web bundle, with neither test routes nor a substitute Host. */
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { chromium, expect } from '@playwright/test';
import { chooseWorkspace, returnHome, selectedSessionId } from './e2e/shell-actions.ts';

const [url, runtime, workspace, mode] = process.argv.slice(2);
if (!url || !runtime || !workspace || (mode !== 'granted' && mode !== 'exact-only')) {
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
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url);
  const sidebar = page.getByRole('region', { name: 'Workspaces and Sessions' });
  const add = sidebar.getByRole('button', { name: 'Add Workspace', exact: true });
  const dialog = page.getByRole('dialog', { name: 'Add Workspace', exact: true });
  const root = sidebar.getByRole('button', { name: 'Select Workspace workspace', exact: true });
  await expect(root).toBeVisible({ timeout: 20_000 });

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
  } else {
    await add.click();
    await dialog.getByRole('button', { name: rootLabel, exact: true }).click();
    await dialog.getByRole('button', { name: 'project', exact: true }).click();
    await dialog.getByRole('button', { name: 'Add folder', exact: true }).click();
    await expect(dialog).toHaveCount(0);

    const pending = (session: string) => join(runtime, 'execution-pending', createHash('sha256').update(session).digest('hex') + '.json');
    async function create(label: string, name: string, cwd: string) {
      await chooseWorkspace(page, label);
      await page.getByRole('textbox', { name: 'Message' }).fill(`exercise ${name}`);
      await page.getByRole('button', { name: 'Send', exact: true }).click();
      await expect(page.getByText(`Installed tools complete for ${name}`, { exact: true })).toBeVisible({ timeout: 40_000 });
      await expect(page.getByRole('button', { name: 'Regenerate', exact: true }).last()).toBeEnabled();
      const id = await selectedSessionId(page);
      const lane = JSON.parse(readFileSync(pending(id), 'utf8'));
      expect(lane).toMatchObject({ session: id, workspace: cwd });
      expect(readFileSync(join(cwd, 'written.txt'), 'utf8')).toBe(`write-${name}\n`);
      expect(readFileSync(join(cwd, 'bash-cwd.txt'), 'utf8').trim()).toBe(cwd);
      const processes = clientProcesses(cwd);
      expect(processes.length).toBeGreaterThan(0);
      return { id, instance: lane.instance, environment: lane.environment, processes };
    }
    const first = await create(rootLabel, 'web-root', workspace);
    const second = await create('project', 'web-child', join(workspace, 'project'));
    expect(first.id).not.toBe(second.id);
    expect(first.instance).not.toBe(second.instance);
    expect(first.environment).not.toBe(second.environment);

    for (const { id, processes } of [first, second]) {
      await returnHome(page);
      const row = page.locator(`button[data-session-id="${id}"]`);
      await row.hover();
      await page.getByRole('treeitem').filter({has: row}).getByRole('button', { name: 'Delete Session', exact: true }).click();
      await page.getByRole('dialog', { name: 'Delete this session?' }).getByRole('button', { name: 'Delete', exact: true }).click();
      await expect(row).toHaveCount(0);
      await expect.poll(() => existsSync(pending(id)), { timeout: 20_000 }).toBe(false);
      await expect.poll(() => processes.every(exited), { timeout: 20_000 }).toBe(true);
    }
    expect(readFileSync(join(workspace, 'written.txt'), 'utf8')).toBe('write-web-root\n');
    expect(readFileSync(join(workspace, 'project', 'written.txt'), 'utf8')).toBe('write-web-child\n');
  }
  expect(errors).toEqual([]);
} finally {
  await browser.close();
}

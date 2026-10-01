import { test, expect } from '@playwright/test';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { choose, connectRemote, connectionAction, openSettingsPage } from './shell-actions';

function deferred() { let resolve!: () => void; const promise = new Promise<void>(done => { resolve = done; }); return { promise, resolve }; }
for (const locale of ['en', 'zh'] as const) test(`UX-04 retains grouped and selected presentation while refresh is held (${locale})`, async ({ page }) => {
  const fixture = await startDogfood();
  const remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
  const gate = deferred(), reached = deferred();
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    const created = await remote.createSession({ cwd: fixture.workspaceA });
    const id = created.session.id;
    const workspace = (await fixture.workspaceHost.host.listWorkspaces()).workspaces[0];
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page).toHaveTitle(/rustX/);
    const group = page.locator(`[data-workspace-group="${workspace.id}"]`);
    const session = group.locator(`button[data-session-id="${id}"]`);
    await expect(session).toBeVisible(); await session.click();
    await expect(group.locator('[aria-current="page"]:not([data-session-id])')).toBeVisible();
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
    // Real reconnect goes through bootstrap/initialization and fresh admission.
    await connectionAction(page, 'Disconnect');
    await expect(session).toBeVisible(); await expect(group.locator('[aria-current="page"]:not([data-session-id])')).toBeVisible();
    await connectionAction(page, 'Reconnect');
    await expect(session).toBeVisible();
    if (locale === 'zh') {
      await page.getByRole('button', { name: 'Settings', exact: true }).click();
      await openSettingsPage(page, 'General');
      await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
      await page.getByRole('button', { name: '关闭设置' }).click();
    }
    await page.route('**/product-host/classify', async route => {
      reached.resolve(); await gate.promise; await route.fallback();
    });
    const tx = locale === 'en' ? { options: 'View options', refresh: 'Refresh list', pending: 'Refreshing Workspace associations…' } : { options: '视图选项', refresh: '刷新列表', pending: '正在刷新工作区关联…' };
    await page.getByRole('button', { name: tx.options, exact: true }).click();
    await page.getByRole('menuitem', { name: tx.refresh, exact: true }).click();
    await reached.promise;
    // These assertions execute before releasing the Host reply.
    await expect(session).toBeVisible(); await expect(group.locator('[aria-current="page"]:not([data-session-id])')).toBeVisible();
    await expect(page.getByRole('status').filter({ hasText: tx.pending })).toBeVisible();
    await session.focus(); await expect(session).toBeFocused();
    await page.screenshot({ path: `test-results/ux-04-held-${locale}.png` });
    gate.resolve();
    await expect(page.getByRole('status').filter({ hasText: tx.pending })).toHaveCount(0);
    await expect(session).toBeVisible(); expect(errors).toEqual([]);
  } finally { gate.resolve(); await remote.shutdown(); await fixture.stop(false); }
});

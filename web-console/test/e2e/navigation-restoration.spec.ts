import { expect, test } from '@playwright/test';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { startDogfood } from './dogfood-server';
import { connectRemote, openConnectionSettings } from './shell-actions';
import { wireProbe } from './wire-probe';
import { routeWorkspaceHost } from './workspace-host';
import { sameTarget } from '../../../protocol/app-server/attachment-identity';

test('a real page reload restores the last selected Session on the same native authority', async ({ page }) => {
  const fixture = await startDogfood();
  let native: AppServerHost | undefined;
  try {
    native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const a = (await native.createSession({ cwd: fixture.workspaceA })).session;
    const b = (await native.createSession({ cwd: fixture.workspaceB })).session;
    const bThread = (await native.readNode(b.id, b.active_node)).conversation_id;
    await native.renameSession(a.id, 'Reload Session A');
    await native.renameSession(b.id, 'Reload Session B');
    const originalIds = (await native.listSessions()).sessions.map(session => session.id).sort();
    expect(originalIds).toEqual([a.id, b.id].sort());

    const wire = await wireProbe(page);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    const rowA = page.locator(`button[data-session-id="${a.id}"]`);
    const rowB = page.locator(`button[data-session-id="${b.id}"]`);
    await rowA.click();
    await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
    await expect(page.getByLabel('Session title', { exact: true })).toHaveText('Reload Session A');
    await rowB.click();
    await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
    await expect(rowB).toHaveAttribute('aria-current', 'page');
    await expect(page.getByLabel('Session title', { exact: true })).toHaveText('Reload Session B');
    const initialized = wire.responses.filter(row => row.method === 'initialize');
    expect(initialized).toHaveLength(1);
    const authority = initialized[0].result.authority_id;
    const beforeReload = wire.requests.length;
    const beforeReloadResponses = wire.responses.length;

    // This replaces the whole document and client, rather than simulating a
    // React remount. The same isolated Native process keeps its endpoint and
    // authority; credentials must be supplied again through ordinary Settings.
    await page.reload();
    await expect.poll(async () => (await native!.client.call('server/diagnostics', {}, 'diagnostics')).snapshot.external_attachments).toBe(0);
    await openConnectionSettings(page);
    await expect(page.getByLabel('Connection mode', { exact: true })).toHaveValue('local');
    await page.getByLabel('Connection mode', { exact: true }).selectOption('remote');
    await expect(page.getByLabel('Access token', { exact: true })).toHaveValue('');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
    await expect(rowB).toHaveAttribute('aria-current', 'page');
    await expect(rowA).not.toHaveAttribute('aria-current', 'page');
    await expect(page.getByLabel('Session title', { exact: true })).toHaveText('Reload Session B');

    const restored = wire.requests.slice(beforeReload).filter(row => row.method === 'thread/resume');
    // Other saved views restore concurrently; their wire order is incidental.
    expect(restored.some(row => row.params.threadId === bThread)).toBe(true);
    const target = await wire.attachedTarget(b.id);
    expect(target.conversation_id).toBe(bThread);
    for (const [method, resultType] of [['session/checkpoint', 'checkpoint'], ['session/subscribe', 'subscribed']]) {
      await expect.poll(() => wire.requests.slice(beforeReload).some(request => request.method === method && sameTarget(request.params.target, target)
        && wire.responses.slice(beforeReloadResponses).some(response => response.id === request.id && response.method === method && response.result?.type === resultType))).toBe(true);
    }
    await expect(rowB).toHaveAttribute('aria-current', 'page');
    await expect(rowA).not.toHaveAttribute('aria-current', 'page');
    await expect(page.getByLabel('Session title', { exact: true })).toHaveText('Reload Session B');
    expect(wire.responses.filter(row => row.method === 'initialize').map(row => row.result.authority_id)).toEqual([authority, authority]);
    expect((await native.listSessions()).sessions.map(session => session.id).sort()).toEqual(originalIds);
    expect(wire.requests.filter(row => ['thread/start', 'session/submissionIntent', 'turn/start'].includes(row.method))).toEqual([]);
    expect((await fixture.control('requests')).requests).toEqual([]);
    expect(await page.evaluate(token => [...Object.values(localStorage), ...Object.values(sessionStorage)].some(value => value.includes(token)), fixture.token)).toBe(false);
  } catch (error) {
    console.error(fixture.diagnostics());
    throw error;
  } finally {
    try { await page.close(); }
    finally {
      try { await native?.shutdown(); }
      finally { await fixture.stop(false); }
    }
  }
});

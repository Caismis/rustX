import { expect, test } from '@playwright/test';
import { appendFileSync } from 'node:fs';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { startDogfood } from './dogfood-server';
import { connectRemote, openConnectionSettings } from './shell-actions';
import { wireProbe } from './wire-probe';
import { routeWorkspaceHost } from './workspace-host';
import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { selectCossOption } from './coss-select';

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
    await expect(page.getByLabel('Connection mode', { exact: true })).toHaveAttribute('data-value', 'local');
    await selectCossOption(page.getByLabel('Connection mode', { exact: true }), 'remote');
    await expect(page.getByLabel('Access token', { exact: true })).toHaveValue('');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
    await expect(rowB).toHaveAttribute('aria-current', 'page');
    await expect(rowA).not.toHaveAttribute('aria-current', 'page');
    await expect(page.getByLabel('Session title', { exact: true })).toHaveText('Reload Session B');

    const restored = wire.requests.slice(beforeReload).filter(row => row.method === 'thread/resume');
    expect(restored.map(row => row.params.threadId)).toEqual([bThread]);
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

test('opening a dozen existing sessions and returning to the first keeps the conversation usable', async ({ page }) => {
  const fixture = await startDogfood();
  let native: AppServerHost | undefined;
  try {
    native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const ids: string[] = [];
    for (let i = 0; i < 12; i++) {
      const { session } = await native.createSession({ cwd: fixture.workspaceA });
      await native.renameSession(session.id, `Saved session ${i + 1}`);
      ids.push(session.id);
    }
    const wire = await wireProbe(page);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    for (const i of [...ids.keys(), 0]) {
      await page.locator(`button[data-session-id="${ids[i]}"]`).click();
      await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
      await expect(page.getByLabel('Session title', { exact: true })).toHaveText(`Saved session ${i + 1}`);
      await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEditable();
    }
    expect(new Set(wire.responses.filter(row => row.method === 'thread/resume' && row.result?.type === 'attached').map(row => row.result.target.session_id))).toEqual(new Set(ids));
  } finally {
    try { await page.close(); }
    finally {
      try { await native?.shutdown(); }
      finally { await fixture.stop(false); }
    }
  }
});

test('a configured full runtime pool still serves the selected conversation history', async ({ page }) => {
  const fixture = await startDogfood('web_fork_feedback');
  let native: AppServerHost | undefined;
  try {
    native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const reader = (await native.createSession({ cwd: fixture.workspaceA })).session;
    const busy = (await native.createSession({ cwd: fixture.workspaceB })).session;
    await native.renameSession(reader.id, "Saved conversation with history");
    const session = await native.attach(reader.id);
    await session.submitInbound([{ type: 'text', text: 'Fork source' }]);
    await expect.poll(() => JSON.stringify(session.state.transcript)).toContain('Completed fork answer.');
    await native.shutdown();
    native = undefined;

    appendFileSync(fixture.settings, '\n[app_server]\nmax_resident_runtimes = 1\n');
    await fixture.restartApp();
    native = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    await native.attach(busy.id);
    const wire = await wireProbe(page);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await page.locator(`button[data-session-id="${reader.id}"]`).click();
    const transcript = page.getByLabel('Canonical conversation');
    await expect(transcript.getByText('Fork source', { exact: true })).toBeVisible();
    await expect(transcript.getByText('Completed fork answer.', { exact: true })).toBeVisible();
    await expect(page.getByText(/Operation rejected.*capacity/)).toHaveCount(0);
    const resumesBeforeTrajectory = wire.requests.filter(row => row.method === 'thread/resume').length;
    expect(resumesBeforeTrajectory).toBe(1);
    await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await expect(ledger).toBeVisible();
    await expect(ledger.getByText('Fork source')).toBeVisible();
    await expect(ledger.getByText('Completed fork answer.')).toBeVisible();
    const requestRow = ledger.locator('[data-request-owner]').first();
    await expect(requestRow).toBeVisible();
    await requestRow.click();
    const inspector = page.getByRole('complementary', { name: 'Event details' });
    await expect(inspector).toBeVisible();
    await expect(inspector.getByRole('tabpanel')).toBeVisible();
    await expect.poll(() => wire.responses.some(row => row.method === 'session/traceHistoryDetail' && row.result?.type === 'session_trace_history_detail')).toBe(true);
    expect(wire.requests.filter(row => row.method === 'thread/resume')).toHaveLength(resumesBeforeTrajectory);
    await page.getByRole('tab', { name: 'Chat', exact: true }).click();
    const resume = page.getByRole('button', { name: 'Resume chat', exact: true });
    await expect(resume).toBeVisible();
    fixture.writeSettings();
    const owner = native;
    await expect.poll(async () => (await owner.client.call('server/diagnostics', {}, 'diagnostics')).snapshot.policy.max_resident_runtimes).toBeUndefined();
    await resume.click();
    await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route', 'session');
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEditable();
    await expect(transcript.getByText('Completed fork answer.', { exact: true })).toBeVisible();
    expect((await fixture.control('requests')).requests).toHaveLength(1);
  } finally {
    try { await page.close(); }
    finally {
      try { await native?.shutdown(); }
      finally { await fixture.stop(false); }
    }
  }
});

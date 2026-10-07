import { expect, test, type Page } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openSettingsPage } from './shell-actions';

/** Observe the real socket and hold selected server responses until the test
 * releases them explicitly. Nothing is manufactured: every delivered byte is
 * the real App Server response, only its delivery moment is controlled. */
async function observationProbe(page: Page) {
  const requests: string[] = [];
  const notifications: string[] = [];
  const heldReads: { payload: any; release: () => void }[] = [];
  const heldWrites: { payload: any; release: () => void }[] = [];
  let holdReads = false;
  let holdWrites = false;
  await page.routeWebSocket(/ws:\/\//, socket => {
    const server = socket.connectToServer();
    const methods = new Map<string | number, string>();
    socket.onMessage(message => {
      const parsed = JSON.parse(String(message));
      if (parsed.method && parsed.id !== undefined) {
        methods.set(parsed.id, parsed.method);
        requests.push(parsed.method);
      }
      server.send(message);
    });
    server.onMessage(message => {
      const parsed = JSON.parse(String(message));
      if (parsed.method) {
        notifications.push(parsed.method);
        socket.send(message);
        return;
      }
      const method = methods.get(parsed.id);
      if (method) methods.delete(parsed.id);
      if (method === 'configuration/sourcesRead' && holdReads && parsed.result) {
        heldReads.push({ payload: parsed.result, release: () => socket.send(message) });
        return;
      }
      if (method === 'configuration/sourceWrite' && holdWrites && parsed.result) {
        heldWrites.push({ payload: parsed.result, release: () => socket.send(message) });
        return;
      }
      socket.send(message);
    });
  });
  return {
    requests, notifications, heldReads, heldWrites,
    armReads: () => { holdReads = true; },
    releaseReads: () => { holdReads = false; for (const read of heldReads.splice(0)) read.release(); },
    armWrites: () => { holdWrites = true; },
    releaseWrites: () => { holdWrites = false; for (const write of heldWrites.splice(0)) write.release(); },
    readsIssued: () => requests.filter(method => method === 'configuration/sourcesRead').length,
    writes: () => requests.filter(method => method === 'configuration/sourceWrite').length,
    changed: () => notifications.filter(method => method === 'configuration/changed').length,
  };
}

test('publications and an acknowledgement during outstanding convergence reads converge without replay', async ({ page }) => {
  const f = await startDogfood();
  const probe = await observationProbe(page);
  try {
    await routeWorkspaceHost(page, f); await page.goto('/'); await connectRemote(page, f.endpoint, f.token);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    // Process policy and source revisions are both on Advanced.
    await openSettingsPage(page, 'Advanced');
    await expect(settings.getByText(/Revision:/)).toBeVisible();
    const field = settings.getByLabel('max_connections', { exact: true });
    const save = settings.getByRole('button', { name: 'Save App Server policy', exact: true });
    const notice = settings.getByText('App Server policy saved. Native coordination owns application.');
    // Settle a save whose acknowledgement already landed: release held
    // authoritative reads one at a time until the post-commit read is adopted.
    // Each stale or superseded read may owe exactly one more bounded read.
    const settle = async () => {
      for (let attempt = 0; attempt < 12 && !(await notice.isVisible()); attempt++) {
        await expect.poll(() => probe.heldReads.length).toBeGreaterThan(0);
        probe.heldReads.shift()!.release();
        await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => resolve(undefined))));
      }
      await expect(notice).toBeVisible();
    };

    // First save: acknowledgements flow, authoritative reads are held.
    probe.armReads();
    await field.fill('19');
    await save.click();
    await expect.poll(() => probe.readsIssued()).toBeGreaterThan(1);
    await settle();
    expect(probe.writes()).toBe(1);

    // Second save while publications and convergence reads are outstanding:
    // the acknowledgement neither cancels the reads nor poses as the
    // observation; every obligation converges through the same read owner.
    const changedBefore = probe.changed();
    await field.fill('20');
    await save.click();
    await expect.poll(() => probe.changed()).toBeGreaterThan(changedBefore);
    await settle();
    probe.releaseReads();
    await expect(field).toHaveValue('20');
    expect(probe.writes()).toBe(2);
    // No spin remains after the obligations settle: nothing else is read.
    const settledReads = probe.readsIssued();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => resolve(undefined))));
    expect(probe.readsIssued()).toBe(settledReads);
    expect(probe.heldReads).toHaveLength(0);
  } finally { await f.stop(false); }
});

test('a delayed write acknowledgement cannot regress a newer authoritative source observation', async ({ page }) => {
  const f = await startDogfood();
  const probe = await observationProbe(page);
  const remote = await AppServerHost.connectRemote({ endpoint: f.endpoint, token: f.token });
  try {
    await routeWorkspaceHost(page, f); await page.goto('/'); await connectRemote(page, f.endpoint, f.token);
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    // Process policy and source revisions are both on Advanced.
    await openSettingsPage(page, 'Advanced');
    await expect(settings.getByText(/Revision:/)).toBeVisible();

    // The write commits natively, but its acknowledgement is held on the wire.
    probe.armWrites();
    await settings.getByLabel('max_connections', { exact: true }).fill('19');
    await settings.getByRole('button', { name: 'Save App Server policy', exact: true }).click();
    await expect.poll(() => probe.heldWrites.length).toBe(1);

    // A later authored change lands outside this client and is observed
    // through native publication and an authoritative read before the
    // acknowledgement arrives.
    const authored = readFileSync(f.settings, 'utf8');
    expect(authored).toContain('stream_idle_timeout_ms = 600000');
    writeFileSync(f.settings, authored.replace('stream_idle_timeout_ms = 600000', 'stream_idle_timeout_ms = 600001'));
    const revision = settings.getByText(/Revision:/);
    const before = (await revision.textContent()) ?? '';
    await remote.client.call('configuration/reconcile', { target: { kind: 'user' } }, 'configuration_application');
    await expect(revision).not.toHaveText(before);
    const observed = (await revision.textContent()) ?? '';

    // The delayed acknowledgement settles only the save outcome. The visible
    // whole projection stays the authoritative one and nothing is replayed.
    probe.releaseWrites();
    await expect(settings.getByText('App Server policy saved. Native coordination owns application.')).toBeVisible();
    await expect(revision).toHaveText(observed);
    expect(probe.writes()).toBe(1);
  } finally { await remote.shutdown(); await f.stop(false); }
});

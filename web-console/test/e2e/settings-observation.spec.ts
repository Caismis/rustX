import { expect, test, type Page } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { AppServerHost } from '../../../tui/src/app-server/host';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openSettingsPage, settledSave } from './shell-actions';
import { readSource } from './source-reading';

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
    const settings = page.getByRole('region', { name: 'Settings', exact: true });
    // Runtime limits live on Advanced; coordination metadata stays internal.
    await openSettingsPage(page, 'Advanced');
    await expect(settings.locator('[data-lifecycle=ready]')).toBeVisible();
    const field = settings.getByLabel('Response start timeout (ms)', { exact: true });
    const save = settings.getByRole('button', { name: 'Save Model timeout', exact: true });
    const notice = settledSave(settings, 'Model timeout');
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

    // The initial observation must finish before holding post-write reads;
    // otherwise the gate itself prevents the policy form becoming editable.
    await expect(field).toBeEnabled();
    // First save: acknowledgements flow, authoritative reads are held.
    probe.armReads();
    await field.fill('19000');
    await save.click();
    await expect.poll(() => probe.readsIssued()).toBeGreaterThan(1);
    await settle();
    expect(probe.writes()).toBe(1);

    // Second save while publications and convergence reads are outstanding:
    // the acknowledgement neither cancels the reads nor poses as the
    // observation; every obligation converges through the same read owner.
    const changedBefore = probe.changed();
    await field.fill('20000');
    await save.click();
    await expect.poll(() => probe.changed()).toBeGreaterThan(changedBefore);
    await settle();
    probe.releaseReads();
    await expect(field).toHaveValue('20000');
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
    const settings = page.getByRole('region', { name: 'Settings', exact: true });
    // Runtime limits live on Advanced; coordination metadata stays internal.
    await openSettingsPage(page, 'Advanced');
    await expect(settings.locator('[data-lifecycle=ready]')).toBeVisible();

    // The write commits natively, but its acknowledgement is held on the wire.
    await expect(settings.getByLabel('Maximum concurrent subagents', { exact: true })).toBeEnabled();
    probe.armWrites();
    probe.armReads();
    await settings.getByLabel('Maximum concurrent subagents', { exact: true }).fill('19');
    await settings.getByRole('button', { name: 'Save Subagent limits', exact: true }).click();
    await expect.poll(() => probe.heldWrites.length).toBe(1);
    await expect.poll(() => probe.heldReads.length).toBeGreaterThan(0);

    // A later authored change lands outside this client and is observed
    // through native publication and an authoritative read before the
    // acknowledgement arrives.
    const authored = readFileSync(f.settings, 'utf8');
    expect(authored).toContain('stream_idle_timeout_ms = 600000');
    writeFileSync(f.settings, authored.replace('stream_idle_timeout_ms = 600000', 'stream_idle_timeout_ms = 600001'));
    const timeout = settings.getByLabel('Response idle timeout (ms)', { exact: true });
    await remote.client.call('configuration/reconcile', { target: { kind: 'user' } }, 'configuration_receipt');
    const latest = await readSource(remote, { kind: 'user' });
    expect(latest.user.authored?.model_timeout_policy?.stream_idle_timeout_ms).toBe('600001');
    // The held convergence read predates the external edit. Its delivery can
    // change the revision once before the newer publication is observed, so
    // wait for the exact external value rather than any unrelated update.
    probe.releaseReads();
    await expect(timeout).toHaveValue('600001');

    // The delayed acknowledgement settles only the save outcome. The visible
    // whole projection stays the authoritative one and nothing is replayed.
    probe.releaseWrites();
    await expect(settledSave(settings, 'Subagent limits')).toBeVisible();
    await expect(timeout).toHaveValue('600001');
    expect(probe.writes()).toBe(1);
  } finally { await remote.shutdown(); await f.stop(false); }
});

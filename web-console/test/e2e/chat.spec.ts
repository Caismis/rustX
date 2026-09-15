import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { AppServerHost } from '../../../tui/src/app-server/host';

test('native history pages, rich streaming settlement, reconnect and attachment refusal', async ({ page }) => {
  const fixture = await startDogfood('web_chat_history');
  let passed = false;
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    // Build durable history via typed native admission before the browser attaches.
    const remote = await AppServerHost.connectRemote({ endpoint: fixture.endpoint, token: fixture.token });
    const created = await remote.client.call('session/create', { settings: { cwd: fixture.workspaceA } }, 'session_transition');
    const id = created.session.id;
    const attached = await remote.client.call('session/attach', { session_id: id }, 'attached');
    await expect(remote.client.call('artifact/read', { target: attached.target, artifact_id: 'artifact_999' }, 'artifact_bytes')).rejects.toThrow();
    await expect(remote.client.call('artifact/upload', { target: attached.target, modality: 'image', data: 'aGk=' }, 'artifact_uploaded')).rejects.toThrow();
    for (let i = 0; i < 34; i++) {
      await remote.client.call('turn/start', { target: attached.target, content: [{ type: 'text', text: `History ${i}` }] }, 'inbound_accepted');
      await expect.poll(async () => {
        const current = await remote.client.call('session/snapshot', { target: attached.target }, 'snapshot');
        return current.snapshot.attempt?.phase.type;
      }).toBe('settled');
    }
    await remote.client.call('session/detach', { target: attached.target }, 'detached');
    await remote.shutdown();
    await page.goto('/');
    await page.getByLabel('WebSocket endpoint').fill(fixture.endpoint);
    await page.getByLabel('Transport token').fill(fixture.token);
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await page.getByRole('button', { name: `Open ${id}`, exact: true }).click();
    await expect(page.getByRole('button', { name: 'Load earlier', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Load earlier', exact: true }).scrollIntoViewIfNeeded();
    const firstKey = await page.locator('[data-chat-anchor-key]').first().getAttribute('data-chat-anchor-key');
    const anchor = page.locator(`[data-chat-anchor-key=${JSON.stringify(firstKey)}]`);
    const anchorTop = await anchor.evaluate(el => el.getBoundingClientRect().top);
    await expect(page.getByText('Answer 0', { exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Load earlier', exact: true }).click();
    await expect(page.getByText('Answer 0', { exact: true })).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Load earlier', exact: true })).toHaveCount(0);
    expect(Math.abs(await anchor.evaluate(el => el.getBoundingClientRect().top) - anchorTop)).toBeLessThan(2);
    await page.getByLabel('Message', { exact: true }).fill('Rich reply');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await fixture.gate('settle-chat');
    await expect(page.getByRole('heading', { name: 'Rich reply' })).toHaveCount(1);
    await fixture.release('settle-chat');
    await expect(page.getByText('Settled', { exact: true })).toHaveCount(1);
    await expect(page.locator('[aria-label^="Streaming ·"]')).toHaveCount(0);
    await expect(page.getByRole('table')).toHaveCount(1);
    await page.getByRole('button', { name: 'Reconnect', exact: true }).click();
    await expect(page.locator('.status strong')).toHaveText('connected');
    await expect(page.getByRole('heading', { name: 'Rich reply' })).toHaveCount(1);
    await page.getByRole('button', { name: 'Load earlier', exact: true }).click();
    await expect(page.getByText('Answer 0', { exact: true })).toHaveCount(1);
    await page.getByLabel('Message', { exact: true }).fill('Keep this draft');
    await page.getByLabel('Attach files').setInputFiles({ name: 'note.txt', mimeType: 'text/plain', buffer: Buffer.from('file content') });
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('does not support');
    await expect(page.getByLabel('Message', { exact: true })).toHaveValue('Keep this draft');
    await expect(page.getByRole('button', { name: 'Remove note.txt' })).toBeVisible();
    expect((await fixture.control('requests')).requests).toHaveLength(35);
    await page.screenshot({ path: 'test-results/chat-history.png', fullPage: true });
    expect(errors).toEqual([]); passed = true;
  } finally { await page.close(); await fixture.stop(passed); }
});

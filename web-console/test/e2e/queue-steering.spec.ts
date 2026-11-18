import { expect, test } from '@playwright/test';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';

test('promotes one queued message and then the remaining queue during the same attempt', async ({ page }) => {
  const fixture = await startDogfood('web_queue_steering');
  const wire = await wireProbe(page);
  let passed = false;
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    const session = await openEmptySession(page, fixture, 'Workspace A');
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    const queue = page.getByRole('region', { name: 'Queue', exact: true });
    await input.fill('Begin queue review');
    await input.press('Enter');
    await fixture.gate('queue-review');
    const target = await wire.attachedTarget(session);
    const checkpoint = async () => (await wire.read('session/checkpoint', target)).checkpoint.snapshot;
    const attempt = (await checkpoint()).attempt!.attempt_id;
    const revisions = ['First revision', 'Second revision', 'Third revision'];
    for (const text of revisions) {
      await input.fill(text);
      await input.press('Enter');
      await expect(input).toHaveValue('', { timeout: 30_000 });
    }
    await queue.getByRole('button', { expanded: false }).click();
    const rows = queue.locator('[data-inbound-sequence]');
    await expect(rows).toHaveCount(3);
    const chat = page.getByLabel('Canonical conversation');
    for (const text of revisions) await expect(chat.getByText(text, { exact: true })).toHaveCount(0);
    const ids = await rows.evaluateAll(nodes => nodes.map(node => node.getAttribute('data-message-id')));
    await rows.nth(1).getByRole('button', { name: /steer/i }).click();
    await expect(rows).toHaveCount(2);
    await expect(input).toHaveAttribute('placeholder', /steers all queued messages/);
    await input.press('Control+Enter');
    await expect.poll(() => wire.requests.filter(row => row.method === 'inbound/steer').map(row => row.params.scope.kind)).toEqual(['row', 'all']);
    await expect(queue).toHaveCount(0);
    await expect(page.getByText(/History changed/)).toHaveCount(0);
    expect((await checkpoint()).attempt?.attempt_id).toBe(attempt);
    await expect(chat.getByText(/^(?:First|Second|Third) revision$/)).toHaveText([revisions[1], revisions[0], revisions[2]]);
    await fixture.release('queue-review');
    await expect(page.getByText('Reviewed all three revisions.', { exact: true })).toBeVisible();
    const messages = (await checkpoint()).messages;
    expect(messages.filter(message => ids.includes(message.id)).map(message => message.id)).toEqual([ids[1], ids[0], ids[2]]);
    const request = JSON.stringify((await fixture.control('requests')).requests.at(-1).body);
    expect(request.match(/(?:First|Second|Third) revision/g)).toEqual([revisions[1], revisions[0], revisions[2]]);
    const revisionBubbles = chat.getByText(/^(?:First|Second|Third) revision$/);
    await expect(revisionBubbles).toHaveText([revisions[1], revisions[0], revisions[2]]);
    await page.reload();
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(revisionBubbles).toHaveText([revisions[1], revisions[0], revisions[2]]);
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});

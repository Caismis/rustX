import { expect, test } from '@playwright/test';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';

test('places promoted steers, direct steer and next-turn input at model adoption, including after reload', async ({ page }) => {
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
    const revisionBubbles = chat.getByText(/^(?:First|Second|Third) revision$/);
    await input.fill('Direct steer');
    await input.press('Control+Enter');
    await expect(input).toHaveValue('');
    const direct = chat.getByText('Direct steer', { exact: true });
    await expect(direct).toHaveCount(0);
    await expect(revisionBubbles).toHaveCount(0);
    await fixture.release('queue-review');
    await fixture.gate('queue-revisions-received');
    await expect(revisionBubbles).toHaveText([revisions[1], revisions[0], revisions[2]]);
    await expect(direct).toHaveCount(1);
    await input.fill('Separate follow-up');
    await input.press('Enter');
    await expect(queue).toBeVisible();
    const followUp = chat.getByText('Separate follow-up', { exact: true });
    await expect(followUp).toHaveCount(0);
    await fixture.release('queue-revisions-received');
    await expect(chat.getByText('Answered the separate follow-up.', { exact: true })).toBeVisible();
    const snapshot = await checkpoint();
    const messages = snapshot.messages;
    // Trace/provider input follows the ledger. Check the durable chat cursor
    // order independently, including the assistant output completed after Send.
    const ledgerIds = messages.filter(message => message.role === 'user' || message.role === 'assistant').map(message => message.id);
    const chatIds = (snapshot.transcript.entries ?? []).flatMap(entry => entry.item.type === 'message'
      && ledgerIds.includes(entry.item.message.id) ? [entry.item.message.id] : []);
    expect(chatIds).toEqual(ledgerIds.filter(id => chatIds.includes(id)));
    expect(chatIds).toEqual(expect.arrayContaining(ids));
    expect(messages.filter(message => ids.includes(message.id)).map(message => message.id)).toEqual([ids[1], ids[0], ids[2]]);
    const request = JSON.stringify((await fixture.control('requests')).requests[1].body);
    expect(request.match(/(?:First|Second|Third) revision|Direct steer/g)).toEqual([revisions[1], revisions[0], revisions[2], 'Direct steer']);
    await expect(revisionBubbles).toHaveText([revisions[1], revisions[0], revisions[2]]);
    const initialAnswer = 'Review started. Finished the original request.';
    const answersAndInputs = chat.getByText(initialAnswer, { exact: true }).or(revisionBubbles).or(direct).or(followUp)
      .or(chat.getByText('Reviewed all three revisions.', { exact: true }))
      .or(chat.getByText('Answered the separate follow-up.', { exact: true }));
    const order = [initialAnswer, revisions[1], revisions[0], revisions[2], 'Direct steer',
      'Reviewed all three revisions.', 'Separate follow-up', 'Answered the separate follow-up.'];
    await expect(answersAndInputs).toHaveText(order);
    await page.reload();
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(answersAndInputs).toHaveText(order);
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});

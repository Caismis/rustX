import { expect, test, type Page } from '@playwright/test';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';
import { routeWorkspaceHost } from './workspace-host';

// A version change must keep the shared first turn readable throughout.
async function monitorEarlierTurn(page: Page, selector: string) {
  await expect(page.locator(selector)).toContainText('Earlier question');
  await page.evaluate(selector => {
    const missing: string[] = [];
    let frame: number;
    const sample = () => {
      const text = document.querySelector(selector)?.textContent ?? '';
      if (!text.includes('Earlier question')) missing.push(text.slice(0, 360));
      frame = requestAnimationFrame(sample);
    };
    sample();
    (window as any).__finishVersionProbe = () => { cancelAnimationFrame(frame); return missing; };
  }, selector);
  return async () => {
    const missing: string[] = await page.evaluate(() => (window as any).__finishVersionProbe());
    expect([...new Set(missing)]).toEqual([]);
  };
}

test('editing the second question retains the first turn and its own version pager in trajectory', async ({ page }) => {
  const fixture = await startDogfood('web_trace_response_retry', undefined, { imageMcp: true, handshakeSeconds: 1 });
  let passed = false;
  try {
    const toolResult = 'Original tool result for the shared turn.';
    writeFileSync(join(fixture.workspaceA, 'version-prefix.txt'), toolResult);
    const wire = await wireProbe(page);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    for (const [prompt, answer] of [
      ['Earlier question', 'Earlier answer stays.'],
      ['Retry this question', 'Old reply to replace.'],
    ]) {
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill(prompt);
      await page.getByRole('button', { name: 'Send', exact: true }).click();
      await expect(page.getByText(answer, { exact: true })).toBeVisible();
    }
    const second = page.getByRole('article', { name: 'Your message' }).filter({ hasText: 'Retry this question' });
    const beforeEdit = wire.requests.length;
    await second.getByRole('button', { name: 'Edit message' }).click();
    await expect(second.getByRole('textbox')).toHaveValue('Retry this question');
    expect(wire.requests.slice(beforeEdit).some(request => typeof request.params?.domain === 'string' && request.params.domain.startsWith('messages.'))).toBe(false);
    await second.getByRole('textbox').fill('Retry this question with more detail');
    const finishEdit = await monitorEarlierTurn(page, '[data-conversation-scroll]');
    const beforeSend = wire.requests.length;
    wire.holdNext('session/branch');
    try {
      await second.getByRole('button', { name: 'Retry', exact: true }).click();
      await expect.poll(wire.held).toBe(1);
      await expect(page.getByText('Retry this question with more detail', { exact: true })).toBeVisible();
      await expect(page.getByText('Old reply to replace.', { exact: true })).toHaveCount(0);
    } finally {
      wire.releaseHeld();
    }
    await fixture.gate('replay-request');
    const replay = wire.requests.slice(beforeSend).map(request => request.method);
    expect(replay.indexOf('session/submissionIntent')).toBeGreaterThan(replay.indexOf('session/switchNode'));
    expect(replay.indexOf('thread/resume')).toBeGreaterThan(replay.indexOf('session/submissionIntent'));
    expect(replay.filter(method => method === 'inbound/queue')).toHaveLength(1);
    await fixture.release('replay-request');
    await fixture.gate('replay-streaming');
    await fixture.release('replay-streaming');
    await expect(page.getByText('Replacement complete.', { exact: true })).toBeVisible();
    await finishEdit();
    await page.getByRole('tab', { name: 'Trajectory' }).click();
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await expect(ledger).toContainText('Earlier question');
    await expect(ledger).toContainText('Earlier answer stays.');
    await expect(ledger.getByText('Initial System Prompt', { exact: true })).toHaveCount(1);
    await expect(ledger.getByText(/Execution permission mode:/)).toHaveCount(1);
    const inspector = page.getByRole('complementary', { name: 'Event details', exact: true });
    const closeDetails = () => inspector.getByRole('button', { name: 'Close details', exact: true }).click();
    const beforeDetails = wire.requests.length;
    await ledger.locator('[data-kind="user"]').filter({ hasText: 'Earlier question' }).click();
    await expect(inspector).toContainText('Earlier question');
    await closeDetails();
    await ledger.locator('[data-kind="tool"]').filter({ hasText: 'version-prefix.txt' }).click();
    await inspector.getByRole('tab', { name: 'Result', exact: true }).click();
    await expect(inspector.getByRole('tabpanel')).toContainText(toolResult);
    await closeDetails();
    await ledger.locator('[data-request-owner]').first().click();
    await inspector.getByRole('tab', { name: 'Options', exact: true }).click();
    await expect(inspector.getByRole('tabpanel')).toContainText('console-model');
    await closeDetails();
    expect(wire.requests.slice(beforeDetails).map(request => request.method)).not.toContain('thread/resume');
    const edited = ledger.locator('[data-kind="user"]').filter({ hasText: 'with more detail' });
    const finishSwitch = await monitorEarlierTurn(page, '[aria-label="Trace ledger"]');
    await edited.getByRole('button', { name: 'Switch to previous message version' }).click();
    await expect(ledger).toContainText('Old reply to replace.');
    await expect(ledger).toContainText('Earlier answer stays.');
    await expect(ledger).not.toContainText('with more detail');
    await ledger.getByRole('button', { name: 'Switch to next message version' }).click();
    await expect(ledger).toContainText('Replacement complete.');
    await expect(ledger).toContainText('Earlier answer stays.');
    await finishSwitch();
    await page.getByRole('tab', { name: 'Chat', exact: true }).click();
    const chat = page.getByLabel('Canonical conversation', { exact: true });
    await expect(chat.getByText('Earlier question', { exact: true })).toBeVisible();
    const finishChatSwitch = await monitorEarlierTurn(page, '[data-conversation-scroll]');
    await expect(chat.getByRole('button', { name: 'Switch to previous message version' })).toBeEnabled();
    const beforeColdVersion = wire.requests.length;
    wire.holdNext('thread/resume');
    try {
      await chat.getByRole('button', { name: 'Switch to previous message version' }).click();
      await expect.poll(wire.held).toBe(1);
      await expect(chat.getByText('Old reply to replace.', { exact: true })).toBeVisible();
      expect(wire.requests.slice(beforeColdVersion).some(request => request.method === 'session/history')).toBe(true);
    } finally {
      wire.releaseHeld();
    }
    await expect(chat.getByRole('button', { name: 'Switch to next message version' })).toBeEnabled();
    wire.holdNext('thread/resume');
    await chat.getByRole('button', { name: 'Switch to next message version' }).click();
    try {
      await expect.poll(wire.held).toBe(1);
      await expect(chat.getByText('Replacement complete.', { exact: true })).toBeVisible();
    } finally {
      wire.releaseHeld();
    }
    await expect(chat.getByText('Replacement complete.', { exact: true })).toBeVisible();
    await expect(chat.getByRole('button', { name: 'Switch to previous message version' })).toBeEnabled();
    await finishChatSwitch();
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});

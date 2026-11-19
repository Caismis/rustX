import { expect, test, type Page } from '@playwright/test';
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
  const fixture = await startDogfood('web_response_retry', undefined, { imageMcp: true });
  let passed = false;
  try {
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
    await second.getByRole('button', { name: 'Edit message' }).click();
    await second.getByRole('textbox').fill('Retry this question with more detail');
    const finishEdit = await monitorEarlierTurn(page, '[data-conversation-scroll]');
    await second.getByRole('button', { name: 'Retry', exact: true }).click();
    await fixture.gate('replay-request');
    await fixture.release('replay-request');
    await fixture.gate('replay-streaming');
    await fixture.release('replay-streaming');
    await expect(page.getByText('Replacement complete.', { exact: true })).toBeVisible();
    await finishEdit();
    await page.getByRole('tab', { name: 'Trajectory' }).click();
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await expect(ledger).toContainText('Earlier question');
    await expect(ledger).toContainText('Earlier answer stays.');
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
    await chat.getByRole('button', { name: 'Switch to previous message version' }).click();
    await expect(chat.getByText('Old reply to replace.', { exact: true })).toBeVisible();
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

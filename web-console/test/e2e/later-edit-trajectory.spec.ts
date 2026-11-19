import { expect, test } from '@playwright/test';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';

test('editing the second question retains the first turn and its own version pager in trajectory', async ({ page }) => {
  const fixture = await startDogfood('web_response_retry');
  let passed = false;
  try {
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
    await second.getByRole('button', { name: 'Retry', exact: true }).click();
    await fixture.gate('replay-request');
    await fixture.release('replay-request');
    await fixture.gate('replay-streaming');
    await fixture.release('replay-streaming');
    await expect(page.getByText('Replacement complete.', { exact: true })).toBeVisible();
    await page.getByRole('tab', { name: 'Trajectory' }).click();
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await expect(ledger).toContainText('Earlier question');
    await expect(ledger).toContainText('Earlier answer stays.');
    const edited = ledger.locator('[data-kind="user"]').filter({ hasText: 'with more detail' });
    await edited.getByRole('button', { name: 'Switch to previous message version' }).click();
    await expect(ledger).toContainText('Old reply to replace.');
    await expect(ledger).toContainText('Earlier answer stays.');
    await expect(ledger).not.toContainText('with more detail');
    await ledger.getByRole('button', { name: 'Switch to next message version' }).click();
    await expect(ledger).toContainText('Replacement complete.');
    await expect(ledger).toContainText('Earlier answer stays.');
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});

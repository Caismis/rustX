import { test, expect } from '@playwright/test';
const fixtureOrigin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

test('durable Agent detail remains selected through interruption and resume; Job reaches final settlement', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error' || message.type() === 'warning') errors.push(message.text()); });
  await page.goto(`${fixtureOrigin}/test/fixtures/activity.html`);
  await expect(page).toHaveTitle('rustX Jobs and Agents');
  const agent = page.locator('[data-agent-id="agent-worker"]');
  await expect(agent).toHaveAttribute('data-activation-id', 'activation-a');
  await agent.getByRole('button', { name: 'Transcript', exact: true }).click();
  await expect(agent.getByRole('heading', { name: 'Final report' })).toBeVisible();
  await agent.getByRole('button', { name: 'Interrupt', exact: true }).click();
  await expect(agent).toHaveAttribute('data-agent-state', 'inactive');
  await expect(agent.getByRole('status').filter({ hasText: /^cancelled$/ })).toBeVisible();
  await agent.getByRole('textbox', { name: 'Message Agent Worker' }).fill('Continue reviewing');
  await agent.getByRole('button', { name: 'Send message', exact: true }).click();
  await expect(agent).toHaveAttribute('data-activation-id', 'activation-b');
  await expect(agent.getByRole('heading', { name: 'Final report' })).toBeVisible();
  await expect(agent).toHaveCount(1);
  const job = page.getByRole('region', { name: 'Job Build' });
  await job.getByRole('button', { name: 'Cancel Job', exact: true }).click();
  await expect(job.getByRole('button', { name: 'Cancel Job', exact: true })).toBeDisabled();
  await job.getByRole('button', { name: /Build/ }).click();
  await expect(job.getByText(/Process settled/)).toBeVisible();
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
  await page.screenshot({ path: '/tmp/rustx-411-web-activity-desktop.png', fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: '/tmp/rustx-411-web-activity-mobile.png', fullPage: true });
});


test('image row previews managed bytes and Bash description expands to authoritative command', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(`${fixtureOrigin}/test/fixtures/activity.html`);
  await expect(page).toHaveTitle('rustX Jobs and Agents');
  const job = page.getByRole('region', { name: 'Job Build' });
  await expect(job).toContainText('Check the build <safely>');
  await job.getByRole('button', { name: /Build/ }).click();
  await expect(job).toContainText('printf authoritative-command');
  const image = page.locator('[data-tool-call-id="read-image"]');
  await expect(image).toHaveAttribute('data-tool-renderer', 'image');
  await image.getByRole('button', { expanded: false }).click();
  await image.getByRole('button', { name: 'Load attachment' }).click();
  await expect(image.getByRole('img', { name: 'artifact_1', exact: true })).toBeVisible();
  await expect.poll(() => image.getByRole('img', { name: 'artifact_1', exact: true }).evaluate((node: HTMLImageElement) => node.complete && node.naturalWidth === 1)).toBe(true);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
  await page.screenshot({ path: '/tmp/rustx-412-tools-desktop.png', fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: '/tmp/rustx-412-tools-mobile.png', fullPage: true });
});

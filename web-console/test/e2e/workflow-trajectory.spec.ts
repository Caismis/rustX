import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession } from './shell-actions';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
for (const width of [1440, 390])
  for (const theme of ['light', 'dark']) {
    test(`workflow trace expands real paged node visits ${width} ${theme}`, async ({ page }) => {
      const errors: string[] = [];
      page.on('pageerror', (error) => errors.push(error.message));
      page.on('console', (message) => {
        if (['error', 'warning'].includes(message.type())) errors.push(message.text());
      });
      await page.setViewportSize({ width, height: 844 });
      await page.goto(`${origin}/test/fixtures/trajectory.html?workflows`);
      if (theme === 'dark') await page.evaluate(() => document.body.setAttribute('data-ds-dark-theme', ''));
      await expect(page).toHaveTitle('Trajectory presentation contracts');
      const ledger = page.getByRole('table', { name: 'Trace ledger' });
      const first = ledger.locator('[data-workflow-entry="trace:30"]');
      await expect(ledger.locator('[data-kind="workflow"]')).toHaveCount(3);
      await expect(ledger.locator('[data-trace-id="trace:20"]')).toHaveAttribute('data-kind', 'tool');
      await expect(ledger.locator('[data-trace-id="trace:30"]')).toHaveCount(0);
      await expect(ledger.locator('[data-workflow-node]')).toHaveCount(0);
      await first.focus();
      await first.press('ArrowRight');
      // 121 visits exceed both the native page size (32) and virtualization threshold (100).
      await expect(ledger).toHaveAttribute('aria-rowcount', '126');
      await expect(ledger.locator('[data-workflow-node="node-0-0"]')).toBeVisible();
      await ledger.evaluate((el) => {
        el.scrollTop = el.scrollHeight;
        el.dispatchEvent(new Event('scroll'));
      });
      await expect(ledger.locator('[data-workflow-node="node-0-120"]')).toBeVisible();
      await ledger.evaluate((el) => {
        el.scrollTop = 0;
        el.dispatchEvent(new Event('scroll'));
      });
      const start = ledger.locator('[data-workflow-node="node-0-0"]');
      await start.click();
      const details = page.getByRole('complementary', { name: 'Event details' });
      await expect(details.getByRole('tabpanel')).toContainText('18446744073709551615');
      await details.getByRole('tab', { name: 'Result', exact: true }).click();
      await expect(details.getByRole('tabpanel')).toContainText('output-0');
      await page.screenshot({ path: `/tmp/rustx-workflow-trajectory-${width}-${theme}.png` });
      await details.getByRole('button', { name: 'Close details', exact: true }).click();
      await first.focus();
      await first.press('ArrowLeft');
      await expect(ledger.locator('[data-workflow-node]')).toHaveCount(0);
      const cancelled = ledger.locator('[data-workflow-entry="trace:31"]');
      await cancelled.getByRole('button', { name: 'Expand workflow nodes' }).click();
      await expect(ledger.locator('[data-workflow-node="node-1-1"]')).toHaveAttribute('data-state', 'cancelled');
      await cancelled.getByRole('button', { name: 'Collapse workflow nodes' }).click();
      const running = ledger.locator('[data-workflow-entry="trace:32"]');
      await running.getByRole('button', { name: 'Expand workflow nodes' }).click();
      await expect(ledger.locator('[data-workflow-node="node-2-1"]')).toHaveAttribute('data-state', 'waiting');
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0);
      expect(errors).toEqual([]);
    });
  }

for (const width of [1440, 390])
  test(`completed native workflow stays inspectable after later Trace publications at ${width}px`, async ({ page }) => {
    const fixture = await startDogfood('web_workflow_conformance'),
      errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    try {
      await page.setViewportSize({ width, height: 1000 });
      await routeWorkspaceHost(page, fixture);
      await page.goto('/');
      await connectRemote(page, fixture.endpoint, fixture.token);
      await openEmptySession(page, fixture, 'Workspace A');
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill('workflow conformance request');
      await page.getByRole('button', { name: 'Send', exact: true }).click();
      await fixture.gate('workflow-child-admitted');
      await expect(page.locator('[data-workflow-run-id]')).toBeVisible();
      await fixture.release('workflow-child-admitted');
      await expect(page.getByText('workflow conformance complete', { exact: true })).toBeVisible();
      await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
      const trajectory = page.getByRole('region', { name: 'Trajectory', exact: true }),
        ledger = trajectory.getByRole('table', { name: 'Trace ledger' });
      const workflow = ledger.locator('[data-workflow-entry]').first();
      await workflow.getByRole('button', { name: 'Expand workflow nodes', exact: true }).click();
      await expect(ledger.locator('[data-workflow-node]')).toHaveCount(4);
      await expect(trajectory.getByRole('alert')).toHaveCount(0);
      const review = ledger.locator('[data-workflow-node]').filter({ has: page.getByText('review', { exact: true }) });
      await review.click();
      const detail = page.getByRole('complementary', { name: 'Event details' });
      await detail.getByRole('tab', { name: 'Payload', exact: true }).click();
      await expect(detail.getByRole('tabpanel')).toContainText('workflow conformance request');
      await detail.getByRole('tab', { name: 'Result', exact: true }).click();
      await expect(detail.getByRole('tabpanel')).toContainText('native workflow child committed');
      await expect(detail.getByRole('alert')).toHaveCount(0);
      await page.screenshot({ path: `/tmp/rustx-workflow-cut-native-${width}.png` });
      await detail.getByRole('button', { name: 'Close details', exact: true }).click();
      await workflow.getByRole('button', { name: 'Collapse workflow nodes', exact: true }).click();
      await expect(ledger.locator('[data-workflow-node]')).toHaveCount(0);
      await workflow.getByRole('button', { name: 'Expand workflow nodes', exact: true }).click();
      await expect(ledger.locator('[data-workflow-node]')).toHaveCount(4);
      expect(errors).toEqual([]);
    } finally {
      await fixture.stop(false);
    }
  });

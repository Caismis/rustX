import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
for (const width of [1440, 390]) for (const theme of ['light', 'dark']) {
  test(`workflow trace expands real paged node visits ${width} ${theme}`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (['error', 'warning'].includes(message.type())) errors.push(message.text()); });
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
    await first.focus(); await first.press('ArrowRight');
    // 121 visits exceed both the native page size (32) and virtualization threshold (100).
    await expect(ledger).toHaveAttribute('aria-rowcount', '126');
    await expect(ledger.locator('[data-workflow-node="node-0-0"]')).toBeVisible();
    await ledger.evaluate(el => { el.scrollTop = el.scrollHeight; el.dispatchEvent(new Event('scroll')); });
    await expect(ledger.locator('[data-workflow-node="node-0-120"]')).toBeVisible();
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const start = ledger.locator('[data-workflow-node="node-0-0"]');
    await start.click();
    const details = page.getByRole('complementary', { name: 'Event details' });
    await expect(details.getByRole('tabpanel')).toContainText('18446744073709551615');
    await details.getByRole('tab', { name: 'Result', exact: true }).click();
    await expect(details.getByRole('tabpanel')).toContainText('output-0');
    await page.screenshot({ path: `/tmp/rustx-workflow-trajectory-${width}-${theme}.png` });
    await details.getByRole('button', { name: 'Close details', exact: true }).click();
    await first.focus(); await first.press('ArrowLeft');
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

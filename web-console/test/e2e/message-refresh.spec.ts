import { expect, test } from '@playwright/test';

declare global {
  interface Window { refreshAudit: { refresh(): void; pending(): number; finish(): void } }
}
const origin = 'http://127.0.0.1:' + (process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174);
for (const width of [1280, 390]) for (const theme of ['light', 'dark']) {
  test(`message refresh preserves complete content without transient layout text (${width}px, ${theme})`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(theme => localStorage.setItem('rustx-appearance-v1', theme), theme);
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error' || message.type() === 'warning') errors.push(message.text()); });
    await page.goto(origin + '/test/fixtures/message-refresh.html');
    await expect(page).toHaveTitle('Message refresh audit');
    await expect(page).toHaveURL(/message-refresh\.html$/);
    const root = page.locator('[data-refresh-audit]');
    const goal = page.getByRole('region', { name: 'Goal', exact: true });
    const workflow = page.locator('[data-workflow-run-id]');
    const job = page.locator('[data-job-id]');
    const approval = page.getByRole('region', { name: 'Approval', exact: true });
    await expect(goal).toContainText('Complete Goal objective');
    await expect(workflow.locator('[data-workflow-node]')).toHaveCount(4);
    await expect(job).toContainText('Original job output');
    await expect(approval.locator('pre')).toHaveText('printf rustX');
    await expect(page.getByRole('button', { name: 'Cancel interaction', exact: true })).toHaveCount(0);
    await page.locator('[data-compaction-checkpoint] button').click();
    await expect(root).toContainText('Visible summary preview');
    await expect(root).not.toContainText(/Loading|加载中|Assembling|组装中/);
    await expect(root.locator(':scope > *')).toHaveCount(5);
    const header = workflow.locator('[data-disclosure-row]').first();
    await header.focus();
    const geometry = await root.evaluate(element => ({ height: element.getBoundingClientRect().height, tops: [...element.children].map(child => child.getBoundingClientRect().top) }));
    await root.locator('[data-workflow-node], [data-terminal], [aria-label="Approval details"] pre').evaluateAll(elements => elements.forEach(element => element.setAttribute('data-retained-reading', 'true')));
    await root.evaluate(element => {
      element.setAttribute('data-loading-flash', 'false');
      new MutationObserver(records => {
        const loading = /Loading|加载中|Assembling|组装中/;
        if (loading.test(element.textContent ?? '') || records.some(record => loading.test(record.oldValue ?? '') || [...record.addedNodes, ...record.removedNodes].some(node => loading.test(node.textContent ?? '')))) element.setAttribute('data-loading-flash', 'true');
      }).observe(element, { childList: true, subtree: true, characterData: true, characterDataOldValue: true });
    });
    await page.evaluate(() => window.refreshAudit.refresh());
    await expect.poll(() => page.evaluate(() => window.refreshAudit.pending())).toBe(4);
    await expect(root.locator('[data-retained-reading]')).toHaveCount(6);
    await expect(goal).toContainText('Complete Goal objective');
    await expect(job).toContainText('Original job output');
    await expect(approval.locator('pre')).toHaveText('printf rustX');
    await expect(header).toBeFocused();
    expect(await root.evaluate(element => ({ height: element.getBoundingClientRect().height, tops: [...element.children].map(child => child.getBoundingClientRect().top) }))).toEqual(geometry);
    await page.screenshot({ path: `/tmp/rustx-message-refresh-pending-${theme}-${width}.png`, fullPage: true });
    await page.evaluate(() => window.refreshAudit.finish());
    await expect(goal).toContainText('New authoritative Goal objective');
    await expect(workflow.locator('[data-workflow-node]')).toHaveCount(5);
    await expect(job).toContainText('Updated job output');
    await expect(root).toContainText('Complete summary ready');
    await expect(root.locator('[data-retained-reading]')).toHaveCount(6);
    await expect(header).toBeFocused();
    await expect(root).toHaveAttribute('data-loading-flash', 'false');
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    expect(errors).toEqual([]);
    await page.screenshot({ path: `/tmp/rustx-message-refresh-${theme}-${width}.png`, fullPage: true });
  });
}

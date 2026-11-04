import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession } from './shell-actions';
import { translator } from '../../src/locale/translation';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
for (const width of [1440, 390])
  for (const recover of [false, true])
  test(`cold Scheduled Workflow selection survives ${recover ? 'failed and retried' : 'successful'} Load Earlier reader replacement at ${width}px`, async ({ page }, info) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (['error', 'warning'].includes(message.type())) errors.push(message.text()); });
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${origin}/test/fixtures/workflow-trace-cold.html`);
    await expect(page).toHaveTitle('Cold workflow trajectory contracts');
    const ledger = page.getByRole('table', { name: 'Trace ledger', exact: true });
    const root = ledger.locator('[data-workflow-entry="trace:30"]');
    await root.getByRole('button', { name: 'Expand workflow nodes', exact: true }).click();
    await expect(ledger.locator('[data-workflow-node]')).toHaveCount(73);
    const selected = ledger.locator('[data-workflow-node="node-0-60"]');
    await selected.scrollIntoViewIfNeeded(); await selected.click();
    const identity = await selected.getAttribute('data-display-key');
    const details = page.getByRole('complementary', { name: 'Event details', exact: true });
    await details.getByRole('tab', { name: 'Result', exact: true }).click();
    await expect(details.getByRole('tabpanel')).toContainText('output-60');
    const epoch = await page.evaluate(() => window.workflowColdTrace.epoch());
    // The timeline stays above the mobile Inspector overlay; its history
    // control invokes the same real cold Load Earlier path as the ledger.
    await page.getByRole('region', { name: 'Trajectory timeline', exact: true })
      .getByRole('button', { name: 'Load earlier history', exact: true }).click();
    const admit = async (offset: number) => {
      await expect.poll(() => page.evaluate(() => window.workflowColdTrace.waiting())).toContain(offset);
      await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => resolve())));
      expect(await page.evaluate(() => window.workflowColdTrace.epoch())).toBe(epoch);
      await expect(ledger.locator('[data-workflow-node]')).toHaveCount(0);
      await expect(root).not.toHaveAttribute('aria-selected', 'true');
      // Retired reader content must not remain visible or be replaced by root
      // details while the complete replacement node set is still pending.
      await expect(page.getByRole('tabpanel').filter({ hasText: 'output-60' })).toHaveCount(0);
      await page.evaluate(offset => window.workflowColdTrace.release(offset), offset);
    };
    if (recover) {
      await admit(0);
      await expect.poll(() => page.evaluate(() => window.workflowColdTrace.waiting())).toContain(32);
      await page.evaluate(() => window.workflowColdTrace.reject(32));
      await expect(details.getByRole('alert')).toContainText('Replacement Workflow page rejected');
      await expect(details.getByRole('button', { name: 'Close details', exact: true })).toBeVisible();
      await expect(details).not.toContainText('output-60');
      await expect(root).not.toHaveAttribute('aria-selected', 'true');
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: info.outputPath(`cold-workflow-failed-${width}.png`) });
      await details.getByRole('button', { name: 'Retry loading details', exact: true }).click();
    }
    for (const offset of [0, 32, 64]) await admit(offset);
    await expect(ledger.locator('[data-workflow-node]')).toHaveCount(73);
    await expect(selected).toHaveAttribute('aria-selected', 'true');
    expect(await selected.getAttribute('data-display-key')).toBe(identity);
    await expect(details.getByRole('tab', { name: 'Result', exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(details.getByRole('tabpanel')).toContainText('output-60');
    await expect(page.getByRole('alert')).toHaveCount(0);
    await page.screenshot({ path: info.outputPath(`cold-workflow-selection-${width}.png`) });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
  });

for (const width of [1440, 390])
  test(`workflow snapshot refresh preserves a selected later-page visit at ${width}px`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (['error', 'warning'].includes(message.type())) errors.push(message.text()); });
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${origin}/test/fixtures/trajectory.html?workflow-refresh`);
    await expect(page).toHaveTitle('Trajectory presentation contracts');
    const ledger = page.getByRole('table', { name: 'Trace ledger', exact: true });
    await ledger.locator('[data-workflow-entry="trace:30"]').getByRole('button', { name: 'Expand workflow nodes', exact: true }).click();
    await expect(ledger).toHaveAttribute('aria-rowcount', '78');
    const selected = ledger.locator('[data-workflow-node="node-0-60"]');
    await selected.scrollIntoViewIfNeeded(); await selected.click();
    const identity = await selected.getAttribute('data-display-key');
    expect(identity).toBeTruthy();
    const details = page.getByRole('complementary', { name: 'Event details', exact: true });
    await details.getByRole('tab', { name: 'Result', exact: true }).click();
    await expect(details.getByRole('tabpanel')).toContainText('output-60');
    await page.getByRole('button', { name: 'Refresh workflow snapshot', exact: true }).click();
    for (const offset of [32, 64]) {
      await expect.poll(() => page.evaluate(() => window.workflowTraceRefresh.requested())).toContain(offset);
      // The next page gate proves the prior page returned. Give React its paint
      // turn before checking that no intermediate prefix migrated selection.
      await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => resolve())));
      await expect(ledger).toHaveAttribute('aria-rowcount', '78');
      await expect(selected).toHaveAttribute('aria-selected', 'true');
      expect(await selected.getAttribute('data-display-key')).toBe(identity);
      await expect(details.getByRole('tabpanel')).toContainText('output-60');
      await expect(details.getByRole('tabpanel')).not.toContainText('refreshed-output-60');
      await page.evaluate(offset => window.workflowTraceRefresh.release(offset), offset);
    }
    await expect(details.getByRole('tabpanel')).toContainText('refreshed-output-60');
    await expect(selected).toHaveAttribute('aria-selected', 'true');
    expect(await selected.getAttribute('data-display-key')).toBe(identity);
    await page.screenshot({ path: `/tmp/rustx-521-round2-workflow-selection-${width}.png` });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    expect(errors).toEqual([]);
  });

for (const width of [1440, 390])
  for (const theme of ['light', 'dark'])
  for (const locale of ['en', 'zh'] as const) {
    test(`workflow trace expands real paged node visits ${width} ${theme} ${locale}`, async ({ page }) => {
      const tx = translator(locale);
      const errors: string[] = [];
      page.on('pageerror', (error) => errors.push(error.message));
      page.on('console', (message) => {
        if (['error', 'warning'].includes(message.type())) errors.push(message.text());
      });
      await page.setViewportSize({ width, height: 844 });
      await page.addInitScript(locale => localStorage.setItem('rustx-locale-v1', locale), locale);
      await page.goto(`${origin}/test/fixtures/trajectory.html?workflows`);
      if (theme === 'dark') await page.evaluate(() => document.body.setAttribute('data-ds-dark-theme', ''));
      await expect(page).toHaveTitle('Trajectory presentation contracts');
      const ledger = page.getByRole('table', { name: tx('trajectory:trajectory.trace-ledger') });
      const first = ledger.locator('[data-workflow-entry="trace:30"]');
      await expect(ledger.locator('[data-kind="workflow"]')).toHaveCount(3);
      await expect(ledger.locator('[data-trace-id="trace:20"]')).toHaveAttribute('data-kind', 'tool');
      await expect(ledger.locator('[data-trace-id="trace:30"]')).toHaveCount(0);
      await expect(ledger.locator('[data-workflow-node]')).toHaveCount(0);
      await first.focus();
      await first.press('ArrowRight');
      // 121 visits exceed both the native page size (32) and virtualization threshold (100).
      await expect(ledger).toHaveAttribute('aria-rowcount', '126');
      const start = ledger.locator('[data-workflow-node="node-0-0"]');
      await expect(start).toBeVisible();
      // The node kind and name belong inside the parent's branch, with the
      // same output column as the parent, ordinary tools and sibling nodes.
      const kind = start.getByText(tx('common:workflow.kind.start'), { exact: true });
      const parentName = (await first.getByText('test_flow', { exact: true }).boundingBox())!;
      const kindBox = (await kind.boundingBox())!;
      const nameBox = (await start.getByText('start', { exact: true }).boundingBox())!;
      expect(kindBox.x).toBeGreaterThan(parentName.x);
      expect(nameBox.x).toBeGreaterThanOrEqual(kindBox.x + kindBox.width);
      expect((await start.boundingBox())!.height).toBe(30);
      await expect(start.locator('[role="cell"]').first()).toHaveText('');
      await expect(start).not.toHaveAttribute('data-workflow-last');
      const parentArrow = (await first.getByText('→', { exact: true }).boundingBox())!;
      for (const row of [start, ledger.locator('[data-workflow-node="node-0-1"]')]) {
        expect(Math.abs((await row.getByText('→', { exact: true }).boundingBox())!.x - parentArrow.x)).toBeLessThan(1);
        const branch = await row.locator('[role="cell"]').nth(1).evaluate(el => {
          const style = getComputedStyle(el, '::before');
          return { left: style.left, border: style.borderLeftWidth, content: style.content };
        });
        expect(branch).toEqual({ left: '14px', border: '1px', content: '""' });
      }
      await first.evaluate(el => (el as HTMLElement).blur());
      await page.screenshot({ path: `/tmp/rustx-workflow-hierarchy-${width}-${theme}-${locale}.png`,
        clip: { x: 0, y: (await first.boundingBox())!.y, width, height: 240 } });
      await ledger.evaluate((el) => {
        el.scrollTop = el.scrollHeight;
        el.dispatchEvent(new Event('scroll'));
      });
      await expect(ledger.locator('[data-workflow-node="node-0-120"]')).toBeVisible();
      await expect(ledger.locator('[data-workflow-node="node-0-120"]')).toHaveAttribute('data-workflow-last', 'true');
      await ledger.evaluate((el) => {
        el.scrollTop = 0;
        el.dispatchEvent(new Event('scroll'));
      });
      await start.click();
      const details = page.getByRole('complementary', { name: tx('trajectory:details.event') });
      await expect(details.getByRole('tabpanel')).toContainText('18446744073709551615');
      await details.getByRole('tab', { name: tx('trajectory:tab.result'), exact: true }).click();
      await expect(details.getByRole('tabpanel')).toContainText('output-0');
      await page.screenshot({ path: `/tmp/rustx-workflow-trajectory-${width}-${theme}-${locale}.png` });
      await details.getByRole('button', { name: tx('trajectory:details.close'), exact: true }).click();
      await first.focus();
      await first.press('ArrowLeft');
      await expect(ledger.locator('[data-workflow-node]')).toHaveCount(0);
      const cancelled = ledger.locator('[data-workflow-entry="trace:31"]');
      await cancelled.getByRole('button', { name: tx('trajectory:workflow.expand') }).click();
      await expect(ledger.locator('[data-workflow-node="node-1-1"]')).toHaveAttribute('data-state', 'cancelled');
      await cancelled.getByRole('button', { name: tx('trajectory:workflow.collapse') }).click();
      const running = ledger.locator('[data-workflow-entry="trace:32"]');
      await running.getByRole('button', { name: tx('trajectory:workflow.expand') }).click();
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
      const parentBox = (await workflow.boundingBox())!, ledgerBox = (await ledger.boundingBox())!;
      await page.screenshot({ path: `/tmp/rustx-workflow-hierarchy-native-${width}.png`,
        clip: { x: ledgerBox.x, y: parentBox.y, width: ledgerBox.width, height: 180 } });
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

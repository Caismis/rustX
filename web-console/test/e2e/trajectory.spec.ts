import { test, expect } from '@playwright/test';
import { expectStableScreenshot } from './screenshot';

for (const width of [1440, 390]) {
  for (const theme of ['light', 'dark']) {
    test(`T1-13/15 semantic ledger, facets and keyboard ${width} ${theme}`, async ({ page }) => {
      const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
      await page.setViewportSize({ width, height: 844 });
      await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html');
      if (theme === 'dark') await page.evaluate(() => document.body.setAttribute('data-ds-dark-theme', ''));
      const ledger = page.getByRole('table', { name: 'Trace ledger' });
      await expect(page).toHaveTitle('Trajectory presentation contracts');
      await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
      await expect(ledger.getByText('Initial System Prompt', { exact: false })).toBeVisible();
      await expect(ledger.getByText('System Prompt Updated', { exact: false }).first()).toBeVisible();
      await expect(ledger.getByText('Tools Updated', { exact: false }).first()).toBeVisible();
      await expect(ledger.getByText('System Prompt and Tools Updated', { exact: false })).toBeVisible();
      await expect(ledger.locator('[data-display-type="ContextRow"]')).toHaveCount(2);
      await expectStableScreenshot(page, `trajectory-ledger-${width}-${theme}.png`);
      const system = ledger.locator('[data-display-type="SystemRow"][data-owner="trace:3"]');
      await system.focus(); await page.keyboard.press('Enter');
      const inspector = page.getByRole('complementary', { name: 'Trace record inspector' });
      await expect(inspector.getByRole('tab', { name: 'System Prompt', exact: true })).toHaveAttribute('aria-selected', 'true');
      await expect(inspector.getByText('You are the historical agent.')).toBeVisible();
      await expectStableScreenshot(page, `trajectory-inspector-${width}-${theme}.png`);
      const context = ledger.locator('[data-display-type="ContextRow"]').last(); await context.click();
      await expect(inspector.getByRole('tab', { name: 'Context', exact: true })).toHaveAttribute('aria-selected', 'true');
      await expect(inspector.locator('[data-context-message-id="context-agent"][data-selected]').first()).toBeVisible();
      await inspector.getByRole('tab', { name: 'Context', exact: true }).focus(); await page.keyboard.press('ArrowRight');
      await expect(inspector.getByRole('tab', { name: 'Tools', exact: true })).toBeFocused();
      await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
      await inspector.getByRole('button', { name: 'Close record' }).click(); await expect(context).toBeFocused();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
    });
  }
}

test('T1-13 library drag, keyboard separator, double-click reset and narrow Ledger on wide viewport', async ({ page }) => {
  await page.setViewportSize({ width: 1050, height: 844 });
  await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html');
  await page.locator('[data-display-type="RequestBoundary"][data-owner="trace:9"]').click();
  const panel = page.locator('#inspector[data-panel]');
  const separator = page.getByRole('separator');
  const initial = (await panel.boundingBox())!.width;
  expect(initial).toBeGreaterThanOrEqual(320); expect(initial).toBeLessThanOrEqual(440);
  const box = (await separator.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + 50); await page.mouse.down(); await page.mouse.move(box.x - 180, box.y + 50); await page.mouse.up();
  await expect.poll(async () => (await panel.boundingBox())!.width).toBeGreaterThan(initial + 100);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await expect(ledger.locator('[data-display-type="RequestBoundary"]').first()).toHaveCSS('grid-template-columns', /90px/);
  await expectStableScreenshot(page, 'trajectory-resized-narrow-ledger.png');
  await separator.focus(); const before = await separator.getAttribute('aria-valuenow'); await page.keyboard.press('ArrowRight');
  await expect(separator).not.toHaveAttribute('aria-valuenow', before!);
  await separator.dblclick(); await expect.poll(async () => Math.abs((await panel.boundingBox())!.width - initial)).toBeLessThan(2);
  await expectStableScreenshot(page, 'trajectory-resize-reset.png');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('T1-08/09/15 Calls warnings, independent background, search and truncated failed Request Diff', async ({ page }) => {
  await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html');
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await page.getByRole('toolbar', { name: 'Trajectory controls' }).getByRole('button', { name: 'Collapse Calls' }).click();
  await expect(ledger.locator('[data-display-type="CollapsedCallSummary"]')).toContainText('2 proposed · 1 loaded matching executions · 1 failed');
  await expect(ledger.locator('[data-display-type="RecordRow"][data-owner="trace:5"]')).toHaveCount(0);
  await expect(ledger.locator('[data-owner="trace:6"]')).toBeVisible();
  await expectStableScreenshot(page, 'trajectory-calls.png');
  await page.getByRole('textbox', { name: 'Search loaded Trace' }).fill('Missing field');
  await expect(ledger.locator('[data-display-type="RecordRow"][data-owner="trace:5"]')).toBeVisible();
  await expectStableScreenshot(page, 'trajectory-search.png');
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '0');
  await page.getByRole('textbox', { name: 'Search loaded Trace' }).fill('');
  await ledger.locator('[data-display-type="RequestBoundary"][data-owner="trace:11"]').click();
  await expect(page.getByRole('complementary')).toContainText('failed');
  await page.getByRole('tab', { name: 'Diff', exact: true }).click();
  await expect(page.getByRole('tabpanel')).toContainText('current prompt truncated; previous prompt truncated');
  await expect(page.getByRole('tabpanel')).not.toContainText('No changes');
  await expectStableScreenshot(page, 'trajectory-truncated-diff.png');
});

for (const scenario of ['long', 'threshold']) {
  test(`T1-10/11 semantic prepend and tail isolation ${scenario}`, async ({ page }) => {
    await page.goto(`http://127.0.0.1:5174/test/fixtures/trajectory.html?${scenario}`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    const expected = scenario === 'long' ? 480 : 90;
    await expect(page.locator('[data-native-count]')).toHaveAttribute('data-native-count', String(expected));
    await ledger.evaluate(el => { el.scrollTop = 400; el.dispatchEvent(new Event('scroll')); });
    const anchor = await ledger.locator('[data-display-type="RequestBoundary"]').evaluateAll(elements => {
      const pane = elements[0]!.closest('[role="table"]')!.getBoundingClientRect();
      const row = elements.find(el => el.getBoundingClientRect().top >= pane.top && el.getBoundingClientRect().bottom < pane.bottom)!;
      return { key: row.getAttribute('data-display-key'), top: row.getBoundingClientRect().top };
    });
    const selected = ledger.locator(`[data-display-key=${JSON.stringify(anchor.key)}]`);
    // The overview history boundary loads without moving the scroll position to a toolbar control.
    await page.getByRole('button', { name: 'Load earlier records into the overview' }).click();
    await expect(page.locator('[data-native-count]')).toHaveAttribute('data-native-count', String(expected + 32));
    await expect.poll(async () => Math.abs((await selected.boundingBox())!.y - anchor.top)).toBeLessThan(2);
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    expect(await ledger.locator('[data-display-key]').count()).toBeLessThan(65);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    const offset = await ledger.evaluate(el => el.scrollTop);
    await page.getByRole('button', { name: 'Update', exact: true }).click();
    expect(await ledger.evaluate(el => el.scrollTop)).toBe(offset);
    await expectStableScreenshot(page, `trajectory-prepend-${scenario}.png`);
    await page.getByRole('button', { name: 'Jump to latest' }).click();
    await expect.poll(() => ledger.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThan(2);
    await page.getByRole('button', { name: 'Append', exact: true }).click();
    await expect.poll(() => ledger.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThan(2);
  });
}

test('T1-10 focused Step segment migrates to its exact native owner after a threshold prepend', async ({ page }) => {
  await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html?threshold');
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
  const segment = ledger.locator('[data-display-type="StepHeader"][data-owner="trace:100"]');
  await segment.focus(); await page.keyboard.press('Enter');
  await expect(segment).toBeFocused();
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
  // Invoke the read while keyboard ownership remains on the segment. No sleep
  // and no click-induced focus transfer can mask migration of the removed node.
  await page.getByRole('button', { name: 'Load earlier records into the overview' }).evaluate((el: HTMLButtonElement) => el.click());
  await expect(segment).toHaveCount(0);
  const owner = ledger.locator('[data-display-type="RequestBoundary"][data-owner="trace:100"]');
  await expect(owner).toHaveAttribute('data-selected', 'true');
  await expect(owner).toBeFocused();
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
});

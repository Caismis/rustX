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
      const system = ledger.locator('[data-display-type="SystemPromptCell"][data-owner="trace:3"]');
      await system.focus(); await page.keyboard.press('Enter');
      const inspector = page.getByRole('complementary', { name: 'Trace record inspector' });
      await expect(inspector.getByRole('tab', { name: 'System Prompt', exact: true })).toHaveAttribute('aria-selected', 'true');
      await expect(inspector.getByText('You are the historical agent.')).toBeVisible();
      await expectStableScreenshot(page, `trajectory-inspector-${width}-${theme}.png`);
      const context = ledger.locator('[data-display-type="ContextRow"]').last(); await context.click();
      await expect(inspector.getByRole('tab', { name: 'Context', exact: true })).toHaveAttribute('aria-selected', 'true');
      await expect(inspector.locator('[data-context-message-id="context-agent"][data-selected]').first()).toBeVisible();
      await inspector.getByRole('tab', { name: 'Context', exact: true }).focus(); await page.keyboard.press('ArrowRight');
      await expect(inspector.getByRole('tab', { name: 'Summary', exact: true })).toBeFocused();
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
    // 20px request rows permit more visible rows; overscan stays finite.
    expect(await ledger.locator('[data-display-key]').count()).toBeLessThan(85);
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

for (const kind of ['request', 'tool']) {
  test(`T1-04/06/10 structural ${kind} anchor stays structural through threshold prepend`, async ({ page }) => {
    await page.goto(`http://127.0.0.1:5174/test/fixtures/trajectory.html?threshold&structure${kind === 'tool' ? '&tool' : ''}`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const segment = ledger.locator('[data-display-type="GroupHeader"][data-anchor="trace:100"]');
    const recordInspector = page.getByRole('complementary', { name: 'Trace record inspector' });
    const structureInspector = page.getByRole('complementary', { name: 'Trace structure inspector' });
    await segment.focus(); await page.keyboard.press('Enter'); await segment.click();
    await expect(segment).toBeFocused();
    await expect(segment).not.toHaveAttribute('data-owner');
    await expect(recordInspector).toHaveCount(0);
    // No native Step is loaded yet: the child never lends its identity.
    await expect(structureInspector).toContainText('The exact native Step record is not loaded at this read cut');
    await expect(structureInspector).not.toContainText('trace:100');
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    // Anchor the top of this structure itself. The old native starts are absent.
    await ledger.evaluate(el => { el.scrollTop = 60; el.dispatchEvent(new Event('scroll')); });
    const before = (await segment.boundingBox())!.y;
    // Keep DOM keyboard ownership on the segment while invoking the controlled prepend.
    await page.getByRole('button', { name: 'Load earlier records into the overview' }).evaluate((el: HTMLButtonElement) => el.click());
    await expect(segment).toHaveCount(0);
    const merged = ledger.locator('[data-display-type="GroupHeader"][data-anchor="trace:51"]');
    await expect(merged).toBeFocused();
    await expect(merged).toHaveAttribute('data-selected', 'true');
    await expect(merged).toHaveAttribute('data-attempt', 'attempt-a');
    await expect(merged).toHaveAttribute('data-step', '1');
    await expect.poll(async () => Math.abs((await merged.boundingBox())!.y - before)).toBeLessThan(2);
    await expect(recordInspector).toHaveCount(0);
    // The exact native Step record now loaded is its own bounded evidence.
    await expect(structureInspector.getByText('Record', { exact: true }).locator('xpath=following-sibling::dd[1]')).toHaveText('trace:51');
    await expect(structureInspector.getByText('Logical Step', { exact: true }).locator('xpath=following-sibling::dd[1]')).toHaveText('1');
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    // Child inspection remains an explicit, separate user action.
    const child = ledger.locator(`[data-display-type="${kind === 'tool' ? 'RecordRow' : 'RequestBoundary'}"][data-owner="trace:100"]`);
    await ledger.evaluate(el => { el.scrollTop = 1700; el.dispatchEvent(new Event('scroll')); });
    await child.click();
    await expect(recordInspector).toBeVisible();
    await expect(structureInspector).toHaveCount(0);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
  });
}

test('T1-04 Calls summary keeps its display identity until explicit expansion', async ({ page }) => {
  await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html');
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await page.getByRole('toolbar').getByRole('button', { name: 'Collapse Calls' }).click();
  const summary = ledger.locator('[data-display-type="CollapsedCallSummary"][data-owner="trace:4"]');
  const assistant = ledger.locator('[data-display-type="RecordRow"][data-owner="trace:4"]');
  await summary.focus(); await page.keyboard.press('Enter');
  await expect(summary).toHaveAttribute('data-selected', 'true');
  await expect(summary).toBeFocused();
  await expect(assistant).toHaveAttribute('aria-selected', 'false');
  await expect(page.getByRole('complementary')).toBeVisible();
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
  await summary.getByRole('button', { name: 'Expand Calls' }).click();
  await expect(summary).toHaveCount(0);
  await expect(assistant).toHaveAttribute('data-selected', 'true');
  await expect(assistant).toBeFocused();
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '0');
});

for (const width of [1440, 390]) {
  test(`407: native identity survives renumbering, search and timeline navigation at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html?renumber');
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    const selected = ledger.locator('[data-display-type="SystemPromptCell"][data-owner="trace:3"]');
    await selected.click();
    const inspector = page.getByRole('complementary');
    await expect(inspector.getByText('You are the historical agent.')).toBeVisible();
    await ledger.getByRole('button', { name: 'Fold Turn 1', exact: true }).click();
    await expect(selected).toHaveCount(0);
    await expect(inspector).toBeVisible();
    await page.getByRole('button', { name: 'Load earlier records into the overview' }).click();
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(ledger.getByRole('button', { name: 'Expand Turn 2' })).toBeVisible();
    await expect(selected).toHaveCount(0);
    const search = page.getByRole('textbox', { name: 'Search loaded Trace' });
    await search.fill('request-3');
    await expect(selected).toHaveAttribute('aria-selected', 'true');
    await expect(ledger.getByRole('row', { name: 'Turn 2', exact: true })).toBeVisible();
    await search.fill('request-50');
    await expect(selected).toHaveCount(0);
    await search.fill('');
    await expect(selected).toHaveCount(0);
    await expect(inspector.getByText('You are the historical agent.')).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    await page.getByRole('button', { name: 'Inspect Request · deepseek-chat · request-3', exact: true }).click();
    const request = ledger.locator('[data-display-type="RequestBoundary"][data-owner="trace:3"]');
    await expect(request).toHaveAttribute('aria-selected', 'true');
    await expect(request).toBeFocused();
    await expect(ledger.getByRole('button', { name: 'Fold Turn 2' })).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

test('407: virtual sticky Turn and drag focus retain native ownership through prepend', async ({ page }) => {
  await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html?long&renumber');
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await ledger.evaluate(el => { el.scrollTop = 1000; el.dispatchEvent(new Event('scroll')); });
  const header = ledger.locator('[data-display-type="TurnHeader"]');
  await expect(header).toHaveCount(1);
  await expect.poll(async () => Math.abs((await header.boundingBox())!.y - (await ledger.boundingBox())!.y)).toBeLessThan(2);
  const canvas = page.getByLabel('Timeline navigation: arrow keys pan, Escape clears focus');
  const box = (await canvas.boundingBox())!;
  await page.mouse.move(box.x + box.width * .35, box.y + 30);
  await page.mouse.down(); await page.mouse.move(box.x + box.width * .45, box.y + 30); await page.mouse.up();
  await ledger.evaluate(el => { el.scrollTop = el.scrollHeight * .4; el.dispatchEvent(new Event('scroll')); });
  await expect(ledger.locator('[data-owner="trace:300"][data-timeline-focus="inside"]').first()).toBeAttached();
  const before = await ledger.locator('[data-timeline-focus="inside"]').evaluateAll(rows => rows.map(row => row.getAttribute('data-owner')));
  expect(before.length).toBeGreaterThan(0);
  await page.getByRole('button', { name: 'Load earlier records into the overview' }).click();
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
  for (const owner of before) await expect(ledger.locator(`[data-owner="${owner}"][data-timeline-focus="inside"]`).first()).toBeAttached();
  await expect(header).toHaveAttribute('aria-label', 'Turn 2');
  await expect(page.locator('[data-trace-epoch]')).toHaveAttribute('data-trace-epoch', '1');
  // Jump to latest rebases the read domain; the focus it owned is retired
  // even though the latest snapshot reuses those native identities.
  await expect(page.locator('[data-focus-range]')).toHaveCount(1);
  await page.getByRole('button', { name: 'Jump to latest' }).click();
  await expect(page.locator('[data-trace-epoch]')).toHaveAttribute('data-trace-epoch', '2');
  await expect(page.locator('[data-native-count]')).toHaveAttribute('data-native-count', '480');
  await expect(page.locator('[data-focus-range]')).toHaveCount(0);
  await expect(ledger.locator('[data-owner]').first()).toBeAttached();
  await expect(ledger.locator('[data-timeline-focus]')).toHaveCount(0);
});

for (const width of [1440, 390]) {
  test(`407: exact native Turn/Step evidence is inspectable by keyboard without detail reads at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html');
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    const inspector = page.getByRole('complementary', { name: 'Trace structure inspector' });
    const fact = (label: string) => inspector.locator('dt').filter({ hasText: new RegExp(`^${label}$`) }).locator('xpath=following-sibling::dd[1]');
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const turn = ledger.getByRole('row', { name: 'Turn 1', exact: true });
    await expect(turn).not.toContainText('attempt-a');
    await turn.focus(); await page.keyboard.press('Enter');
    await expect(inspector.getByText('Turn 1 · native Attempt')).toBeVisible();
    await expect(fact('Record')).toHaveText('trace:1');
    await expect(fact('Attempt')).toHaveText('attempt-a');
    await expect(fact('State')).toHaveText('completed');
    await expect(page.getByRole('complementary', { name: 'Trace record inspector' })).toHaveCount(0);
    const step = ledger.getByRole('row', { name: 'Step 1', exact: true });
    await step.focus(); await page.keyboard.press('Enter');
    await expect(step).toHaveAttribute('aria-selected', 'true');
    await expect(fact('Record')).toHaveText('trace:2');
    await expect(fact('Logical Step')).toHaveText('1');
    await inspector.getByRole('button', { name: 'Close structure' }).click();
    await expect(inspector).toHaveCount(0);
    await expect(step).toBeFocused();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

for (const kind of ['request', 'tool']) {
  test(`407: selected ${kind} cell and detail retain focus across virtual threshold`, async ({ page }) => {
    await page.goto(`http://127.0.0.1:5174/test/fixtures/trajectory.html?threshold${kind === 'tool' ? '&tool' : ''}`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const row = ledger.locator(`[data-display-type="${kind === 'tool' ? 'RecordRow' : 'RequestBoundary'}"][data-owner="trace:100"]`);
    await row.focus(); await page.keyboard.press('Enter');
    await expect(page.getByRole('complementary')).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    await page.getByRole('button', { name: 'Load earlier records into the overview' }).evaluate((button: HTMLButtonElement) => button.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(row).toHaveAttribute('aria-selected', 'true');
    await expect(row).toBeFocused();
    await expect(page.getByRole('complementary')).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
  });
}

for (const width of [1440, 390]) {
  test(`structural search shares native Timeline membership and restores folded Turns at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html?structural-search');
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await ledger.getByRole('button', { name: 'Fold Turn 1' }).click();
    await ledger.getByRole('button', { name: 'Fold Turn 2' }).click();
    const foldedKeys = await ledger.locator('[data-display-key]').evaluateAll(rows => rows.map(row => row.getAttribute('data-display-key')));
    const search = page.getByRole('textbox', { name: 'Search loaded Trace' });
    for (const [query, attempt, expected] of [
      ['Step 2', 'attempt-a', ['trace:5', 'trace:8']],
      ['Turn 2', 'attempt-b', ['trace:7']],
      ['Message', 'attempt-a', ['trace:2']],
    ] as const) {
      await search.fill(query);
      await expect(ledger.getByRole('row', { name: query, exact: true })).toHaveAttribute('data-attempt', attempt);
      await expect(ledger.getByRole('row', { name: attempt === 'attempt-a' ? 'Turn 1' : 'Turn 2', exact: true })).toBeVisible();
      await expect.poll(() => page.locator('[data-record-id]:not([data-dimmed])').evaluateAll(spans => spans.map(span => span.getAttribute('data-record-id')))).toEqual([...expected]);
      await expect.poll(() => ledger.locator('[data-owner]').evaluateAll(rows => rows.map(row => row.getAttribute('data-owner')))).toEqual([...expected]);
      for (const id of expected) await expect(ledger.locator(`[data-owner="${id}"]`)).toBeVisible();
      await expect(page.locator('[data-record-id="trace:3"]')).toHaveAttribute('data-dimmed');
    }
    await search.fill('');
    await expect(ledger.getByRole('button', { name: 'Expand Turn 1' })).toBeVisible();
    await expect(ledger.getByRole('button', { name: 'Expand Turn 2' })).toBeVisible();
    expect(await ledger.locator('[data-display-key]').evaluateAll(rows => rows.map(row => row.getAttribute('data-display-key')))).toEqual(foldedKeys);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '0');
  });
}

const fixtureOrigin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
import { test, expect } from '@playwright/test';
import { expectStableScreenshot } from './screenshot';

for (const width of [1440, 390]) {
  for (const theme of ['light', 'dark']) {
    test(`T1-13/15 semantic ledger, facets and keyboard ${width} ${theme}`, async ({ page }) => {
      const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
      await page.setViewportSize({ width, height: 844 });
      await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
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
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
  await page.locator('[data-request-owner="trace:9"]').click();
  const panel = page.locator('#inspector[data-panel]');
  const separator = page.getByRole('separator');
  const initial = (await panel.boundingBox())!.width;
  expect(initial).toBeGreaterThanOrEqual(320); expect(initial).toBeLessThanOrEqual(440);
  const box = (await separator.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + 50); await page.mouse.down(); await page.mouse.move(box.x - 180, box.y + 50); await page.mouse.up();
  await expect.poll(async () => (await panel.boundingBox())!.width).toBeGreaterThan(initial + 100);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await expect(ledger.locator('[role="row"]').first()).toHaveCSS('grid-template-columns', /50px/);
  await expectStableScreenshot(page, 'trajectory-resized-narrow-ledger.png');
  await separator.focus(); const before = await separator.getAttribute('aria-valuenow'); await page.keyboard.press('ArrowRight');
  await expect(separator).not.toHaveAttribute('aria-valuenow', before!);
  await separator.dblclick(); await expect.poll(async () => Math.abs((await panel.boundingBox())!.width - initial)).toBeLessThan(2);
  await expectStableScreenshot(page, 'trajectory-resize-reset.png');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('T1-08/09/15 Calls warnings, independent background, search and truncated failed Request Diff', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await page.getByRole('toolbar', { name: 'Trajectory controls' }).getByRole('button', { name: 'Collapse Calls' }).click();
  await expect(ledger.locator('[data-display-type="CollapsedCallSummary"]')).toContainText('2 proposed · 1 loaded matching executions · 1 failed');
  await expect(ledger.locator('[data-display-type="RecordRow"][data-owner="trace:5"]')).toHaveCount(0);
  await expect(ledger.locator('[data-owner="trace:6"]')).toBeVisible();
  await expectStableScreenshot(page, 'trajectory-calls.png');
  await page.getByRole('searchbox', { name: 'Search loaded Trace' }).fill('Missing field');
  await expect(ledger.locator('[data-display-type="RecordRow"][data-owner="trace:5"]')).toBeVisible();
  await expectStableScreenshot(page, 'trajectory-search.png');
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '0');
  await page.getByRole('searchbox', { name: 'Search loaded Trace' }).fill('');
  await ledger.locator('[data-request-owner="trace:11"]').click();
  await expect(page.getByRole('complementary')).toContainText('failed');
  await page.getByRole('tab', { name: 'Diff', exact: true }).click();
  await expect(page.getByRole('tabpanel')).toContainText('current prompt truncated; previous prompt truncated');
  await expect(page.getByRole('tabpanel')).not.toContainText('No changes');
  await expectStableScreenshot(page, 'trajectory-truncated-diff.png');
});

for (const scenario of ['long', 'threshold']) {
  test(`T1-10/11 semantic prepend and tail isolation ${scenario}`, async ({ page }) => {
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?${scenario}`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    const expected = scenario === 'long' ? 480 : 90;
    await expect(page.locator('[data-native-count]')).toHaveAttribute('data-native-count', String(expected));
    await ledger.evaluate(el => { el.scrollTop = Math.min(400, (el.scrollHeight - el.clientHeight) / 2); el.dispatchEvent(new Event('scroll')); });
    // Include a structural Request seat: prepend can move its chrome away and
    // shrink it from 20px to 10px. Anchor that same seat, not its next sibling.
    const anchor = await ledger.locator('[role="row"][data-owner]').evaluateAll(elements => {
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
    // The smallest seat is 10px. Count measurable rows, not nested action keys.
    const capacity = await ledger.evaluate(el => Math.ceil(el.clientHeight / 10) + 2 * 12 + 1);
    expect(await ledger.locator('[role="row"]').count()).toBeLessThanOrEqual(capacity);
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
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?threshold&structure${kind === 'tool' ? '&tool' : ''}`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const segment = ledger.locator('[data-attempt="attempt-a"] [data-structural="turn"]');
    const recordInspector = page.getByRole('complementary', { name: 'Trace record inspector' });
    const structureInspector = page.getByRole('complementary', { name: 'Trace structure inspector' });
    await segment.focus(); await page.keyboard.press('Enter'); await segment.click();
    await expect(segment).toBeFocused();
    await expect(segment).not.toHaveAttribute('data-owner');
    await expect(recordInspector).toHaveCount(0);
    // No native Attempt is loaded yet: the child never lends its identity.
    await expect(structureInspector).toContainText('The exact native Attempt record is not loaded at this read cut');
    await expect(structureInspector).not.toContainText('trace:100');
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    // Anchor the top of this structure itself. The old native starts are absent.
    await segment.evaluate(el => { const pane = el.closest('[data-trajectory-scroll]')!; pane.scrollTop += el.getBoundingClientRect().top - pane.getBoundingClientRect().top; pane.dispatchEvent(new Event('scroll')); });
    const before = (await segment.boundingBox())!.y;
    // Keep DOM keyboard ownership on the segment while invoking the controlled prepend.
    await page.getByRole('button', { name: 'Load earlier records into the overview' }).evaluate((el: HTMLButtonElement) => el.click());
    // Same native Turn control survives a changed loaded anchor.
    const merged = ledger.locator('[data-attempt="attempt-a"] [data-structural="turn"]');
    await expect(merged).toBeFocused();
    await expect(merged).toHaveAttribute('data-selected', 'true');
    await expect.poll(async () => Math.abs((await merged.boundingBox())!.y - before)).toBeLessThan(2);
    await expect(recordInspector).toHaveCount(0);
    // The exact native Attempt record now loaded is its own bounded evidence.
    await expect(structureInspector.getByText('Record', { exact: true }).locator('xpath=following-sibling::dd[1]')).toHaveText('trace:50');
    await expect(structureInspector.locator('dt').filter({ hasText: /^Attempt$/ }).locator('xpath=following-sibling::dd[1]')).toHaveText('attempt-a');
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    // Child inspection remains an explicit, separate user action.
    const child = ledger.locator(kind === 'tool' ? '[data-display-type="RecordRow"][data-owner="trace:100"]' : '[data-request-owner="trace:100"]');
    await ledger.evaluate(el => { el.scrollTop = 1700; el.dispatchEvent(new Event('scroll')); });
    await child.click();
    await expect(recordInspector).toBeVisible();
    await expect(structureInspector).toHaveCount(0);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
  });
}

test('T1-04 Calls summary keeps its display identity until explicit expansion', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
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
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?renumber`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    const selected = ledger.locator('[data-display-type="SystemPromptCell"][data-owner="trace:3"]');
    await selected.click();
    const inspector = page.getByRole('complementary');
    await expect(inspector.getByText('You are the historical agent.')).toBeVisible();
    await ledger.getByRole('button', { name: 'Fold Turn 1', exact: true }).click();
    await expect(selected).toBeVisible();
    await expect(inspector).toBeVisible();
    await page.getByRole('button', { name: 'Load earlier records into the overview' }).click();
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(ledger.getByRole('button', { name: 'Expand Turn 2' })).toBeVisible();
    await expect(selected).toBeVisible();
    const search = page.getByRole('searchbox', { name: 'Search loaded Trace' });
    await search.fill('request-3');
    await expect(selected).toHaveAttribute('aria-selected', 'true');
    await expect(ledger.getByRole('button', { name: 'Turn 2', exact: true })).toBeVisible();
    await search.fill('request-50');
    await expect(selected).toHaveCount(0);
    await search.fill('');
    await expect(selected).toBeVisible();
    await expect(inspector.getByText('You are the historical agent.')).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    await page.getByRole('button', { name: 'Inspect Request · deepseek-chat · request-3', exact: true }).click();
    const request = ledger.locator('[data-request-owner="trace:3"]');
    await expect(request).toHaveAttribute('aria-pressed', 'true');
    await expect(request).toBeFocused();
    await expect(ledger.getByRole('button', { name: 'Fold Turn 2' })).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

test('421: virtual semantic ledger and drag focus retain native ownership through prepend', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?long&renumber`);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await ledger.evaluate(el => { el.scrollTop = 1000; el.dispatchEvent(new Event('scroll')); });
  await expect(ledger.locator('[data-display-type="TurnHeader"]')).toHaveCount(0);
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
  await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
  await expect(ledger.getByRole('button', { name: 'Turn 2', exact: true })).toBeVisible();
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
  test(`407: Timeline gesture and viewport cannot cross a Trace epoch at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?long`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    const epoch = page.locator('[data-trace-epoch]');
    const canvas = page.getByLabel('Timeline navigation: arrow keys pan, Escape clears focus');
    const jump = page.getByRole('button', { name: 'Jump to latest' });
    const domain = () => canvas.evaluate(el => [el.getAttribute('data-domain-start'), el.getAttribute('data-domain-end')]);
    const full = await domain();
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    // Zoom and pan in E1; the latest snapshot has the identical numeric domain.
    await canvas.focus(); await page.keyboard.press('+');
    await canvas.focus(); await page.keyboard.press('ArrowRight');
    await expect.poll(domain).not.toEqual(full);
    await jump.click();
    await expect(epoch).toHaveAttribute('data-trace-epoch', '2');
    await expect.poll(domain).toEqual(full);
    // Hold an E1 drag across the rebase and release it only in E2.
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    await expect(jump).toBeVisible();
    const box = (await canvas.boundingBox())!;
    await page.mouse.move(box.x + box.width * .35, box.y + 30);
    await page.mouse.down(); await page.mouse.move(box.x + box.width * .45, box.y + 30);
    await expect(page.locator('[data-focus-range]')).toHaveCount(1);
    await expect(ledger.locator('[data-timeline-focus]')).toHaveCount(0);
    // A DOM click keeps the physical button pressed while the domain is replaced.
    await jump.evaluate((button: HTMLButtonElement) => button.click());
    await expect(epoch).toHaveAttribute('data-trace-epoch', '3');
    await expect(page.locator('[data-focus-range]')).toHaveCount(0);
    await page.mouse.up();
    await expect(page.locator('[data-focus-range]')).toHaveCount(0);
    await expect(ledger.locator('[data-timeline-focus]')).toHaveCount(0);
    await expect(ledger.locator('[aria-selected="true"]')).toHaveCount(0);
    await expect(page.getByRole('complementary')).toHaveCount(0);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    // A gesture begun in E3 focuses E3 normally. The toolbar lost its Jump
    // button, so the canvas may have moved: measure it again.
    const current = (await canvas.boundingBox())!;
    await page.mouse.move(current.x + current.width * .35, current.y + 30);
    await page.mouse.down(); await page.mouse.move(current.x + current.width * .45, current.y + 30); await page.mouse.up();
    await expect(page.locator('[data-focus-range]')).toHaveCount(1);
    await expect(ledger.locator('[data-timeline-focus]').first()).toBeAttached();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

for (const width of [1440, 390]) {
  test(`407: exact native Turn evidence is inspectable by keyboard without detail reads at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    const inspector = page.getByRole('complementary', { name: 'Trace structure inspector' });
    const fact = (label: string) => inspector.locator('dt').filter({ hasText: new RegExp(`^${label}$`) }).locator('xpath=following-sibling::dd[1]');
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const turn = ledger.getByRole('button', { name: 'Turn 1', exact: true });
    await expect(turn).not.toContainText('attempt-a');
    await turn.focus(); await page.keyboard.press('Enter');
    await expect(inspector.locator('header')).toContainText('Attempt'); await expect(inspector.locator('header').getByText('Turn 1', { exact: true })).toBeVisible();
    await expect(fact('Record')).toHaveText('trace:1');
    await expect(fact('Attempt')).toHaveText('attempt-a');
    await expect(fact('State')).toHaveText('completed');
    await expect(page.getByRole('complementary', { name: 'Trace record inspector' })).toHaveCount(0);
    await expect(turn).toHaveAttribute('aria-pressed', 'true');
    // As in Harness, a Step has no chrome to inspect.
    await expect(ledger.getByRole('button', { name: 'Step 1', exact: true })).toHaveCount(0);
    await inspector.getByRole('button', { name: 'Close structure' }).click();
    await expect(inspector).toHaveCount(0);
    await expect(turn).toBeFocused();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

for (const kind of ['request', 'tool']) {
  test(`407: selected ${kind} cell and detail retain focus across virtual threshold`, async ({ page }) => {
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?threshold${kind === 'tool' ? '&tool' : ''}`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const row = ledger.locator(kind === 'tool' ? '[data-display-type="RecordRow"][data-owner="trace:100"]' : '[data-request-owner="trace:100"]');
    await row.focus(); await page.keyboard.press('Enter');
    await expect(page.getByRole('complementary')).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    await page.getByRole('button', { name: 'Load earlier records into the overview' }).evaluate((button: HTMLButtonElement) => button.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(row).toHaveAttribute(kind === 'tool' ? 'aria-selected' : 'aria-pressed', 'true');
    await expect(row).toBeFocused();
    await expect(page.getByRole('complementary')).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
  });
}

for (const width of [1440, 390]) {
  test(`structural search shares native Timeline membership and restores folded Turns at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?structural-search`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await ledger.getByRole('button', { name: 'Fold Turn 1' }).click();
    await ledger.getByRole('button', { name: 'Fold Turn 2' }).click();
    const foldedKeys = await ledger.locator('[data-display-key]').evaluateAll(rows => rows.map(row => row.getAttribute('data-display-key')));
    const search = page.getByRole('searchbox', { name: 'Search loaded Trace' });
    for (const [query, attempt, expected] of [
      ['Step 2', 'attempt-a', ['trace:5', 'trace:8']],
      ['Turn 2', 'attempt-b', ['trace:7']],
      ['Message', 'attempt-a', ['trace:2']],
    ] as const) {
      await search.fill(query);
      // As in Harness, only a Turn has chrome; Step and Message queries expose
      // their member cells without a header of their own.
      if (query.startsWith('Turn')) await expect(ledger.getByRole('button', { name: query, exact: true }).locator('xpath=ancestor::*[@data-attempt][1]')).toHaveAttribute('data-attempt', attempt);
      else await expect(ledger.getByRole('button', { name: query, exact: true })).toHaveCount(0);
      await expect(ledger.getByRole('button', { name: attempt === 'attempt-a' ? 'Turn 1' : 'Turn 2', exact: true })).toBeVisible();
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

for (const width of [1440, 390]) {
  test(`407: a real held drag cannot cross same-epoch prepend at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?renumber`);
    const canvas = page.getByLabel('Timeline navigation: arrow keys pan, Escape clears focus');
    const box = (await canvas.boundingBox())!;
    const epoch = await page.locator('[data-trace-epoch]').getAttribute('data-trace-epoch');
    const end = await canvas.getAttribute('data-domain-end');
    await page.mouse.move(box.x + box.width * .30, box.y + 30);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * .45, box.y + 30);
    await expect(page.locator('[data-focus-range]')).toHaveCount(1);
    // Synchronous fixture action while the actual browser pointer is held.
    await page.getByRole('button', { name: 'Load earlier records into the overview' }).evaluate((button: HTMLButtonElement) => button.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(page.locator('[data-trace-epoch]')).toHaveAttribute('data-trace-epoch', epoch!);
    await expect(canvas).not.toHaveAttribute('data-domain-end', end!);
    await expect(page.locator('[data-focus-range]')).toHaveCount(0);
    await page.mouse.up();
    await expect(page.locator('[data-timeline-focus]')).toHaveCount(0);
    await expect(page.getByRole('complementary')).toHaveCount(0);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    const current = (await canvas.boundingBox())!;
    await page.mouse.move(current.x + current.width * .30, current.y + 30);
    await page.mouse.down();
    await page.mouse.move(current.x + current.width * .45, current.y + 30);
    await page.mouse.up();
    await expect(page.locator('[data-focus-range]')).toHaveCount(1);
    await expect(page.locator('[data-timeline-focus="inside"]').first()).toBeAttached();
    await expect(page.getByRole('complementary')).toHaveCount(0);
  });
}

for (const dimension of ['prompt', 'tools']) {
  test(`independent ${dimension} change survives unavailable other predecessor`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?mixed=${dimension}`);
    await expect(page).toHaveTitle('Trajectory presentation contracts');
    const cell = page.locator('[data-display-type="SystemPromptCell"]');
    await expect(cell).toContainText(dimension === 'prompt' ? 'System Prompt Updated' : 'Tools Updated');
    await expect(cell).toContainText(dimension === 'prompt' ? 'Previous Tool catalog unavailable' : 'Previous System Prompt unavailable');
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    await cell.click();
    await expect(page.getByRole('tab', { name: dimension === 'prompt' ? 'Diff' : 'Tools', exact: true })).toHaveAttribute('aria-selected', 'true');
    if (dimension === 'prompt') {
      await expect(page.getByLabel('System prompt diff')).toContainText('Previous prompt.');
    } else {
      await expect(page.getByRole('tab', { name: 'Diff', exact: true })).toHaveCount(0);
    }
    await page.getByRole('tab', { name: 'Tools', exact: true }).click();
    await expect(page.getByRole('tabpanel')).toContainText('Run one command.');
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    expect(errors).toEqual([]);
  });
}

for (const width of [1440, 390]) for (const locale of ['en', 'zh'] as const) {
  test(`421: semantic ledger acceptance ${width}px ${locale}`, async ({ page }) => {
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(locale => localStorage.setItem('rustx-locale-v1', locale), locale);
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?ledger`);
    const ledger = page.locator('[data-trajectory-scroll]');
    await expect(page).toHaveTitle('Trajectory presentation contracts');
    await expect(ledger.locator('[role="row"][data-display-type="TurnHeader"], [role="row"][data-display-type="GroupHeader"], [role="row"][data-display-type="RequestBoundary"]')).toHaveCount(0);
    const system = ledger.locator('[data-display-type="SystemPromptCell"]').first();
    const turn = ledger.locator('[data-structural="turn"]').first();
    expect((await system.boundingBox())!.y).toBeLessThan((await turn.boundingBox())!.y);
    const assertHierarchy = async () => {
      const positions = await ledger.locator('[role="row"]').evaluateAll(rows => rows.flatMap(row => [...row.querySelectorAll<HTMLElement>('[data-structural]')].map(control => ({ attempt: row.getAttribute('data-attempt'), kind: control.dataset.structural, y: control.getBoundingClientRect().y }))));
      // As in Harness, a Request dot sits on its row's top boundary or inside
      // its own marker seat; every other control stays inside its row.
      const contained = await ledger.locator('[role="row"]').evaluateAll(rows => rows.every(row => {
        const box = row.getBoundingClientRect();
        return [...row.querySelectorAll('button')].every(button => { const control = button.getBoundingClientRect(); const center = (control.top + control.bottom) / 2;
          return button.hasAttribute('data-request-owner') ? center >= box.top - .5 && center <= box.bottom : control.top >= box.top && control.bottom <= box.bottom; });
      }));
      expect(contained).toBe(true);
      // As in Harness, the Turn label is the only structural chrome.
      expect(positions.every(control => control.kind === 'turn')).toBe(true);
    };
    await assertHierarchy();
    await expect(system.locator('[data-structural="step"]')).toHaveCount(0);
    // As in Harness, the System row names its change; the prompt is inspected.
    await expect(system).toContainText(locale === 'zh' ? '初始系统提示词' : 'Initial System Prompt');
    await expect(system).not.toContainText('historical agent');
    await expect(ledger.locator('[data-owner="trace:104"]')).toContainText('git diff --stat');
    await expect(ledger.locator('[data-owner="trace:104"]')).toContainText('3 files changed');
    const marker = ledger.locator('[data-request-owner="trace:108"]');
    await expect(marker).toHaveAttribute('data-request-id', 'request-108');
    // The recovery Request's dot sits above its own output; the failed retry
    // before it has no output, so it keeps its own 10px marker seat.
    await expect(marker.locator('xpath=ancestor::*[@role="row"]')).toHaveAttribute('data-owner', 'trace:109');
    const failedSeat = ledger.locator('[data-request-owner="trace:107"]').locator('xpath=ancestor::*[@role="row"]');
    await expect(failedSeat).toHaveAttribute('data-display-type', 'MarkerSeat');
    expect((await failedSeat.boundingBox())!.height).toBe(10);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    // Native failure wins over the Request lane's normal violet color.
    const errorColor = await page.locator('[data-record-id="trace:105"]').evaluate(el => getComputedStyle(el).backgroundColor);
    await expect(page.locator('[data-record-id="trace:107"]')).toHaveCSS('background-color', errorColor);
    await expectStableScreenshot(page, `ledger-421-${width}-${locale}.png`);
    await turn.click(); await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    await page.getByRole('complementary').getByRole('button').click();
    const fold = ledger.locator('[data-turn-start]').first().getByRole('button').first();
    await fold.click();
    await expect(system).toBeVisible();
    await expect(ledger.locator('[data-owner="trace:101"]')).toBeVisible();
    await expect(ledger.locator('[data-display-type="TurnSummary"]')).toHaveCount(1);
    await assertHierarchy();
    await expectStableScreenshot(page, `ledger-421-fold-${width}-${locale}.png`);
    const search = page.getByRole('searchbox');
    await search.fill('Step 2'); await expect(marker).toBeVisible(); await assertHierarchy();
    await search.fill('historical agent'); await assertHierarchy();
    await search.fill(''); await expect(ledger.locator('[data-display-type="TurnSummary"]')).toHaveCount(1);
    await fold.click(); await marker.click();
    await expect(marker).toHaveAttribute('aria-pressed', 'true');
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    await expectStableScreenshot(page, `ledger-421-inspector-${width}-${locale}.png`);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  });
}

for (const virtual of [false, true]) {
  test(`424: exact structural arrows, seat geometry and prepend ${virtual ? 'virtual' : 'plain'}`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 600 });
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?chrome${virtual ? '&long' : ''}`);
    const ledger = page.locator('[data-trajectory-scroll]');
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const system = ledger.locator('[data-display-type="SystemPromptCell"][data-owner="trace:3"]');
    const turn = ledger.locator('[data-attempt="attempt-a"] [data-structural="turn"]');
    const initialRequest = ledger.locator('[data-request-owner="trace:3"]');
    const reads = page.locator('[data-detail-reads]');
    await expect(system).toBeVisible();
    expect((await system.boundingBox())!.y).toBeLessThan((await turn.boundingBox())!.y);
    // As in Harness, Steps have no chrome.
    await expect(ledger.locator('[data-structural="step"]')).toHaveCount(0);
    await expect(system.locator('[data-request-owner]')).toHaveCount(0);
    await expect(ledger.locator('[data-request-owner="trace:3"]')).toHaveAttribute('data-request-id', 'request-3');
    // Fixed model/virtual estimates must contain actual controls and not overlap.
    const geometry = await ledger.locator('[role="row"]').evaluateAll(rows => rows.map((row, index) => {
      const box = row.getBoundingClientRect(); const next = rows[index + 1]?.getBoundingClientRect();
      return { kind: row.getAttribute('data-display-type'), height: box.height, model: Number.parseFloat((row as HTMLElement).style.height),
        contained: [...row.querySelectorAll('button')].every(button => { const b = button.getBoundingClientRect(); const center = (b.top + b.bottom) / 2;
          return button.hasAttribute('data-request-owner') ? center >= box.top - .5 && center <= box.bottom : b.top >= box.top && b.bottom <= box.bottom; }),
        nonoverlap: !next || box.bottom <= next.top + .01 };
    }));
    expect(geometry.every(row => row.height === row.model && row.contained && row.nonoverlap)).toBe(true);
    // Only Turn openings grow to 20px: the output-less initial Request and the empty Turn.
    expect(geometry.filter(row => row.kind === 'StructuralSeat').map(row => row.height)).toEqual([20, 20]);
    expect(geometry.some(row => row.kind === 'MarkerSeat' && row.height === 10)).toBe(true);
    if (virtual) expect(await ledger.locator('[role="row"]').count()).toBeLessThan(150);
    await system.focus(); await page.keyboard.press('ArrowDown'); await expect(turn).toBeFocused();
    await expect(page.getByRole('complementary')).toContainText('trace:1');
    await expect(reads).toHaveAttribute('data-detail-reads', '0');
    await page.keyboard.press('ArrowDown'); await expect(initialRequest).toBeFocused();
    await expect(reads).toHaveAttribute('data-detail-reads', '1');
    await page.keyboard.press('ArrowUp'); await expect(turn).toBeFocused();
    await expect(page.getByRole('complementary')).toContainText('trace:1');
    // Give the disappearing history affordance real scroll extent in both modes.
    await turn.evaluate(el => { const pane = el.closest('[data-trajectory-scroll]')!; pane.scrollTop += el.getBoundingClientRect().top - pane.getBoundingClientRect().top; pane.dispatchEvent(new Event('scroll')); });
    const key = await turn.getAttribute('data-display-key'); const before = (await turn.boundingBox())!.y;
    await page.getByRole('button', { name: 'Load earlier records into the overview' }).evaluate((el: HTMLButtonElement) => el.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(turn).toHaveAttribute('data-display-key', key!); await expect(turn).toBeFocused();
    await expect.poll(async () => Math.abs((await turn.boundingBox())!.y - before)).toBeLessThan(2);
    await expect(page.getByRole('complementary')).toContainText('trace:1');
    await expect(reads).toHaveAttribute('data-detail-reads', '1');
    await page.getByRole('button', { name: 'Close structure' }).click(); await expect(turn).toBeFocused();
    const user = ledger.locator('[data-display-type="RecordRow"][data-owner="trace:91"]');
    const request = ledger.locator('[data-request-owner="trace:100"]');
    await user.focus(); await page.keyboard.press('ArrowDown'); await expect(request).toBeFocused();
    await expect(request).toHaveAttribute('data-request-id', 'request-100');
    await expect(reads).toHaveAttribute('data-detail-reads', '2');
    await page.keyboard.press('ArrowUp'); await expect(user).toBeFocused();
  });
}

for (const width of [1440, 390]) for (const locale of ['en', 'zh'] as const) {
  test(`424 order: native groups, virtual fallback and prepend ${width} ${locale}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(locale => localStorage.setItem('rustx-locale-v1', locale), locale);
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?ordered`);
    const ledger = page.locator('[data-trajectory-scroll]');
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const turn = ledger.locator('[data-attempt="ordered-turn"] [data-structural="turn"]');
    const system = ledger.locator('[data-display-type="SystemPromptCell"][data-owner="trace:702"]');
    const request = ledger.locator('[data-request-owner="trace:702"]');
    const later = ledger.locator('[data-display-type="RecordRow"][data-owner="trace:706"]');
    const reads = page.locator('[data-detail-reads]');
    const opening = turn.locator('xpath=ancestor::*[@role="row"]');
    await expect(turn).toBeVisible();
    expect((await system.boundingBox())!.y).toBeLessThan((await turn.boundingBox())!.y);
    // As in Harness, Steps have no chrome, and the empty middle Steps add no row:
    // the output-less initial Request opens the Turn and the later output follows it.
    await expect(ledger.locator('[data-step]')).toHaveCount(0);
    expect(await ledger.locator('[role="row"]').count()).toBeLessThan(150);
    const geometry = await opening.evaluate(row => {
      const box = row.getBoundingClientRect(); const next = row.nextElementSibling as HTMLElement;
      return { height: box.height, estimate: Number((row as HTMLElement).style.height.replace('px', '')), kind: (row as HTMLElement).dataset.displayType,
        owner: (row as HTMLElement).dataset.owner, next: next.dataset.owner, nonoverlap: box.bottom <= next.getBoundingClientRect().top };
    });
    expect(geometry).toEqual({ height: 20, estimate: 20, kind: 'StructuralSeat', owner: 'trace:702', next: 'trace:706', nonoverlap: true });
    await expect(opening.locator('[data-request-owner="trace:702"]')).toHaveCount(1);
    await expectStableScreenshot(page, `ledger-order-${width}-${locale}.png`);
    await turn.focus();
    await page.keyboard.press('ArrowDown'); await expect(request).toBeFocused();
    await page.keyboard.press('ArrowDown'); await expect(later).toBeFocused();
    await expect(reads).toHaveAttribute('data-detail-reads', '2');
    await expectStableScreenshot(page, `ledger-order-inspector-${width}-${locale}.png`);
    await page.getByRole('complementary').getByRole('button', { name: locale === 'en' ? 'Close record' : '关闭记录' }).click();
    const fold = ledger.locator('[data-attempt="ordered-turn"][data-turn-start]').getByRole('button').first();
    await fold.click();
    await expect(later).toBeVisible();
    await expect(ledger.locator('[data-attempt="ordered-turn"] [data-request-owner]')).toHaveCount(0);
    await expect(ledger.locator('[data-attempt="ordered-turn"][data-display-type="StructuralSeat"]')).toHaveCount(0);
    await expect(ledger.locator('[data-display-type="TurnSummary"]')).toHaveCount(1);
    await expectStableScreenshot(page, `ledger-order-fold-${width}-${locale}.png`);
    await fold.click();
    await turn.click();
    await turn.evaluate(el => { const pane = el.closest('[data-trajectory-scroll]')!; pane.scrollTop += el.getBoundingClientRect().top - pane.getBoundingClientRect().top; pane.dispatchEvent(new Event('scroll')); });
    const key = await turn.getAttribute('data-display-key'); const before = (await turn.boundingBox())!.y;
    // Invoke the real controlled page callback without transferring DOM focus.
    await page.getByRole('button', { name: locale === 'en' ? 'Load earlier records into the overview' : '将更早记录加载到概览' }).evaluate((el: HTMLButtonElement) => el.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    // The prepended record now opens the Turn; its focused label keeps its place.
    await expect(turn).toBeFocused(); await expect(turn).toHaveAttribute('data-display-key', key!);
    await expect.poll(async () => Math.abs((await turn.boundingBox())!.y - before)).toBeLessThan(2);
    await expect(opening).toHaveAttribute('data-owner', 'trace:650');
    expect((await system.boundingBox())!.y).toBeLessThan((await turn.boundingBox())!.y);
    await expect(reads).toHaveAttribute('data-detail-reads', '2');
  });
}

for (const width of [1440, 390]) for (const locale of ['en', 'zh'] as const) {
  test(`424 compact: fifty Steps collapse, expand and prepend ${width} ${locale}`, async ({ page }) => {
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize({ width, height: 1500 });
    await page.addInitScript(locale => localStorage.setItem('rustx-locale-v1', locale), locale);
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?compact`);
    await expect(page).toHaveTitle('Trajectory presentation contracts');
    const ledger = page.locator('[data-trajectory-scroll]');
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const turnRows = ledger.locator('[role="row"][data-attempt="ordered-turn"]');
    const turn = turnRows.locator('[data-structural="turn"]');
    const toggle = ledger.locator('[data-attempt="ordered-turn"][data-turn-start]').getByRole('button').first();
    const owners = () => turnRows.evaluateAll(rows => rows.map(row => row.getAttribute('data-owner')));
    const virtualHeight = () => ledger.locator(':scope > div').first().evaluate(el => Number.parseFloat((el as HTMLElement).style.height));
    // As in Harness, fifty native Steps cost no rows: the Turn shows its System
    // cell, the output-less initial Request that opens it, and its later output.
    await expect(ledger.locator('[data-step]')).toHaveCount(0);
    await expect(turnRows).toHaveCount(3);
    expect(await owners()).toEqual(['trace:702', 'trace:702', 'trace:706']);
    await expect(ledger).toHaveAttribute('aria-rowcount', '154');
    expect(await ledger.locator('[role="row"]').count()).toBeLessThan(154);
    expect(await virtualHeight()).toBe(1620);
    await expectStableScreenshot(page, `ledger-compact-expanded-${width}-${locale}.png`);
    const keys = await turnRows.evaluateAll(rows => rows.map(row => row.getAttribute('data-display-key')));
    await toggle.click();
    await expect(toggle).toBeFocused();
    await expect(ledger).toHaveAttribute('aria-rowcount', '154');
    await expect(turnRows).toHaveCount(3);
    expect(await owners()).toEqual(['trace:702', 'trace:706', null]);
    await expect(ledger.locator('[data-attempt="ordered-turn"][data-display-type="StructuralSeat"]')).toHaveCount(0);
    await expect(ledger.locator('[data-attempt="ordered-turn"][data-display-type="TurnSummary"]')).toHaveCount(1);
    // As in Harness, a folded Turn shows no Request markers.
    await expect(turnRows.locator('[data-request-owner]')).toHaveCount(0);
    expect(await virtualHeight()).toBe(1620);
    const geometry = await turnRows.evaluateAll(rows => {
      const boxes = rows.map(row => row.getBoundingClientRect());
      return { heights: boxes.map(box => box.height), span: boxes.at(-1)!.bottom - boxes[0]!.top,
        contained: rows.every((row, i) => [...row.querySelectorAll('button')].every(button => { const b = button.getBoundingClientRect(); return b.top >= boxes[i]!.top && b.bottom <= boxes[i]!.bottom; })),
        ordered: boxes.every((box, i) => i === 0 || box.top >= boxes[i - 1]!.bottom) };
    });
    expect(geometry).toEqual({ heights: [30, 30, 20], span: 80, contained: true, ordered: true });
    await expectStableScreenshot(page, `ledger-compact-fold-${width}-${locale}.png`);
    await turn.focus(); await page.keyboard.press('ArrowDown');
    await expect(turnRows.filter({ has: page.locator('[data-structural="turn"]') })).toBeFocused();
    await page.keyboard.press('ArrowUp'); await expect(turn).toBeFocused();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    await page.getByRole('complementary').getByRole('button', { name: locale === 'en' ? 'Close structure' : '关闭结构' }).click();
    // Search exposes folded content without changing saved folds.
    const collapsedKeys = await turnRows.evaluateAll(rows => rows.map(row => row.getAttribute('data-display-key')));
    const search = page.getByRole('searchbox'); await search.fill('order-match');
    expect(await owners()).toEqual(['trace:702', 'trace:706']);
    await search.fill('');
    expect(await turnRows.evaluateAll(rows => rows.map(row => row.getAttribute('data-display-key')))).toEqual(collapsedKeys);
    await toggle.click();
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    expect(await turnRows.evaluateAll(rows => rows.map(row => row.getAttribute('data-display-key')))).toEqual(keys);
    // Collapse again and prepend while the exact Turn action owns the top anchor.
    await toggle.click(); await turn.click();
    await turn.evaluate(el => { const pane = el.closest('[data-trajectory-scroll]')!; pane.scrollTop += el.getBoundingClientRect().top - pane.getBoundingClientRect().top; pane.dispatchEvent(new Event('scroll')); });
    const key = await turn.getAttribute('data-display-key'); const before = (await turn.boundingBox())!.y;
    await page.getByRole('button', { name: locale === 'en' ? 'Load earlier records into the overview' : '将更早记录加载到概览' }).evaluate((el: HTMLButtonElement) => el.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(turn).toBeFocused(); await expect(turn).toHaveAttribute('data-display-key', key!);
    await expect.poll(async () => Math.abs((await turn.boundingBox())!.y - before)).toBeLessThan(2);
    await expect(turnRows).toHaveCount(3); expect(await owners()).toEqual(['trace:702', 'trace:650', null]);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  });
}


test('424: JSON null Step records remain exactly owned through prepend, fold, search and keyboard inspection', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?step-less`);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  const rows = ledger.locator('[data-display-type="RecordRow"]');
  await expect(rows).toHaveCount(2);
  await page.getByRole('button', { name: 'Load earlier records into the overview' }).click();
  await expect(rows).toHaveCount(3);
  expect(await rows.evaluateAll(elements => elements.map(el => el.getAttribute('data-owner')))).toEqual(['trace:910', 'trace:911', 'trace:912']);
  await page.getByRole('button', { name: 'Fold Turn 1' }).click();
  await expect(rows).toHaveCount(1);
  await expect(ledger.locator('[data-structural="step"]')).toHaveCount(0);
  const search = page.getByRole('searchbox', { name: 'Search loaded Trace' });
  await search.fill('adopted second');
  await expect(rows).toHaveCount(1);
  await search.fill('');
  await expect(rows).toHaveCount(1);
  const turn = page.getByRole('button', { name: 'Turn 1', exact: true });
  await turn.click();
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
  await turn.focus(); await page.keyboard.press('ArrowDown');
  await expect(rows).toBeFocused();
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
  await page.keyboard.press('Enter');
  await page.getByRole('tab', { name: 'Native', exact: true }).click();
  const inspector = page.getByRole('tabpanel');
  await expect(inspector).toContainText('trace:910');
  await expect(inspector).toContainText('adopted-attempt');
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
});

for (const width of [1440, 390]) test(`retained input ownership repair orders the initial prompt and both turns at ${width}`, async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?retained-inputs`);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await page.getByRole('button', { name: 'Refresh native ownership' }).click();
  const rows = ledger.locator('[role="row"][data-owner]');
  await expect(rows).toHaveCount(5);
  expect(await rows.evaluateAll(elements => elements.map(row => row.getAttribute('data-owner')))).toEqual(['trace:2', 'trace:0', 'trace:3', 'trace:4', 'trace:5']);
  await expect(rows.nth(0)).toContainText('Initial System Prompt');
  await expect(rows.nth(1).getByRole('button', { name: 'Turn 1', exact: true })).toBeVisible();
  await expect(rows.nth(3).getByRole('button', { name: 'Turn 2', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Fold Turn 1', exact: true }).click();
  await expect(ledger.locator('[data-owner="trace:0"]')).toContainText('First input');
  await expect(ledger.locator('[data-owner="trace:3"]')).toHaveCount(0);
  await expect(ledger.locator('[data-owner="trace:4"]')).toContainText('Second input');
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '0');
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
  expect(errors).toEqual([]);
});

test('system prompt input span and model request preserve distinct selections and historical text', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
  const system = page.locator('[data-record-id][data-kind="system"]').first();
  const model = page.locator('[data-record-id="trace:3"]');
  await system.click();
  const inspector = page.getByRole('complementary', { name: 'Trace record inspector' });
  await expect(system).toHaveAttribute('aria-pressed', 'true');
  await expect(model).toHaveAttribute('aria-pressed', 'false');
  await expect(inspector.getByRole('tab', { name: 'System Prompt', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(inspector.getByRole('tabpanel')).toContainText('You are the historical agent.');
  await model.click();
  await expect(inspector.getByRole('tab', { name: 'Summary', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(system).toHaveAttribute('aria-pressed', 'false');
});

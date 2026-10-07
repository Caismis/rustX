const fixtureOrigin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
import { test, expect, type Page } from '@playwright/test';
import { expectStableScreenshot } from './screenshot';

const timelineTrack = (page: Page) => page.getByLabel('Timeline overview; drag horizontally to focus events');
const loadEarlier = (page: Page) => page.getByRole('region', { name: /^(Trajectory timeline|轨迹时间线)$/ }).getByRole('button', { name: /^(Load earlier history|加载更早的历史)$/ });
const details = (page: Page) => page.getByRole('complementary', { name: /^(Event details|事件详情)$/ });
const turnLabel = (scope: ReturnType<Page['locator']>, label: string) => scope.locator(`[data-turn-start] span[aria-label="${label}"]`);

for (const width of [1440, 390]) {
  for (const theme of ['light', 'dark']) {
    test(`T1-13/15 semantic ledger, details and keyboard ${width} ${theme}`, async ({ page }) => {
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
      // As in Harness: no rustX-only fold carets, call toggles or state words in rows.
      await expect(ledger.locator('[data-structural]')).toHaveCount(0);
      await expect(ledger.getByRole('button', { name: /Collapse Calls|Expand Calls/ })).toHaveCount(0);
      await expect(page.getByRole('button', { name: 'Jump to latest' })).toHaveCount(0);
      await expectStableScreenshot(page, `trajectory-ledger-${width}-${theme}.png`);
      const system = ledger.locator('[data-display-type="SystemPromptCell"][data-owner="trace:3"]');
      await system.focus(); await page.keyboard.press('Enter');
      const inspector = details(page);
      await expect(inspector.getByRole('tab', { name: 'System Prompt', exact: true })).toHaveAttribute('aria-selected', 'true');
      await expect(inspector.getByText('You are the historical agent.')).toBeVisible();
      await expectStableScreenshot(page, `trajectory-inspector-${width}-${theme}.png`);
      // Below 760px the details overlay the ledger, as in Harness: close them first.
      await inspector.getByRole('button', { name: 'Close details' }).click();
      const context = ledger.locator('[data-display-type="ContextRow"]').last(); await context.click();
      await expect(inspector.getByRole('tab')).toHaveText(['Summary', 'Preview', 'Raw', 'Source']);
      await expect(inspector.getByRole('tab', { name: 'Summary', exact: true })).toHaveAttribute('aria-selected', 'true');
      await expect(inspector.getByRole('tabpanel')).toContainText('Review the implementation');
      await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
      await inspector.getByRole('button', { name: 'Close details' }).click(); await expect(context).toBeFocused();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
    });
  }
}

test('T1-13 details resize handle drags, steps by key, resets on double-click and narrows the ledger', async ({ page }) => {
  await page.setViewportSize({ width: 1050, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
  await page.locator('[data-request-owner="trace:9"]').click();
  const panel = details(page);
  const handle = page.getByRole('separator', { name: 'Resize event details' });
  const initial = (await panel.boundingBox())!.width;
  expect(initial).toBeGreaterThanOrEqual(320); expect(initial).toBeLessThanOrEqual(440);
  const box = (await handle.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + 50); await page.mouse.down(); await page.mouse.move(box.x - 180, box.y + 50); await page.mouse.up();
  await expect.poll(async () => (await panel.boundingBox())!.width).toBeGreaterThan(initial + 100);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await expect(ledger.locator('[role="row"]').first()).toHaveCSS('grid-template-columns', /50px/);
  await expectStableScreenshot(page, 'trajectory-resized-narrow-ledger.png');
  const dragged = (await panel.boundingBox())!.width;
  await handle.focus(); await page.keyboard.press('ArrowRight');
  await expect.poll(async () => Math.round((await panel.boundingBox())!.width)).toBe(Math.round(dragged - 16));
  await handle.dblclick(); await expect.poll(async () => Math.abs((await panel.boundingBox())!.width - initial)).toBeLessThan(2);
  await expectStableScreenshot(page, 'trajectory-resize-reset.png');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('T1-08/09/15 folded calls, independent background, search and truncated failed Request Diff', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await page.getByRole('toolbar', { name: 'Trajectory toolbar' }).getByRole('button', { name: 'Collapse calls' }).click();
  await expect(ledger.locator('[data-display-type="CallsSummary"]')).toHaveText('…1 tool call · bash');
  await expect(ledger.locator('[data-display-type="RecordRow"][data-owner="trace:5"]')).toHaveCount(0);
  await expect(ledger.locator('[data-owner="trace:6"]')).toBeVisible();
  await expectStableScreenshot(page, 'trajectory-calls.png');
  await page.getByRole('searchbox', { name: 'Search trajectory' }).fill('Missing field');
  await expect(ledger.locator('[data-display-type="RecordRow"][data-owner="trace:5"]')).toBeVisible();
  await expectStableScreenshot(page, 'trajectory-search.png');
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '0');
  await page.getByRole('searchbox', { name: 'Search trajectory' }).fill('');
  await ledger.locator('[data-request-owner="trace:11"]').click();
  await expect(details(page)).toContainText('Failed');
  await ledger.locator('[data-display-type="SystemPromptCell"][data-owner="trace:11"]').click();
  await page.getByRole('tab', { name: 'Diff', exact: true }).click();
  await expect(page.getByRole('tabpanel')).toContainText('current prompt truncated; previous prompt truncated');
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
    // The overview history boundary loads without moving the reading position.
    await loadEarlier(page).click();
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
    // As in Harness, returning to the tail by scrolling resumes following it.
    // A virtual ledger settles its total height after the scroll; scroll again until it rests at the tail.
    await expect.poll(() => ledger.evaluate(el => { el.scrollTop = el.scrollHeight; el.dispatchEvent(new Event('scroll')); return el.scrollHeight - el.clientHeight - el.scrollTop; })).toBeLessThan(2);
    await page.getByRole('button', { name: 'Append', exact: true }).click();
    await expect.poll(() => ledger.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThan(2);
  });
}

for (const kind of ['request', 'tool']) {
  test(`T1-04/06/10 ${kind} anchor keeps its Turn label in place through threshold prepend`, async ({ page }) => {
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?threshold&structure${kind === 'tool' ? '&tool' : ''}`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const label = ledger.locator('[data-attempt="attempt-a"][data-turn-start]');
    await expect(label).toHaveCount(1);
    // The Turn label is not a control and owns no inspection.
    await expect(label.locator('[data-turn-start] button[aria-label^="Turn"]')).toHaveCount(0);
    await label.evaluate(el => { const pane = el.closest('[data-trajectory-scroll]')!; pane.scrollTop += el.getBoundingClientRect().top - pane.getBoundingClientRect().top; pane.dispatchEvent(new Event('scroll')); });
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    await loadEarlier(page).evaluate((el: HTMLButtonElement) => el.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(details(page)).toHaveCount(0);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    // Child inspection remains an explicit, separate user action.
    const child = ledger.locator(kind === 'tool' ? '[data-display-type="RecordRow"][data-owner="trace:100"]' : '[data-request-owner="trace:100"]');
    await ledger.evaluate(el => { el.scrollTop = 1700; el.dispatchEvent(new Event('scroll')); });
    await child.click();
    await expect(details(page)).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
  });
}

test('T1-04 folded calls are a toggle row that expands without selecting or reading detail', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  const assistant = ledger.locator('[data-display-type="RecordRow"][data-owner="trace:4"]');
  // As in Harness, double-clicking an Assistant folds its calls.
  await assistant.dblclick();
  const summary = ledger.locator('[data-display-type="CallsSummary"]');
  await expect(summary).toHaveText('…1 tool call · bash');
  await expect(ledger.locator('[data-display-type="RecordRow"][data-owner="trace:5"]')).toHaveCount(0);
  await summary.focus(); await page.keyboard.press('Enter');
  await expect(summary).toHaveCount(0);
  await expect(ledger.locator('[data-display-type="RecordRow"][data-owner="trace:5"]')).toBeVisible();
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '0');
});

for (const width of [1440, 390]) {
  test(`407: native identity survives renumbering, search and timeline navigation at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?renumber`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    const selected = ledger.locator('[data-display-type="SystemPromptCell"][data-owner="trace:3"]');
    // As in Harness, double-clicking a Turn's opening row folds it.
    await ledger.locator('[data-attempt="attempt-a"][data-turn-start]').dispatchEvent('dblclick');
    await expect(ledger.locator('[data-display-type="TurnSummary"][data-attempt="attempt-a"]')).toBeVisible();
    await selected.click();
    const inspector = details(page);
    await expect(inspector.getByText('You are the historical agent.')).toBeVisible();
    await expect(selected).toBeVisible();
    await expect(inspector).toBeVisible();
    await loadEarlier(page).click();
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(turnLabel(ledger, 'Turn 2')).toBeVisible();
    await expect(ledger.locator('[data-display-type="TurnSummary"][data-attempt="attempt-a"]')).toBeVisible();
    await expect(selected).toBeVisible();
    const search = page.getByRole('searchbox', { name: 'Search trajectory' });
    await search.fill('request-3');
    await expect(selected).toHaveAttribute('aria-selected', 'true');
    await search.fill('request-50');
    await expect(selected).toHaveCount(0);
    await search.fill('');
    await expect(selected).toBeVisible();
    await expect(inspector.getByText('You are the historical agent.')).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    await page.locator('[data-record-id="trace:3"]').click();
    const request = ledger.locator('[data-request-owner="trace:3"]');
    await expect(request).toHaveAttribute('aria-pressed', 'true');
    // Opening a record from the overview unfolds its Turn.
    await expect(ledger.locator('[data-display-type="TurnSummary"][data-attempt="attempt-a"]')).toHaveCount(0);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

test('421: virtual semantic ledger and drag focus retain native ownership through prepend', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?long&renumber`);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await ledger.evaluate(el => { el.scrollTop = 1000; el.dispatchEvent(new Event('scroll')); });
  const box = (await timelineTrack(page).boundingBox())!;
  await page.mouse.move(box.x + box.width * .35, box.y + 25);
  await page.mouse.down(); await page.mouse.move(box.x + box.width * .45, box.y + 25); await page.mouse.up();
  await ledger.evaluate(el => { el.scrollTop = el.scrollHeight * .4; el.dispatchEvent(new Event('scroll')); });
  await expect(ledger.locator('[data-owner="trace:300"][data-timeline-focus="inside"]').first()).toBeAttached();
  const before = await ledger.locator('[data-timeline-focus="inside"]').evaluateAll(rows => rows.map(row => row.getAttribute('data-owner')));
  expect(before.length).toBeGreaterThan(0);
  await loadEarlier(page).click();
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
  for (const owner of before) await expect(ledger.locator(`[data-owner="${owner}"][data-timeline-focus="inside"]`).first()).toBeAttached();
  await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
  await expect(turnLabel(ledger, 'Turn 2')).toBeVisible();
  await expect(page.locator('[data-trace-epoch]')).toHaveAttribute('data-trace-epoch', '1');
  await expect(page.locator('[data-focus-range]')).toHaveCount(1);
});

for (const kind of ['request', 'tool']) {
  test(`407: selected ${kind} cell and detail retain focus across virtual threshold`, async ({ page }) => {
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?threshold${kind === 'tool' ? '&tool' : ''}`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const row = ledger.locator(kind === 'tool' ? '[data-display-type="RecordRow"][data-owner="trace:100"]' : '[data-request-owner="trace:100"]');
    await row.focus(); await page.keyboard.press('Enter');
    await expect(details(page)).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    await loadEarlier(page).evaluate((button: HTMLButtonElement) => button.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(row).toHaveAttribute(kind === 'tool' ? 'aria-selected' : 'aria-pressed', 'true');
    await expect(row).toBeFocused();
    await expect(details(page)).toBeVisible();
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
  });
}

for (const width of [1440, 390]) {
  test(`structural search shares native Timeline membership at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?structural-search`);
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    const keys = await ledger.locator('[data-display-key]').evaluateAll(rows => rows.map(row => row.getAttribute('data-display-key')));
    const search = page.getByRole('searchbox', { name: 'Search trajectory' });
    for (const [query, attempt, expected] of [
      ['Step 2', 'attempt-a', ['trace:5', 'trace:8']],
      ['Turn 2', 'attempt-b', ['trace:7']],
      ['Message', 'attempt-a', ['trace:2']],
    ] as const) {
      await search.fill(query);
      // As in Harness, a Step or Message has no chrome; its members carry the Turn label.
      await expect(turnLabel(ledger, attempt === 'attempt-a' ? 'Turn 1' : 'Turn 2').locator('xpath=ancestor::*[@data-attempt][1]')).toHaveAttribute('data-attempt', attempt);
      await expect.poll(() => page.locator('[data-record-id][data-search-match="true"]').evaluateAll(spans => spans.map(span => span.getAttribute('data-record-id')))).toEqual([...expected]);
      await expect.poll(() => ledger.locator('[data-owner]').evaluateAll(rows => rows.map(row => row.getAttribute('data-owner')))).toEqual([...expected]);
      await expect(page.locator('[data-record-id="trace:3"]')).toHaveAttribute('data-search-match', 'false');
    }
    await search.fill('');
    expect(await ledger.locator('[data-display-key]').evaluateAll(rows => rows.map(row => row.getAttribute('data-display-key')))).toEqual(keys);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '0');
  });
}

for (const width of [1440, 390]) {
  test(`407: a real held drag cannot cross same-epoch prepend at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?renumber`);
    const track = timelineTrack(page);
    const box = (await track.boundingBox())!;
    const epoch = await page.locator('[data-trace-epoch]').getAttribute('data-trace-epoch');
    const end = await track.getAttribute('data-domain-end');
    await page.mouse.move(box.x + box.width * .30, box.y + 25);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * .45, box.y + 25);
    await expect(page.locator('[data-focus-range]')).toHaveCount(1);
    // Synchronous fixture action while the actual browser pointer is held.
    await loadEarlier(page).evaluate((button: HTMLButtonElement) => button.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(page.locator('[data-trace-epoch]')).toHaveAttribute('data-trace-epoch', epoch!);
    await expect(track).not.toHaveAttribute('data-domain-end', end!);
    await expect(page.locator('[data-focus-range]')).toHaveCount(0);
    await page.mouse.up();
    await expect(page.locator('[data-timeline-focus]')).toHaveCount(0);
    await expect(details(page)).toHaveCount(0);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    const current = (await track.boundingBox())!;
    await page.mouse.move(current.x + current.width * .30, current.y + 25);
    await page.mouse.down();
    await page.mouse.move(current.x + current.width * .45, current.y + 25);
    await page.mouse.up();
    await expect(page.locator('[data-focus-range]')).toHaveCount(1);
    await expect(page.locator('[data-timeline-focus="inside"]').first()).toBeAttached();
    await expect(details(page)).toHaveCount(0);
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
    if (dimension === 'prompt') await expect(page.getByRole('tabpanel')).toContainText('-Previous prompt.');
    else await expect(page.getByRole('tab', { name: 'Diff', exact: true })).toHaveCount(0);
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
    const opening = ledger.locator('[data-turn-start]').first();
    expect((await system.boundingBox())!.y).toBeLessThan((await opening.boundingBox())!.y);
    const assertContained = async () => {
      // As in Harness, a Request dot sits on its row's top boundary or inside
      // its own marker seat; every other control stays inside its row.
      const contained = await ledger.locator('[role="row"]').evaluateAll(rows => rows.every(row => {
        const box = row.getBoundingClientRect();
        return [...row.querySelectorAll('button')].every(button => { const control = button.getBoundingClientRect(); const center = (control.top + control.bottom) / 2;
          return button.hasAttribute('data-request-owner') ? center >= box.top - .5 && center <= box.bottom : control.top >= box.top && control.bottom <= box.bottom; });
      }));
      expect(contained).toBe(true);
    };
    await assertContained();
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
    // Native failure wins over the Model lane's normal violet color.
    const errorColor = await page.locator('[data-record-id="trace:105"]').evaluate(el => getComputedStyle(el).backgroundColor);
    await expect(page.locator('[data-record-id="trace:107"]')).toHaveCSS('background-color', errorColor);
    await expectStableScreenshot(page, `ledger-421-${width}-${locale}.png`);
    await opening.dispatchEvent('dblclick');
    await expect(system).toBeVisible();
    await expect(ledger.locator('[data-owner="trace:101"]')).toBeVisible();
    const summary = ledger.locator('[data-display-type="TurnSummary"]');
    await expect(summary).toHaveCount(1);
    await expect(summary).toHaveText(locale === 'zh' ? '…2 个步骤 · 2 个工具调用' : '…2 steps · 2 tool calls');
    await assertContained();
    await expectStableScreenshot(page, `ledger-421-fold-${width}-${locale}.png`);
    const search = page.getByRole('searchbox');
    await search.fill('Step 2'); await expect(marker).toBeVisible(); await assertContained();
    await search.fill('historical agent'); await assertContained();
    await search.fill(''); await expect(summary).toHaveCount(1);
    await summary.click(); await marker.click();
    await expect(marker).toHaveAttribute('aria-pressed', 'true');
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '1');
    await expectStableScreenshot(page, `ledger-421-inspector-${width}-${locale}.png`);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  });
}

for (const virtual of [false, true]) {
  test(`424: exact arrows, seat geometry and prepend ${virtual ? 'virtual' : 'plain'}`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 600 });
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?chrome${virtual ? '&long' : ''}`);
    const ledger = page.locator('[data-trajectory-scroll]');
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const system = ledger.locator('[data-display-type="SystemPromptCell"][data-owner="trace:3"]');
    const opening = ledger.locator('[data-attempt="attempt-a"][data-turn-start]');
    const initialRequest = ledger.locator('[data-request-owner="trace:3"]');
    const reads = page.locator('[data-detail-reads]');
    await expect(system).toBeVisible();
    expect((await system.boundingBox())!.y).toBeLessThan((await opening.boundingBox())!.y);
    await expect(system.locator('[data-request-owner]')).toHaveCount(0);
    await expect(initialRequest).toHaveAttribute('data-request-id', 'request-3');
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
    // As in Harness, a Turn is not a navigation target: arrows go to the Request.
    await system.focus(); await page.keyboard.press('ArrowDown'); await expect(initialRequest).toBeFocused();
    await expect(reads).toHaveAttribute('data-detail-reads', '1');
    await page.keyboard.press('ArrowUp'); await expect(system).toBeFocused();
    await initialRequest.focus();
    await loadEarlier(page).evaluate((el: HTMLButtonElement) => el.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(initialRequest).toBeFocused();
    await page.getByRole('button', { name: 'Close details' }).click();
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
    const opening = ledger.locator('[data-attempt="ordered-turn"][data-turn-start]');
    const system = ledger.locator('[data-display-type="SystemPromptCell"][data-owner="trace:702"]');
    const request = ledger.locator('[data-request-owner="trace:702"]');
    const later = ledger.locator('[data-display-type="RecordRow"][data-owner="trace:706"]');
    const reads = page.locator('[data-detail-reads]');
    await expect(opening).toBeVisible();
    expect((await system.boundingBox())!.y).toBeLessThan((await opening.boundingBox())!.y);
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
    await system.focus();
    await page.keyboard.press('ArrowDown'); await expect(request).toBeFocused();
    await page.keyboard.press('ArrowDown'); await expect(later).toBeFocused();
    await expect(reads).toHaveAttribute('data-detail-reads', '2');
    await expectStableScreenshot(page, `ledger-order-inspector-${width}-${locale}.png`);
    await details(page).getByRole('button', { name: locale === 'en' ? 'Close details' : '关闭详情' }).click();
    await opening.evaluate(el => { const pane = el.closest('[data-trajectory-scroll]')!; pane.scrollTop += el.getBoundingClientRect().top - pane.getBoundingClientRect().top; pane.dispatchEvent(new Event('scroll')); });
    // Invoke the real controlled page callback without transferring DOM focus.
    await loadEarlier(page).evaluate((el: HTMLButtonElement) => el.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    // The prepended record now opens the Turn, below the promoted initial prompt.
    await expect(ledger.locator('[data-attempt="ordered-turn"][data-turn-start]')).toHaveAttribute('data-owner', 'trace:650');
    expect((await system.boundingBox())!.y).toBeLessThan((await ledger.locator('[data-attempt="ordered-turn"][data-turn-start]').boundingBox())!.y);
    await expect(reads).toHaveAttribute('data-detail-reads', '2');
  });
}

for (const width of [1440, 390]) for (const locale of ['en', 'zh'] as const) {
  test(`424 compact: fifty Steps cost no rows, then fold once prepend adds content ${width} ${locale}`, async ({ page }) => {
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize({ width, height: 1500 });
    await page.addInitScript(locale => localStorage.setItem('rustx-locale-v1', locale), locale);
    await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?compact`);
    await expect(page).toHaveTitle('Trajectory presentation contracts');
    const ledger = page.locator('[data-trajectory-scroll]');
    await ledger.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    const turnRows = ledger.locator('[role="row"][data-attempt="ordered-turn"]');
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
    // One content row has nothing to fold, exactly as in Harness.
    await ledger.locator('[data-attempt="ordered-turn"][data-turn-start]').dispatchEvent('dblclick');
    await expect(ledger.locator('[data-display-type="TurnSummary"]')).toHaveCount(0);
    await loadEarlier(page).evaluate((el: HTMLButtonElement) => el.click());
    await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
    await expect(turnRows).toHaveCount(4); expect(await owners()).toEqual(['trace:702', 'trace:650', 'trace:702', 'trace:706']);
    const opening = ledger.locator('[data-attempt="ordered-turn"][data-turn-start]');
    await opening.dispatchEvent('dblclick');
    await expect(ledger.locator('[data-attempt="ordered-turn"][data-display-type="TurnSummary"]')).toHaveText(locale === 'zh' ? '…50 个步骤 · 0 个工具调用' : '…50 steps · 0 tool calls');
    expect(await owners()).toEqual(['trace:702', 'trace:650', null]);
    // As in Harness, a folded Turn shows no Request markers.
    await expect(turnRows.locator('[data-request-owner]')).toHaveCount(0);
    await expectStableScreenshot(page, `ledger-compact-fold-${width}-${locale}.png`);
    // Search exposes folded content without changing saved folds.
    const collapsedKeys = await turnRows.evaluateAll(rows => rows.map(row => row.getAttribute('data-display-key')));
    const search = page.getByRole('searchbox'); await search.fill('order-match');
    expect(await owners()).toEqual(['trace:702', 'trace:706']);
    await search.fill('');
    expect(await turnRows.evaluateAll(rows => rows.map(row => row.getAttribute('data-display-key')))).toEqual(collapsedKeys);
    await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  });
}

test('424: JSON null Step records remain exactly owned through prepend, fold, search and keyboard inspection', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?step-less`);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  const rows = ledger.locator('[data-display-type="RecordRow"]');
  await expect(rows).toHaveCount(2);
  await loadEarlier(page).click();
  await expect(rows).toHaveCount(3);
  expect(await rows.evaluateAll(elements => elements.map(el => el.getAttribute('data-owner')))).toEqual(['trace:910', 'trace:911', 'trace:912']);
  await rows.first().dispatchEvent('dblclick');
  await expect(rows).toHaveCount(1);
  const search = page.getByRole('searchbox', { name: 'Search trajectory' });
  await search.fill('adopted second');
  await expect(rows).toHaveCount(1);
  await search.fill('');
  await expect(rows).toHaveCount(1);
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
  await rows.focus(); await page.keyboard.press('Enter');
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '1');
  await expect(rows).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('tab')).toHaveText(['Summary', 'Preview', 'Raw', 'Source']);
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
  await expect(rows.nth(1).locator('span[aria-label="Turn 1"]')).toBeVisible();
  await expect(rows.nth(3).locator('span[aria-label="Turn 2"]')).toBeVisible();
  await rows.nth(1).dispatchEvent('dblclick');
  await expect(ledger.locator('[data-owner="trace:0"]')).toContainText('First input');
  await expect(ledger.locator('[data-owner="trace:3"]')).toHaveCount(0);
  await expect(ledger.locator('[data-owner="trace:4"]')).toContainText('Second input');
  await expect(page.locator('[data-history-reads]')).toHaveAttribute('data-history-reads', '0');
  await expect(page.locator('[data-detail-reads]')).toHaveAttribute('data-detail-reads', '0');
  expect(errors).toEqual([]);
});

test('system prompt input block and model request preserve distinct selections and historical text', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
  const system = page.locator('[data-timeline-span="system"]').first();
  const model = page.locator('[data-record-id="trace:3"]');
  await system.click();
  const inspector = details(page);
  await expect(system).toHaveAttribute('data-current', 'true');
  await expect(model).not.toHaveAttribute('data-current');
  await expect(inspector.getByRole('tab', { name: 'System Prompt', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(inspector.getByRole('tabpanel')).toContainText('You are the historical agent.');
  await model.click();
  await expect(inspector.getByRole('tab', { name: 'Summary', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(system).not.toHaveAttribute('data-current');
});

for (const width of [1440, 390]) test(`overview tooltip names the block and zoom never covers the lane labels at ${width}`, async ({ page }) => {
  await page.setViewportSize({ width, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html?ledger`);
  const track = timelineTrack(page);
  // As in Harness, a block names its role, recorded range and total after a delay.
  await page.locator('[data-record-id="trace:104"]').hover();
  const tooltip = page.getByRole('region', { name: 'Trajectory timeline' }).getByRole('tooltip');
  await expect(tooltip).toBeVisible();
  await expect(tooltip).toContainText('TOOL');
  await expect(tooltip).toContainText('→');
  await expect(tooltip).toContainText('Total');
  const box = (await track.boundingBox())!;
  await page.mouse.move(box.x + box.width * .2, box.y + 25);
  for (let step = 0; step < 6; step++) await page.mouse.wheel(0, -400);
  await expect.poll(async () => Number(await track.getAttribute('data-domain-end')) - Number(await track.getAttribute('data-domain-start'))).toBeLessThan(10);
  // Pan the zoomed domain both ways: blocks are clipped at the track edge.
  await page.mouse.move(box.x + box.width * .5, box.y + 25); await page.mouse.down({ button: 'right' });
  await page.mouse.move(box.x + box.width * .9, box.y + 25, { steps: 6 }); await page.mouse.up({ button: 'right' });
  await page.mouse.move(box.x + box.width * .9, box.y + 25); await page.mouse.down({ button: 'right' });
  await page.mouse.move(box.x + box.width * .1, box.y + 25, { steps: 6 }); await page.mouse.up({ button: 'right' });
  const covered = await page.evaluate(() => {
    const labels = [...document.querySelectorAll<HTMLElement>('[class*="labels"] span')];
    return labels.map(label => { const r = label.getBoundingClientRect(); return document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) === label; });
  });
  expect(covered).toEqual([true, true, true]);
  const outside = await page.evaluate(() => {
    const track = document.querySelector<HTMLElement>('[aria-label="Timeline overview; drag horizontally to focus events"]')!.getBoundingClientRect();
    return [...document.querySelectorAll<HTMLElement>('[data-timeline-span]')].filter(span => {
      const r = span.getBoundingClientRect();
      const point = document.elementFromPoint(Math.max(r.left, track.left - 4), r.top + r.height / 2);
      return point === span && r.left < track.left;
    }).length;
  });
  expect(outside).toBe(0);
  await expectStableScreenshot(page, `trajectory-timeline-zoomed-${width}.png`);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

for (const width of [1440, 390]) test(`Harness details isolate context and show message Markdown source at ${width}`, async ({ page }) => {
  await page.setViewportSize({ width, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/trajectory.html`);
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await ledger.locator('[data-display-type="ContextRow"]').last().click();
  const inspector = details(page);
  await expect(inspector.getByRole('tab')).toHaveText(['Summary', 'Preview', 'Raw', 'Source']);
  await expect(inspector.getByRole('tabpanel')).toContainText('Full content: Review the implementation');
  await expect(inspector.getByRole('tabpanel')).not.toContainText('Full content: Workspace');
  await inspector.getByRole('tab', { name: 'Raw', exact: true }).click();
  await expect(inspector.getByRole('tabpanel')).toContainText('Full content: Review the implementation');
  await expect(inspector.getByRole('tabpanel')).not.toContainText('message_id');
  await inspector.getByRole('button', { name: 'Close details' }).click();
  await ledger.locator('[data-display-type="RecordRow"][data-owner="trace:4"]').click();
  await inspector.getByRole('tab', { name: 'Preview', exact: true }).click();
  await expect(inspector.getByRole('tabpanel').locator('strong')).toHaveText('working tree');
  await inspector.getByRole('tab', { name: 'Raw', exact: true }).click();
  await expect(inspector.getByRole('tabpanel')).toContainText('**working tree**');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
});

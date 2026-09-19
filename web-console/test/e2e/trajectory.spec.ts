import { test, expect } from '@playwright/test';

for (const width of [1440, 390]) {
  test(`Harness-first Trajectory, native inspector and keyboard at ${width}px`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize({ width, height: 900 });
    await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html');
    await expect(page).toHaveTitle('Trajectory presentation contracts');
    const ledger = page.getByRole('table', { name: 'Trace ledger' });
    await expect(ledger.getByRole('row')).toHaveCount(15);
    // An ordinary row names its artifacts: the native name, a compact count
    // for the rest, and an image marked as an image rather than a file.
    const user = ledger.locator('[data-trace-id="trace:0"]');
    await expect(user.getByText('brief.md', { exact: true })).toBeVisible();
    await expect(user.getByText('+1', { exact: true })).toBeVisible();
    await expect(user.locator('[data-image]')).toHaveCount(0);
    await expect(
      ledger.locator('[data-trace-id="trace:10"] [data-image]'),
    ).toHaveCount(1);
    await expect(ledger.getByText('diagram.png', { exact: true })).toBeVisible();
    // A request is named by model and native identity, never "Request #N".
    await expect(ledger.getByText('Request #')).toHaveCount(0);
    // rustX-specific domain kinds stay distinguishable with no hover and no
    // help from preview text, which deliberately never names its own kind.
    for (const [id, full, short] of [
      ['trace:6', 'Background', 'BG'],
      ['trace:12', 'Subagent', 'SUBAGENT'],
      ['trace:13', 'Workflow', 'WORKFLOW'],
      ['trace:14', 'Interaction', 'INTERACT'],
    ] as const) {
      const row = ledger.locator(`[data-trace-id="${id}"]`);
      await expect(row.getByText(width === 390 ? short : full, { exact: true })).toBeVisible();
    }
    await expect(page).toHaveScreenshot(`trajectory-${width}.png`);
    const tool = ledger.locator('[data-trace-id="trace:5"]');
    await tool.focus();
    await page.keyboard.press('Enter');
    await expect(tool).toHaveAttribute('aria-selected', 'true');
    const inspector = page.getByRole('complementary', { name: 'Trace record inspector' });
    await expect(inspector.getByText('Review complete', { exact: true })).toBeVisible();
    await expect(page).toHaveScreenshot(`trajectory-summary-${width}.png`);
    await inspector.getByRole('tab', { name: 'Code', exact: true }).click();
    await expect(inspector.getByRole('button', { name: 'Copy source' })).toBeVisible();
    await expect(inspector.getByText('git diff --stat', { exact: true })).toBeVisible();
    await expect(page).toHaveScreenshot(`trajectory-code-${width}.png`);
    await inspector.getByRole('tab', { name: 'Input', exact: true }).click();
    await expect(inspector.getByRole('tree')).toContainText('/workspace/rustX');
    await inspector.getByRole('tab', { name: 'Result', exact: true }).click();
    await expect(inspector.getByRole('tree')).toContainText('insertions');
    await expect(inspector.getByText('Review complete', { exact: true })).toBeVisible();
    await expect(page).toHaveScreenshot(`trajectory-result-${width}.png`);
    await inspector.getByRole('tab', { name: 'Artifacts', exact: true }).click();
    await expect(inspector.getByText('review.md', { exact: true })).toBeVisible();
    await inspector.getByRole('button', { name: 'Close record' }).click();
    await page.getByRole('button', { name: 'Fold Attempts', exact: true }).click();
    await expect(ledger.getByText('git diff --stat', { exact: true })).toHaveCount(0);
    await expect(page).toHaveScreenshot(`trajectory-folded-${width}.png`);
    await page.getByRole('textbox', { name: 'Search loaded Trace' }).fill('git diff');
    await expect(tool).toBeVisible();
    await page.getByRole('textbox', { name: 'Search loaded Trace' }).fill('');
    // Exact native selection from the Overview also reveals the folded group.
    await page.getByRole('button', { name: 'Inspect Tool · bash · call-5', exact: true }).click();
    await expect(tool).toHaveAttribute('aria-selected', 'true');
    await inspector.getByRole('button', { name: 'Close record' }).click();
    await ledger.locator('[data-trace-id="trace:3"]').click();
    await inspector.getByRole('tab', { name: 'Prompt' }).click();
    await expect(inspector.getByText('You are the historical agent.')).toBeVisible();
    await inspector.getByRole('tab', { name: 'Prompt' }).focus();
    await page.keyboard.press('ArrowRight');
    await expect(inspector.getByRole('tab', { name: 'Context' })).toBeFocused();
    await expect(inspector.getByText('Inspect the trajectory.')).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

test('timeline focus, wheel zoom, right-drag pan and keyboard reset preserve selection', async ({ page }) => {
  await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html');
  const canvas = page.getByLabel('Timeline navigation: arrow keys pan, Escape clears focus');
  const box = (await canvas.boundingBox())!;
  await page.mouse.move(box.x + box.width * .3, box.y + 5);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * .6, box.y + 5);
  await page.mouse.up();
  await expect(page.locator('[data-timeline-focus="outside"]').first()).toBeVisible();
  await page.mouse.wheel(0, -100);
  await expect(canvas).not.toHaveAttribute('data-domain-end', '15');
  const before = Number(await canvas.getAttribute('data-domain-start'));
  await page.mouse.move(box.x + box.width * .5, box.y + 5);
  await page.mouse.down({ button: 'right' });
  await page.mouse.move(box.x + box.width * .4, box.y + 5);
  await page.mouse.up({ button: 'right' });
  await expect.poll(async () => Number(await canvas.getAttribute('data-domain-start'))).toBeGreaterThan(before);
  await page.mouse.click(box.x + box.width * .4, box.y + 5, { button: 'right' });
  await expect(page.locator('[data-timeline-focus]')).toHaveCount(0);
  await page.getByRole('button', { name: 'Reset timeline' }).click();
  await expect(canvas).toHaveAttribute('data-domain-start', '0');
  await expect(canvas).toHaveAttribute('data-domain-end', '15');
});

test('virtual history preserves prepend anchor and follows append only at tail', async ({ page }) => {
  await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html?long');
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await expect(ledger).toHaveAttribute('aria-rowcount', '160');
  await expect(ledger.locator('[data-trace-id="trace:259"]')).toBeVisible();
  expect(await ledger.getByRole('row').count()).toBeLessThan(60);
  await ledger.evaluate(el => { el.scrollTop = 700; });
  await expect.poll(async () => ledger.evaluate(el => el.scrollTop)).toBe(700);
  const anchor = ledger.locator('[data-trace-id="trace:125"]');
  await expect(anchor).toBeVisible();
  const y = (await anchor.boundingBox())!.y;
  await page.getByRole('button', { name: 'Load older', exact: true }).click();
  await expect(ledger).toHaveAttribute('aria-rowcount', '192');
  await expect.poll(async () => (await anchor.boundingBox())!.y).toBe(y);
  const top = await ledger.evaluate(el => el.scrollTop);
  await page.getByRole('button', { name: 'Append record' }).click();
  await expect(ledger).toHaveAttribute('aria-rowcount', '193');
  await expect.poll(async () => ledger.evaluate(el => el.scrollTop)).toBe(top);
  await ledger.evaluate(el => { el.scrollTop = el.scrollHeight; });
  await expect(ledger.locator('[data-trace-id="trace:260"]')).toBeVisible();
  await page.getByRole('button', { name: 'Append record' }).click();
  await expect(ledger.locator('[data-trace-id="trace:261"]')).toBeVisible();
  await expect.poll(async () => ledger.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop)).toBeLessThanOrEqual(2);
});

test('the overview offers earlier history, loads it once and keeps the reader anchored', async ({ page }) => {
  await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html?long');
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  const overview = page.getByLabel('Timing overview');
  const boundary = overview.getByLabel('Load earlier records into the overview');
  await expect(ledger).toHaveAttribute('aria-rowcount', '160');
  await expect(boundary).toBeVisible();
  await expect(boundary).toHaveAttribute('aria-disabled', 'false');

  await ledger.evaluate(el => { el.scrollTop = 700; });
  await expect.poll(async () => ledger.evaluate(el => el.scrollTop)).toBe(700);
  const anchor = ledger.locator('[data-trace-id="trace:125"]');
  await expect(anchor).toBeVisible();
  const y = (await anchor.boundingBox())!.y;

  // Keyboard reaches the same single paging operation the ledger uses.
  await boundary.focus();
  await expect(boundary).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(ledger).toHaveAttribute('aria-rowcount', '192');
  // A prepend triggered from the overview preserves the reader's anchor.
  await expect.poll(async () => (await anchor.boundingBox())!.y).toBe(y);
  // The fixture's older page is the last one, so the affordance retires.
  await expect(boundary).toHaveCount(0);
});

test('selecting a dimmed overview record clears a search that hides it', async ({ page }) => {
  await page.goto('http://127.0.0.1:5174/test/fixtures/trajectory.html');
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  const search = page.getByRole('textbox', { name: 'Search loaded Trace' });
  const hidden = ledger.locator('[data-trace-id="trace:5"]');

  await search.fill('cargo check');
  await expect(ledger.getByText('cargo check', { exact: true })).toBeVisible();
  await expect(hidden).toHaveCount(0);
  // The record is still in the overview, dimmed as a non-match.
  const span = page.getByRole('button', { name: 'Inspect Tool · bash · call-5', exact: true });
  await expect(span).toHaveAttribute('data-dimmed', '');

  await span.click();
  await expect(search).toHaveValue('');
  await expect(hidden).toBeVisible();
  await expect(hidden).toHaveAttribute('aria-selected', 'true');
  const inspector = page.getByRole('complementary', { name: 'Trace record inspector' });
  await expect(inspector.getByText('Tool · bash', { exact: true })).toBeVisible();
  await expect(span).toHaveAttribute('aria-pressed', 'true');
});

import { expect, test } from '@playwright/test';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

for (const width of [1440, 390]) {
  test(`workflow run canvas renders immutable graph, preserves viewport and focus, supports keyboard, EN/ZH, and themes at ${width}px`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', msg => {
      if (['error', 'warning'].includes(msg.type())) errors.push(msg.text());
    });

    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${origin}/test/fixtures/workflow-run-canvas.html`);
    await expect(page).toHaveTitle('Workflow run canvas fixture');

    // Keyboard activation from WorkflowRunPanel opens the modal dialog cleanly and Escape restores focus with visible indicator.
    const inspectButton = page.getByRole('button', { name: 'Inspect worker on run graph' }).last();
    await inspectButton.focus();
    await expect(inspectButton).toBeFocused();
    await page.keyboard.press('Enter');

    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await expect(inspectButton).toBeFocused();
    expect(await page.evaluate(() => {
      const el = document.activeElement;
      return !!el && (el.matches(':focus-visible') || getComputedStyle(el).outlineStyle !== 'none');
    })).toBe(true);

    // Reopen via Enter for full run canvas interaction checks.
    await page.keyboard.press('Enter');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole('navigation', { name: 'Block path' })).toContainText('outer');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);

    // Wait for ReactFlow canvas nodes and arbitrary executable Start node badge (`workflow_start`).
    await expect(dialog.getByTestId('rf__wrapper')).toBeVisible();
    expect(await page.evaluate(() => document.getElementById('root')?.inert)).toBe(true);
    const closeButton = dialog.getByRole('button', { name: 'Close run graph', exact: true });
    await closeButton.focus();
    await page.keyboard.press('Shift+Tab');
    expect(await page.evaluate(() => !!document.activeElement?.closest('[role="dialog"]'))).toBe(true);
    await page.keyboard.press('Tab');
    await expect(closeButton).toBeFocused();
    const focusableCount = await dialog.locator('button,input,select,textarea,a[href],[tabindex]').count();
    for (let index = 0; index < focusableCount + 2; index++) {
      await page.keyboard.press('Tab');
      expect(await page.evaluate(() => !!document.activeElement?.closest('[role="dialog"]'))).toBe(true);
    }
    const startBadge = dialog.locator('[data-run-node-status="completed"]').first();
    await expect(startBadge).toBeVisible();

    // No eager value reads merely from opening and rendering the graph.
    expect(await page.evaluate(() => window.runCanvasFixture.valueReads())).toEqual([]);

    // Output tab triggers lazy value read once for selected visit.
    const outputTab = dialog.getByRole('tab', { name: 'Output' });
    await outputTab.focus();
    await page.keyboard.press('Enter');
    await expect(dialog.getByRole('tabpanel')).toContainText('lazy-output-e2e');
    expect(await page.evaluate(() => window.runCanvasFixture.valueReads().length)).toBe(1);

    // Capture the viewport after React Flow's actual movement completion.
    await dialog.getByRole('button', { name: 'Zoom in', exact: true }).click();
    const zoomLevel = dialog.getByRole('button', { name: 'Zoom', exact: true });
    await expect(zoomLevel).not.toHaveText('100%');
    const viewport = dialog.locator('.react-flow__viewport');
    await expect(dialog.getByRole('region', { name: 'Workflow canvas', exact: true })).toHaveAttribute('aria-busy', 'false');
    const transformBefore = await viewport.getAttribute('style');
    expect(transformBefore).toBeTruthy();

    // Status update preserves viewport zoom/pan and keeps focus on Output tab!
    await outputTab.focus();
    await expect(outputTab).toBeFocused();
    await page.evaluate(() => window.runCanvasFixture.completeLiveRun());
    await expect(dialog.locator('[data-run-node-status="running"]')).toHaveCount(0);
    await expect(outputTab).toBeFocused();
    expect(await viewport.getAttribute('style')).toBe(transformBefore);

    // Live owner disconnect shows disconnected notice rather than leaving live-owner status positive.
    await page.evaluate(() => window.runCanvasFixture.disconnectLiveOwner());
    await expect(dialog.getByText('Live owner disconnected')).toBeVisible();

    // Switch to ZH locale and Dark theme, verify translated UI + distinct dark panel background + no overflow.
    const surface = dialog.locator('section').first();
    const lightBg = await surface.evaluate(el => getComputedStyle(el).backgroundColor);
    await page.evaluate(() => {
      window.runCanvasFixture.setLocale('zh');
      window.runCanvasFixture.setTheme('dark');
    });
    await expect(dialog.getByRole('tab', { name: '输出' })).toBeVisible();
    const darkBg = await surface.evaluate(el => getComputedStyle(el).backgroundColor);
    expect(darkBg).not.toBe(lightBg);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);

    // Escape closes the dialog in ZH/dark mode.
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    expect(errors).toEqual([]);
    expect(await page.evaluate(() => document.getElementById('root')?.inert)).toBe(false);
  });
}

test('unavailable snapshot renders safe state while keeping authorized trace visit inspection', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width: 1280, height: 844 });
  await page.goto(`${origin}/test/fixtures/workflow-run-canvas.html?unavailable=1`);
  await page.getByRole('button', { name: 'Inspect worker on run graph' }).last().click();

  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('status')).toContainText('unavailable');

  // Visit selector and Output lazy read remain usable from authorized trace nodes.
  await dialog.getByRole('tab', { name: 'Output' }).click();
  await expect(dialog.getByRole('tabpanel')).toContainText('lazy-output-e2e');
  expect(errors).toEqual([]);
});

test('retiring the conversation closes its canvas without restoring focus to an obsolete opener', async ({ page }) => {
  await page.goto(`${origin}/test/fixtures/workflow-run-canvas.html`);
  const opener = page.getByRole('button', { name: 'Inspect worker on run graph', exact: true }).last();
  await opener.click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.evaluate(() => window.runCanvasFixture.retireCanvasOwner());
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(opener).not.toBeFocused();
  expect(await page.evaluate(() => document.getElementById('root')?.inert)).toBe(false);
});

test('paging trace visits and live cut advancement keep viewport transform, lazy value budget, and dialog focus intact', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width: 1280, height: 844 });
  await page.goto(`${origin}/test/fixtures/workflow-run-canvas.html?paged=1`);

  await page.getByRole('button', { name: 'Inspect worker on run graph' }).last().click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.locator('.react-flow__node')).toHaveCount(6);

  // Lazy value read count remains 0 until Output/Input tab is opened.
  expect(await page.evaluate(() => window.runCanvasFixture.valueReads().length)).toBe(0);

  // Zoom in so viewport transform is non-default before paging & live advancement, then wait for its transition to settle.
  const viewport = dialog.locator('.react-flow__viewport');
  const beforeZoom = await viewport.evaluate(el => el.style.transform);
  const zoomIn = dialog.getByRole('button', { name: 'Zoom in', exact: true });
  await zoomIn.click();
  await expect(dialog.getByRole('region', { name: 'Workflow canvas', exact: true })).toHaveAttribute('aria-busy', 'false');
  let transformBefore = beforeZoom;
  await expect.poll(async () => {
    const first = await viewport.evaluate(el => el.style.transform);
    await page.waitForTimeout(50);
    const second = await viewport.evaluate(el => el.style.transform);
    transformBefore = second;
    return first !== beforeZoom && first === second;
  }).toBe(true);

  // Paging trace visits expands count while keeping exact viewport transform and dialog focus.
  const loadMore = dialog.getByRole('button', { name: 'Load more trace visits' });
  await expect(loadMore).toBeVisible();
  await loadMore.focus();
  await loadMore.click();
  await expect(loadMore).toBeHidden();
  expect(await viewport.evaluate(el => el.style.transform)).toBe(transformBefore);
  expect(await dialog.evaluate(el => el.contains(document.activeElement))).toBe(true);

  // Live run settlement + cut advancement updates status in-place without resetting zoom/viewport or reading values eagerly.
  await page.evaluate(() => window.runCanvasFixture.completeLiveRun());
  await expect(dialog.locator('[data-run-node-status="running"]')).toHaveCount(0);
  await expect(loadMore).toBeVisible();
  expect(await viewport.evaluate(el => el.style.transform)).toBe(transformBefore);
  expect(await page.evaluate(() => window.runCanvasFixture.valueReads().length)).toBe(0);

  // Opening Output triggers lazy checkpointValueRead cleanly.
  await dialog.getByRole('tab', { name: 'Output' }).click();
  await expect(dialog.getByRole('tabpanel')).toContainText('lazy-output-e2e');
  expect(await page.evaluate(() => window.runCanvasFixture.valueReads().length)).toBeGreaterThan(0);
  expect(await viewport.evaluate(el => el.style.transform)).toBe(transformBefore);
  expect(errors).toEqual([]);
});

test('run inspection preserves the real authoring editor invalid field, undo history and selection', async ({ page }) => {
  await page.goto(`${origin}/test/fixtures/workflow-run-canvas.html?authoring=1`);
  const author = page.locator('#author-editor');
  const node = author.getByLabel('Tool node ready', { exact: true });
  await node.click();
  const literal = author.getByLabel('Literal JSON', { exact: true });
  await literal.fill('"edited"'); await literal.blur();
  await author.getByRole('button', { name: 'Undo', exact: true }).click();
  await literal.fill('"unclosed'); await literal.blur();
  const before = await page.evaluate(() => window.runCanvasFixture.authorState());
  expect(before).toMatchObject({ writes: 0 });
  await expect(node).toHaveClass(/selected/);
  await page.getByRole('button', { name: 'Inspect worker on run graph' }).last().click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.locator('.react-flow__node')).toHaveCount(6);
  await dialog.getByRole('tab', { name: 'Output' }).click();
  await expect(dialog.getByRole('tabpanel')).toContainText('lazy-output-e2e');
  await page.evaluate(() => window.runCanvasFixture.completeLiveRun());
  await expect(dialog.locator('[data-run-node-status="running"]')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  expect(await page.evaluate(() => window.runCanvasFixture.authorState())).toEqual(before);
  await expect(literal).toHaveValue('"unclosed');
  await expect(node).toHaveClass(/selected/);
});

for (const width of [1440, 390]) {
  test(`retry trace recovers from initial batch error while keeping focus contained, layout/snapshot un-reloaded, and viewport stable at ${width}px`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', msg => {
      if (['error', 'warning'].includes(msg.type())) errors.push(msg.text());
    });

    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${origin}/test/fixtures/workflow-run-canvas.html?traceFailOnce=1`);
    await expect(page).toHaveTitle('Workflow run canvas fixture');

    const opener = page.getByRole('button', { name: 'Inspect worker on run graph', exact: true }).last();
    await opener.click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId('rf__wrapper')).toBeVisible();
    await expect(dialog.locator('.react-flow__node')).toHaveCount(6);
    await expect(dialog.getByRole('region', { name: 'Workflow canvas', exact: true })).toHaveAttribute('aria-busy', 'false');

    // Initial trace failure shows visible alert and Retry trace button without failing snapshot or eager I/O.
    const alert = dialog.getByRole('alert').filter({ hasText: 'Injected initial trace batch failure' });
    await expect(alert).toBeVisible();
    const retryButton = dialog.getByRole('button', { name: 'Retry trace', exact: true });
    await expect(retryButton).toBeVisible();
    expect(await page.evaluate(() => window.runCanvasFixture.snapshotReads())).toEqual({ headers: 1, chunks: 1 });
    expect(await page.evaluate(() => window.runCanvasFixture.valueReads())).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);

    // Zoom in so camera transform is non-default, wait for animation frames / idle aria-busy witness, and capture layout instance + transform.
    const viewport = dialog.locator('.react-flow__viewport');
    const zoomLevel = dialog.getByRole('button', { name: 'Zoom', exact: true });
    await dialog.getByRole('button', { name: 'Zoom in', exact: true }).click();
    await expect(zoomLevel).not.toHaveText('100%');
    await expect(dialog.getByRole('region', { name: 'Workflow canvas', exact: true })).toHaveAttribute('aria-busy', 'false');
    await page.evaluate(() => new Promise<void>(resolve => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    }));
    const transformBefore = await viewport.evaluate(el => el.style.transform);
    expect(transformBefore).toBeTruthy();
    await page.evaluate(() => {
      (window as unknown as { __runCanvasFlowEl?: Element | null }).__runCanvasFlowEl = document.querySelector('[role="dialog"] .react-flow');
    });

    // Focus Retry trace and activate via Enter; focus must stay inside modal (tab/dialog), never fall to BODY or #root.
    await retryButton.focus();
    await expect(retryButton).toBeFocused();
    await page.keyboard.press('Enter');

    await expect(retryButton).toHaveCount(0);
    await expect(alert).toHaveCount(0);
    await expect(dialog.locator('[data-run-node-status="completed"]').first()).toBeVisible();

    const focusState = await page.evaluate(() => {
      const active = document.activeElement;
      const dialogEl = document.querySelector('[role="dialog"]');
      const rootEl = document.getElementById('root');
      const flowSame = dialogEl?.querySelector('.react-flow') === (window as unknown as { __runCanvasFlowEl?: Element | null }).__runCanvasFlowEl;
      return {
        onBody: active === document.body,
        inBackground: !!active && !!rootEl && rootEl.contains(active),
        inDialog: !!active && !!dialogEl && dialogEl.contains(active),
        selectedTabOrDialogFocused: !!active && (active.matches('[role="tab"][aria-selected="true"]') || active === dialogEl || !!active.closest('[role="dialog"]')),
        flowSame,
        overflow: document.documentElement.scrollWidth > window.innerWidth,
      };
    });
    expect(focusState).toEqual({
      onBody: false,
      inBackground: false,
      inDialog: true,
      selectedTabOrDialogFocused: true,
      flowSame: true,
      overflow: false,
    });

    // Snapshot was not reloaded, pan/zoom transform stayed identical, and zero eager values were read.
    expect(await page.evaluate(() => window.runCanvasFixture.snapshotReads())).toEqual({ headers: 1, chunks: 1 });
    expect(await viewport.evaluate(el => el.style.transform)).toBe(transformBefore);
    expect(await page.evaluate(() => window.runCanvasFixture.valueReads())).toEqual([]);

    // Opening Output after Retry recovery triggers a single lazy value read cleanly.
    await dialog.getByRole('tab', { name: 'Output' }).click();
    await expect(dialog.getByRole('tabpanel')).toContainText('lazy-output-e2e');
    expect(await page.evaluate(() => window.runCanvasFixture.valueReads())).toHaveLength(1);
    expect(await viewport.evaluate(el => el.style.transform)).toBe(transformBefore);

    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await expect(opener).toBeFocused();
    expect(errors).toEqual([]);
  });

  test(`frontier refresh failure preserves coherent trace, viewport, focus, and snapshot until explicit Retry recovers at ${width}px`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', msg => {
      if (['error', 'warning'].includes(msg.type())) errors.push(msg.text());
    });

    await page.setViewportSize({ width, height: 844 });
    await page.goto(`${origin}/test/fixtures/workflow-run-canvas.html?frontierRefreshFailOnce=1`);
    await expect(page).toHaveTitle('Workflow run canvas fixture');

    const opener = page.getByRole('button', { name: 'View run graph', exact: true });
    await opener.click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId('rf__wrapper')).toBeVisible();
    await expect(dialog.locator('.react-flow__node')).toHaveCount(6);
    await expect(dialog.getByRole('region', { name: 'Workflow canvas', exact: true })).toHaveAttribute('aria-busy', 'false');

    // Select the worker node, then open Output tab on coherent cut 1 before frontier advances.
    await dialog.locator('.react-flow__node', { hasText: 'worker' }).first().click();
    const outputTab = dialog.getByRole('tab', { name: 'Output' });
    await outputTab.click();
    await expect(dialog.getByRole('tabpanel')).toContainText('lazy-output-e2e');
    expect(await page.evaluate(() => window.runCanvasFixture.snapshotReads())).toEqual({ headers: 1, chunks: 1 });
    expect(await page.evaluate(() => window.runCanvasFixture.valueReads())).toHaveLength(1);

    // Non-default zoom transform + wait for zoomIn transition completion (target scale(1.2)) + layout DOM identity capture.
    const viewport = dialog.locator('.react-flow__viewport');
    const zoomLevel = dialog.getByRole('button', { name: 'Zoom', exact: true });
    await dialog.getByRole('button', { name: 'Zoom in', exact: true }).click();
    await expect(zoomLevel).toHaveText('120%');
    await expect(viewport).toHaveAttribute('style', /scale\(1\.2\)/);
    await expect(dialog.getByRole('region', { name: 'Workflow canvas', exact: true })).toHaveAttribute('aria-busy', 'false');
    await page.evaluate(() => new Promise<void>(resolve => {
      requestAnimationFrame(() => resolve());
    }));
    const transformBefore = await viewport.evaluate(el => el.style.transform);
    expect(transformBefore).toBeTruthy();
    await page.evaluate(() => {
      (window as unknown as { __runCanvasFlowEl?: Element | null }).__runCanvasFlowEl = document.querySelector('[role="dialog"] .react-flow');
    });

    // Advance frontier -> cut 2 locate fails once -> coherent cut 1 Output & non-default zoom remain intact, alert & Retry appear!
    await page.evaluate(() => window.runCanvasFixture.completeLiveRun());
    const alert = dialog.getByRole('alert').filter({ hasText: 'Injected frontier refresh failure' });
    await expect(alert).toBeVisible();
    const retryButton = dialog.getByRole('button', { name: 'Retry trace', exact: true });
    await expect(retryButton).toBeVisible();
    expect(await outputTab.getAttribute('aria-selected')).toBe('true');
    await expect(dialog.getByRole('tabpanel')).toContainText('lazy-output-e2e');
    expect(await page.evaluate(() => window.runCanvasFixture.snapshotReads())).toEqual({ headers: 1, chunks: 1 });
    expect(await page.evaluate(() => window.runCanvasFixture.valueReads())).toHaveLength(1);
    expect(await viewport.evaluate(el => el.style.transform)).toBe(transformBefore);

    // Focus Retry trace and activate via Enter -> recovers cut 2 without further frontier advance, focus stays inside dialog, layout/transform unchanged.
    await retryButton.focus();
    await expect(retryButton).toBeFocused();
    await page.keyboard.press('Enter');

    await expect(retryButton).toHaveCount(0);
    await expect(alert).toHaveCount(0);
    await expect(dialog.locator('[data-run-node-status="completed"]').first()).toBeVisible();

    const postRetryState = await page.evaluate(() => {
      const active = document.activeElement;
      const dialogEl = document.querySelector('[role="dialog"]');
      const rootEl = document.getElementById('root');
      const flowSame = dialogEl?.querySelector('.react-flow') === (window as unknown as { __runCanvasFlowEl?: Element | null }).__runCanvasFlowEl;
      return {
        onBody: active === document.body,
        inBackground: !!active && !!rootEl && rootEl.contains(active),
        inDialog: !!active && !!dialogEl && dialogEl.contains(active),
        selectedTabOrDialogFocused: !!active && (active.matches('[role="tab"][aria-selected="true"]') || active === dialogEl || !!active.closest('[role="dialog"]')),
        flowSame,
        overflow: document.documentElement.scrollWidth > window.innerWidth,
      };
    });
    expect(postRetryState).toEqual({
      onBody: false,
      inBackground: false,
      inDialog: true,
      selectedTabOrDialogFocused: true,
      flowSame: true,
      overflow: false,
    });

    expect(await outputTab.getAttribute('aria-selected')).toBe('true');
    await expect(dialog.getByRole('tabpanel')).toContainText('lazy-output-e2e');
    expect(await page.evaluate(() => window.runCanvasFixture.snapshotReads())).toEqual({ headers: 1, chunks: 1 });
    expect(await viewport.evaluate(el => el.style.transform)).toBe(transformBefore);

    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await expect(opener).toBeFocused();
    expect(errors).toEqual([]);
  });
}

import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

for (const [width, locale] of [[1440, 'en'], [390, 'zh']] as const) test(`tool location keeps following new Trace rows without a page reload (${width}, ${locale})`, async ({ page }) => {
  const zh = locale === 'zh', errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.clock.setFixedTime(new Date('2026-09-15T00:00:05Z'));
  await page.setViewportSize({ width, height: 1000 });
  await page.addInitScript(locale => localStorage.setItem('rustx-locale-v1', locale), locale);
  await page.goto(`${origin}/test/fixtures/startup.html?existing&trace-following`);
  await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.allow('session/attach'); f.resumeCatalog(); });
  if (zh) await page.getByRole('button', { name: '打开侧边栏', exact: true }).click();
  await page.locator('button[data-session-id=A]').click();
  await expect(page).toHaveTitle('Session A — rustX');
  await page.getByRole('tab', { name: zh ? '轨迹' : 'Trajectory', exact: true }).click();
  await page.evaluate(() => window.traceFollowingFixture.locate());
  const latest = page.getByRole('button', { name: zh ? '返回最新' : 'Return to latest', exact: true });
  await expect(latest).toBeVisible();
  const inspector = page.getByRole('complementary', { name: zh ? '事件详情' : 'Event details', exact: true });
  await expect(inspector).toBeVisible();
  const row = page.locator('[data-display-type="RecordRow"][data-owner="trace:7"]');
  await row.evaluate(element => element.setAttribute('data-retained-live-tool', ''));
  for (const text of ['First live response', 'The next turn is visible', 'The tool result has settled']) {
    await page.evaluate(text => window.traceFollowingFixture.append(text), text);
    await expect(page.getByText(text, { exact: true })).toBeVisible();
    await expect(latest).toBeVisible();
    await expect(page.locator('[data-retained-live-tool]')).toHaveCount(1);
    await expect(inspector).toBeVisible();
  }
  await page.screenshot({ path: `/tmp/rustx-trace-following-${width}-${locale}.png` });
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
});

for (const theme of ['dark', 'light'] as const) for (const width of [1440, 390]) test(`running inspector keeps content, focus and scroll ${theme} ${width}`, async ({ page }) => {
  const zh = width === 390, errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
  await page.addInitScript(locale => localStorage.setItem('rustx-locale-v1', locale), zh ? 'zh' : 'en');
  await page.goto(`${origin}/test/fixtures/startup.html?existing&models&trace-live`);
  await page.waitForFunction(() => (window as any).startupFixture);
  await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.allow('session/attach'); f.resumeCatalog(); });
  if (zh) await page.getByRole('button', { name: '打开侧边栏', exact: true }).click();
  await page.locator('button[data-session-id=A]').click();
  await expect(page).toHaveTitle('Session A — rustX');
  await page.getByRole('tab', { name: zh ? '轨迹' : 'Trajectory', exact: true }).click();
  await page.locator('[data-request-owner="trace:4"]').click();
  const inspector = page.getByRole('complementary', { name: zh ? '事件详情' : 'Event details', exact: true });
  const options = inspector.getByRole('tab', { name: zh ? '选项' : 'Options', exact: true });
  await options.click(); await expect(inspector.getByRole('tree')).toContainText('ORIGINAL_MODEL');
  await options.focus();
  await inspector.getByRole('tabpanel').evaluate(el => { el.scrollTop = 80; });
  await page.evaluate(() => {
    const panel = document.querySelector('[role=tabpanel]')!, tree = panel.querySelector('[role=tree]')!;
    const proof = { panel, tree, lost: 0, loading: 0 };
    (window as any).traceProof = proof;
    new MutationObserver(() => {
      if (!panel.isConnected || !tree.isConnected) proof.lost++;
      if (panel.querySelector('[role=status]')) proof.loading++;
    }).observe(panel, { subtree: true, childList: true, characterData: true });
  });
  await page.screenshot({ path: `/tmp/rustx-trace-live-before-${theme}-${width}.png` });
  await page.evaluate(() => window.traceLiveFixture.holdSummary());
  await expect.poll(() => page.evaluate(() => window.traceLiveFixture.summaries())).toBe(1);
  await page.evaluate(() => window.traceLiveFixture.run(30));
  await page.evaluate(() => window.traceLiveFixture.releaseSummary());
  await expect.poll(() => page.evaluate(() => window.traceLiveFixture.summaries())).toBe(2);
  expect(await page.evaluate(() => window.traceLiveFixture.reads())).toBe(1);
  await expect(options).toBeFocused();
  expect(await inspector.getByRole('tabpanel').evaluate(el => el.scrollTop)).toBe(80);
  await page.evaluate(() => window.traceLiveFixture.change());
  await expect.poll(() => page.evaluate(() => window.traceLiveFixture.reads())).toBe(2);
  await page.evaluate(() => window.traceLiveFixture.run(20));
  await expect(inspector.getByRole('tree')).toContainText('ORIGINAL_MODEL');
  const values = await page.evaluate(() => window.traceLiveFixture.values());
  await page.evaluate(() => window.traceLiveFixture.releaseHeader());
  await expect.poll(() => page.evaluate(() => window.traceLiveFixture.values())).toBe(values + 1);
  await expect(inspector.getByRole('tree')).toContainText('ORIGINAL_MODEL');
  await expect(inspector.getByRole('status')).toHaveCount(0);
  await page.evaluate(() => window.traceLiveFixture.releaseContent());
  await expect(inspector.getByRole('tree')).toContainText('UPDATED_MODEL');
  await expect(options).toBeFocused();
  expect(await inspector.getByRole('tabpanel').evaluate(el => el.scrollTop)).toBe(80);
  expect(await page.evaluate(() => { const proof = (window as any).traceProof; return { lost: proof.lost, loading: proof.loading, same: document.querySelector('[role=tabpanel]') === proof.panel && proof.panel.querySelector('[role=tree]') === proof.tree }; })).toEqual({ lost: 0, loading: 0, same: true });
  expect(await page.evaluate(() => window.traceLiveFixture.reads())).toBe(2);
  await page.screenshot({ path: `/tmp/rustx-trace-live-after-${theme}-${width}.png` });
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  expect(errors).toEqual([]);
});

for (const width of [1440, 390]) test(`historical Trace keeps appending while its reading position stays fixed (${width})`, async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.setViewportSize({ width, height: 1000 });
  await page.addInitScript(() => localStorage.setItem('rustx-locale-v1', 'en'));
  await page.goto(`${origin}/test/fixtures/startup.html?existing&trace-following&trace-history`);
  await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.allow('session/attach'); f.resumeCatalog(); });
  if (width === 390) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
  await page.locator('button[data-session-id=A]').click();
  await expect(page).toHaveTitle('Session A — rustX');
  await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
  await page.evaluate(() => window.traceFollowingFixture.locate());
  if (width === 390) await page.getByRole('button', { name: 'Close details', exact: true }).click();
  const port = page.locator('[data-trajectory-scroll]');
  await expect(port.getByText('Historical event 20', { exact: true })).toBeVisible();
  await port.evaluate(element => { element.scrollTop = 180; element.dispatchEvent(new Event('scroll')); });
  const top = await port.evaluate(element => element.scrollTop);
  expect(top).toBeGreaterThan(0);
  const anchor = port.locator('[data-owner="trace:20"][data-display-type="RecordRow"]');
  const offset = await anchor.evaluate(element => element.getBoundingClientRect().top);
  for (let n = 0; n < 45; n++) {
    await page.evaluate(n => window.traceFollowingFixture.append(`Live event ${n}`), n);
    await expect.poll(() => page.evaluate(() => window.traceFollowingFixture.ids().at(-1))).toBe(`trace:${166+n}`);
    expect(await port.evaluate(element => element.scrollTop)).toBe(top);
    expect(await anchor.evaluate(element => element.getBoundingClientRect().top)).toBe(offset);
  }
  await expect.poll(() => page.evaluate(() => window.traceFollowingFixture.gaps())).toBe(1);
  await page.screenshot({ path: `/tmp/rustx-trace-history-${width}.png` });
  // The tail was updated while offscreen, and becomes visible by scrolling alone.
  await port.evaluate(element => { element.scrollTop = element.scrollHeight; element.dispatchEvent(new Event('scroll')); });
  await expect(port.getByText('Live event 44', { exact: true })).toBeVisible();
  await page.evaluate(() => window.traceFollowingFixture.append('Following resumes at the bottom'));
  await expect(port.getByText('Following resumes at the bottom', { exact: true })).toBeVisible();
  await expect.poll(() => port.evaluate(element => element.scrollHeight - element.clientHeight - element.scrollTop)).toBeLessThanOrEqual(2);
  // Load the middle boundary through the real control, including virtualization.
  while (await page.evaluate(() => window.traceFollowingFixture.gaps())) {
    await port.evaluate(element => { element.scrollTop = 32 * 30; element.dispatchEvent(new Event('scroll')); });
    const more = port.getByRole('button', { name: 'Load earlier history', exact: true });
    await more.scrollIntoViewIfNeeded();
    const before = await page.evaluate(() => window.traceFollowingFixture.ids().length);
    const reading = await port.evaluate(element => {
      const top = element.getBoundingClientRect().top;
      const row = [...element.querySelectorAll<HTMLElement>('[role=row][data-display-key]')].find(row => row.getBoundingClientRect().top >= top)!;
      return { key: row.dataset.displayKey!, offset: row.getBoundingClientRect().top - top };
    });
    await more.click();
    await expect.poll(() => page.evaluate(() => window.traceFollowingFixture.ids().length)).toBeGreaterThan(before);
    await expect.poll(() => port.evaluate((element, key) => {
      const row = [...element.querySelectorAll<HTMLElement>('[data-display-key]')].find(row => row.dataset.displayKey === key);
      return row ? row.getBoundingClientRect().top - element.getBoundingClientRect().top : null;
    }, reading.key)).toBe(reading.offset);
  }
  expect(await page.evaluate(() => window.traceFollowingFixture.ids())).toEqual(Array.from({length:206},(_,n)=>`trace:${n+6}`));
  await page.getByRole('button', { name: 'Return to latest', exact: true }).click();
  await expect(port.getByText('Following resumes at the bottom', { exact: true })).toBeVisible();
  await expect.poll(() => port.evaluate(element => element.scrollHeight - element.clientHeight - element.scrollTop)).toBeLessThanOrEqual(2);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
});

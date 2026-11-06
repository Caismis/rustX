import { test, expect } from '@playwright/test';
const fixture = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/startup.html?existing&models&subagents&child-turn-navigation`;
for (const width of [1440, 390]) test(`child native turn directory locates distant history and short final turns at ${width}`, async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
  await page.goto(fixture); await expect(page).toHaveTitle('rustX');
  await page.evaluate(() => { const f = (window as any).startupFixture; f.allow('session/summary'); f.allow('session/attach'); f.resumeCatalog(); });
  if (width < 600) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
  await page.locator('button[data-session-id=A]').click();
  if (width < 600) await expect(page.getByRole('button', { name: 'Expand Sidebar', exact: true })).toBeFocused();
  const parentTitle = await page.locator('#session-title').innerText();
  const trigger = page.getByRole('button', { name: 'Subagents', exact: true });
  await trigger.click(); await page.getByRole('treeitem', { name: /Verify findings/ }).click();
  const rail = page.getByRole('navigation', { name: 'Turn navigation', exact: true });
  if (width < 600) {
    // Harness hides the rail below 900px; narrow child views still use the
    // native finite history windows and keep composing while browsing.
    await expect(rail).toHaveCount(0);
    await page.getByRole('button', { name: 'Load earlier', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Child report 126', exact: true })).toBeVisible();
    const input = page.getByRole('textbox', { name: 'Message Agent Verify findings', exact: true });
    await input.fill('Keep composing while reading history');
    await page.getByRole('button', { name: 'Load earlier', exact: true }).scrollIntoViewIfNeeded();
    await expect(page.getByRole('button', { name: 'Load later', exact: true })).toHaveCount(0);
    await page.screenshot({ path: '/tmp/rustx-history-controls-mobile.png' });
    await page.getByRole('button', { name: 'Return to latest', exact: true }).click();
    await expect(input).toHaveValue('Keep composing while reading history');
    await expect(page.getByRole('heading', { name: 'Child report 126', exact: true })).toBeAttached();
    const calls = await page.evaluate(() => (window as any).startupFixture.requests());
    expect(calls.some((request: any) => request.method === 'agent/turns')).toBe(true);
    expect(calls.some((request: any) => request.method === 'agent/transcript' && request.params.at.type === 'older')).toBe(true);
    expect(errors).toEqual([]); return;
  }
  await expect(page.getByRole('button', { name: 'Load later', exact: true })).toHaveCount(0);
  await expect(rail).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Child report 130', exact: true })).toBeVisible();
  await rail.locator(':scope > div').first().evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
  const first = rail.getByRole('button', { name: 'Load and jump to turn 1', exact: true });
  await first.click();
  const loadedFirst = rail.getByRole('button', { name: 'Jump to turn 1', exact: true });
  await expect(loadedFirst).toHaveAttribute('aria-current', 'true');
  await expect(page.getByRole('heading', { name: 'Child report 1', exact: true })).toBeVisible();
  await loadedFirst.hover(); await expect(page.getByRole('tooltip')).toContainText('Child task 1');
  await expect(page.getByRole('tooltip')).toContainText('Child report 1');
  await page.screenshot({ path: `/tmp/rustx-child-turn-navigation-${width}.png` });
  await page.getByRole('button', { name: parentTitle, exact: true }).click();
  await trigger.click(); await page.getByRole('treeitem', { name: /Verify findings/ }).click();
  await expect(page.getByRole('heading', { name: 'Child report 1', exact: true })).toBeVisible();
  await expect(rail.getByRole('button', { name: 'Jump to turn 1', exact: true })).toHaveAttribute('aria-current', 'true');
  await rail.locator(':scope > div').first().evaluate(el => { el.scrollTop = el.scrollHeight; el.dispatchEvent(new Event('scroll')); });
  await rail.getByRole('button', { name: 'Load and jump to turn 130', exact: true }).click();
  await expect(rail.getByRole('button', { name: 'Jump to turn 130', exact: true })).toHaveAttribute('aria-current', 'true');
  await expect(page.getByRole('heading', { name: 'Child report 130', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Return to latest', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Return to latest', exact: true })).toHaveCount(0);
  const calls = await page.evaluate(() => (window as any).startupFixture.requests());
  const located = calls.filter((request: any) => request.method === 'agent/transcript' && request.params.at.type === 'turn_ordinal');
  expect(located.map((request: any) => request.params.at.ordinal)).toEqual([1, 130]);
  for (const request of located) {
    expect(request.params.agent_id).toBe('child-1');
    expect(request.params.at.cut).toEqual({ conversation_id: 'child-conversation-1', journal: '1000', transcript: '1000', mutation_revision: '0', ledger_through: '0', surface_revision: '0', transcript_total: '0', pending_through: '0', pending_total: '0' });
  }
  expect(calls.filter((request: any) => request.method === 'agent/turns').some((request: any) => request.params.offset === 0)).toBe(true);
  if (width === 1440) {
    await page.getByRole('button', { name: parentTitle, exact: true }).click();
    await trigger.click(); await page.getByRole('button', { name: 'Open Verify findings in sidebar', exact: true }).click();
    const aside = page.locator('[data-workbench]');
    const asideRail = aside.getByRole('navigation', { name: 'Turn navigation', exact: true });
    await expect(asideRail).toHaveCount(0);
    await aside.getByRole('button', { name: 'Fullscreen', exact: true }).click();
    await expect(asideRail).toBeVisible();
    await asideRail.locator(':scope > div').first().evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event('scroll')); });
    await asideRail.getByRole('button', { name: 'Load and jump to turn 1', exact: true }).click();
    await expect(asideRail.getByRole('button', { name: 'Jump to turn 1', exact: true })).toHaveAttribute('aria-current', 'true');
    await expect(aside.getByRole('heading', { name: 'Child report 1', exact: true })).toBeVisible();
    const input = aside.getByRole('textbox', { name: 'Message Agent Verify findings', exact: true });
    await expect(input).toBeVisible(); await input.fill('Sidebar draft remains available while reading');
    const box = (await input.boundingBox())!;
    expect(box.y + box.height).toBeLessThanOrEqual(1000);
    await page.screenshot({ path: '/tmp/rustx-child-sidebar-turn-navigation.png' });
    const before = await page.evaluate(() => (window as any).startupFixture.requests().filter((request: any) => request.method === 'agent/transcript' && request.params.at.type === 'newer').length);
    await aside.locator('[data-chat-history-boundary]').scrollIntoViewIfNeeded();
    await aside.locator('[data-conversation-scroll]').last().hover();
    await page.mouse.wheel(0, 80);
    await expect.poll(() => page.evaluate(() => (window as any).startupFixture.requests().filter((request: any) => request.method === 'agent/transcript' && request.params.at.type === 'newer').length)).toBe(before + 1);
    await expect(aside.getByRole('heading', { name: 'Child report 5', exact: true })).toHaveCount(1);
    await expect(aside.getByRole('button', { name: 'Load later', exact: true })).toHaveCount(0);
    await page.screenshot({ path: '/tmp/rustx-history-continuation-desktop.png' });
  }
  expect(errors).toEqual([]);
});

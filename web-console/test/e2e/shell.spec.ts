import { test, expect } from '@playwright/test';
import { expectStableScreenshot } from './screenshot';
import { choose } from './shell-actions';
test('stream publications and a turn-local clock preserve chrome identity, geometry and caret', async ({ page }) => {
  await page.clock.install({ time: new Date('2026-09-24T23:00:00Z') });
  await page.goto('http://127.0.0.1:5174/test/fixtures/shell.html');
  await page.clock.pauseAt(new Date('2026-09-25T00:00:00Z'));
  await page.evaluate(() => window.sessionFixture.stream('Token zero'));
  await expect(page.getByText('Token zero', { exact: true })).toBeVisible();
  const input = page.getByRole('textbox', { name: 'Message', exact: true });
  await input.fill('Keep this draft'); await input.focus();
  await input.evaluate((node: HTMLTextAreaElement) => node.setSelectionRange(3, 7));
  const inputNode = await input.elementHandle();
  const header = await page.locator('#session-view > header').elementHandle();
  const card = page.locator('[data-composer-card]');
  const geometry = { header: await header!.boundingBox(), card: await card.boundingBox() };
  for (let index = 1; index <= 5; index++) {
    const text = `Token ${index}: ` + 'streamed content '.repeat(index * 20);
    await page.evaluate(text => window.sessionFixture.stream(text), text);
    await expect(page.getByLabel('Streaming response')).toContainText(`Token ${index}:`);
    expect(await inputNode!.evaluate(node => node === document.querySelector('textarea[aria-label="Message"]'))).toBe(true);
    expect(await header!.evaluate(node => node === document.querySelector('#session-view > header'))).toBe(true);
    expect({ header: await header!.boundingBox(), card: await card.boundingBox() }).toEqual(geometry);
  }
  await page.clock.runFor(5000);
  await expect(page.getByRole('button', { name: 'Deep diving for 5s', exact: true })).toBeVisible();
  expect({ header: await header!.boundingBox(), card: await card.boundingBox() }).toEqual(geometry);
  await expect(input).toBeFocused(); await expect(input).toHaveValue('Keep this draft');
  expect(await input.evaluate((node: HTMLTextAreaElement) => [node.selectionStart, node.selectionEnd])).toEqual([3, 7]);
});
for (const mode of ['empty', 'preview', 'named', 'delete', 'other-uncertain', 'background'] as const) test(`Sidebar-only Session surface: ${mode}`, async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('http://127.0.0.1:5174/test/fixtures/shell.html');
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  if (mode !== 'background') await page.evaluate(mode => window.sessionFixture.presentation(mode), mode);
  await expect(page.getByRole('tree', { name: 'Session browser' })).toHaveCount(1);
  await expect(page.getByRole('tablist')).toHaveCount(1);
  if (mode === 'background') {
    await page.locator('button[data-session-id="B"]').click();
    await expect(page.getByLabel('Session title')).toHaveText('Session B');
    await expect(page.locator('button[data-session-id="A"]')).not.toContainText('Working…');
  }
  if (mode === 'other-uncertain') {
    await expect(page.getByLabel('Session status')).toHaveCount(0);
    await expect(page.getByRole('button', { name: /^Deep diving/ }).first()).toBeVisible();
    await expect(page.locator('button[data-session-id="B"]')).toContainText('Needs verification');
  }
  if (mode === 'delete') {
    await page.locator('button[data-session-id="A"]').hover();
    await page.locator('button[data-session-actions="A"]').click();
    await page.getByRole('menuitem', { name: 'Delete Session' }).click();
    await expect(page.getByRole('dialog', { name: 'Confirm Session deletion' })).toContainText('Delete Inspect the Session ownership boundary?');
  }
  await expectStableScreenshot(page, `sidebar-${mode}-light.png`);
  if (mode === 'other-uncertain') {
    await page.getByRole('button', { name: 'Toggle Inspector' }).click();
    await page.getByText('Complete native runtime facts', { exact: true }).click();
    expect(JSON.parse(await page.getByLabel('Native diagnostic JSON').innerText()).uncertain_operations).toEqual([]);
    await expectStableScreenshot(page, 'sidebar-scoped-inspector.png');
  }
  expect(errors).toEqual([]);
});
// Intentional updates in the pinned browser environment: pnpm test:e2e:update
// Fixed time, native-protocol fixture and reduced motion keep evidence reviewable.
test('Harness shell reference states and presentation-only navigation', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('http://127.0.0.1:5174/test/fixtures/shell.html');
  await expect(page).toHaveTitle('rustX shell reference');
  await expect(page.getByLabel('Sidebar', { exact: true })).toContainText('rustX');
  await expect(page.locator('body')).not.toContainText('DeepSeek');
  await expect(page.locator('img[src*="deepseek" i]')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Open Session A', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Open Session A', exact: true })).toHaveAttribute('aria-current', 'page');
  await expect(page.getByRole('tree', { name: 'Session browser' })).toContainText('Waiting for approval');
  await expect(page.getByRole('tree', { name: 'Session browser' })).toContainText('Running');
  await expectStableScreenshot(page, 'desktop-expanded-light.png');
  await page.getByRole('button', { name: 'Toggle Inspector' }).click();
  await expect(page.getByRole('complementary', { name: 'Developer inspector' })).toBeVisible();
  await expectStableScreenshot(page, 'desktop-right-panel.png');
  await page.getByRole('button', { name: 'Close Inspector' }).click();
  await page.getByRole('button', { name: 'Collapse Sidebar' }).click();
  await expect(page.locator('[data-sidebar-wide]')).toHaveAttribute('data-sidebar-wide', 'false');
  expect((await page.locator('main').boundingBox())!.x).toBe(56);
  await expectStableScreenshot(page, 'desktop-collapsed-rail.png');
  await page.getByRole('button', { name: 'Expand Sidebar' }).click();
  await page.getByRole('button', { name: 'Search Sessions', exact: true }).click();
  await page.getByLabel('Search Session metadata').fill('Session');
  await expectStableScreenshot(page, 'workspace-session-browser.png');
  await page.getByRole('button', { name: 'Clear search' }).click();
  // Global Settings opens at General, which holds Appearance.
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expectStableScreenshot(page, 'settings-shell-light.png');
  await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Theme', 'Dark');
  await expectStableScreenshot(page, 'settings-shell-dark.png');
  await page.getByRole('button', { name: 'Close Settings' }).click();
  await expectStableScreenshot(page, 'desktop-expanded-dark.png');
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.locator('[data-sidebar-wide]')).toHaveAttribute('data-sidebar-wide', 'false');
  expect((await page.locator('main').boundingBox())!.x).toBe(56);
  await expectStableScreenshot(page, 'mobile-rail-dark.png');
  await page.getByRole('button', { name: 'Expand Sidebar' }).click();
  await expectStableScreenshot(page, 'mobile-expanded-dark.png');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expectStableScreenshot(page, 'mobile-settings-dark.png');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('Session product states stay concise and recovery evidence remains in Inspector', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  for (const mode of ['idle', 'queued', 'stopping', 'reconnect', 'uncertain'] as const) {
    await page.goto('http://127.0.0.1:5174/test/fixtures/shell.html');
    await expect(page.getByLabel('Session status')).toHaveCount(0);
    await expect(page.getByRole('button', { name: /^Deep diving/ }).first()).toBeVisible();
    await page.evaluate(mode => window.sessionFixture.state(mode), mode);
    const expected = { idle: undefined, queued: 'Queued', stopping: 'Stopping…', reconnect: 'Connection interrupted', uncertain: 'Needs verification' }[mode];
    if (expected) await expect(page.getByLabel('Session status')).toContainText(expected);
    else await expect(page.getByLabel('Session status')).toHaveCount(0);
    const ordinary = await page.locator('main').innerText();
    expect(ordinary).not.toMatch(/attempt-A|runtime_incarnation|connection_generation|Attach \/ cold resume|Unload runtime|Detach|Resync/);
    await expectStableScreenshot(page, `session-${mode}-light.png`);
    if (mode === 'uncertain') {
      await page.getByRole('button', { name: 'Toggle Inspector' }).click();
      await page.getByText('Uncertain operations and reconciliation evidence', { exact: true }).click();
      await expect(page.getByRole('complementary', { name: 'Developer inspector' })).toContainText('turn/cancel');
      await page.getByRole('button', { name: 'Close Inspector' }).click();
      await page.setViewportSize({ width: 390, height: 844 });
      await expectStableScreenshot(page, 'session-uncertain-mobile.png');
    }
  }
  expect(errors).toEqual([]);
});

/** A Session row's actions menu held the keyboard when the row was scrolled
 * out of the Session list. The menu closes because its anchor is no longer a
 * visible interaction anchor, and the keyboard goes to the owner the Workspace
 * browser names — the Session tree, which stays rendered and visible around
 * the row — without scrolling the list back to the hidden row, and never to
 * the page body, the hidden row or its trigger, or a removed menu row. */
test('a Session row scrolled out from under its open actions menu leaves the keyboard on the Session tree', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('http://127.0.0.1:5174/test/fixtures/shell.html');
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await page.evaluate(() => window.sessionFixture.presentation('many'));
  const tree = page.getByRole('tree', { name: 'Session browser' });
  await expect(tree.locator('button[data-session-id="S24"]')).toBeAttached();
  // A known starting point: the list at its top, with room to scroll.
  expect(await tree.evaluate(el => { el.scrollTop = 0; return el.scrollTop; })).toBe(0);
  expect(await tree.evaluate(el => el.scrollHeight - el.clientHeight)).toBeGreaterThan(200);
  const id = (await tree.locator('button[data-session-id]').first().getAttribute('data-session-id'))!;
  const title = tree.locator(`button[data-session-id="${id}"]`);
  const actions = tree.locator(`button[data-session-actions="${id}"]`);
  const menu = page.getByRole('menu');
  /** The list's scroll, whether the row lies wholly above the list's clipping
   * region, where the keyboard is, and how many menu rows remain. */
  const observe = () => tree.evaluate((el, id) => {
    const row = el.querySelector(`button[data-session-id="${id}"]`)!.closest('[role="treeitem"]')!;
    const trigger = el.querySelector(`button[data-session-actions="${id}"]`)!;
    const active = document.activeElement!;
    return {
      scrollTop: el.scrollTop,
      pageScroll: window.scrollY,
      clipped: row.getBoundingClientRect().bottom <= el.getBoundingClientRect().top,
      keyboard: active === el ? 'tree' : active === document.body ? 'body' : active === trigger ? 'trigger' : row.contains(active) ? 'row' : active.getAttribute('role') ?? active.tagName,
      menuitems: document.querySelectorAll('[role="menuitem"]').length,
    };
  }, id);

  // From the keyboard: the row's title, Tab to its actions, Enter opens the
  // menu, ArrowDown moves the keyboard into it.
  await title.focus(); await page.keyboard.press('Tab');
  await expect(actions).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(menu).toBeVisible();
  await expect(actions).toHaveAttribute('aria-expanded', 'true');
  await page.keyboard.press('ArrowDown');
  await expect(menu.getByRole('menuitem').first()).toBeFocused();
  expect(await observe()).toMatchObject({ scrollTop: 0, clipped: false, keyboard: 'menuitem' });

  // The Session list scrolls the row wholly out of its clipping region.
  const scrolledTo = await tree.evaluate(el => { el.scrollTop = el.scrollHeight; return el.scrollTop; });
  expect(scrolledTo).toBeGreaterThan(200);
  await expect(menu).toHaveCount(0);
  // The owner's open state settled closed with it.
  await expect(actions).toHaveAttribute('aria-expanded', 'false');
  // The scroll stands, nothing scrolled back to the row, no menu row remains,
  // and the keyboard is on the Session tree: rendered, visible and enabled.
  expect(await observe()).toEqual({ scrollTop: scrolledTo, pageScroll: 0, clipped: true, keyboard: 'tree', menuitems: 0 });
  await expect(tree).toBeFocused();
  await expect(tree).toBeInViewport();
  await expect(tree).toBeEnabled();

  // The Workspace browser stays keyboard-operable from there: Tab enters the
  // tree at its first row, the Workspace header, and Enter collapses it.
  await page.keyboard.press('Tab');
  const workspace = tree.locator('[role="treeitem"][aria-expanded]').first();
  await expect(workspace).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(workspace).toHaveAttribute('aria-expanded', 'false');
  await expect(title).toHaveCount(0);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

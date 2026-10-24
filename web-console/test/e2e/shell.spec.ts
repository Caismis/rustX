const fixtureOrigin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
import { test, expect, type Page } from '@playwright/test';
import { expectStableScreenshot } from './screenshot';
import { openWorkspaceSettings } from './shell-actions';

/** The shell reference owns its cold-load interleaving. Present the unclassified
 * baseline before publishing the Host reply, then await both shared consumers.
 * An immediately resolved fixture reply can collapse those paints; Chromium's
 * circular Commands background then differs from the cold-load reference even
 * with identical final styles. Do not couple this contract to promise timing. */
async function openClassifiedShell(page: Page, initial?: 'other-uncertain') {
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html?association-gate&initial=${initial ?? ''}`);
  const tree = page.getByRole('tree', { name: 'Session browser' });
  await expect(tree).toContainText('Workspace not yet classified');
  await expect(page.getByRole('button', { name: 'Add', exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Workspace permissions', exact: true })).toHaveCount(0);
  // Two frame boundaries present the asserted cold shell before the reply.
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await page.evaluate(() => window.sessionFixture.releaseAssociations());
  await expect(tree).not.toContainText('Workspace not yet classified');
  await expect(tree).not.toContainText('Refreshing Workspace associations');
  await expect(page.getByRole('button', { name: 'Workspace permissions', exact: true })).toBeVisible();
}

test('narrow sidebar overlays the conversation without squeezing its composer', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
  const center = page.locator('main');
  await expect(page.getByRole('button', { name: 'Expand Sidebar' })).toBeVisible();
  const width = (await center.boundingBox())!.width;
  await page.getByRole('button', { name: 'Expand Sidebar' }).click();
  await expect(page.getByRole('button', { name: 'Collapse Sidebar' })).toBeVisible();
  expect((await center.boundingBox())!.width).toBe(width);
  await expect(center).toHaveAttribute('inert', '');
  await page.getByRole('button', { name: 'Collapse Sidebar' }).press('Escape');
  await expect(page.getByRole('button', { name: 'Expand Sidebar' })).toBeVisible();
  await expect(center).not.toHaveAttribute('inert', '');
  await page.getByRole('button', { name: 'Expand Sidebar' }).click();
  await page.getByRole('button', { name: 'Close navigation' }).click({ position: { x: 370, y: 400 } });
  await expect(page.getByRole('button', { name: 'Expand Sidebar' })).toBeVisible();
});

test('responsive sidebar dismissal restores interaction across breakpoint changes', async ({ page }) => {
  const errors: string[]=[]; page.on('pageerror',error=>errors.push(error.message));
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
  await expect(page).toHaveTitle(/rustX/);
  const frame=page.locator('[data-harness-frame]'), center=page.locator('main');
  const expand=page.getByRole('button',{name:'Expand Sidebar',exact:true});
  const collapse=page.getByRole('button',{name:'Collapse Sidebar',exact:true});
  const message=page.getByRole('textbox',{name:'Message',exact:true});
  await expect(collapse).toBeVisible();
  await page.setViewportSize({width:390,height:844});
  await expect(expand).toBeVisible();
  for(const dismissal of ['toggle','escape','backdrop'] as const){
    await expand.click();
    await expect(frame).toHaveAttribute('data-sidebar-overlay','true');
    await expect(center).toHaveAttribute('inert','');
    await expect(collapse).toBeFocused();
    expect(await message.evaluate(el=>el.closest('[inert]')!==null)).toBe(true);
    if(dismissal==='toggle') await collapse.click();
    else if(dismissal==='escape') await page.keyboard.press('Escape');
    else await page.getByRole('button',{name:'Close navigation'}).click({position:{x:370,y:400}});
    await expect(frame).not.toHaveAttribute('data-sidebar-overlay');
    await expect(center).not.toHaveAttribute('inert','');
    await expect(page.getByRole('button',{name:'Close navigation'})).toHaveCount(0);
    await expect(expand).toBeFocused();
    await message.click(); await expect(message).toBeFocused();
    expect(await message.evaluate(el=>{const r=el.getBoundingClientRect();return el.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2));})).toBe(true);
  }
  await page.screenshot({path:'/tmp/pr458-sidebar-mobile-closed.png'});
  await expand.click();
  await page.setViewportSize({width:1440,height:1000});
  await expect(frame).not.toHaveAttribute('data-sidebar-overlay');
  await expect(center).not.toHaveAttribute('inert','');
  await message.click(); await expect(message).toBeFocused();
  await page.setViewportSize({width:390,height:844});
  await expect(expand).toBeVisible();
  await expect(center).not.toHaveAttribute('inert','');
  await message.click(); await expect(message).toBeFocused();
  expect(errors).toEqual([]);
});

test('stream publications and a turn-local clock preserve chrome identity, geometry and caret', async ({ page }) => {
  await page.clock.install({ time: new Date('2026-09-24T23:00:00Z') });
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
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
  await expect(page.locator('[data-chat-running]').getByText('Deep diving for 5s ···', { exact: true })).toBeVisible();
  expect({ header: await header!.boundingBox(), card: await card.boundingBox() }).toEqual(geometry);
  await expect(input).toBeFocused(); await expect(input).toHaveValue('Keep this draft');
  expect(await input.evaluate((node: HTMLTextAreaElement) => [node.selectionStart, node.selectionEnd])).toEqual([3, 7]);
});
for (const mode of ['empty', 'preview', 'named', 'delete', 'other-uncertain', 'background'] as const) test(`Sidebar-only Session surface: ${mode}`, async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  if (mode === 'other-uncertain') await openClassifiedShell(page, mode);
  else await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  if (mode !== 'background' && mode !== 'other-uncertain') await page.evaluate(mode => window.sessionFixture.presentation(mode), mode);
  await expect(page.getByRole('tree', { name: 'Session browser' })).toHaveCount(1);
  await expect(page.getByRole('tablist')).toHaveCount(1);
  if (mode === 'empty') {
    await expect(page.getByLabel('Session title')).toHaveText('A');
    await expect(page.locator('button[data-session-id="A"]')).toHaveAttribute('aria-label', 'Open A');
  }
  if (mode === 'background') {
    await page.locator('button[data-session-id="B"]').click();
    await expect(page.getByLabel('Session title')).toHaveText('Session B');
    await expect(page.locator('button[data-session-id="A"]')).not.toContainText('Working…');
  }
  if (mode === 'other-uncertain') {
    await expect(page.getByLabel('Session status')).toHaveCount(0);
    await expect(page.getByRole('status').filter({ hasText: 'Deep diving' }).first()).toBeVisible();
    await expect(page.locator('button[data-session-id="B"]')).toContainText('Needs verification');
  }
  if (mode === 'delete') {
    await page.locator('button[data-session-id="A"]').hover();
    await page.locator('button[data-session-actions="A"]').click();
    await page.getByRole('menuitem', { name: 'Delete Session' }).click();
    await expect(page.getByRole('dialog', { name: 'Delete this session?' })).toContainText('“Inspect the Session ownership boundary”');
  }
  await expectStableScreenshot(page, `sidebar-${mode}-light.png`);

  expect(errors).toEqual([]);
});
// Intentional updates in the pinned browser environment: pnpm test:e2e:update
// Fixed time, native-protocol fixture and reduced motion keep evidence reviewable.
test('Harness shell reference states and presentation-only navigation', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await openClassifiedShell(page);
  await expect(page).toHaveTitle('rustX shell reference');
  await expect(page.getByLabel('Sidebar', { exact: true })).toContainText('rustX');
  await expect(page.locator('body')).not.toContainText('DeepSeek');
  await expect(page.locator('img[src*="deepseek" i]')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Open Session A', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Open Session A', exact: true })).toHaveAttribute('aria-current', 'page');
  await expect(page.getByRole('tree', { name: 'Session browser' })).toContainText('Waiting for approval');
  await expect(page.getByRole('tree', { name: 'Session browser' })).toContainText('Running');
  await expectStableScreenshot(page, 'desktop-expanded-light.png');
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
  await page.getByRole('group', { name: 'Appearance', exact: true }).getByRole('button', { name: 'Dark', exact: true }).click();
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

test('Session product states stay concise without developer diagnostics', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  for (const mode of ['idle', 'queued', 'stopping', 'reconnect', 'uncertain'] as const) {
    await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
    await expect(page.getByLabel('Session status')).toHaveCount(0);
    await expect(page.getByRole('status').filter({ hasText: 'Deep diving' }).first()).toBeVisible();
    await page.evaluate(mode => window.sessionFixture.state(mode), mode);
    const expected = { idle: undefined, queued: 'Queued', stopping: 'Stopping…', reconnect: 'Connection interrupted', uncertain: 'Needs verification' }[mode];
    if (expected) await expect(page.getByLabel('Session status')).toContainText(expected);
    else await expect(page.getByLabel('Session status')).toHaveCount(0);
    const ordinary = await page.locator('main').innerText();
    expect(ordinary).not.toMatch(/attempt-A|runtime_incarnation|connection_generation|Attach \/ cold resume|Unload runtime|Detach|Resync/);
    if (mode === 'reconnect' || mode === 'uncertain') {
      await expect(page.locator('[data-workspace-group="project"] button[data-session-id="A"]')).toBeVisible();
      await expect(page.getByRole('status').filter({ hasText: 'Disconnected. Last confirmed groups are shown.' })).toBeVisible();
    }
    await expectStableScreenshot(page, `session-${mode}-light.png`);
    await expect(page.getByRole('button', { name: /Inspector/ })).toHaveCount(0);
    await expect(page.getByText('More conversation details', { exact: true })).toHaveCount(0);
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
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
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

test('sidebar recency updates grouped, flat and search rows while preserving the composer', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
  const tree = page.getByRole('tree', { name: 'Session browser' });
  const rows = tree.locator('button[data-session-id]:visible');
  const order = () => rows.evaluateAll(nodes => nodes.map(node => node.getAttribute('data-session-id')));
  const input = page.getByRole('textbox', { name: 'Message', exact: true });
  await input.fill('Keep my draft');
  await input.focus();
  await page.evaluate(() => window.sessionFixture.activity('B', '2026-10-07T12:00:00Z'));
  await expect.poll(order).toEqual(['B', 'A']);
  await expect(input).toHaveValue('Keep my draft');
  await expect(input).toBeFocused();
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await page.getByRole('button', { name: 'View options', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Flat view', exact: true }).click();
  await expect.poll(order).toEqual(['B', 'A', 'C']);
  await page.evaluate(() => window.sessionFixture.activity('A', '2026-10-07T13:00:00Z'));
  await expect.poll(order).toEqual(['A', 'B', 'C']);
  await page.getByRole('button', { name: 'Search Sessions', exact: true }).click();
  await page.getByRole('textbox', { name: 'Search Session metadata' }).fill('Session');
  await expect.poll(order).toEqual(['A', 'B', 'C']);
});


test('primary sidebar navigation retires the narrow overlay without changing runtime ownership', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
  const frame = page.locator('[data-harness-frame]'), center = page.locator('main');
  const expand = page.getByRole('button', { name: 'Expand Sidebar', exact: true });
  const composer = page.getByRole('textbox', { name: 'Message', exact: true });
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await page.evaluate(() => window.sessionFixture.readyNavigation());
  const before = await page.evaluate(() => window.sessionFixture.requests().filter(request => /^(session\/(attach|detach|switchNode|unload|create)|turn\/)/.test(request.method)));
  for (const destination of ['session', 'new', 'workspace'] as const) {
    await expand.click();
    await expect(frame).toHaveAttribute('data-sidebar-overlay', 'true');
    await expect(center).toHaveAttribute('inert', '');
    if (destination === 'session') {
      await page.locator('button[data-session-id="B"]').click();
      await expect(page.getByLabel('Session title')).toHaveText('Session B');
    } else if (destination === 'new') await page.getByRole('button', { name: 'New Conversation', exact: true }).first().click();
    else await page.getByRole('button', { name: 'Select Workspace rustX', exact: true }).click();
    await expect(frame).not.toHaveAttribute('data-sidebar-overlay');
    await expect(center).not.toHaveAttribute('inert', '');
    await expect(page.getByRole('button', { name: 'Close navigation' })).toHaveCount(0);
    await expect(expand).toBeFocused();
    await composer.fill(`Destination ${destination}`); await expect(composer).toBeFocused();
    expect(await composer.evaluate(el => el.closest('[inert]') === null)).toBe(true);
  }
  expect(await page.evaluate(() => window.sessionFixture.requests().filter(request => /^(session\/(attach|detach|switchNode|unload|create)|turn\/)/.test(request.method)))).toEqual(before);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.locator('button[data-session-id="A"]').click();
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await expect(page.getByRole('button', { name: 'Collapse Sidebar', exact: true })).toBeVisible();
  await expect(frame).not.toHaveAttribute('data-sidebar-collapsed');
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(frame).not.toHaveAttribute('data-sidebar-overlay');
  await composer.click(); await expect(composer).toBeFocused();
  expect(errors).toEqual([]);
});


test('idle overlay dismissal preserves the committed conversation navigation fence', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  const before = await page.evaluate(() => window.sessionFixture.navigationFacts());
  for (const dismissal of ['escape', 'backdrop', 'toggle'] as const) {
    await page.evaluate(() => window.sessionFixture.captureNavigation());
    await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
    const collapse = page.getByRole('button', { name: 'Collapse Sidebar', exact: true });
    if (dismissal === 'escape') await collapse.press('Escape');
    else if (dismissal === 'backdrop') await page.getByRole('button', { name: 'Close navigation', exact: true }).click({ position: { x: 370, y: 400 } });
    else await collapse.click();
    await expect(page.locator('[data-harness-frame]')).not.toHaveAttribute('data-sidebar-overlay');
    expect(await page.evaluate(() => window.sessionFixture.navigationCurrent())).toBe(true);
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeEnabled();
    expect(await page.evaluate(() => window.sessionFixture.navigationFacts())).toEqual(before);
  }
});

test('authoritative narrow navigation commits while a background snapshot is held', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await page.evaluate(() => window.sessionFixture.readyNavigation());
  const before = await page.evaluate(() => window.sessionFixture.navigationFacts());
  expect(before.views.A.authoritative && before.views.B.authoritative).toBe(true);
  const mutations = nativeMutations(await page.evaluate(() => window.sessionFixture.requests()));
  await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
  await page.evaluate(() => window.sessionFixture.holdBackgroundCheckpoint());
  const snapshots = (await page.evaluate(() => window.sessionFixture.requests())).filter(request => request.method === 'session/checkpoint').length;
  await page.locator('button[data-session-id="B"]').click();
  await expect(page.getByLabel('Session title')).toHaveText('Session B');
  await expect(page.locator('[data-harness-frame]')).not.toHaveAttribute('data-sidebar-overlay');
  await expect(page.locator('main')).not.toHaveAttribute('inert');
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Interactive before snapshot settlement');
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeFocused();
  expect((await page.evaluate(() => window.sessionFixture.requests())).filter(request => request.method === 'session/checkpoint').length).toBe(snapshots);
  expect(nativeMutations(await page.evaluate(() => window.sessionFixture.requests()))).toEqual(mutations);
  expect(await page.evaluate(() => window.sessionFixture.navigationFacts())).toEqual(before);
  await page.evaluate(() => window.sessionFixture.settleSnapshot());
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await expect(page.getByLabel('Session title')).toHaveText('Session B');
  await expect(page.locator('[data-harness-frame]')).not.toHaveAttribute('data-sidebar-overlay');
});

const nativeMutations = (requests: import('../../../protocol/app-server/v57').Request[]) => requests.filter(request => /^(session\/(attach|detach|switchNode|unload)|turn\/)/.test(request.method));

for (const outcome of ['escape', 'backdrop', 'toggle', 'newer', 'success', 'failed', 'replacement', 'breakpoint', 'desktop'] as const) test(`sidebar attachment ownership: ${outcome}`, async ({ page }) => {
  await page.setViewportSize({ width: outcome === 'desktop' ? 1440 : 390, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
  const frame = page.locator('[data-harness-frame]');
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await page.evaluate(() => window.sessionFixture.readyNavigation());
  const before = await page.evaluate(() => window.sessionFixture.navigationFacts());
  const mutations = nativeMutations(await page.evaluate(() => window.sessionFixture.requests()));
  if (outcome !== 'desktop') await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
  await page.getByText('Sessions outside registered Workspaces', { exact: true }).click();
  await page.evaluate(() => window.sessionFixture.holdAttach());
  if (outcome === 'success') await frame.evaluate(element => {
    const frame = element as HTMLElement;
    frame.dataset.observedClosures = '0';
    new MutationObserver(records => {
      for (const record of records) if (record.oldValue === 'true' && !frame.hasAttribute('data-sidebar-overlay')) frame.dataset.observedClosures = String(Number(frame.dataset.observedClosures) + 1);
    }).observe(frame, { attributes: true, attributeOldValue: true, attributeFilter: ['data-sidebar-overlay'] });
  });
  await page.locator('button[data-session-id="C"]').click();
  await expect.poll(() => page.evaluate(() => window.sessionFixture.requests().filter(request => request.method === 'session/attach' && request.params.session_id === 'C').length)).toBe(1);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  expect((await page.evaluate(() => window.sessionFixture.navigationFacts())).views.C.attachment).toBe('attaching');
  if (outcome === 'escape') await page.locator('button[data-session-id="C"]').press('Escape');
  if (outcome === 'backdrop') await page.getByRole('button', { name: 'Close navigation', exact: true }).click({ position: { x: 370, y: 400 } });
  if (outcome === 'toggle') await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
  if (outcome === 'breakpoint') { await page.setViewportSize({ width: 1440, height: 844 }); await expect(frame).not.toHaveAttribute('data-sidebar-overlay'); await page.setViewportSize({ width: 390, height: 844 }); }
  if (outcome === 'newer') {
    await page.locator('button[data-session-id="B"]').click();
    await expect(page.getByLabel('Session title')).toHaveText('Session B');
    await expect(frame).not.toHaveAttribute('data-sidebar-overlay');
  }
  if (outcome === 'replacement') { await page.evaluate(() => window.sessionFixture.replaceConnection()); await expect.poll(() => page.evaluate(() => window.sessionFixture.navigationFacts().generation)).not.toBe(before.generation); }
  await page.evaluate(success => window.sessionFixture.settleAttach(success), outcome !== 'failed');
  if (outcome === 'failed') {
    await expect.poll(() => page.evaluate(() => window.sessionFixture.navigationFacts().views.C.attachment)).toBe('error');
    await expect(page.getByRole('alert').filter({ hasText: 'Navigation fixture rejected' })).toBeVisible();
  } else if (outcome === 'replacement') {
    // Transport replacement leaves a dispatched lost acknowledgement uncertain;
    // it must not replay the attach to manufacture fresh presentation authority.
    const after = await page.evaluate(() => window.sessionFixture.navigationFacts());
    expect(after.views.C.authoritative).toBe(false);
    expect(after.views.C.target).toBeUndefined();
    expect(after.claims.some(target => target.session_id === 'C')).toBe(false);
    expect((await page.evaluate(() => window.sessionFixture.requests())).filter(request => request.method === 'session/attach' && request.params.session_id === 'C')).toHaveLength(1);
  } else {
    await expect.poll(() => page.evaluate(() => window.sessionFixture.navigationFacts().views.C.authoritative)).toBe(true);
    expect((await page.evaluate(() => window.sessionFixture.navigationFacts())).claims.some(target => target.session_id === 'C')).toBe(true);
  }
  const destination = outcome === 'newer' ? 'B' : outcome === 'success' || outcome === 'desktop' ? 'C' : 'A';
  await expect(page.getByLabel('Session title')).toHaveText(`Session ${destination}`);
  if (outcome === 'failed' || outcome === 'replacement') await expect(frame).toHaveAttribute('data-sidebar-overlay', 'true');
  else await expect(frame).not.toHaveAttribute('data-sidebar-overlay');
  if (outcome === 'success') await expect(frame).toHaveAttribute('data-observed-closures', '1');
  if (outcome !== 'replacement') {
    const after = await page.evaluate(() => window.sessionFixture.navigationFacts());
    expect(after.views.A).toEqual(before.views.A); expect(after.views.B).toEqual(before.views.B);
    const added = nativeMutations(await page.evaluate(() => window.sessionFixture.requests())).slice(mutations.length);
    expect(added.map(request => request.method)).toEqual(['session/attach']);
  }
  if (outcome === 'desktop') await expect(page.getByRole('button', { name: 'Collapse Sidebar', exact: true })).toBeVisible();
  if (destination === 'A') await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeEnabled();
  if (outcome !== 'failed' && outcome !== 'replacement') {
    await expect(page.locator('main')).not.toHaveAttribute('inert');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Destination remains usable');
  }
});

test('a stale retained attachment waits for authoritative reconciliation before narrow navigation', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await page.evaluate(() => window.sessionFixture.staleDestination());
  expect((await page.evaluate(() => window.sessionFixture.navigationFacts())).views.B.authoritative).toBe(false);
  await page.evaluate(() => window.sessionFixture.holdSnapshot());
  await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
  await page.locator('button[data-session-id="B"]').click();
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await expect(page.locator('[data-harness-frame]')).toHaveAttribute('data-sidebar-overlay', 'true');
  await page.evaluate(() => window.sessionFixture.settleSnapshot());
  await expect(page.getByLabel('Session title')).toHaveText('Session B');
  expect((await page.evaluate(() => window.sessionFixture.navigationFacts())).views.B.authoritative).toBe(true);
  await expect(page.locator('[data-harness-frame]')).not.toHaveAttribute('data-sidebar-overlay');
});

async function openFixtureSettings(page: Page, workspace: boolean) {
  const expand = page.getByRole('button', { name: 'Expand Sidebar', exact: true });
  if (await expand.isVisible()) await expand.click();
  if (workspace) await openWorkspaceSettings(page, 'rustX');
  else await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  await expect(settings).toBeVisible();
  await expect(settings.getByRole('heading', { name: workspace ? 'Workspace Settings — rustX' : 'User Settings', exact: true })).toBeVisible();
  if (workspace) expect((await page.evaluate(() => window.sessionFixture.settingsTargets())).at(-1)).toBe('project');
  return settings;
}

for (const scenario of ['global', 'workspace', 'global-refusal', 'workspace-refusal', 'competing-global', 'competing-workspace', 'desktop-global', 'desktop-workspace'] as const) test(`Settings supersedes sidebar intent: ${scenario}`, async ({ page }) => {
  const desktop = scenario.startsWith('desktop'), competing = scenario.startsWith('competing'), refused = scenario.endsWith('refusal');
  const workspace = scenario.includes('workspace');
  await page.setViewportSize({ width: desktop ? 1440 : 390, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await page.evaluate(() => window.sessionFixture.readyNavigation());
  const before = await page.evaluate(() => window.sessionFixture.navigationFacts());
  const mutations = nativeMutations(await page.evaluate(() => window.sessionFixture.requests()));
  if (!desktop) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
  await page.getByText('Sessions outside registered Workspaces', { exact: true }).click();
  await page.evaluate(() => window.sessionFixture.holdAttach());
  await page.locator('button[data-session-id="C"]').click();
  await expect.poll(() => page.evaluate(() => window.sessionFixture.requests().filter(request => request.method === 'session/attach' && request.params.session_id === 'C').length)).toBe(1);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  expect((await page.evaluate(() => window.sessionFixture.navigationFacts())).views.C.attachment).toBe('attaching');
  if (competing) {
    await page.locator('button[data-session-id="B"]').click();
    await expect(page.getByLabel('Session title')).toHaveText('Session B');
  }
  const settings = await openFixtureSettings(page, workspace);
  const frame = page.locator('[data-harness-frame]');
  await expect(frame).not.toHaveAttribute('data-sidebar-overlay');
  await page.evaluate(success => window.sessionFixture.settleAttach(success), !refused);
  // Wait for the lifecycle-owned Open to settle, not merely its attachment ACK.
  await page.evaluate(() => window.sessionFixture.waitForNavigation());
  const after = await page.evaluate(() => window.sessionFixture.navigationFacts());
  // A known refusal of an obsolete Open settles absent, without a stale error.
  expect(after.views.C.attachment).toBe(refused ? 'detached' : 'attached');
  expect(after.views.C.authoritative).toBe(!refused);
  expect(after.claims.some(target => target.session_id === 'C')).toBe(!refused);
  expect(after.views.A).toEqual(before.views.A); expect(after.views.B).toEqual(before.views.B);
  expect(nativeMutations(await page.evaluate(() => window.sessionFixture.requests())).slice(mutations.length).map(request => request.method)).toEqual(['session/attach']);
  await expect(settings).toBeVisible();
  await expect(settings.getByRole('heading', { name: workspace ? 'Workspace Settings — rustX' : 'User Settings', exact: true })).toBeVisible();
  await expect(page.getByRole('alert').filter({ hasText: 'Navigation fixture rejected' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Close Settings', exact: true }).click();
  await expect(settings).toHaveCount(0);
  await expect(page.getByLabel('Session title')).toHaveText(`Session ${competing ? 'B' : 'A'}`);
  await expect(frame).not.toHaveAttribute('data-sidebar-overlay');
  await expect(page.locator('main')).not.toHaveAttribute('inert');
  if (!desktop && !competing) await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeEnabled();
  if (desktop) await expect(page.getByRole('button', { name: 'Collapse Sidebar', exact: true })).toBeVisible();
});

for (const width of [390, 1440]) for (const destination of ['A', 'B']) test(`idle Settings preserves completed ${destination} navigation at ${width}px`, async ({ page }) => {
  await page.setViewportSize({ width, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html`);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await page.evaluate(() => window.sessionFixture.readyNavigation());
  if (destination === 'B') {
    if (width === 390) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
    await page.locator('button[data-session-id="B"]').click();
    await expect(page.getByLabel('Session title')).toHaveText('Session B');
  }
  const before = await page.evaluate(() => window.sessionFixture.navigationFacts());
  const mutations = nativeMutations(await page.evaluate(() => window.sessionFixture.requests()));
  const snapshots = (await page.evaluate(() => window.sessionFixture.requests())).filter(request => request.method === 'session/checkpoint').length;
  await page.evaluate(() => window.sessionFixture.captureNavigation());
  for (const workspace of [false, true]) {
    await openFixtureSettings(page, workspace);
    expect(await page.evaluate(() => window.sessionFixture.navigationCurrent())).toBe(true);
    await page.getByRole('button', { name: 'Close Settings', exact: true }).click();
    await expect(page.getByLabel('Session title')).toHaveText(`Session ${destination}`);
    expect(await page.evaluate(() => window.sessionFixture.navigationCurrent())).toBe(true);
    if (destination === 'A') await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeEnabled();
  }
  expect(await page.evaluate(() => window.sessionFixture.navigationFacts())).toEqual(before);
  expect(nativeMutations(await page.evaluate(() => window.sessionFixture.requests()))).toEqual(mutations);
  expect((await page.evaluate(() => window.sessionFixture.requests())).filter(request => request.method === 'session/checkpoint').length).toBe(snapshots);
  if (width === 1440) await expect(page.getByRole('button', { name: 'Collapse Sidebar', exact: true })).toBeVisible();
});

async function openForkOwnershipFixture(page: Page, width = 390) {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html?fork`);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await expect(page.getByRole('button', { name: 'View source', exact: true })).toBeEnabled();
  await page.evaluate(() => window.sessionFixture.readyNavigation());
  const before = await page.evaluate(() => window.sessionFixture.navigationFacts());
  const requests = await page.evaluate(() => window.sessionFixture.requests());
  await page.evaluate(() => window.sessionFixture.captureNavigation());
  return { before, requests, errors };
}
async function startHeldFork(page: Page, stage: 'node' | 'history') {
  const before = await page.evaluate(stage => window.sessionFixture.requests().filter(request => request.method === (stage === 'node' ? 'session/node' : 'session/history') && 'session_id' in request.params && request.params.session_id === 'C').length, stage);
  await page.evaluate(stage => { if (stage === 'node') window.sessionFixture.holdNode(); else window.sessionFixture.holdHistory(); }, stage);
  await page.getByRole('button', { name: 'View source', exact: true }).click();
  await expect.poll(() => page.evaluate(stage => window.sessionFixture.requests().filter(request => request.method === (stage === 'node' ? 'session/node' : 'session/history') && 'session_id' in request.params && request.params.session_id === 'C').length, stage)).toBe(before + 1);
}
async function proveCommittedControls(page: Page) {
  expect(await page.evaluate(() => window.sessionFixture.navigationCurrent())).toBe(true);
  const target = (await page.evaluate(() => window.sessionFixture.navigationFacts())).views.A.target;
  const stop = page.getByRole('button', { name: 'Stop', exact: true });
  await expect(stop).toBeEnabled(); await stop.click();
  await expect.poll(() => page.evaluate(() => window.sessionFixture.requests().filter(request => request.method === 'turn/cancel').length)).toBe(1);
  const cancellation = (await page.evaluate(() => window.sessionFixture.requests())).filter(request => request.method === 'turn/cancel');
  expect(cancellation[0].params).toEqual({ target });
  // Both new uploads and reconciliation use ConversationSeat's committed fence.
  await page.evaluate(() => window.sessionFixture.uncertainUpload());
  await page.getByLabel('Attach files', { exact: true }).setInputFiles({ name: 'owned.txt', mimeType: 'text/plain', buffer: Buffer.from('committed A') });
  const reconcile = page.getByRole('button', { name: /^Check status for / });
  await expect(reconcile).toBeVisible();
  await page.evaluate(() => window.sessionFixture.resolveUpload());
  await reconcile.click(); await expect(page.locator('[data-attachment-state=ready]')).toBeVisible();
  const requests = await page.evaluate(() => window.sessionFixture.requests());
  expect(requests.filter(request => request.method === 'session/uploadPrepare')).toHaveLength(1);
  expect(requests.filter(request => request.method === 'session/uploadStatus').every(request => request.params.target.session_id === 'A')).toBe(true);
  await expect(page.getByRole('alert').filter({ hasText: 'Upload view replaced' })).toHaveCount(0);
}

for (const failure of ['node', 'history'] as const) test(`Fork ownership failure preserves committed controls: ${failure}`, async ({ page }) => {
  const { before, requests, errors } = await openForkOwnershipFixture(page);
  await startHeldFork(page, failure);
  expect(await page.evaluate(() => window.sessionFixture.navigationCurrent())).toBe(true);
  if (failure === 'node') await page.evaluate(() => window.sessionFixture.settleNode(false));
  else await page.evaluate(() => window.sessionFixture.settleHistory(false));
  await expect(page.getByRole('alert').filter({ hasText: failure === 'node' ? 'Unknown Node' : 'Navigation fixture rejected' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'View source', exact: true })).toBeEnabled();
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  const after = await page.evaluate(() => window.sessionFixture.navigationFacts());
  expect(after.views.A).toEqual(before.views.A); expect(after.views.B).toEqual(before.views.B);
  const delta = (await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length);
  expect(nativeMutations(delta).map(request => request.method)).toEqual([]);
  expect(delta.filter(request => request.method === 'session/checkpoint')).toHaveLength(0);
  await proveCommittedControls(page); expect(errors).toEqual([]);
});

for (const workspace of [false, true]) for (const stage of ['node', 'history'] as const) for (const success of [false, true]) test(`Settings supersedes Fork ownership: ${workspace ? 'workspace' : 'user'} / ${stage} / ${success ? 'success' : 'failure'}`, async ({ page }) => {
  const { before, requests, errors } = await openForkOwnershipFixture(page);
  await startHeldFork(page, stage);
  const settings = await openFixtureSettings(page, workspace);
  expect(await page.evaluate(() => window.sessionFixture.navigationCurrent())).toBe(true);
  await expect(page.locator('[data-harness-frame]')).not.toHaveAttribute('data-sidebar-overlay');
  if (stage === 'node') await page.evaluate(success => window.sessionFixture.settleNode(success), success);
  else await page.evaluate(success => window.sessionFixture.settleHistory(success), success);
  await expect(settings).toBeVisible();
  await expect(settings.getByRole('heading', { name: workspace ? 'Workspace Settings — rustX' : 'User Settings', exact: true })).toBeVisible();
  await expect(page.getByRole('alert').filter({ hasText: /Unknown Node|Navigation fixture rejected/ })).toHaveCount(0);
  const after = await page.evaluate(() => window.sessionFixture.navigationFacts());
  expect(after.views.A).toEqual(before.views.A); expect(after.views.B).toEqual(before.views.B);
  const delta = (await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length);
  expect(nativeMutations(delta).map(request => request.method)).toEqual([]);
  expect(after.views.C).toBeUndefined();
  expect(delta.filter(request => request.method === 'session/checkpoint' && request.params.target.session_id === 'A')).toHaveLength(0);
  await page.getByRole('button', { name: 'Close Settings', exact: true }).click();
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await proveCommittedControls(page); expect(errors).toEqual([]);
});

for (const width of [390, 1440]) test(`idle Settings keeps committed Stop and upload controls: ${width}px`, async ({ page }) => {
  const { before, requests, errors } = await openForkOwnershipFixture(page, width);
  for (const workspace of [false, true]) {
    await openFixtureSettings(page, workspace);
    expect(await page.evaluate(() => window.sessionFixture.navigationCurrent())).toBe(true);
    await page.getByRole('button', { name: 'Close Settings', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeEnabled();
  }
  expect(await page.evaluate(() => window.sessionFixture.navigationFacts())).toEqual(before);
  const delta = (await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length);
  expect(nativeMutations(delta)).toEqual([]); expect(delta.filter(request => request.method === 'session/checkpoint')).toEqual([]);
  await proveCommittedControls(page); expect(errors).toEqual([]);
});

test('obsolete Fork failure cannot retire a newer pending sidebar owner', async ({ page }) => {
  await openForkOwnershipFixture(page);
  await startHeldFork(page, 'node');
  await page.evaluate(() => { window.sessionFixture.allowNodeReads(); window.sessionFixture.holdAttach(); });
  await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
  await page.getByText('Sessions outside registered Workspaces', { exact: true }).click();
  await page.locator('button[data-session-id="C"]').click();
  await expect.poll(() => page.evaluate(() => window.sessionFixture.requests().filter(request => request.method === 'session/attach' && request.params.session_id === 'C').length)).toBe(1);
  await page.evaluate(() => window.sessionFixture.settleNode(false));
  await expect(page.getByRole('alert').filter({ hasText: 'Unknown Node' })).toHaveCount(0);
  await page.evaluate(() => window.sessionFixture.settleAttach(true)); await page.evaluate(() => window.sessionFixture.waitForNavigation());
  await expect(page.getByLabel('Session title')).toHaveText('Session C');
  await expect(page.locator('[data-harness-frame]')).not.toHaveAttribute('data-sidebar-overlay');
  expect((await page.evaluate(() => window.sessionFixture.navigationFacts())).views.C.authoritative).toBe(true);
});

test('successful View Source commits read-only exact Node and Message while preserving executable claims', async ({ page }) => {
  const { before, requests, errors } = await openForkOwnershipFixture(page);
  await startHeldFork(page, 'history'); await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await page.evaluate(() => window.sessionFixture.settleHistory(true));
  const history = page.locator('[data-history-session="C"]');
  await expect(history).toHaveAttribute('data-history-node', 'source-node-C'); await expect(history).toHaveAttribute('data-history-message', 'source-message');
  await expect(page.getByRole('heading', { name: 'Conversation history', exact: true })).toBeVisible();
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toHaveCount(0);
  const after = await page.evaluate(() => window.sessionFixture.navigationFacts()); expect(after).toEqual(before);
  const delta = (await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length); expect(nativeMutations(delta)).toEqual([]);
  expect(delta.filter(request => request.method === 'session/history').map(request => request.params)).toEqual([{ session_id: 'C', node_id: 'source-node-C', at: { type: 'message', id: 'source-message' }, limit: 32 }]);
  await page.getByRole('button', { name: 'Return to conversation', exact: true }).click();
  await expect(page.getByLabel('Session title')).toHaveText('Session A'); await proveCommittedControls(page); expect(errors).toEqual([]);
});

for (const width of [390, 1440]) test(`catalog search owns preparation without revoking committed controls: ${width}px`, async ({ page }) => {
  const { before, requests, errors } = await openForkOwnershipFixture(page, width);
  if (width === 390) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
  await page.getByRole('button', { name: 'Search Sessions', exact: true }).click();
  await page.evaluate(() => window.sessionFixture.holdSearch());
  const search = page.getByLabel('Search Session metadata', { exact: true });
  for (const query of ['Session B', 'Session A']) {
    await search.fill(query);
    await expect.poll(() => page.evaluate(query => window.sessionFixture.requests().filter(request => request.method === 'session/list' && request.params.query === query).length, query)).toBe(1);
    expect(await page.evaluate(() => window.sessionFixture.navigationCurrent())).toBe(true);
  }
  // The latest result commits first; obsolete completion must not revoke the
  // conversation fence or replace either the catalog page or selected route.
  await page.evaluate(() => window.sessionFixture.settleSearch('Session A'));
  await expect(page.locator('button[data-session-id="A"]')).toHaveCount(1);
  await expect(page.locator('button[data-session-id="B"]')).toHaveCount(0);
  await page.evaluate(() => window.sessionFixture.settleSearch('Session B'));
  await expect(page.locator('button[data-session-id="B"]')).toHaveCount(0);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  if (width === 390) await page.getByRole('button', { name: 'Collapse Sidebar', exact: true }).click();
  expect(await page.evaluate(() => window.sessionFixture.navigationFacts())).toEqual(before);
  const delta = (await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length);
  expect(nativeMutations(delta)).toEqual([]); expect(delta.filter(request => request.method === 'session/checkpoint')).toEqual([]);
  await proveCommittedControls(page); expect(errors).toEqual([]);
});

for (const width of [390, 1440]) for (const back of ['return', 'sidebar'] as const) test(`read-only visits via ${back} retain editor text, selection and attachments without reauthorizing old controls: ${width}px`, async ({ page }) => {
  const { before, errors } = await openForkOwnershipFixture(page, width);
  const editor = page.getByRole('textbox', { name: 'Message', exact: true });
  const text = 'Unsent source inspection draft\nsecond line';
  await editor.fill(text);
  await page.getByLabel('Attach files', { exact: true }).setInputFiles([
    { name: 'one.txt', mimeType: 'text/plain', buffer: Buffer.from('one') },
    { name: 'two.txt', mimeType: 'text/plain', buffer: Buffer.from('two') },
  ]);
  await expect(page.locator('[data-attachment-state=ready]')).toHaveCount(2);
  await editor.evaluate((element: HTMLTextAreaElement) => { element.focus(); element.setSelectionRange(7, 12, 'backward'); element.dispatchEvent(new Event('select', { bubbles: true })); });
  const requests = await page.evaluate(() => window.sessionFixture.requests());
  const editorIdentity = await page.locator('[data-editor-incarnation]').evaluate(element => [element.getAttribute('data-editor-incarnation'), element.getAttribute('data-editor-revision')]);
  for (let visit = 0; visit < 2; visit++) {
    await startHeldFork(page, 'history'); await page.evaluate(() => window.sessionFixture.settleHistory(true));
    const anchor = page.locator('[data-history-session="C"] [data-chat-anchor-key="message:source-message"]');
    await expect(anchor).toBeVisible();
    await expect.poll(() => anchor.evaluate(element => { const port = element.closest('[data-conversation-scroll]')!; const a = element.getBoundingClientRect(), p = port.getBoundingClientRect(); return a.top >= p.top - 1 && a.top < p.bottom && a.bottom > p.top; })).toBe(true);
    await expect(editor).toHaveCount(0);
    if (back === 'sidebar') {
      if (width === 390) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
      await page.getByRole('button', { name: 'Open Session A', exact: true }).click();
      await expect(page.locator('[data-sidebar-overlay]')).toHaveCount(0); await expect(page.locator('main')).not.toHaveAttribute('inert');
    } else await page.getByRole('button', { name: 'Return to conversation', exact: true }).click();
    expect(await page.locator('[data-editor-incarnation]').evaluate(element => [element.getAttribute('data-editor-incarnation'), element.getAttribute('data-editor-revision')])).toEqual(editorIdentity);
    await expect(page.getByLabel('Session title')).toHaveText('Session A'); await expect(editor).toHaveValue(text); await expect(editor).toBeEditable();
    expect(await editor.evaluate((element: HTMLTextAreaElement) => [element.selectionStart, element.selectionEnd, element.selectionDirection])).toEqual([7, 12, 'backward']);
    await expect(page.getByText('one.txt', { exact: true })).toBeVisible(); await expect(page.getByText('two.txt', { exact: true })).toBeVisible();
    await expect(page.locator('[data-attachment-state=ready]')).toHaveCount(2);
    expect(await page.evaluate(() => window.sessionFixture.navigationFacts())).toEqual(before);
  }
  const delta = (await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length);
  expect(nativeMutations(delta)).toEqual([]); expect(delta.filter(request => ['session/checkpoint', 'session/uploadPrepare', 'session/uploadStatus'].includes(request.method))).toEqual([]);
  await page.screenshot({ path: `/tmp/pr458-history-draft-${back}-${width}.png` });
  await editor.focus(); await editor.press('Escape'); await editor.press('Escape');
  await expect.poll(() => page.evaluate(() => window.sessionFixture.requests().filter(request => request.method === 'turn/cancel').length)).toBe(1);
  expect(errors).toEqual([]);
});

test('View Source installs cancellation in the click stack before the next user scroll', async ({ page }) => {
  const { requests, errors } = await openForkOwnershipFixture(page, 1440);
  await page.setViewportSize({ width: 1440, height: 300 });
  await page.evaluate(() => window.sessionFixture.stream(Array.from({ length: 80 }, (_, i) => `Reading line ${i}`).join('\n\n')));
  await expect(page.getByText('Reading line 79', { exact: true })).toHaveCount(1);
  await page.evaluate(() => {
    window.sessionFixture.holdHistory();
    document.addEventListener('click', function immediatelyScroll(event) {
      const button = (event.target as Element).closest('button');
      if (button?.textContent?.trim() !== 'View source') return;
      document.removeEventListener('click', immediatelyScroll);
      const port = document.querySelector<HTMLElement>('[data-conversation-scroll]')!;
      if (port.scrollHeight <= port.clientHeight) throw new Error('Regression requires a real scrollable viewport');
      // Runs after React's delegated click handler, still in the native click
      // dispatch stack; no animation frame or later event establishes ownership.
      port.scrollTop = port.scrollTop > 20 ? port.scrollTop - 20 : 20;
      port.dispatchEvent(new Event('scroll'));
    });
  });
  await page.getByRole('button', { name: 'View source', exact: true }).click();
  await page.evaluate(() => window.sessionFixture.wireBarrier());
  await expect(page.locator('[data-shell-route]')).toHaveAttribute('data-shell-route', 'session');
  await expect(page.locator('[data-history-session]')).toHaveCount(0);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  const delta = (await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length);
  // Browser microtask checkpoints between native event listeners may already
  // transmit the read. Its held settlement must remain harmless after cancellation.
  const reads = delta.filter(request => request.method === 'session/history');
  expect(reads.length).toBeLessThanOrEqual(1); expect(nativeMutations(delta)).toEqual([]);
  const port = page.locator('[data-conversation-scroll]'), position = await port.evaluate(element => element.scrollTop);
  if (reads.length) await page.evaluate(() => window.sessionFixture.settleHistory(true));
  await expect(page.locator('[data-history-session]')).toHaveCount(0);
  expect(await port.evaluate(element => element.scrollTop)).toBe(position);
  await expect(page.getByRole('alert')).toHaveCount(0); expect(errors).toEqual([]);
});

for (const beforeReturn of [false, true]) test(`known input acceptance settles its revision during history beforeReturn=${beforeReturn}`, async ({ page }) => {
  const { before, requests, errors } = await openForkOwnershipFixture(page);
  const editor = page.getByRole('textbox', { name: 'Message', exact: true });
  await editor.fill('Accepted revision R1'); await page.evaluate(() => window.sessionFixture.holdInput()); await editor.press('Enter');
  await expect.poll(() => page.evaluate(() => window.sessionFixture.requests().filter(row => row.method === 'turn/start').length)).toBe(1);
  await startHeldFork(page, 'history'); await page.evaluate(() => window.sessionFixture.settleHistory(true));
  await expect(page.locator('[data-history-session="C"]')).toBeVisible();
  if (beforeReturn) await page.evaluate(() => window.sessionFixture.settleInput());
  await page.getByRole('button', { name: 'Return to conversation', exact: true }).click();
  if (!beforeReturn) await page.evaluate(() => window.sessionFixture.settleInput());
  await expect(editor).toHaveValue(''); await expect(editor).toBeEditable();
  const delta = (await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length);
  expect(nativeMutations(delta).map(row => row.method)).toEqual(['turn/start']);
  expect(await page.evaluate(() => window.sessionFixture.navigationFacts())).toEqual(before);
  expect(errors).toEqual([]);
});
test('Discard creates a stable replacement editor through repeated read-only history', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${fixtureOrigin}/test/fixtures/shell.html?fork&initial=cold`);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  const editor = page.getByRole('textbox', { name: 'Message', exact: true });
  await editor.fill('Discarded First Submission');
  await page.getByLabel('Attach files', { exact: true }).setInputFiles({ name: 'first.txt', mimeType: 'text/plain', buffer: Buffer.from('first') });
  await page.evaluate(() => window.sessionFixture.uncertainUpload()); await editor.press('Enter');
  await expect.poll(() => page.evaluate(() => window.sessionFixture.requests().filter(row => row.method === 'session/attach' && row.params.session_id === 'A').length)).toBe(1);
  await page.evaluate(() => window.sessionFixture.settleAttach(true, 'A'));
  const discard = page.getByRole('button', { name: /Discard/i }); await expect(discard).toBeVisible(); await discard.click();
  await page.evaluate(() => window.sessionFixture.resolveUpload());
  await editor.fill('Stable replacement after Discard');
  await page.getByLabel('Attach files', { exact: true }).setInputFiles({ name: 'replacement.txt', mimeType: 'text/plain', buffer: Buffer.from('replacement') });
  await expect(page.locator('[data-attachment-state=ready]')).toHaveCount(1);
  const incarnation = await page.locator('[data-editor-incarnation]').getAttribute('data-editor-incarnation');
  await editor.evaluate((element: HTMLTextAreaElement) => { element.focus(); element.setSelectionRange(3, 8, 'backward'); element.dispatchEvent(new Event('select', { bubbles: true })); });
  const requests = await page.evaluate(() => window.sessionFixture.requests());
  for (let visit = 0; visit < 2; visit++) {
    await startHeldFork(page, 'history'); await page.evaluate(() => window.sessionFixture.settleHistory(true));
    await expect(page.locator('[data-history-session="C"]')).toBeVisible();
    await page.getByRole('button', { name: 'Return to conversation', exact: true }).click();
    await expect(editor).toHaveValue('Stable replacement after Discard'); await expect(editor).toBeEditable();
    await expect(page.locator('[data-editor-incarnation]')).toHaveAttribute('data-editor-incarnation', incarnation!);
    await expect(page.getByText('replacement.txt', { exact: true })).toBeVisible();
    expect(await editor.evaluate((element: HTMLTextAreaElement) => [element.selectionStart, element.selectionEnd, element.selectionDirection])).toEqual([3, 8, 'backward']);
  }
  expect(nativeMutations((await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length))).toEqual([]);
});

for (const width of [390, 1440]) test(`reselecting the current Session preserves unsent text, caret and file: ${width}px`, async ({ page }) => {
  const { errors } = await openForkOwnershipFixture(page, width);
  const editor = page.getByRole('textbox', { name: 'Message', exact: true });
  await editor.fill('Authored current Session draft');
  await page.getByLabel('Attach files', { exact: true }).setInputFiles({ name: 'retained.txt', mimeType: 'text/plain', buffer: Buffer.from('local') });
  await expect(page.locator('[data-attachment-state=ready]')).toHaveCount(1);
  await editor.evaluate((element: HTMLTextAreaElement) => { element.focus(); element.setSelectionRange(4, 9, 'backward'); element.dispatchEvent(new Event('select', { bubbles: true })); });
  const incarnation = await page.locator('[data-editor-incarnation]').getAttribute('data-editor-incarnation'), revision = await page.locator('[data-editor-revision]').getAttribute('data-editor-revision');
  const requests = await page.evaluate(() => window.sessionFixture.requests()), facts = await page.evaluate(() => window.sessionFixture.navigationFacts());
  for (let click = 0; click < 2; click++) {
    if (width === 390) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
    await page.getByRole('button', { name: 'Open Session A', exact: true }).click();
    await expect(editor).toHaveValue('Authored current Session draft'); await expect(editor).toBeEditable();
    if (width === 390) await expect(page.locator('[data-harness-frame]')).not.toHaveAttribute('data-sidebar-overlay');
  }
  await expect(page.getByText('retained.txt', { exact: true })).toBeVisible();
  await expect(page.locator('[data-editor-incarnation]')).toHaveAttribute('data-editor-incarnation', incarnation!); await expect(page.locator('[data-editor-revision]')).toHaveAttribute('data-editor-revision', revision!);
  expect(await editor.evaluate((element: HTMLTextAreaElement) => [element.selectionStart, element.selectionEnd, element.selectionDirection])).toEqual([4, 9, 'backward']);
  expect(await page.evaluate(() => window.sessionFixture.navigationFacts())).toEqual(facts);
  expect((await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length).filter(row => /^(turn\/|session\/(attach|detach|switchNode|uploadPrepare|snapshot)$)/.test(row.method))).toEqual([]);
  expect(errors).toEqual([]);
});
for (const width of [390, 1440]) test(`a native Fork seed remains authored after destination reselect: ${width}px`, async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message)); page.on('console', item => { if (item.type() === 'error' || item.type() === 'warning') errors.push(item.text()); });
  await page.setViewportSize({ width, height: 900 }); await page.goto(`${fixtureOrigin}/test/fixtures/shell.html?editor-seed`);
  await expect(page.getByLabel('Session title')).toHaveText('Session A');
  await page.getByRole('button', { name: 'Branch into a new Session', exact: true }).click();
  const editor = page.getByRole('textbox', { name: 'Message', exact: true });
  await expect(page.getByLabel('Session title')).toHaveText('Session seed-child'); await expect(editor).toHaveValue('Native restored Fork draft');
  await expect(page.getByText('Native restored upload batch restored-batch', { exact: true })).toBeVisible();
  await editor.fill('Edited native Fork draft'); await editor.evaluate((element: HTMLTextAreaElement) => { element.focus(); element.setSelectionRange(3, 8, 'backward'); element.dispatchEvent(new Event('select', { bubbles: true })); });
  const incarnation = await page.locator('[data-editor-incarnation]').getAttribute('data-editor-incarnation'), revision = await page.locator('[data-editor-revision]').getAttribute('data-editor-revision');
  const requests = await page.evaluate(() => window.sessionFixture.requests()), facts = await page.evaluate(() => window.sessionFixture.navigationFacts());
  for (let click = 0; click < 2; click++) {
    if (width === 390) await page.getByRole('button', { name: 'Expand Sidebar', exact: true }).click();
    await page.getByRole('button', { name: 'Open Session seed-child', exact: true }).click(); await expect(editor).toHaveValue('Edited native Fork draft');
  }
  await expect(page.getByText('Native restored upload batch restored-batch', { exact: true })).toBeVisible();
  await expect(page.locator('[data-editor-incarnation]')).toHaveAttribute('data-editor-incarnation', incarnation!); await expect(page.locator('[data-editor-revision]')).toHaveAttribute('data-editor-revision', revision!);
  expect(await editor.evaluate((element: HTMLTextAreaElement) => [element.selectionStart, element.selectionEnd, element.selectionDirection])).toEqual([3, 8, 'backward']);
  expect(await page.evaluate(() => window.sessionFixture.navigationFacts())).toEqual(facts);
  expect((await page.evaluate(() => window.sessionFixture.requests())).slice(requests.length).filter(row => /^(turn\/|session\/(attach|detach|switchNode|uploadPrepare|snapshot)$)/.test(row.method))).toEqual([]);
  expect(requests.filter(row => row.method === 'session/fork')).toHaveLength(1); expect(errors).toEqual([]);
  await page.screenshot({ path: `/tmp/pr458-seed-reselect-${width}.png` });
});

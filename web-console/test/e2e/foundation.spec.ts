import { test, expect, type Locator, type Page } from '@playwright/test';
test('Harness primitive keyboard, menu, modal, hover and disclosure contracts', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:5174/test/fixtures/foundation.html');
  await expect(page).toHaveTitle('Foundation contracts');
  await page.getByRole('button', { name: 'Ordinary button' }).click();
  await expect(page.getByLabel('Submissions')).toHaveText('0');
  await page.getByRole('button', { name: 'Submit form' }).click();
  await expect(page.getByLabel('Submissions')).toHaveText('1');
  await expect(page.getByRole('button', { name: 'Disabled button' })).toBeDisabled();
  const trigger = page.getByRole('button', { name: 'Actions', exact: true });
  await trigger.click();
  await expect(page.getByRole('menuitem', { name: 'Alpha' })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('menuitem', { name: 'Charlie' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.getByLabel('Selection')).toHaveText('c');
  await expect(trigger).toBeFocused();
  await trigger.click(); await page.keyboard.press('Escape'); await expect(trigger).toBeFocused();
  await page.getByRole('button', { name: 'Details', exact: true }).hover();
  await expect(page.getByText('Hover details')).toBeVisible();
  const open = page.getByRole('button', { name: 'Open dialog' });
  await open.click(); await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Close dialog' })).toBeFocused();
  await page.keyboard.press('Shift+Tab'); await expect(page.getByRole('button', { name: 'Done', exact: true })).toBeFocused();
  await page.keyboard.press('Tab'); await expect(page.getByRole('button', { name: 'Close dialog' })).toBeFocused();
  await page.keyboard.press('Escape'); await expect(page.getByRole('dialog')).toHaveCount(0); await expect(open).toBeFocused();
  const disclosure = page.getByRole('button', { name: 'Expand details' });
  await disclosure.click(); await expect(disclosure).toHaveAttribute('aria-expanded', 'true');
  await expect(page.getByText('Expanded content')).toBeVisible();
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

/** The shared Menu's geometry is Floating UI's alone. Each assertion is
 * about the rendered list against its real anchor and the real viewport:
 * side/align placement, flipping to the side that fits, shifting to stay 12px
 * inside the viewport, taking only the height the viewport leaves, and
 * following its anchor when the layout changes while it is open. */
test('portaled Menu placement, flip, shift, bounded height and anchor tracking', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('http://127.0.0.1:5174/test/fixtures/foundation.html');
  const trigger = page.getByRole('button', { name: 'Actions', exact: true });
  const list = page.getByRole('menu');
  const box = async () => (await list.boundingBox())!;
  const anchor = async () => (await trigger.boundingBox())!;
  const viewport = () => page.viewportSize()!;
  /** Pin the anchor's wrapper to one viewport position. */
  const pin = (style: string) => trigger.evaluate((el, css) => { el.parentElement!.setAttribute('style', css); }, style);
  const open = async () => { await trigger.click(); await expect(page.getByRole('menuitem', { name: 'Alpha' })).toBeFocused(); };
  /** Floating UI rounds coordinates to device pixels; the contract is the
   * placement, not a sub-pixel. */
  const near = (actual: number, expected: number) => expect(Math.abs(actual - expected)).toBeLessThanOrEqual(1);
  const close = async () => { await page.keyboard.press('Escape'); await expect(list).toHaveCount(0); await expect(trigger).toBeFocused(); };

  // Default: bottom-start, 4px below the anchor, left edges aligned.
  await open();
  let a = await anchor(), m = await box();
  near(m.y, a.y + a.height + 4);
  near(m.x, a.x);
  await close();

  // Anchored in the bottom-right corner: no room below, so the list flips
  // above the anchor, and it shifts left to stay 12px inside the viewport.
  await pin('position: fixed; right: 2px; bottom: 2px;');
  await open();
  a = await anchor(); m = await box();
  near(m.y + m.height, a.y - 4);
  expect(m.x + m.width).toBeLessThanOrEqual(viewport().width - 12 + 0.5);
  expect(m.x).toBeGreaterThanOrEqual(12 - 0.5);

  // While it stays open, a narrower viewport moves the anchor and the list
  // follows it — no event of the page repositions it, Floating UI does.
  await page.setViewportSize({ width: 700, height: 800 });
  await expect.poll(async () => (await box()).x + (await box()).width).toBeLessThanOrEqual(700 - 12 + 0.5);
  a = await anchor(); m = await box();
  near(m.y + m.height, a.y - 4);
  await close();

  // A viewport too short for the rows: the list takes only the height the
  // viewport leaves and scrolls its rows inside itself.
  await page.setViewportSize({ width: 700, height: 150 });
  await pin('position: fixed; left: 8px; top: 8px;');
  await open();
  m = await box();
  expect(m.y).toBeGreaterThanOrEqual(12 - 0.5);
  expect(m.y + m.height).toBeLessThanOrEqual(150 - 12 + 0.5);
  expect(await list.evaluate(el => { const rows = el.firstElementChild!; return rows.scrollHeight > rows.clientHeight; })).toBe(true);
  await close();
  expect(errors).toEqual([]);
});

/** Shared geometry probes for the nested-menu fixture: every floating surface
 * is measured against the real viewport and its real anchor row. */
function nestedMenu(page: Page) {
  const trigger = page.getByRole('button', { name: 'Nested actions', exact: true });
  const row = (name: string) => page.getByRole('menuitem', { name, exact: true });
  const parent = page.getByRole('menu').filter({ has: row('Tango') });
  const submenu = page.getByRole('menu').filter({ hasNot: row('Tango') });
  const rect = (locator: Locator) => locator.evaluate(el => { const r = el.getBoundingClientRect(); return { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height }; });
  /** Pin the anchor's wrapper to one viewport position. */
  const pin = (style: string) => trigger.evaluate((el, css) => { el.parentElement!.setAttribute('style', css); }, style);
  const open = async () => { await trigger.click(); await expect(row('A deliberately long dynamic label that must truncate at the design width of the shared menu card')).toBeFocused(); };
  /** Whether a surface lies wholly inside the viewport's 12px margin. */
  const inside = async (locator: Locator) => {
    const r = await rect(locator);
    const { width, height } = page.viewportSize()!;
    return r.left >= 12 - 0.5 && r.top >= 12 - 0.5 && r.right <= width - 12 + 0.5 && r.bottom <= height - 12 + 0.5;
  };
  /** Whether the rows of a surface scroll inside it. */
  const scrolls = (locator: Locator) => locator.evaluate(el => { const rows = el.firstElementChild!; return rows.scrollHeight > rows.clientHeight; });
  /** The side the submenu opened on, when it sits beside its row: across the
   * design's 10px row-to-card gap and ending 4px below the row (the parent
   * card's inset), or null while it does not. */
  const besideRow = async (name: string) => {
    const s = await rect(submenu), r = await rect(row(name));
    const bottomAligned = Math.abs(s.bottom - (r.bottom + 4)) <= 1;
    if (bottomAligned && Math.abs(s.left - (r.right + 10)) <= 1) return 'right';
    if (bottomAligned && Math.abs(s.right - (r.left - 10)) <= 1) return 'left';
    return null;
  };
  return { trigger, row, parent, submenu, rect, pin, open, inside, scrolls, besideRow };
}

/** Design dimensions decide the card; the viewport can only take room away. */
test('portaled Menu keeps its 218–360px design width inside the viewport', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('http://127.0.0.1:5174/test/fixtures/foundation.html');
  const menu = nestedMenu(page);
  const long = menu.row('A deliberately long dynamic label that must truncate at the design width of the shared menu card');
  await menu.pin('position: fixed; left: 8px; top: 8px;');
  await menu.open();
  // A wide viewport leaves far more than 360px: the long label still stops at
  // the design cap, on one line, cut off by the ellipsis.
  const wide = await menu.rect(menu.parent);
  expect(wide.width).toBeLessThanOrEqual(360 + 0.5);
  expect(wide.width).toBeGreaterThanOrEqual(218 - 0.5);
  expect(await menu.inside(menu.parent)).toBe(true);
  const label = await long.evaluate(el => {
    const text = Array.from(el.querySelectorAll('span')).find(span => span.textContent?.startsWith('A deliberately'))!;
    const style = getComputedStyle(text);
    return { truncated: text.scrollWidth > text.clientWidth, whiteSpace: style.whiteSpace, textOverflow: style.textOverflow, lines: Math.round(text.getBoundingClientRect().height / parseFloat(style.lineHeight)) };
  });
  expect(label).toEqual({ truncated: true, whiteSpace: 'nowrap', textOverflow: 'ellipsis', lines: 1 });
  await page.keyboard.press('Escape'); await expect(menu.parent).toHaveCount(0);

  // A viewport narrower than the design minimum: the room wins, and the card
  // stays wholly inside it.
  await page.setViewportSize({ width: 200, height: 800 });
  await menu.open();
  const narrow = await menu.rect(menu.parent);
  expect(narrow.width).toBeLessThanOrEqual(200 - 24 + 0.5);
  expect(await menu.inside(menu.parent)).toBe(true);
  await page.keyboard.press('Escape'); await expect(menu.parent).toHaveCount(0);
  expect(errors).toEqual([]);
});

/** A submenu is a floating surface of its own, placed against its real row. */
test('Menu submenu flips, bounds its height, scrolls and follows its row', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('http://127.0.0.1:5174/test/fixtures/foundation.html');
  const menu = nestedMenu(page);
  const close = async () => { await page.keyboard.press('Escape'); await expect(page.getByRole('menu')).toHaveCount(0); await expect(menu.trigger).toBeFocused(); };
  /** Keyboard-show a row's submenu: focusing the row shows it. */
  const show = async (name: string, steps: number) => {
    for (let step = 0; step < steps; step += 1) await page.keyboard.press('ArrowDown');
    await expect(menu.row(name)).toBeFocused();
    await expect(menu.row(name)).toHaveAttribute('aria-expanded', 'true');
  };

  // Room on the right: the submenu opens there, beside its row.
  await menu.pin('position: fixed; left: 300px; top: 8px;');
  await menu.open(); await show('Sort by', 2);
  expect(await menu.besideRow('Sort by')).toBe('right');
  expect(await menu.inside(menu.submenu)).toBe(true);

  // B. Moved against the right edge while open: the submenu follows its row
  // and flips to the side that fits instead of overflowing the viewport.
  await menu.pin('position: fixed; right: 8px; top: 8px;');
  await expect.poll(() => menu.besideRow('Sort by')).toBe('left');
  expect(await menu.inside(menu.submenu)).toBe(true);
  expect(await menu.inside(menu.parent)).toBe(true);

  // D. The viewport resizes under the open menus: parent and submenu follow
  // the moving anchor and row.
  await page.setViewportSize({ width: 900, height: 800 });
  await expect.poll(async () => (await menu.rect(menu.parent)).right).toBeLessThanOrEqual(900 - 12 + 0.5);
  await expect.poll(() => menu.besideRow('Sort by')).toBe('left');
  expect(await menu.inside(menu.submenu)).toBe(true);

  // Back on the left, a layout change moves the row down and right.
  await menu.pin('position: fixed; left: 200px; top: 120px;');
  await expect.poll(() => menu.besideRow('Sort by')).toBe('right');
  await close();

  // C. A viewport too short for either card: each takes only the height the
  // viewport leaves and scrolls its own rows — the submenu no longer costs
  // the parent its bound.
  await page.setViewportSize({ width: 900, height: 300 });
  await menu.pin('position: fixed; left: 200px; top: 8px;');
  await menu.open();
  expect(await menu.inside(menu.parent)).toBe(true);
  expect(await menu.scrolls(menu.parent)).toBe(true);
  await show('More options', 1);
  expect(await menu.inside(menu.submenu)).toBe(true);
  expect(await menu.scrolls(menu.submenu)).toBe(true);
  expect(await menu.inside(menu.parent)).toBe(true);
  expect(await menu.scrolls(menu.parent)).toBe(true);
  // The submenu's last row is reachable inside its own scroll.
  await page.keyboard.press('ArrowRight'); await expect(menu.row('Option 1')).toBeFocused();
  await page.keyboard.press('End'); await expect(menu.row('Option 40')).toBeFocused();
  expect(await menu.inside(menu.row('Option 40'))).toBe(true);
  await page.keyboard.press('Escape'); await expect(menu.submenu).toHaveCount(0); await expect(menu.row('More options')).toBeFocused();
  await close();

  // D. Scrolling the parent's rows moves the row; its submenu follows.
  await menu.open(); await show('Sort by', 2);
  expect(await menu.besideRow('Sort by')).toBe('right');
  const before = (await menu.rect(menu.submenu)).top;
  await menu.parent.evaluate(el => { el.firstElementChild!.scrollTop = 24; });
  await expect.poll(async () => (await menu.rect(menu.submenu)).top).toBeLessThan(before - 20);
  expect(await menu.besideRow('Sort by')).toBe('right');
  await close();
  expect(errors).toEqual([]);
});

/** A floating surface lives only while its reference is a visible
 * interaction anchor: when the reference leaves rendered layout, or scrolls
 * out of its clipping context, the surface settles closed and keeps no
 * keyboard. */
test('Menu surfaces close when their anchor leaves layout', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('http://127.0.0.1:5174/test/fixtures/foundation.html');
  const trigger = page.getByRole('button', { name: 'Actions', exact: true });
  const hideAnchor = (hidden: boolean) => page.getByRole('button', { name: 'Actions', exact: true, includeHidden: true }).evaluate((el, value) => { el.parentElement!.style.display = value ? 'none' : ''; }, hidden);
  const keyboard = () => page.evaluate(() => { const active = document.activeElement!; return { menuitem: active.getAttribute('role') === 'menuitem', rendered: active === document.body || active.getClientRects().length > 0 }; });

  // The anchor leaves layout while the list holds the keyboard: the list
  // closes through its owner, and the keyboard stays on no row and on no
  // hidden trigger.
  await trigger.click(); await expect(page.getByRole('menuitem', { name: 'Alpha' })).toBeFocused();
  await hideAnchor(true);
  await expect(page.getByRole('menu')).toHaveCount(0);
  expect(await keyboard()).toEqual({ menuitem: false, rendered: true });
  // The owner's state settled closed with it: the anchor back in layout does
  // not bring the list back, and one press opens it again.
  await hideAnchor(false);
  await expect(page.getByRole('menu')).toHaveCount(0);
  await trigger.click(); await expect(page.getByRole('menuitem', { name: 'Alpha' })).toBeFocused();
  await page.keyboard.press('Escape'); await expect(page.getByRole('menu')).toHaveCount(0); await expect(trigger).toBeFocused();

  // A submenu's row scrolled out of the parent card: only the submenu closes.
  await page.setViewportSize({ width: 900, height: 300 });
  const menu = nestedMenu(page);
  await menu.pin('position: fixed; left: 200px; top: 8px;');
  await menu.open();
  await page.keyboard.press('ArrowDown'); await page.keyboard.press('ArrowDown');
  await expect(menu.row('Sort by')).toBeFocused(); await expect(menu.submenu).toBeVisible();
  await menu.parent.evaluate(el => { el.firstElementChild!.scrollTop = el.firstElementChild!.scrollHeight; });
  await expect(menu.submenu).toHaveCount(0);
  await expect(menu.row('Sort by')).toHaveAttribute('aria-expanded', 'false');
  await expect(menu.parent).toBeVisible();
  // With the keyboard inside the submenu, it goes back to the row.
  await menu.parent.evaluate(el => { el.firstElementChild!.scrollTop = 0; });
  await menu.row('Sort by').focus(); await page.keyboard.press('ArrowRight');
  await expect(menu.row('Name')).toBeFocused();
  await menu.parent.evaluate(el => { el.firstElementChild!.scrollTop = el.firstElementChild!.scrollHeight; });
  await expect(menu.submenu).toHaveCount(0);
  await expect(menu.row('Sort by')).toBeFocused();
  await expect(menu.parent).toBeVisible();
  await page.keyboard.press('Escape'); await expect(page.getByRole('menu')).toHaveCount(0); await expect(menu.trigger).toBeFocused();
  expect(errors).toEqual([]);
});

/** One keyboard and pointer contract across the two layers. */
test('Menu submenu keyboard layers, pointer crossing and focus return', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('http://127.0.0.1:5174/test/fixtures/foundation.html');
  const menu = nestedMenu(page);
  const selection = page.getByLabel('Nested choice');
  await menu.pin('position: fixed; left: 300px; top: 8px;');

  // Focus shows a submenu without entering it; ArrowRight enters it, the
  // arrows then walk only the submenu (skipping its disabled row and
  // wrapping inside it), and ArrowLeft hands the keyboard back to the row
  // with the submenu closed.
  await menu.open();
  await page.keyboard.press('ArrowDown'); await expect(menu.row('More options')).toHaveAttribute('aria-expanded', 'true');
  await page.keyboard.press('ArrowDown'); await expect(menu.row('Sort by')).toBeFocused();
  await expect(menu.row('More options')).toHaveAttribute('aria-expanded', 'false');
  await page.keyboard.press('ArrowRight'); await expect(menu.row('Name')).toBeFocused();
  await page.keyboard.press('ArrowDown'); await expect(menu.row('Date')).toBeFocused();
  await page.keyboard.press('ArrowDown'); await expect(menu.row('Name')).toBeFocused();
  await page.keyboard.press('ArrowLeft');
  await expect(menu.row('Sort by')).toBeFocused(); await expect(menu.submenu).toHaveCount(0);
  await expect(menu.row('Sort by')).toHaveAttribute('aria-expanded', 'false');
  // Enter enters it too; Escape closes only the topmost layer.
  await page.keyboard.press('Enter'); await expect(menu.row('Name')).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(menu.row('Sort by')).toBeFocused(); await expect(menu.submenu).toHaveCount(0); await expect(menu.parent).toBeVisible();
  // Tab settles the row by entering; End and Enter select; both layers close
  // and the keyboard returns to the trigger.
  await page.keyboard.press('Tab'); await expect(menu.row('Name')).toBeFocused();
  await page.keyboard.press('End'); await expect(menu.row('Date')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(selection).toHaveText('date'); await expect(page.getByRole('menu')).toHaveCount(0); await expect(menu.trigger).toBeFocused();

  // Escape on a parent row closes the whole menu even while it shows a
  // submenu; Shift+Tab from inside a submenu leaves both layers.
  await menu.open(); await page.keyboard.press('ArrowDown');
  await expect(menu.submenu).toBeVisible();
  await page.keyboard.press('Escape'); await expect(page.getByRole('menu')).toHaveCount(0); await expect(menu.trigger).toBeFocused();
  await menu.open(); await page.keyboard.press('ArrowDown'); await page.keyboard.press('ArrowRight');
  await expect(menu.row('Option 1')).toBeFocused();
  await page.keyboard.press('Shift+Tab'); await expect(page.getByRole('menu')).toHaveCount(0); await expect(menu.trigger).toBeFocused();

  // Pointer: hovering shows the submenu, the pointer crosses the row-to-card
  // gap without closing it, a disabled row neither selects nor closes, and a
  // selection returns focus to the trigger.
  await menu.open();
  await menu.row('Sort by').hover(); await expect(menu.submenu).toBeVisible();
  const from = await menu.rect(menu.row('Sort by')), to = await menu.rect(menu.row('Date'));
  const y = (to.top + to.bottom) / 2;
  await page.mouse.move(from.right - 8, y);
  await page.mouse.move((to.left + to.right) / 2, y, { steps: 12 });
  await expect(menu.submenu).toBeVisible();
  await menu.row('Size').click({ force: true });
  await expect(menu.submenu).toBeVisible(); await expect(selection).toHaveText('date');
  await menu.row('Name').click();
  await expect(selection).toHaveText('name'); await expect(page.getByRole('menu')).toHaveCount(0); await expect(menu.trigger).toBeFocused();

  // A press outside both surfaces closes them.
  await menu.open(); await menu.row('More options').hover(); await expect(menu.submenu).toBeVisible();
  // Left of the parent card; the submenu opened on its right.
  const card = await menu.rect(menu.parent);
  await page.mouse.click(card.left - 20, card.bottom - 10);
  await expect(page.getByRole('menu')).toHaveCount(0);
  expect(errors).toEqual([]);
});

import { test, expect } from '@playwright/test';
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

/** The shared Menu's portal geometry is Floating UI's alone. Each assertion is
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

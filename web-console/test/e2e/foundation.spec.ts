import { test, expect } from '@playwright/test';
test.beforeEach(async ({ page }) => {
  await page.goto('http://127.0.0.1:5174/test/fixtures/foundation.html');
  await expect(page).toHaveTitle('Foundation contracts');
  await expect(page.getByRole('heading', { name: 'Foundation contracts' })).toBeVisible();
});
test('native form, menu, popover, dialog and disclosure interaction contracts', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.getByRole('button', { name: 'Ordinary button' }).click();
  await expect(page.getByLabel('Submissions')).toHaveText('0');
  await page.getByRole('button', { name: 'Submit form' }).click();
  await expect(page.getByLabel('Submissions')).toHaveText('1');
  await expect(page.getByRole('button', { name: 'Disabled button' })).toBeDisabled();
  const trigger = page.getByRole('button', { name: 'Actions', exact: true });
  await trigger.focus(); await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('menuitem', { name: 'Alpha' })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('menuitem', { name: 'Charlie' })).toBeFocused();
  await page.keyboard.press('Home'); await expect(page.getByRole('menuitem', { name: 'Alpha' })).toBeFocused();
  await page.keyboard.press('End'); await page.keyboard.press('Enter');
  await expect(page.getByLabel('Selection')).toHaveText('c'); await expect(trigger).toBeFocused();
  await page.keyboard.press('Space'); await page.keyboard.press('Escape'); await expect(trigger).toBeFocused();
  await page.keyboard.press('ArrowUp'); await expect(page.getByRole('menuitem', { name: 'Charlie' })).toBeFocused();
  await page.keyboard.press('a'); await expect(page.getByRole('menuitem', { name: 'Alpha' })).toBeFocused();
  await page.keyboard.press('Tab'); await expect(page.getByRole('menu')).toHaveCount(0);
  await trigger.click(); await page.getByRole('button', { name: 'Outside target' }).click();
  await expect(page.getByRole('menu')).toHaveCount(0);
  const popover = page.getByRole('button', { name: 'Details', exact: true });
  await popover.focus(); await page.keyboard.press('Enter'); await expect(page.getByLabel('Popover value')).toBeFocused();
  await page.keyboard.press('Tab'); await expect(page.getByRole('button', { name: 'Popover action' })).toBeFocused();
  await page.keyboard.press('Escape'); await expect(popover).toBeFocused();
  await popover.click(); await page.getByRole('button', { name: 'Outside target' }).click(); await expect(page.getByRole('dialog')).toHaveCount(0);
  const open = page.getByRole('button', { name: 'Open dialog' });
  await open.click(); await expect(page.getByRole('dialog')).toBeVisible(); await expect(page.getByRole('button', { name: 'Close dialog' })).toBeFocused();
  await page.keyboard.press('Shift+Tab'); await expect(page.getByRole('button', { name: 'Done', exact: true })).toBeFocused();
  await page.keyboard.press('Tab'); await expect(page.getByRole('button', { name: 'Close dialog' })).toBeFocused();
  await page.getByRole('button', { name: 'Nested details' }).click();
  await expect(page.getByRole('button', { name: 'Nested action' })).toBeFocused();
  await page.keyboard.press('Escape'); await expect(page.getByRole('button', { name: 'Nested details' })).toBeFocused();
  await page.keyboard.press('Escape'); await expect(page.getByRole('dialog')).toHaveCount(0); await expect(open).toBeFocused();
  await open.click(); await page.getByRole('button', { name: 'Done', exact: true }).click(); await expect(open).toBeFocused();
  await open.click(); await page.mouse.click(2, 2); await expect(page.getByRole('dialog')).toHaveCount(0); await expect(open).toBeFocused();
  const disclosure = page.getByRole('button', { name: 'Expand details' });
  await disclosure.focus(); await page.keyboard.press('Space'); await expect(disclosure).toHaveAttribute('aria-expanded', 'true');
  await expect(page.getByText('Expanded content')).toBeVisible(); await page.keyboard.press('Enter'); await expect(page.getByText('Expanded content')).toHaveCount(0);
  expect(errors).toEqual([]);
});
for (const width of [390, 900, 1440]) test(`shell geometry at ${width}px`, async ({ page }) => {
  await page.setViewportSize({ width, height: 1000 });
  const main = await page.getByRole('main').boundingBox();
  const navigation = await page.getByRole('complementary', { name: 'Navigation' }).boundingBox();
  const dock = await page.getByRole('complementary', { name: 'Inspector' }).boundingBox();
  expect(main).not.toBeNull(); expect(navigation).not.toBeNull(); expect(dock).not.toBeNull();
  if (width > 1100) { expect(main!.x).toBeGreaterThan(navigation!.x); expect(dock!.x).toBeGreaterThan(main!.x); }
  else if (width > 650) { expect(main!.x).toBeGreaterThan(navigation!.x); expect(dock!.y).toBeGreaterThan(main!.y); }
  else { expect(main!.y).toBeGreaterThan(navigation!.y); expect(dock!.y).toBeGreaterThan(main!.y); }
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  await page.screenshot({ path: `test-results/foundation-${width}.png`, fullPage: true });
});

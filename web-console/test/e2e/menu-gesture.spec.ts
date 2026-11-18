import { expect, test, type Locator, type Page } from '@playwright/test';

const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

async function center(locator: Locator) {
  const box = await locator.boundingBox();
  if (!box) throw new Error('menu row has no box');
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

async function open(page: Page) {
  await page.getByRole('button', { name: 'Actions for guide' }).click();
}

test('a real pointer keeps the pressed menu row and follows a settled enable', async ({ page }) => {
  await page.goto(`${origin}/test/fixtures/menu-gesture.html`);
  await open(page);
  await page.evaluate(() => window.menuGesture.setDisabled(true));
  const pending = page.getByRole('menuitem', { name: 'Move' });
  await expect(pending).toBeDisabled();
  await page.evaluate(() => { window.menuGesture.setToken('settled'); window.menuGesture.setDisabled(false); });
  await expect(pending).toBeEnabled();
  const settled = await center(pending);
  await page.mouse.move(settled.x, settled.y);
  await page.mouse.down();
  await page.mouse.up();
  await expect(page.getByTestId('selected')).toHaveText('settled:move');

  await page.evaluate(() => { window.menuGesture.setToken('opened'); window.menuGesture.setDisabled(false); });
  await open(page);
  const pressed = page.getByRole('menuitem', { name: 'Move' });
  const at = await center(pressed);
  await page.mouse.move(at.x, at.y);
  await page.mouse.down();
  await page.evaluate(() => { window.menuGesture.setToken('replacement'); window.menuGesture.setDisabled(true); });
  await expect(pressed).toBeEnabled();
  await page.mouse.up();
  await expect(page.getByTestId('selected')).toHaveText('opened:move');

  await page.evaluate(() => { window.menuGesture.setToken('outside'); window.menuGesture.setDisabled(false); });
  await open(page);
  const outside = page.getByRole('menuitem', { name: 'Move' });
  await expect(outside).toBeEnabled();
  const again = await center(outside);
  await page.mouse.move(again.x, again.y);
  await page.mouse.down();
  await page.evaluate(() => window.menuGesture.setDisabled(true));
  await expect(outside).toBeEnabled();
  await page.mouse.move(8, 8);
  await page.mouse.up();
  await expect(page.getByTestId('selected')).toHaveText('opened:move');
  await expect(outside).toBeDisabled();

  await page.getByRole('menuitem', { name: 'Edit' }).click();
  await expect(page.getByRole('menuitem', { name: 'Rename' })).toBeVisible();
  await page.evaluate(() => window.menuGesture.setDisabled(true));
  await expect(page.getByRole('menuitem', { name: 'Move' })).toBeDisabled();
});

import { test, expect } from '@playwright/test';

test('short final turn navigation retains the clicked mark until reader scrolling', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/turn-navigation.html`);
  const first = page.getByRole('button', { name: 'first', exact: true });
  const last = page.getByRole('button', { name: 'last', exact: true });
  await first.click(); await expect(first).toHaveAttribute('aria-current', 'true');
  await last.click(); await expect(last).toHaveAttribute('aria-current', 'true');
  const box = (await page.locator('[data-conversation-scroll]').first().boundingBox())!;
  await page.mouse.move(box.x + 100, box.y + 100);
  await page.mouse.wheel(0, -120);
  await expect(first).toHaveAttribute('aria-current', 'true');
  expect(errors).toEqual([]);
});

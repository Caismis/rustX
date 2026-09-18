import { test, expect } from '@playwright/test';
test('Harness Agent references use native snapshots in light, dark and narrow states', async ({ page }) => {
 const errors: string[] = [];
 page.on('pageerror', error => errors.push(error.message));
 await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
 await page.emulateMedia({ reducedMotion: 'reduce' });
 for (const mode of ['settled', 'streaming', 'tools', 'error', 'approval', 'questionnaire', 'selectors']) {
   await page.goto(`http://127.0.0.1:5174/test/fixtures/agent.html?mode=${mode}`);
   await expect(page).toHaveTitle('rustX Agent reference');
   await expect(page.getByLabel('Canonical conversation')).toBeVisible();
   if (mode === 'error') {
     await page.locator('[data-tool-call-id="bash-1"]').getByRole('button').click();
     await page.locator('[data-tool-call-id="edit-1"]').getByRole('button').click();
   }
   if (mode === 'questionnaire') {
     await page.getByRole('radio', { name: 'Keep native' }).click();
     await page.getByRole('button', { name: 'Next question' }).click();
     await page.getByRole('checkbox', { name: 'Native contracts' }).click();
   }
   if (mode === 'selectors') {
     await page.getByRole('button', { name: 'Approval mode' }).click();
     await expect(page.getByRole('menuitem', { name: 'Full access' })).toBeEnabled();
     await page.keyboard.press('Escape');
     await page.getByRole('button', { name: 'Model and reasoning' }).click();
     await expect(page.getByText('Reading native models…')).toHaveCount(0);
     await page.getByRole('menuitem', { name: 'Reasoning profile' }).click();
     await expect(page.getByRole('menuitem', { name: 'deliberate' })).toBeVisible();
   }
   await expect(page).toHaveScreenshot(`agent-${mode}-light.png`);
 }
 await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
 await page.getByRole('button', { name: 'Settings', exact: true }).click();
 await page.getByRole('button', { name: 'Appearance', exact: true }).click();
 await page.getByLabel('Theme', { exact: true }).selectOption('dark');
 await page.getByRole('button', { name: 'Close Settings' }).click();
 await expect(page).toHaveScreenshot('agent-dark-desktop.png');
 await page.setViewportSize({ width: 390, height: 844 });
 await expect(page).toHaveScreenshot('agent-dark-narrow.png');
 expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
 await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

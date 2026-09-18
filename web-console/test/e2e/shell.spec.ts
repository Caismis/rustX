import { test, expect } from '@playwright/test';
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
  await expect(page).toHaveScreenshot('desktop-expanded-light.png');
  await page.getByRole('button', { name: 'Toggle Inspector' }).click();
  await expect(page.getByRole('complementary', { name: 'Developer inspector' })).toBeVisible();
  await expect(page).toHaveScreenshot('desktop-right-panel.png');
  await page.getByRole('button', { name: 'Close Inspector' }).click();
  await page.getByRole('button', { name: 'Collapse Sidebar' }).click();
  await expect(page.locator('[data-sidebar-wide]')).toHaveAttribute('data-sidebar-wide', 'false');
  expect((await page.locator('main').boundingBox())!.x).toBe(56);
  await expect(page).toHaveScreenshot('desktop-collapsed-rail.png');
  await page.getByRole('button', { name: 'Expand Sidebar' }).click();
  await page.getByRole('button', { name: 'Search Sessions', exact: true }).click();
  await page.getByLabel('Search Session metadata').fill('Session');
  await expect(page).toHaveScreenshot('workspace-session-browser.png');
  await page.getByRole('button', { name: 'Clear search' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Appearance', exact: true }).click();
  await expect(page).toHaveScreenshot('settings-shell-light.png');
  await page.getByLabel('Theme', { exact: true }).selectOption('dark');
  await expect(page).toHaveScreenshot('settings-shell-dark.png');
  await page.getByRole('button', { name: 'Close Settings' }).click();
  await expect(page).toHaveScreenshot('desktop-expanded-dark.png');
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.locator('[data-sidebar-wide]')).toHaveAttribute('data-sidebar-wide', 'false');
  expect((await page.locator('main').boundingBox())!.x).toBe(56);
  await expect(page).toHaveScreenshot('mobile-rail-dark.png');
  await page.getByRole('button', { name: 'Expand Sidebar' }).click();
  await expect(page).toHaveScreenshot('mobile-expanded-dark.png');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Appearance', exact: true }).click();
  await expect(page).toHaveScreenshot('mobile-settings-dark.png');
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
    await expect(page.getByLabel('Session status')).toHaveText('Working…');
    await page.evaluate(mode => window.sessionFixture.state(mode), mode);
    const expected = { idle: undefined, queued: 'Queued', stopping: 'Stopping…', reconnect: 'Connection interrupted', uncertain: 'Needs verification' }[mode];
    if (expected) await expect(page.getByLabel('Session status')).toContainText(expected);
    else await expect(page.getByLabel('Session status')).toHaveCount(0);
    const ordinary = await page.locator('main').innerText();
    expect(ordinary).not.toMatch(/attempt-A|runtime_incarnation|connection_generation|Attach \/ cold resume|Unload runtime|Detach|Resync/);
    await expect(page).toHaveScreenshot(`session-${mode}-light.png`);
    if (mode === 'uncertain') {
      await page.getByRole('button', { name: 'Toggle Inspector' }).click();
      await page.getByText('Uncertain operations and reconciliation evidence', { exact: true }).click();
      await expect(page.getByRole('complementary', { name: 'Developer inspector' })).toContainText('turn/cancel');
      await page.getByRole('button', { name: 'Close Inspector' }).click();
      await page.setViewportSize({ width: 390, height: 844 });
      await expect(page).toHaveScreenshot('session-uncertain-mobile.png');
    }
  }
  expect(errors).toEqual([]);
});

import { expect, test } from '@playwright/test';
import { expectStableScreenshot } from './screenshot';
import { choose, closeSettings, openSettingsPage, openWorkspaceSettings } from './shell-actions';
test('native Settings reference pages, resource details, keyboard scopes and narrow theme', async ({ page }) => {
 const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
 await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z')); await page.emulateMedia({ reducedMotion: 'reduce' });
 await page.goto('http://127.0.0.1:5174/test/fixtures/settings.html');
 await expect(page).toHaveTitle('rustX native Settings reference');
 await page.getByRole('button', { name: 'Settings', exact: true }).click();
 const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
 const tab = (name: string) => page.getByRole('tablist', { name: 'Settings pages' }).getByRole('tab', { name, exact: true });
 await expect(tab('General')).toHaveAttribute('aria-selected', 'true');
 await openSettingsPage(page, 'Models');
 await expect(settings.getByRole('row', { name: 'transport', exact: true })).toBeVisible();
 await expectStableScreenshot(settings, 'settings-user-catalog-light.png');
 // Source revisions are diagnostics, reported on Advanced only.
 await openSettingsPage(page, 'Advanced');
 await expect(settings.getByText(/Revision: user-1/)).toBeVisible();
 await closeSettings(page);
 await openWorkspaceSettings(page, 'Workspace A');
 await expect(tab('Models')).toHaveAttribute('aria-selected', 'true');
 await settings.getByRole('button', { name: /^All Models/ }).click();
 await settings.getByRole('row', { name: 'main', exact: true }).click();
 await expect(settings.getByRole('heading', { name: 'Model main', exact: true })).toBeVisible();
 await expectStableScreenshot(settings, 'settings-model-editor-light.png');
 await openSettingsPage(page, 'Extensions');
 await settings.getByRole('tab', { name: 'Native', exact: true }).click();
 await expect(settings.getByRole('switch', { name: 'Enable Goal' })).toBeChecked();
 await expectStableScreenshot(settings, 'settings-native-extensions-light.png');
 await settings.getByRole('tab', { name: 'Skills', exact: true }).click();
 await expect(settings.getByText('Invalid definition')).toBeVisible(); await expectStableScreenshot(settings, 'settings-inventory-light.png');
 await closeSettings(page);
 // Appearance is client-owned, on the global General page.
 await page.getByRole('button', { name: 'Settings', exact: true }).click();
 await choose(settings, 'Theme', 'Dark');
 await expectStableScreenshot(settings, 'settings-general-dark.png');
 await closeSettings(page);
 await openWorkspaceSettings(page, 'Workspace A');
 await page.setViewportSize({ width: 390, height: 844 });
 await openSettingsPage(page, 'Extensions');
 await settings.getByRole('tab', { name: 'Agents', exact: true }).click();
 await settings.getByRole('row', { name: 'reviewer', exact: true }).click();
 await settings.getByRole('form', { name: 'Agent reviewer' }).evaluate(el => el.scrollIntoView({ block: 'start' }));
 await expect(settings.getByLabel('Description', { exact: true })).toBeVisible();
 await expectStableScreenshot(page, 'settings-agent-narrow-dark.png');
 expect(await settings.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
 await page.keyboard.press('Escape'); await expect(settings).toHaveCount(0);
 // The global entry restores focus to its own trigger; the Workspace entry is
 // opened from the exact Workspace object action instead.
 await page.getByRole('button', { name: 'Settings', exact: true }).click();
 await expect(settings).toBeVisible();
 await page.keyboard.press('Escape'); await expect(settings).toHaveCount(0);
 await expect(page.getByRole('button', { name: 'Settings', exact: true })).toBeFocused();
 await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

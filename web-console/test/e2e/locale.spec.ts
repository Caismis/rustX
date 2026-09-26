import { expect, test } from '@playwright/test';
import { choose } from './shell-actions';
import { expectStableScreenshot } from './screenshot';

test('Chinese General switches the resident UI without requests, keeps native identities and persists through reload', async ({ page }) => {
  await page.addInitScript(() => {
    if (sessionStorage.getItem('locale-test-seeded') !== 'yes') {
      localStorage.setItem('rustx-locale-v1', 'zh');
      sessionStorage.setItem('locale-test-seeded', 'yes');
    }
  });
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('http://127.0.0.1:5174/test/fixtures/settings.html');
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  await expect(page.getByRole('textbox', { name: '消息', exact: true })).toBeVisible();
  await page.getByRole('button', { name: '设置', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: '设置', exact: true });
  await expect(dialog.getByRole('heading', { name: '通用设置' })).toBeVisible();
  await expectStableScreenshot(page, 'locale-general-zh.png');
  const before = await page.evaluate(() => (window as unknown as { rustxNativeRequests(): unknown[] }).rustxNativeRequests());
  await choose(dialog, '语言', 'English');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  const english = page.getByRole('dialog', { name: 'Settings', exact: true });
  await expect(english.getByRole('heading', { name: 'General' })).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { rustxNativeRequests(): unknown[] }).rustxNativeRequests())).toEqual(before);
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
  await page.getByRole('button', { name: '关闭设置' }).click();
  await expect(page.getByRole('textbox', { name: '消息', exact: true })).toBeVisible();
  await expect(page.locator('button[data-session-id="A"]')).toContainText('Session A');
  expect(errors).toEqual([]);
});

test('Chinese new conversation preserves the user draft and localizes composer controls', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('rustx-locale-v1', 'zh'));
  await page.goto('http://127.0.0.1:5174/test/fixtures/settings.html?conversation=ready');
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  await page.getByRole('button', { name: '新建会话', exact: true }).first().click();
  const input = page.getByRole('textbox', { name: '消息', exact: true });
  await input.fill('保留草稿 /raw/native');
  await expect(input).toHaveValue('保留草稿 /raw/native');
  await expect(page.getByRole('button', { name: '发送', exact: true })).toBeVisible();
  await page.getByRole('button', { name: '选择工作区', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Workspace A', exact: true }).click();
  await expect(page.locator('[data-model-select]')).toBeEnabled();
  await input.fill('/model');
  await input.press('Enter');
  await expect(page.getByRole('menu').filter({ has: page.getByRole('menuitem', { name: '模型', exact: true }) })).toBeVisible();
  await expect(page.locator('[data-model-select]')).toHaveAttribute('aria-expanded', 'true');
  await page.getByRole('menuitem', { name: 'fixture/model', exact: true }).click();
  await expect(input).toHaveValue('');
  expect(await page.evaluate(() => (window as unknown as { rustxNativeRequests(): { method: string }[] }).rustxNativeRequests().filter(request => request.method === 'session/create'))).toEqual([]);
});

test('Chinese approval localizes actions while preserving the native request reason', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('rustx-locale-v1', 'zh'));
  await page.goto('http://127.0.0.1:5174/test/fixtures/agent.html?mode=approval');
  await expect(page.locator('.interaction').getByText('bash：Developer approval required', { exact: true })).toBeVisible();
  const allow = page.getByRole('button', { name: '允许一次', exact: true });
  await expect(allow).toBeVisible();
  await allow.click();
  await expect(allow).toHaveCount(0);
  await expect(page.getByRole('textbox', { name: '消息', exact: true })).toBeVisible();
});

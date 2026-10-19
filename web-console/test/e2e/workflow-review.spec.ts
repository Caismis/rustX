import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, chooseWorkspace, choose, selectedSessionId } from './shell-actions';

for (const locale of ['en', 'zh'] as const) test(`native workflow review and node lifecycle ${locale}`, async ({ page }) => {
  page.setDefaultTimeout(15_000);
  const fixture = await startDogfood('web_workflow_review');
  let passed = false;
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  const copy = (en: string, zh: string) => locale === 'zh' ? zh : en;
  try {
    await page.setViewportSize({ width: locale === 'zh' ? 390 : 1440, height: 1000 });
    await page.addInitScript(theme => localStorage.setItem('rustx-appearance-v1', theme), locale === 'zh' ? 'dark' : 'light');
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await chooseWorkspace(page, 'Workspace A');
    if (locale === 'zh') {
      await page.getByRole('button', { name: 'Settings', exact: true }).click();
      await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
      await page.getByRole('button', { name: '关闭设置', exact: true }).click();
    }
    await expect(page).toHaveTitle(/rustX/);
    const message = page.getByRole('textbox', { name: copy('Message', '消息'), exact: true });
    await message.fill('workflow conformance request'); await page.getByRole('button', { name: copy('Send', '发送'), exact: true }).click();
    await fixture.gate('workflow-child-admitted');
    const run = page.locator('[data-workflow-run-id]');
    await expect(run.locator('[data-workflow-node="review"]')).toBeVisible();
    await expect(run.locator('[data-workflow-node="decision"]')).toBeVisible();
    await expect(page.locator('[aria-label="' + copy('Canonical conversation', '规范对话') + '"] [data-workflow-run-id]')).toHaveCount(1);
    await fixture.release('workflow-child-admitted');
    const review = page.getByRole('region', { name: copy('Review', '评审'), exact: true });
    await expect(review.getByRole('heading', { name: 'Review report' })).toBeVisible();
    await expect(review).toContainText('Check the release version before publishing.');
    await expect(review.locator('pre')).toHaveCount(0);
    await expect(run.locator('[data-workflow-node="gate"]')).toContainText(copy('Waiting for review', '等待审阅'));
    await page.screenshot({ path: `/tmp/rustx-workflow-review-${locale}.png` });
    // The native review remains pending through reconnect; no choice is sent by rendering it.
    const sidebar = page.getByRole('button', { name: copy('Expand Sidebar', '打开侧边栏'), exact: true });
    if (await sidebar.isVisible()) await sidebar.click();
    const sessionId = await selectedSessionId(page), runId = await run.getAttribute('data-workflow-run-id');
    await page.reload();
    // Remote credentials stay in page memory, so both locales reconnect explicitly.
    await page.getByRole('button', { name: copy('Settings', '设置'), exact: true }).click();
    const settings = page.getByRole('dialog', { name: copy('Settings', '设置'), exact: true });
    const section = settings.getByRole('button', { name: locale === 'en' ? /^Settings page: / : /^设置页面：/ });
    if (await section.isVisible()) {
      await section.click(); await page.getByRole('menuitem', { name: copy('Advanced', '高级'), exact: true }).click();
    } else await settings.getByRole('tab', { name: copy('Advanced', '高级'), exact: true }).click();
    await settings.getByRole('button', { name: copy('Connection', '连接'), exact: true }).click();
    await page.getByLabel(copy('Connection mode', '连接模式')).selectOption('remote');
    await page.getByLabel(copy('WebSocket endpoint', 'WebSocket 地址')).fill(fixture.endpoint);
    await page.getByLabel(copy('Transport token', '传输令牌')).fill(fixture.token);
    await settings.getByRole('button', { name: copy('Connect', '连接'), exact: true }).click();
    await expect(page.locator('.connection-status')).toHaveText(copy('Connected', '已连接'));
    await page.getByRole('button', { name: copy('Close Settings', '关闭设置'), exact: true }).click();
    const expand = page.getByRole('button', { name: copy('Expand Sidebar', '打开侧边栏'), exact: true });
    if (await expand.isVisible()) await expand.click();
    await page.locator(`button[data-session-id="${sessionId}"]`).click();
    await expect(run).toHaveAttribute('data-workflow-run-id', runId!);
    await expect(review.getByRole('heading', { name: 'Review report' })).toBeVisible();
    if (locale === 'zh') {
      await review.getByRole('radio', { name: '附反馈拒绝', exact: true }).click();
      await expect(review.getByRole('button', { name: '提交', exact: true })).toBeDisabled();
      await review.getByLabel('评审反馈', { exact: true }).fill('Please revise the release version.');
    }
    await review.getByRole('button', { name: copy('Submit', '提交'), exact: true }).click();
    await expect(review).toHaveCount(0);
    await expect(page.getByText('workflow conformance complete', { exact: true })).toBeVisible();
    await expect(run).toContainText(copy('completed', '已完成'));
    await expect(run.getByRole('button').first()).toHaveAttribute('aria-expanded', 'false');
    await run.getByRole('button').first().click();
    await run.getByRole('button', { name: new RegExp(copy('Main flow', '主流程')) }).click();
    await expect(run).toContainText(copy('Steps used', '已执行'));
    await expect(run).not.toContainText('/ 10');
    await expect(run.locator('[data-workflow-node="gate"]')).toContainText(copy('Accepted', '已拒绝'));
    await expect(run.locator('[data-workflow-node="' + (locale === 'zh' ? 'success' : 'failure') + '"]')).toContainText(copy('Not run', '未执行'));
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: `/tmp/rustx-workflow-completed-${locale}.png` });
    expect(errors).toEqual([]); passed = true;
  } catch (error) { console.error(fixture.diagnostics()); await page.screenshot({ path: `/tmp/rustx-workflow-failure-${locale}.png` }).catch(() => {}); throw error; }
  finally { await fixture.stop(passed); }
});

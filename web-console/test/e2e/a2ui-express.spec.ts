import { expect, test } from '@playwright/test';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { choose, connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';
import { routeWorkspaceHost } from './workspace-host';

test('renders a fenced Express rating card in assistant history', async ({ page }, info) => {
  const fixture = await startDogfood('web_a2ui_express');
  let passed = false;
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Render ratings');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    const chat = page.getByLabel('Canonical conversation', { exact: true });
    const assertCard = async () => {
      for (const text of ['评分如下。', '模型嘉豪指数', '模型与评价', 'Claude Mythos 5.2', '赛级嘉豪', 'Gemini 4 Neon', 'Carbon 换皮', '以上为示例。']) {
        await expect(chat.getByText(text, { exact: true })).toBeVisible();
      }
      await expect(chat).not.toContainText('root = Card');
      const card = chat.locator('[data-a2ui-express]');
      await expect(card.getByRole('button', { name: '打开 HN' })).toBeDisabled();
    };
    await assertCard();
    await page.screenshot({ path: info.outputPath('express-ratings.png') });
    await page.reload();
    await connectRemote(page, fixture.endpoint, fixture.token);
    await assertCard();
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});

test('Express inputs update only their own card and never submit another turn', async ({ page }, info) => {
  const fixture = await startDogfood('web_a2ui_controls');
  let passed = false;
  try {
    const wire = await wireProbe(page);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await composer.fill('Show controls');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    const chat = page.getByLabel('Canonical conversation', { exact: true });
    const cards = chat.locator('[data-a2ui-express]');
    await expect(cards).toHaveCount(2);
    const card = cards.first();
    await expect(card).toHaveAttribute('data-ds', 'shadcn');
    const slider = card.getByRole('slider', { name: '评分' });
    await slider.focus();
    await slider.press('End');
    await expect(card.locator('span').filter({ hasText: /^10$/ }).last()).toBeVisible();
    await expect(cards.last().getByRole('slider')).toHaveValue('3');
    await card.getByRole('checkbox', { name: '显示详情' }).check();
    await expect(card.getByText('true', { exact: true })).toBeVisible();
    await card.getByRole('textbox', { name: '名称' }).fill('新名字');
    await expect(card.getByText('新名字', { exact: true })).toBeVisible();
    await expect(cards.last().getByText('原名', { exact: true })).toBeVisible();
    await card.getByLabel('日期', { exact: true }).fill('2026-12-01');
    await expect(card.getByText('2026-12-01', { exact: true })).toBeVisible();
    const choice = card.getByRole('button', { name: '乙组' });
    const filter = card.getByRole('searchbox', { name: /filter options/i });
    await filter.fill('甲');
    await expect(choice).toHaveCount(0);
    await expect(cards.last().getByRole('button', { name: '乙组' })).toBeVisible();
    await filter.clear();
    await choice.click();
    await expect(choice).toHaveAttribute('aria-pressed', 'true');
    await expect(cards.last().getByRole('button', { name: '乙组' })).toHaveAttribute('aria-pressed', 'false');
    const submit = card.getByRole('button', { name: '提交到会话' });
    await expect(submit).toBeVisible();
    await expect(submit).toHaveAttribute('data-variant', 'default');
    await expect(submit).toBeDisabled();
    await expect(composer).toHaveValue('');
    expect(wire.requests.filter(request => request.method === 'inbound/queue')).toHaveLength(1);
    expect((await fixture.control('requests')).requests).toHaveLength(1);
    await page.screenshot({ path: info.outputPath('express-controls.png') });
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});

test('updating a bound row redraws both charts and the table', async ({ page }, info) => {
  const fixture = await startDogfood('web_a2ui_charts');
  let passed = false;
  try {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Show charts');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    const card = page.locator('[data-a2ui-express]');
    const table = card.getByRole('table');
    const row = table.getByRole('row').filter({ hasText: '二月' });
    await expect(row.getByRole('cell', { name: '3', exact: true })).toBeVisible();
    const charts = card.locator('svg');
    await expect(charts).toHaveCount(2);
    await expect(card.getByText('No data', { exact: true })).toBeVisible();
    const firstSeries = charts.first().locator('circle').first();
    await expect(firstSeries).toHaveCSS('fill', 'oklch(0.646 0.222 41.116)');
    const februaryPoint = charts.first().locator('circle').nth(1);
    const februaryBar = charts.last().locator('rect').nth(1);
    const initialPointY = Number(await februaryPoint.getAttribute('cy'));
    const initialBarHeight = Number(await februaryBar.getAttribute('height'));
    await card.getByRole('slider', { name: '二月销量' }).press('End');
    await expect(row.getByRole('cell', { name: '10', exact: true })).toBeVisible();
    expect(Number(await februaryPoint.getAttribute('cy'))).toBeLessThan(initialPointY);
    expect(Number(await februaryBar.getAttribute('height'))).toBeGreaterThan(initialBarHeight);
    await page.setViewportSize({ width: 1440, height: 1800 });
    await card.screenshot({ path: info.outputPath('linked-charts.png') });
    await page.emulateMedia({ colorScheme: 'dark' });
    await expect(firstSeries).toHaveCSS('fill', 'oklch(0.623 0.214 259.815)');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await choose(page.getByRole('region', { name: 'Settings', exact: true }), 'Language', '中文');
    await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
    await page.getByRole('button', { name: '主页', exact: true }).click();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(table).toBeVisible();
    await expect(card.getByText('暂无数据', { exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await charts.first().scrollIntoViewIfNeeded();
    await page.screenshot({ path: info.outputPath('linked-charts-narrow-dark.png') });
    expect((await fixture.control('requests')).requests).toHaveLength(1);
    expect(errors).toEqual([]);
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});

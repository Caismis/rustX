import { expect, test } from '@playwright/test';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { choose, connectRemote, openEmptySession } from './shell-actions';
import { wireProbe } from './wire-probe';
import { routeWorkspaceHost } from './workspace-host';

for (const reducedMotion of ['no-preference', 'reduce'] as const) {
  test(`draws the live Express layout with ${reducedMotion} motion`, async ({ page }, info) => {
    const fixture = await startDogfood('web_a2ui_generation');
    let passed = false;
    try {
      const errors: string[] = [];
      page.on('pageerror', error => errors.push(error.message));
      await page.emulateMedia({ reducedMotion });
      if (reducedMotion === 'reduce') {
        await page.setViewportSize({ width: 390, height: 900 });
        await page.addInitScript(() => localStorage.setItem('rustx-appearance-v1', 'dark'));
      }
      await routeWorkspaceHost(page, fixture);
      await page.goto('/');
      await expect(page).toHaveTitle('rustX');
      await connectRemote(page, fixture.endpoint, fixture.token);
      await openEmptySession(page, fixture, 'Workspace A');
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Draw the market panel');
      await page.getByRole('button', { name: 'Send', exact: true }).click();

      const chat = page.getByLabel('Canonical conversation', { exact: true });
      const draft = chat.locator('[data-a2ui-generation="draft"]');
      await expect(draft.getByRole('status')).toHaveText('Preparing inline view…');
      await expect(chat.getByText('The prices are ready.', { exact: true })).toBeVisible();
      await fixture.release('express-open');
      await expect(draft.locator('[data-a2ui-blueprint="StockTicker"]')).toBeVisible();
      await expect(draft.locator('[data-a2ui-blueprint=""]')).toHaveCount(1);
      await expect(chat).not.toContainText('root = Column');
      if (reducedMotion === 'reduce') {
        expect(await draft.evaluate(node => node.getAnimations({ subtree: true }).length)).toBe(0);
      }
      await draft.screenshot({ path: info.outputPath('express-generating.png') });

      await fixture.release('express-structure');
      const reveal = chat.locator('[data-a2ui-generation="reveal"]');
      if (reducedMotion === 'no-preference') {
        await expect(reveal).toBeVisible();
        // Capture the real layout drawing before its synchronized scan begins.
        await reveal.evaluate(node => {
          for (const animation of node.getAnimations({ subtree: true })) {
            animation.pause();
            animation.currentTime = 470;
          }
        });
        await expect(reveal.locator('svg[focusable="false"] rect').first()).toBeVisible();
        await reveal.screenshot({ path: info.outputPath('express-wireframe.png') });
        await reveal.evaluate(node => {
          for (const animation of node.getAnimations({ subtree: true })) animation.play();
        });
      }
      await expect(draft).toHaveCount(0);
      await expect(reveal).toHaveCount(0);
      const result = chat.locator('[data-a2ui-express]');
      await expect(result.getByText('NVIDIA Corporation', { exact: true })).toBeVisible();
      await expect(result.getByRole('img', { name: 'Daily candles' })).toBeVisible();
      await expect(chat.getByText('The series is complete.', { exact: true })).toBeVisible();
      await result.screenshot({ path: info.outputPath('express-settled.png') });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await fixture.release('express-complete');
      await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeVisible();
      await page.reload();
      await connectRemote(page, fixture.endpoint, fixture.token);
      await expect(result.getByText('NVIDIA Corporation', { exact: true })).toBeVisible();
      await expect(chat.locator('[data-a2ui-generation]')).toHaveCount(0);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0);
      expect(errors).toEqual([]);
      passed = true;
    } finally {
      await stopDogfoodPage(page, fixture, passed);
    }
  });
}

test('opens only valid HTTPS destinations from an Express card in assistant history', async ({ page, context }, info) => {
  const fixture = await startDogfood('web_a2ui_express');
  let passed = false;
  try {
    const wire = await wireProbe(page);
    await routeWorkspaceHost(page, fixture);
    const hn = 'https://news.ycombinator.com/';
    await context.route(hn, route => route.fulfill({ contentType: 'text/html', body: '<title>Hacker News fixture</title>' }));
    await page.goto('/');
    await expect(page).toHaveTitle('rustX');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Render ratings');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    const chat = page.getByLabel('Canonical conversation', { exact: true });
    const card = chat.locator('[data-a2ui-express]');
    const openHn = card.getByRole('button', { name: '打开 HN', exact: true });
    const assertCard = async () => {
      for (const text of ['评分如下。', '模型嘉豪指数', '模型与评价', 'Claude Mythos 5.2', '赛级嘉豪', 'Gemini 4 Neon', 'Carbon 换皮', '以上为示例。']) {
        await expect(chat.getByText(text, { exact: true })).toBeVisible();
      }
      await expect(chat).not.toContainText('root = Card');
      await expect(openHn).toBeEnabled();
      for (const name of ['HTTP 链接', '相对链接', '脚本链接', '无效链接']) {
        await expect(card.getByRole('button', { name, exact: true })).toBeDisabled();
      }
    };
    await assertCard();
    await page.screenshot({ path: info.outputPath('express-ratings.png') });
    await page.reload();
    await connectRemote(page, fixture.endpoint, fixture.token);
    await assertCard();
    expect(await openHn.evaluate((button: HTMLButtonElement) => {
      const open = window.open;
      let calls = 0;
      window.open = () => { calls++; return null; };
      try { button.click(); } finally { window.open = open; }
      return calls;
    })).toBe(0);
    const opened = context.waitForEvent('page');
    await openHn.click();
    const destination = await opened;
    await expect(destination).toHaveURL(hn);
    await expect(destination).toHaveTitle('Hacker News fixture');
    expect(await destination.evaluate(() => [window.opener === null, document.referrer])).toEqual([true, '']);
    await destination.close();
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(wire.requests.filter(request => request.method === 'inbound/queue')).toHaveLength(1);
    expect((await fixture.control('requests')).requests).toHaveLength(1);
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
    await expect(card.getByText('10 分', { exact: true })).toBeVisible();
    await expect(cards.last().getByRole('slider')).toHaveValue('3');
    await expect(cards.last().getByText('3 分', { exact: true })).toBeVisible();
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
    await expect(submit).toBeEnabled();
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
    await februaryPoint.hover();
    const tooltip = card.getByRole('tooltip');
    await expect(tooltip).toContainText('二月');
    await expect(tooltip).toContainText('10');
    await februaryBar.hover();
    await expect(tooltip).toContainText('二月');
    await expect(tooltip).toContainText('10');
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

test('renders the Express spec examples and sends only a real button activation', async ({ page }, info) => {
  const fixture = await startDogfood('web_a2ui_official');
  let passed = false;
  try {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    const wire = await wireProbe(page);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await composer.fill('Render official Express');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    const chat = page.getByLabel('Canonical conversation', { exact: true });
    const cards = chat.locator('[data-a2ui-express]');
    await expect(cards).toHaveCount(3);
    await expect(cards.last().getByRole('listitem')).toHaveText([
      'Connect your repository', 'Pick a framework preset', 'Deploy the main branch',
    ]);
    const template = cards.nth(1);
    await expect(cards.first().getByRole('heading', { name: 'Enable notification' })).toBeVisible();
    await expect(cards.first().locator('svg')).toHaveCount(1);
    await expect(template.getByText('2026-11-20', { exact: true })).toBeVisible();
    const rows = template.getByRole('listitem');
    await expect(rows).toHaveCount(2);
    const alpha = rows.nth(0);
    const beta = rows.nth(1);
    await expect(alpha.getByText('1', { exact: true })).toBeVisible();
    await expect(alpha.getByText('$1,299.50', { exact: true })).toBeVisible();
    await expect(beta.getByText('2', { exact: true })).toBeVisible();
    await expect(beta.getByText('$1,499.25', { exact: true })).toBeVisible();
    await beta.getByRole('slider', { name: 'Quantity' }).press('End');
    await expect(beta.getByText('Quantity: 10.0', { exact: true })).toBeVisible();
    await expect(alpha.getByText('Quantity: 3.0', { exact: true })).toBeVisible();

    const reply = beta.getByRole('button', { name: 'Reply', exact: true });
    await expect(reply).toBeEnabled();
    await reply.evaluate((button: HTMLButtonElement) => button.click());
    expect(wire.requests.filter(request => request.method === 'inbound/queue')).toHaveLength(1);
    await composer.fill('Keep this draft');
    await reply.click();
    await expect(chat.getByText('Replying to Beta: 10', { exact: true })).toBeVisible();
    await expect(chat.getByText('Reply accepted.', { exact: true })).toBeVisible();
    await expect(composer).toHaveValue('Keep this draft');

    await cards.first().getByRole('button', { name: 'Yes', exact: true }).click();
    await expect(chat.getByText('Notifications accepted.', { exact: true })).toBeVisible();
    expect(wire.requests.filter(request => request.method === 'inbound/queue')).toHaveLength(3);
    expect((await fixture.control('requests')).requests).toHaveLength(3);
    expect(errors).toEqual([]);
    await template.screenshot({ path: info.outputPath('express-official-template.png') });
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});

test('renders the Playground NVDA dashboard with all 30 reported OHLC bars', async ({ page }, info) => {
  const fixture = await startDogfood('web_a2ui_market');
  let passed = false;
  try {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await expect(page).toHaveTitle('rustX');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Show the NVDA dashboard');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    const card = page.locator('[data-a2ui-express]');
    await expect(card.getByText('NVIDIA Corporation')).toBeVisible();
    await expect(card.getByText('$145.98', { exact: true })).toHaveCount(2);
    await expect(card.getByText('Sample data', { exact: true })).toBeVisible();
    const sparkline = card.locator('[data-a2-type="StockTicker"] svg[viewBox="0 0 320 72"]');
    await expect(sparkline.locator('path')).toHaveCount(2);

    const chart = card.getByRole('img', { name: 'Daily candles' });
    const bars = chart.locator('.ts-chart__rect rect');
    await expect(bars).toHaveCount(30);
    await expect(chart.getByText('10-09', { exact: true })).toBeVisible();
    await expect(chart.getByText('11-19', { exact: true })).toBeVisible();
    await expect(card.getByText('Key stats', { exact: true })).toBeVisible();
    await expect(card.getByText('259.9M', { exact: true })).toBeVisible();
    await expect(card.getByText('Data source', { exact: true })).toBeVisible();
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);

    await page.setViewportSize({ width: 1440, height: 1250 });
    await card.screenshot({ path: info.outputPath('nvda-dashboard.png') });
    await bars.nth(24).hover();
    const tooltip = card.locator('.ts-chart-tooltip');
    for (const text of ['11-12', 'Open', '$151.01', 'High', '$167.14', 'Low', '$148.80', 'Close', '$166.19', 'Change', '+10.05%']) {
      await expect(tooltip.getByText(text, { exact: true })).toBeVisible();
    }
    await card.screenshot({ path: info.outputPath('nvda-dashboard-hover.png') });
    await card.getByText('Daily candles', { exact: true }).hover();
    await expect(tooltip).not.toBeVisible();
    await chart.press('ArrowRight');
    await expect(tooltip).toBeVisible();
    await chart.evaluate(element => (element as SVGElement).blur());

    await page.setViewportSize({ width: 390, height: 900 });
    await expect(chart).toBeVisible();
    expect(await card.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await card.screenshot({ path: info.outputPath('nvda-dashboard-narrow.png') });
    expect(errors).toEqual([]);
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});

test('keeps Playground chart data, prose and supported siblings inline', async ({ page }, info) => {
  const fixture = await startDogfood('web_a2ui_dashboard');
  let passed = false;
  try {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Show the sales dashboard');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    const chat = page.getByLabel('Canonical conversation', { exact: true });
    const card = chat.locator('[data-a2ui-express]');
    await expect(chat.getByText('Regional revenue was higher in February.', { exact: true })).toBeVisible();
    await expect(chat.getByText('Both regions contributed to the increase.', { exact: true })).toBeVisible();
    await expect(card.getByRole('heading', { name: 'Regional sales' })).toBeVisible();
    await expect(card.getByText('$5,000', { exact: true })).toBeVisible();
    const line = card.getByRole('img', { name: 'Monthly trend' });
    const area = card.getByRole('img', { name: 'Revenue by region' });
    const bar = card.getByRole('img', { name: 'Revenue comparison' });
    await expect(line.locator('.ts-chart__marks path')).toHaveCount(2);
    await expect(area.locator('.ts-chart__marks path').first()).toBeVisible();
    await bar.scrollIntoViewIfNeeded();
    await expect(bar.locator('.ts-chart__marks :is(rect, path)')).toHaveCount(4);
    await line.scrollIntoViewIfNeeded();
    const region = await line.boundingBox();
    if (!region) throw new Error('Monthly trend chart was not laid out');
    await page.mouse.move(region.x + region.width * .85, region.y + region.height * .42);
    const tooltip = card.locator('.ts-chart-tooltip').filter({ hasText: 'February' });
    for (const text of ['February', 'North', '$1,800', 'South', '$1,100']) {
      await expect(tooltip.getByText(text, { exact: true })).toBeVisible();
    }
    const timeline = card.locator('[data-a2-type="Timeline"]');
    await expect(timeline.getByText('February close', { exact: true })).toBeVisible();
    await expect(card.locator('code')).toHaveCount(0);
    await line.screenshot({ path: info.outputPath('inline-playground-line.png') });

    const marker = timeline.locator('li').filter({ hasText: 'February close' }).locator('svg');
    await expect(marker).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeVisible();
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: 900 });
      // Resizing can restore the scroll anchor before the sticky composer settles.
      await expect.poll(() => marker.evaluate(async node => {
        const scroll = node.closest<HTMLElement>('[data-conversation-scroll]')!;
        const composer = scroll.querySelector('[data-composer-card]')!;
        const center = (element: Element) => {
          const { left, top, width, height } = element.getBoundingClientRect();
          return { x: left + width / 2, y: top + height / 2 };
        };
        scroll.scrollBy({ top: center(node).y - center(composer).y, behavior: 'instant' });
        await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
        const { x, y } = center(node);
        const hits = document.elementsFromPoint(x, y);
        return { overlapsComposer: hits.includes(composer), composerInFront: composer.contains(hits[0] ?? null) };
      }), { message: `Timeline marker scrolls beneath the composer at ${width}px` }).toEqual({ overlapsComposer: true, composerInFront: true });
      await page.screenshot({ path: info.outputPath(`inline-timeline-under-composer-${width}.png`) });
    }
    expect(errors).toEqual([]);
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});

async function stubPlaygroundAssets(page: import('@playwright/test').Page) {
  await page.route(/^https:\/\//, route => {
    const type = route.request().resourceType();
    if (type === 'image') {
      const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jPe0AAAAASUVORK5CYII=', 'base64');
      return route.fulfill({ contentType: 'image/png', body: png });
    }
    if (type === 'fetch' || type === 'xhr') {
      return route.fulfill({ json: { version: 8, sources: {}, layers: [] }, headers: { 'access-control-allow-origin': '*' } });
    }
    return route.fulfill({ contentType: 'text/html', body: '<title>Playground media fixture</title>' });
  });
}

for (let batch = 1; batch <= 8; batch++) {
  test(`mounts the published catalog inline, batch ${batch} of 8`, async ({ page }) => {
    const fixture = await startDogfood(`web_a2ui_catalog_gallery_${batch}`);
    let passed = false;
    try {
      const errors: string[] = [];
      page.on('pageerror', error => errors.push(error.message));
      await stubPlaygroundAssets(page);
      await routeWorkspaceHost(page, fixture);
      await page.goto('/');
      await expect(page).toHaveTitle('rustX');
      await connectRemote(page, fixture.endpoint, fixture.token);
      await openEmptySession(page, fixture, 'Workspace A');
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Show Playground gallery');
      await page.getByRole('button', { name: 'Send', exact: true }).click();
      const chat = page.getByLabel('Canonical conversation', { exact: true });
      const response = chat.getByRole('article', { name: 'Assistant response' });
      await expect(response.getByText('Catalog preview ends here.', { exact: true })).toHaveCount(1);
      await expect(response.locator('[data-a2ui-catalog="playground"]')).toHaveCount(batch === 8 ? 9 : 16, { timeout: 25000 });
      await expect(response.locator('[data-generative-ui-error]')).toHaveCount(0);
      await expect(response.locator('[data-a2ui-catalog="playground"]:has([data-a2-type])')).toHaveCount(batch === 8 ? 9 : 16);
      await expect(page.locator('vite-error-overlay')).toHaveCount(0);
      expect(errors).toEqual([]);
      passed = true;
    } finally {
      await stopDogfoodPage(page, fixture, passed);
    }
  });
}

test('uses the Playground interactions for weather, diagrams, tables, and form controls', async ({ page }, info) => {
  const fixture = await startDogfood('web_a2ui_catalog_interactions');
  let passed = false;
  try {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    const wire = await wireProbe(page);
    await stubPlaygroundAssets(page);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await expect(page).toHaveTitle('rustX');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await composer.fill('Show Playground controls');
    await page.getByRole('button', { name: 'Send', exact: true }).click();

    const chat = page.getByLabel('Canonical conversation', { exact: true });
    const weather = chat.locator('[data-a2-type="WeatherCard"]');
    await expect(weather.getByText('San Francisco, CA', { exact: true })).toBeVisible();
    await expect(weather.getByText('Partly cloudy', { exact: true })).toBeVisible();
    await expect(weather.getByText('Thu', { exact: true })).toBeVisible();
    await weather.locator('.a2-card').screenshot({ path: info.outputPath('playground-weather.png') });
    const svg = chat.getByRole('img', { name: 'A sun over two hills' });
    await expect(svg.locator('path')).toHaveCount(2);
    await expect(svg.locator('circle')).toHaveCount(1);
    await svg.evaluate(node => node.scrollIntoView({ block: 'center', behavior: 'instant' }));
    await expect.poll(() => svg.evaluate(node => {
      const { x, y, width, height } = node.getBoundingClientRect();
      return node.contains(document.elementFromPoint(x + width / 2, y + height / 2));
    })).toBe(true);
    await svg.screenshot({ path: info.outputPath('playground-svg.png') });
    const html = chat.locator('iframe[title="Inline SVG markup"]');
    await expect(html).toHaveAttribute('sandbox', '');
    await expect(html).toHaveAttribute('srcdoc', /Content-Security-Policy/);
    await expect(html.contentFrame().getByRole('img', { name: 'Inline vector arrows' })).toBeVisible();
    await html.evaluate(node => node.scrollIntoView({ block: 'center', behavior: 'instant' }));
    await html.screenshot({ path: info.outputPath('playground-inline-svg.png') });

    const tableView = chat.locator('[data-a2-type="DataTable"]');
    const table = tableView.getByRole('table');
    await expect(table.getByRole('row').nth(1)).toContainText('Northwind Labs');
    await tableView.getByRole('button', { name: 'MRR', exact: true }).click();
    await expect(table.getByRole('row').nth(1)).toContainText('Quarry Finance');
    const search = tableView.getByRole('textbox', { name: 'Search rows' });
    await search.fill('Helix');
    await expect(table.getByRole('row')).toHaveCount(2);
    await expect(table.getByRole('row').last()).toContainText('Helix Health');
    await tableView.locator('.a2-card').screenshot({ path: info.outputPath('playground-table-filter.png') });

    const specs = chat.getByRole('tab', { name: 'Specs' });
    await specs.click();
    await expect(specs).toHaveAttribute('aria-selected', 'true');
    await expect(chat.getByText('22 liters, 1.1 kg, fits a 16-inch laptop.', { exact: true })).toBeVisible();
    const toggle = chat.getByRole('switch', { name: 'Auto-approve small refunds' });
    await expect(toggle).toHaveAttribute('aria-checked', 'false');
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-checked', 'true');
    expect(wire.requests.filter(request => request.method === 'inbound/queue')).toHaveLength(1);
    const action = chat.getByRole('button', { name: 'Select forecast', exact: true });
    await action.evaluate((element: HTMLElement) => {
      element.click();
      element.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    });
    expect(wire.requests.filter(request => request.method === 'inbound/queue')).toHaveLength(1);
    await composer.fill('Keep draft');
    await action.press('Enter');
    await expect(chat.getByText('Forecast action confirmed.', { exact: true })).toBeVisible();
    await expect(composer).toHaveValue('Keep draft');
    await page.setViewportSize({ width: 390, height: 900 });
    await expect(svg).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);

    await page.setViewportSize({ width: 1440, height: 1080 });
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await choose(page.getByRole('region', { name: 'Settings', exact: true }), 'Language', '中文');
    await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
    await page.getByRole('button', { name: '主页', exact: true }).click();
    await expect(page.locator('[data-a2-type="WeatherCard"]').getByText('体感温度')).toBeVisible();
    await expect(page.getByRole('textbox', { name: '搜索表格' })).toBeVisible();
    await expect(page.getByText('San Francisco, CA', { exact: true })).toBeVisible();
    expect(errors).toEqual([]);
    passed = true;
  } finally {
    await stopDogfoodPage(page, fixture, passed);
  }
});

import { test, expect } from '@playwright/test';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';
import { connectRemote, openEmptySession, choose, selectedSessionId } from './shell-actions';

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
    await openEmptySession(page, fixture, 'Workspace A');
    if (locale === 'zh') {
      await page.getByRole('button', { name: 'Settings', exact: true }).click();
      await choose(page.getByRole('region', { name: 'Settings', exact: true }), 'Language', '中文');
      await page.getByRole('button', { name: '主页', exact: true }).click();
    }
    await expect(page).toHaveTitle(/rustX/);
    const message = page.getByRole('textbox', { name: copy('Message', '消息'), exact: true });
    await message.fill('workflow conformance request'); await page.getByRole('button', { name: copy('Send', '发送'), exact: true }).click();
    await fixture.gate('workflow-child-admitted');
    const run = page.locator('[data-workflow-run-id]');
    await expect(run.locator('[data-workflow-node="review"]')).toBeVisible();
    await expect(run.locator('[data-workflow-node]')).toHaveCount(2);
    await expect(run.locator('[data-workflow-node="decision"]')).toHaveCount(0);
    await expect(page.locator('[aria-label="' + copy('Canonical conversation', '规范对话') + '"] [data-workflow-run-id]')).toHaveCount(1);
    const parentTitle = await page.locator('#session-title').innerText();
    await run.getByRole('button', { name: copy('View review', '查看 review'), exact: true }).click();
    const childView = page.locator('[data-agent-id][data-view="chat"]:visible');
    await expect(childView).toContainText('Review the request and commit the result.');
    await expect(childView.getByRole('textbox')).toHaveCount(0);
    await expect(childView).toContainText(copy('One-shot subagent record', '一次性子智能体记录'));
    await expect.poll(() => page.locator('#session-title span').first().evaluate(element => element.clientWidth)).toBeGreaterThan(20);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: `/tmp/rustx-workflow-child-${locale}.png` });
    await page.getByRole('button', { name: parentTitle, exact: true }).click();
    await fixture.release('workflow-child-admitted');
    // A Human Input asks in chat: its content, with its actions as the options.
    const review = page.getByRole('region', { name: copy('Questionnaire', '问卷'), exact: true });
    await expect(review.getByRole('heading', { name: copy('Human Input', '人工介入'), exact: true })).toBeVisible();
    await expect(review.getByRole('heading', { name: 'Review report' })).toBeVisible();
    await expect(review).toContainText('Check the release version before publishing.');
    await expect(review.getByRole('radio')).toHaveCount(2);
    const transcript = page.locator(`[aria-label="${copy('Canonical conversation', '规范对话')}"]:visible`);
    const reviewRecord = transcript.locator('[data-review-interaction]');
    await expect(reviewRecord).toHaveCount(1);
    await expect(reviewRecord).toContainText(copy('waiting', '等待回答'));
    await expect(reviewRecord.locator('[data-disclosure-row]')).not.toHaveAttribute('aria-expanded');
    await review.getByRole('radio', { name: 'Accept', exact: true }).click();
    await review.getByRole('button', { name: copy('Collapse the question card', '收起问题卡片'), exact: true }).click();
    const pendingActivity = reviewRecord.locator('xpath=ancestor::*[@data-step-process][1]').locator(':scope > [data-process-activity]');
    if (await pendingActivity.isVisible() && await pendingActivity.getAttribute('aria-expanded') === 'false') await pendingActivity.press('Enter');
    await reviewRecord.getByRole('button', { name: copy('Answer', '回答'), exact: true }).press('Enter');
    await expect(review.getByRole('radio', { name: 'Accept', exact: true })).toBeChecked();
    await expect(run.locator('[data-workflow-node="gate"]')).toContainText(copy('Waiting for review', '等待审阅'));
    await expect.poll(() => run.locator('[data-workflow-node]').evaluateAll(nodes => nodes.map(node => node.getAttribute('data-workflow-node')))).toEqual(['workflow_start', 'review', 'gate']);
    await page.screenshot({ path: `/tmp/rustx-workflow-review-${locale}.png` });
    // The native review remains pending through reconnect; no choice is sent by rendering it.
    const sidebar = page.getByRole('button', { name: copy('Home', '主页'), exact: true });
    if (await sidebar.isVisible()) await sidebar.click();
    const sessionId = await selectedSessionId(page), runId = await run.getAttribute('data-workflow-run-id');
    await page.reload();
    // Remote credentials stay in page memory, so both locales reconnect explicitly.
    await page.getByRole('button', { name: copy('Settings', '设置'), exact: true }).click();
    const settings = page.getByRole('region', { name: copy('Settings', '设置'), exact: true });
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
    await page.getByRole('button', { name: copy('Home', '主页'), exact: true }).click();
    const expand = page.getByRole('button', { name: copy('Home', '主页'), exact: true });
    if (await expand.isVisible()) await expand.click();
    await page.locator(`button[data-session-id="${sessionId}"]`).click();
    await expect(run).toHaveAttribute('data-workflow-run-id', runId!);
    await expect(review.getByRole('heading', { name: 'Review report' })).toBeVisible();
    await expect.poll(() => run.locator('[data-workflow-node]').evaluateAll(nodes => nodes.map(node => node.getAttribute('data-workflow-node')))).toEqual(['workflow_start', 'review', 'gate']);
    // The chosen action is the exit the run takes.
    await review.getByRole('radio', { name: locale === 'zh' ? 'Revise' : 'Accept', exact: true }).click();
    await review.getByRole('button', { name: copy('Submit', '提交'), exact: true }).click();
    await expect(review).toHaveCount(0);
    await expect(page.getByText('workflow conformance complete', { exact: true })).toBeVisible();
    await expect(run.getByRole('button').first()).toContainText(copy('completed', '已完成'));
    await expect(run.getByRole('button').first()).toHaveAttribute('aria-expanded', 'false');
    await run.getByRole('button').first().click();
    await expect(run).not.toContainText(copy('Steps used', '已执行'));
    await expect(run).not.toContainText('/ 10');
    await expect(run.locator('[data-workflow-node="' + (locale === 'zh' ? 'success' : 'failure') + '"]')).toHaveCount(0);
    await expect.poll(() => run.locator('[data-workflow-node]').evaluateAll(nodes => nodes.map(node => node.getAttribute('data-workflow-node')))).toEqual(['workflow_start', 'review', 'gate', locale === 'zh' ? 'failure' : 'success']);
    await expect(run).toContainText(copy('Nodes 4', '4 个节点'));
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: `/tmp/rustx-workflow-completed-${locale}.png` });
    // The original pending record becomes one read-only ask_user-style card.
    await transcript.locator('[data-turn-process]').first().press('Enter');
    const activity = reviewRecord.locator('xpath=ancestor::*[@data-step-process][1]').locator(':scope > [data-process-activity]');
    if (await activity.isVisible() && await activity.getAttribute('aria-expanded') === 'false') await activity.press('Enter');
    await expect(reviewRecord).toContainText(copy('1/1 answered', '1/1 已回答'));
    await reviewRecord.locator('[data-disclosure-row] button[aria-expanded]').press('Enter');
    await expect(reviewRecord.getByRole('heading', { name: 'Review report' })).toBeVisible();
    await expect(reviewRecord.getByText(locale === 'zh' ? 'Revise' : 'Accept', { exact: true })).toHaveCount(1);
    await page.screenshot({ path: `/tmp/rustx-native-review-question-record-${locale}.png` });
    // The real native execution is nested at its Tool call, with original node IO.
    await page.getByRole('tab', { name: copy('Trajectory', '轨迹'), exact: true }).click();
    const ledger = page.getByRole('table', { name: copy('Trace ledger', '追踪明细'), exact: true });
    const entry = ledger.locator('[data-workflow-entry]');
    await expect(entry).toHaveCount(1);
    await expect(entry).toHaveAttribute('data-kind', 'workflow');
    await entry.getByRole('button', { name: copy('Expand workflow nodes', '展开工作流节点'), exact: true }).click();
    await expect.poll(() => ledger.locator('[data-workflow-instance]').evaluateAll(nodes => nodes.map(node => node.getAttribute('data-workflow-instance')))).toEqual(['workflow_start', 'review', 'gate', locale === 'zh' ? 'failure' : 'success']);
    await ledger.locator('[data-workflow-instance="gate"]').click();
    const details = page.getByRole('complementary', { name: copy('Event details', '事件详情'), exact: true });
    await expect(details.getByRole('tabpanel')).toContainText('workflow conformance request');
    await details.getByRole('tab', { name: copy('Result', '结果'), exact: true }).click();
    await expect(details.getByRole('tabpanel')).toContainText(locale === 'zh' ? 'revise' : 'accept');
    await page.screenshot({ path: `/tmp/rustx-native-workflow-trajectory-${locale}.png` });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    expect(errors).toEqual([]); passed = true;
  } catch (error) { console.error(fixture.diagnostics()); await page.screenshot({ path: `/tmp/rustx-workflow-failure-${locale}.png` }).catch(() => {}); throw error; }
  finally { await fixture.stop(passed); }
});

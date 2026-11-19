import { expect, test } from '@playwright/test';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
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

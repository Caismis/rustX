import { test, expect } from '@playwright/test';
const origin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;

test('new conversation appears above history and disappears when leaving the unsent draft', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto(`${origin}/test/fixtures/shell.html`);
  await page.getByRole('button', { name: 'Open Session A', exact: true }).click();
  const group = page.locator('[data-workspace-group] [role=treeitem][aria-expanded]').first();
  await group.press('Enter');
  const create = page.locator('#home-sidebar button[aria-label="New Conversation"]');
  await create.click();
  const draft = page.locator('[data-draft-conversation]');
  await expect(draft).toBeVisible();
  await expect(draft).toHaveAttribute('aria-selected', 'true');
  await expect(group).toHaveAttribute('aria-expanded', 'true');
  expect(await draft.boundingBox()).toBeTruthy();
  expect((await draft.boundingBox())!.y).toBeLessThan((await page.locator('[data-session-id=A]').boundingBox())!.y);
  await page.locator('textarea').fill('unsent input');
  await draft.getByRole('button').click();
  await expect(page.locator('textarea')).toBeFocused();
  await create.click();
  await expect(draft).toHaveCount(1);
  await expect(page.locator('textarea')).toHaveValue('unsent input');
  await page.screenshot({ path: '/tmp/rustx-nav-qa/draft-sidebar.png' });
  await page.getByRole('button', { name: 'View options' }).click();
  await page.getByRole('menuitem', { name: 'Flat view', exact: true }).click();
  await expect(draft).toBeVisible();
  await page.getByRole('button', { name: 'Search Sessions', exact: true }).click();
  await page.getByRole('textbox', { name: 'Search Session metadata', exact: true }).fill('Session A');
  await expect(draft).toHaveCount(0);
  await page.getByRole('button', { name: 'Clear search' }).click();
  await expect(draft).toBeVisible();
  await page.getByRole('button', { name: 'Open Session A', exact: true }).click();
  await expect(draft).toHaveCount(0);
  await expect(page.locator('[data-session-id=A]')).toBeVisible();
  expect(errors).toEqual([]);
});

test('workspace session lists fold independently and keep active sessions visible', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => localStorage.setItem('rustx-locale-v1', 'zh'));
  await page.goto(`${origin}/test/fixtures/shell.html?session-folding`);

  const project = page.locator('[data-workspace-group="project"]');
  const other = page.locator('[data-workspace-group="other"]');
  const rows = project.locator('[data-session-id]');
  const otherRows = other.locator('[data-session-id]');
  const more = (count: number) => project.getByRole('button', { name: `展开其余 ${count} 个会话`, exact: true });
  const less = project.getByRole('button', { name: '收起', exact: true });
  await expect(rows).toHaveCount(5);
  await expect(otherRows).toHaveCount(5);
  await more(7).click();
  await expect(rows).toHaveCount(10);
  await expect(otherRows).toHaveCount(5);
  await more(2).click();
  await expect(rows).toHaveCount(12);
  await other.getByRole('button', { name: '展开其余 1 个会话', exact: true }).click();
  await expect(otherRows).toHaveCount(6);

  const older = project.locator('[data-session-id="History-09"]');
  await older.click();
  await expect(older).toHaveAttribute('aria-current', 'page');
  await less.click();
  await expect(rows).toHaveCount(11);
  await expect(older).toBeVisible();
  await project.locator('[data-session-id="A"]').click();
  await expect(rows).toHaveCount(11);

  const folder = project.locator('[role=treeitem][aria-expanded]');
  await folder.press('Enter');
  await expect(rows).toHaveCount(0);
  await folder.press('Enter');
  await expect(rows).toHaveCount(5);
  await expect(otherRows).toHaveCount(6);

  await page.evaluate(() => window.sessionFixture.setHistoryRunning('History-06', true));
  await expect(project.locator('[data-session-id="History-06"]')).toBeVisible();
  await expect(rows).toHaveCount(8);
  await page.evaluate(() => window.sessionFixture.setHistoryRunning('History-06', false));
  await expect(more(4)).toBeVisible();
  await more(4).click();
  await less.click();
  await expect(rows).toHaveCount(5);
  expect(errors).toEqual([]);
});

const fixtureOrigin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
import { test, expect, type Page } from '@playwright/test';
const fixture = `${fixtureOrigin}/test/fixtures/startup.html`;
const methods = (page: Page): Promise<string[]> => page.evaluate(() => (window as any).startupFixture.requests().map((r: any) => r.method));
const release = (page: Page, method: string, failure?: string) => page.evaluate(({ method, failure }) => (window as any).startupFixture.release(method, failure), { method, failure });
async function begin(page: Page, language = 'en', files = false, model?: 'explicit' | 'preference') {
  await page.addInitScript(({ language, model }) => { localStorage.clear(); localStorage.setItem('rustx-locale-v1', language); if (model === 'preference') localStorage.setItem('rustx-new-session-model-v1', JSON.stringify({ 'ws://127.0.0.1:8080/': { model: 'fixture/second' } })); }, { language, model });
  await page.goto(fixture);
  await page.getByRole('button', { name: language === 'en' ? 'Choose Workspace' : '选择工作区', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Workspace A', exact: true }).click();
  if (model === 'explicit') { await page.getByRole('button', { name: 'Model and reasoning' }).click(); await page.getByRole('menuitem', { name: 'Model', exact: true }).click(); await page.getByRole('menuitem', { name: 'fixture/second', exact: true }).click(); }
  await page.locator('textarea').fill('Retained first input');
  if (files) await page.locator('input[type=file]').setInputFiles([{ name: 'first.txt', mimeType: 'text/plain', buffer: Buffer.from('first') }, { name: 'second.txt', mimeType: 'text/plain', buffer: Buffer.from('second') }]);
  await page.locator('[data-composer-primary]').click();
  await expect.poll(() => methods(page)).toContain('session/create');
}
for (const language of ['en', 'zh']) test(`create ACK renders Conversation before attach/catalog; remount owns exactly one send (${language})`, async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', e => errors.push(e.message));
  await begin(page, language, true);
  await release(page, 'session/create');
  await expect(page.locator('#session-view')).toHaveAttribute('data-phase', 'active');
  await expect(page.locator('[data-first-submission]')).toHaveAttribute('data-first-submission', 'attaching');
  await expect.poll(() => methods(page)).toContain('session/attach');
  expect((await methods(page)).filter(m => ['session/upload', 'turn/start', 'session/setModel'].includes(m))).toEqual([]);
  await expect(page.locator('button[data-session-id=created]')).toHaveCount(0);
  await page.screenshot({ path: `test-results/startup-${language}-attaching.png`, fullPage: true });
  await page.evaluate(() => (window as any).startupFixture.remount());
  await expect(page.locator('#session-view')).toHaveAttribute('data-phase', 'active');
  await expect(page.locator('textarea')).toHaveValue('Retained first input');
  await release(page, 'session/attach');
  await expect.poll(() => methods(page)).toContain('turn/start');
  await expect(page.getByText(language === 'en' ? 'Uploaded' : '已上传', { exact: true })).toHaveCount(2);
  const requests = await page.evaluate(() => (window as any).startupFixture.requests());
  expect(requests.filter((r: any) => r.method === 'session/attach')).toHaveLength(1);
  expect(requests.filter((r: any) => r.method === 'session/upload').map((r: any) => r.params.files[0].name)).toEqual(['first.txt', 'second.txt']);
  const sent = requests.find((r: any) => r.method === 'turn/start');
  expect(sent.params.content.map((b: any) => b.type === 'upload' ? b.token : b.text)).toEqual(['token-1', 'token-2', 'Retained first input']);
  await page.evaluate(() => (window as any).startupFixture.remount());
  await expect(page.locator('textarea')).toHaveValue('Retained first input');
  await release(page, 'turn/start');
  await expect(page.locator('textarea')).toHaveValue('');
  expect((await methods(page)).filter(m => m === 'turn/start')).toHaveLength(1);
  expect((await methods(page)).filter(m => m === 'session/setModel')).toHaveLength(0);
  // The independent catalog owner can now observe native facts.
  await page.evaluate(() => (window as any).startupFixture.resumeCatalog());
  await expect(page.locator('button[data-session-id=created]')).toBeVisible();
  expect(errors).toEqual([]);
});
for (const language of ['en', 'zh']) for (const phase of ['session/attach', 'turn/start']) test(`post-create ${phase} rejection is Session-scoped and not replayed (${language})`, async ({ page }) => {
  await begin(page, language); await release(page, 'session/create');
  await expect.poll(() => methods(page)).toContain('session/attach');
  await release(page, 'session/attach', phase === 'session/attach' ? 'startup rejected' : undefined);
  if (phase === 'turn/start') { await expect.poll(() => methods(page)).toContain('turn/start'); await release(page, 'turn/start', 'admission rejected'); }
  await expect(page.locator('[data-first-submission]')).toHaveAttribute('data-first-submission', 'failed');
  await page.evaluate(() => (window as any).startupFixture.remount());
  await expect(page.locator('#session-view')).toHaveAttribute('data-phase', 'active');
  await expect(page.locator('textarea')).toHaveValue('Retained first input');
  expect((await methods(page)).filter(m => m === phase)).toHaveLength(1);
  expect((await methods(page)).filter(m => m === 'session/create')).toHaveLength(1);
  await page.screenshot({ path: `test-results/startup-${language}-${phase.replace('/', '-')}-failed.png`, fullPage: true });
});
test('lost admission response survives remount/reconnect without replay', async ({ page }) => {
  await begin(page); await release(page, 'session/create');
  await expect.poll(() => methods(page)).toContain('session/attach'); await release(page, 'session/attach');
  await expect.poll(() => methods(page)).toContain('turn/start');
  await page.evaluate(() => (window as any).startupFixture.disconnect());
  await expect(page.locator('[data-first-submission]')).toHaveAttribute('data-first-submission', 'uncertain');
  await page.evaluate(() => (window as any).startupFixture.remount());
  await page.evaluate(() => (window as any).startupFixture.reconnect());
  await expect(page.locator('textarea')).toHaveValue('Retained first input');
  expect((await methods(page)).filter(m => m === 'turn/start')).toHaveLength(1);
});

for (const method of ['session/create', 'session/upload']) test(`lost ${method} response is not replayed by remount/reconnect`, async ({ page }) => {
  await begin(page, 'en', method === 'session/upload');
  if (method === 'session/upload') {
    await page.evaluate(() => (window as any).startupFixture.hold('session/upload'));
    await release(page, 'session/create');
    await expect.poll(() => methods(page)).toContain('session/attach'); await release(page, 'session/attach');
    await expect.poll(() => methods(page)).toContain('session/upload');
  }
  await page.evaluate(method => (window as any).startupFixture.lose(method), method);
  await expect(page.getByRole('alert').last()).toContainText(method === 'session/create' ? 'uncertain' : 'committed');
  await page.evaluate(() => (window as any).startupFixture.remount());
  await page.evaluate(() => (window as any).startupFixture.reconnect());
  await expect(page.locator('textarea')).toHaveValue('Retained first input');
  if (method === 'session/upload') await expect(page.getByText('Upload outcome uncertain. Reconnect and inspect authoritative state; do not replay.', { exact: true })).toBeVisible();
  expect((await methods(page)).filter(m => m === method)).toHaveLength(1);
  expect((await methods(page)).filter(m => m === 'turn/start')).toHaveLength(0);
});

test('failed catalog refresh cannot reject attach or block admission; later native invalidation repairs it', async ({ page }) => {
  await begin(page); await release(page, 'session/create');
  await expect.poll(() => methods(page)).toContain('session/attach');
  await release(page, 'session/list', 'catalog unavailable');
  await release(page, 'session/attach');
  await expect.poll(() => methods(page)).toContain('turn/start'); await release(page, 'turn/start');
  await expect(page.locator('textarea')).toHaveValue('');
  await page.evaluate(() => { (window as any).startupFixture.allow('session/list'); (window as any).startupFixture.invalidateCatalog(); });
  await expect(page.locator('button[data-session-id=created]')).toBeVisible();
  expect((await methods(page)).filter(m => m === 'turn/start')).toHaveLength(1);
});

for (const intent of ['explicit', 'preference', 'omitted'] as const) test(`creation owns ${intent} model intent without a second mutation`, async ({ page }) => {
  await begin(page, 'en', false, intent === 'omitted' ? undefined : intent);
  const create = await page.evaluate(() => (window as any).startupFixture.requests().find((r: any) => r.method === 'session/create'));
  expect(create.params.settings.model).toEqual(intent === 'omitted' ? undefined : { model: 'fixture/second' });
  // A display-only default changed after draft render and request dispatch.
  if (intent === 'omitted') await page.evaluate(() => (window as any).startupFixture.defaultModel('fixture/second'));
  await release(page, 'session/create'); await expect.poll(() => methods(page)).toContain('session/attach'); await release(page, 'session/attach');
  await expect.poll(() => methods(page)).toContain('turn/start');
  expect(await page.evaluate(() => (window as any).startupFixture.model().configured.model)).toBe('fixture/second');
  const preference = await page.evaluate(() => localStorage.getItem('rustx-new-session-model-v1'));
  if (intent === 'omitted') expect(preference).toBeNull();
  else expect(JSON.parse(preference!)['ws://127.0.0.1:8080/']).toEqual({ model: 'fixture/second' });
  expect((await methods(page)).filter(m => m === 'session/setModel')).toHaveLength(0);
  expect((await methods(page)).filter(m => m === 'session/snapshot')).toHaveLength(0);
});

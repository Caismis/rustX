import { expect, test } from '@playwright/test';
declare global { interface Window { continuity: { publish(): void; changeGoal(): void; reads(): number; advancePrefix(): void; finishPrefix(): void; setRows(count: number): void; publishTools(): void; hydrateWorkflow(): void; reorderTasks(): void; previewState(): { loading: boolean; watched: boolean; pages: number }; probePreview(): void; finishPreview(): void; completeCode(): void; disconnectReading(): void; reconnectReading(): Promise<void>; readingState(): { connection: string; ready: boolean }; otherReading(): Promise<void>; holdFirst(): void; attachFirst(): void; ackFirst(): void; firstState(): { phase?: string; fileReferences?: number; pending: number; requests: number }; observeFirst(): Promise<void> } } }
const origin = 'http://127.0.0.1:' + (process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174);
test('a deferred Goal update keeps expanded hydrated Todo rows in place', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html');
  await page.getByRole('button', { name: /To-dos/ }).click();
  const rows = page.locator('[data-task-id]'); await expect(rows).toHaveCount(3);
  await rows.evaluateAll(elements => elements.forEach(element => element.setAttribute('data-retained-row', 'true')));
  const positions = await rows.evaluateAll(elements => elements.map(element => element.getBoundingClientRect().top));
  await page.evaluate(() => window.continuity.changeGoal());
  await expect(page.locator('[data-retained-row]')).toHaveCount(3);
  expect(await rows.evaluateAll(elements => elements.map(element => element.getBoundingClientRect().top))).toEqual(positions);
  expect(await page.evaluate(() => window.continuity.reads())).toBe(1);
  await expect(page.getByRole('button', { name: /To-dos/ })).toHaveAttribute('aria-expanded', 'true');
});
for (const mode of ['standard', 'compact']) test(`published reasoning keeps its expanded DOM and reading scroll in ${mode}`, async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html?mode=' + mode);
  await page.locator('[data-step-process] > button').click();
  await page.locator('[data-variant="think"] [data-disclosure-row]').click();
  const body = page.locator('[data-step-process-body]');
  await body.evaluate(element => { element.setAttribute('data-retained-body', 'true'); element.scrollTop = 96; });
  await expect.poll(() => body.evaluate(element => element.scrollTop)).toBe(96);
  const before = await body.evaluate(element => element.querySelector('[data-variant="think"]')!.getBoundingClientRect().top - element.getBoundingClientRect().top);
  await page.evaluate(() => window.continuity.publish());
  await expect(page.locator('[data-step-process] > button')).toHaveAttribute('aria-expanded', 'true');
  await expect(page.locator('[data-retained-body]')).toHaveCount(1);
  await expect(page.locator('[data-variant="think"]')).toHaveAttribute('data-expanded', 'true');
  expect(await body.evaluate(element => element.scrollTop)).toBe(96);
  // Publication can add the native Attempt header; the inner reading offset stays fixed.
  const offset = await body.evaluate(element => element.querySelector('[data-variant="think"]')!.getBoundingClientRect().top - element.getBoundingClientRect().top);
  expect(offset).toBe(before);
});
test('an advancing exact live reference preserves released text and its reading node during the read', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html');
  const body = page.locator('[data-live-prefix]');
  await expect.poll(() => body.textContent().then(text => text?.length)).toBe(30000);
  await body.evaluate(element => { element.setAttribute('data-retained-prefix', 'true'); element.scrollTop = 96; });
  await page.evaluate(() => window.continuity.advancePrefix());
  await expect(page.locator('[data-retained-prefix]')).toHaveCount(1);
  expect((await body.textContent())?.length).toBe(30000); expect(await body.evaluate(element => element.scrollTop)).toBe(96);
  await page.evaluate(() => window.continuity.finishPrefix());
  await expect.poll(() => body.textContent().then(text => text?.length)).toBe(30020);
  await expect(page.locator('[data-retained-prefix]')).toHaveCount(1);
  expect(await body.evaluate(element => element.scrollTop)).toBe(96);
});
test('off-tail Trace reading survives append, fold and filter transitions across the virtualization threshold', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html');
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await ledger.evaluate(element => { element.scrollTop = 900; element.dispatchEvent(new Event('scroll')); });
  for (const count of [101, 90, 101]) {
    await page.evaluate(count => window.continuity.setRows(count), count);
    await expect(ledger).toHaveAttribute('aria-rowcount', String(count));
    expect(await ledger.evaluate(element => element.scrollTop)).toBe(900);
  }
});
test('Trace still follows appends at the tail when virtualization is enabled', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html');
  const ledger = page.getByRole('table', { name: 'Trace ledger' });
  await ledger.evaluate(element => { element.scrollTop = element.scrollHeight; element.dispatchEvent(new Event('scroll')); });
  await page.evaluate(() => window.continuity.setRows(101));
  await expect(ledger).toHaveAttribute('aria-rowcount', '101');
  await expect.poll(() => ledger.evaluate(element => element.scrollHeight - element.clientHeight - element.scrollTop)).toBeLessThanOrEqual(2);
});
test('nested Tool disclosure survives a multi-message process merge', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html?scene=tools');
  await page.locator('[data-step-process] > button').last().click();
  const header = page.locator('[data-tool-call-id="second-call"] [data-disclosure-row]');
  await header.click(); await expect(header).toHaveAttribute('aria-expanded', 'true');
  await header.focus(); await header.evaluate(element => element.setAttribute('data-retained-tool-focus', 'true'));
  await page.evaluate(() => window.continuity.publishTools());
  await expect(page.locator('[data-step-process]')).toHaveCount(1);
  await expect(header).toHaveAttribute('aria-expanded', 'true'); await expect(header).toBeVisible();
  await expect(page.locator('[data-retained-tool-focus]')).toHaveCount(1); await expect(header).toBeFocused();
});
test('completed Workflow hydration retains manual expansion', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html?scene=workflow');
  const header = page.locator('[data-workflow-run-id] [data-disclosure-row]').first();
  await header.click(); await expect(header).toHaveAttribute('aria-expanded', 'true');
  await page.evaluate(() => window.continuity.hydrateWorkflow());
  await expect(header).toContainText('Nodes 2'); await expect(header).toHaveAttribute('aria-expanded', 'true');
});
test('a no-op file probe retains highlighted code and keyboard focus', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html?scene=preview');
  const code = page.locator('pre.shiki'); await expect(code).toBeVisible();
  await expect.poll(() => page.evaluate(() => window.continuity.previewState().watched)).toBe(true);
  await code.focus(); await code.evaluate(element => element.setAttribute('data-retained-code', 'true'));
  await page.evaluate(() => window.continuity.probePreview());
  await expect.poll(() => page.evaluate(() => window.continuity.previewState().loading)).toBe(true);
  await expect(page.locator('[data-retained-code]')).toHaveCount(1); await expect(code).toBeFocused();
  await page.evaluate(() => window.continuity.finishPreview());
  await expect.poll(() => page.evaluate(() => window.continuity.previewState().loading)).toBe(false);
  await expect(page.locator('[data-retained-code]')).toHaveCount(1); await expect(code).toBeFocused();
  expect(await page.evaluate(() => window.continuity.previewState().pages)).toBe(1);
});
test('mounted task measurements follow their identities through a catalog reorder', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html?scene=tasks');
  const tall = page.getByRole('button', { name: /Tall task/ }).locator('..'), short = page.getByRole('button', { name: /Short task/ }).locator('..');
  await tall.evaluate(element => { (element as HTMLElement).style.height = '120px'; element.setAttribute('data-retained-task', 'tall'); });
  await short.evaluate(element => { (element as HTMLElement).style.height = '60px'; element.setAttribute('data-retained-task', 'short'); });
  await expect.poll(() => short.evaluate(element => (element as HTMLElement).style.transform)).toBe('translateY(122px)');
  await page.evaluate(() => window.continuity.reorderTasks());
  await expect.poll(() => short.evaluate(element => (element as HTMLElement).style.transform)).toBe('translateY(0px)');
  await expect.poll(() => tall.evaluate(element => (element as HTMLElement).style.transform)).toBe('translateY(62px)');
  await expect(page.locator('[data-retained-task]')).toHaveCount(2);
});
test('conversation code completion keeps its focused rendering node', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html?scene=code-complete');
  const code = page.locator('pre.shiki'); await expect(code).toBeVisible(); await code.focus();
  await code.evaluate(element => element.setAttribute('data-completion-code', 'true'));
  await page.evaluate(() => window.continuity.completeCode());
  await expect(page.locator('[data-completion-code]')).toHaveCount(1); await expect(code).toBeFocused();
});
test('conversation detached reading survives disconnect and reconnect, while a new logical scope follows', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html?scene=reading');
  const port = page.locator('[data-conversation-scroll]');
  await port.evaluate(element => { element.scrollTop = 250; element.dispatchEvent(new Event('scroll')); element.dispatchEvent(new Event('scrollend')); });
  await expect(page.locator('[data-chat-latest]')).toBeVisible();
  await page.evaluate(() => window.continuity.disconnectReading());
  await expect.poll(() => page.evaluate(() => window.continuity.readingState().ready)).toBe(false);
  expect(await port.evaluate(element => element.scrollTop)).toBe(250);
  await page.evaluate(() => window.continuity.reconnectReading());
  await expect.poll(() => page.evaluate(() => window.continuity.readingState().ready)).toBe(true);
  expect(await port.evaluate(element => element.scrollTop)).toBe(250);
  await page.evaluate(() => window.continuity.otherReading());
  await expect.poll(() => port.evaluate(element => element.scrollHeight - element.clientHeight - element.scrollTop)).toBeLessThanOrEqual(2);
});
test('first-input ACK retains exactly one message until canonical observation takes it', async ({ page }) => {
  await page.goto(origin + '/test/fixtures/ui-continuity.html?scene=first');
  await page.getByRole('button', { name: 'Open Session B' }).click();
  await page.getByRole('textbox', { name: 'Message' }).fill('First message stays visible');
  await page.getByLabel('Attach files').setInputFiles({ name: 'first.txt', mimeType: 'text/plain', buffer: Buffer.from('attachment') });
  await page.evaluate(() => window.continuity.holdFirst());
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await page.evaluate(() => window.continuity.attachFirst());
  await expect.poll(() => page.evaluate(() => window.continuity.firstState().requests)).toBe(1);
  await expect(page.getByText('First message stays visible', { exact: true })).toHaveCount(1);
  await page.evaluate(() => window.continuity.ackFirst());
  await expect.poll(() => page.evaluate(() => window.continuity.firstState().phase)).toBe('admitted');
  await expect(page.getByText('First message stays visible', { exact: true })).toHaveCount(1);
  await expect(page.locator('[data-pending-message]').getByText('first.txt', { exact: true })).toHaveCount(1);
  expect(await page.evaluate(() => window.continuity.firstState().fileReferences)).toBe(0);
  await page.evaluate(() => window.continuity.observeFirst());
  await expect(page.locator('[data-pending-message]')).toHaveCount(0);
  await expect(page.getByText('First message stays visible', { exact: true })).toHaveCount(1);
});

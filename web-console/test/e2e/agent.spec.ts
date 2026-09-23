import { test, expect } from '@playwright/test';
import { expectStableScreenshot } from './screenshot';
// Each reference owns a fresh context/page, including its renderer paint caches.
for (const mode of ['settled', 'streaming', 'tools', 'error', 'approval', 'questionnaire', 'selectors']) test(`Harness Agent ${mode} reference uses native snapshots`, async ({ page }) => {
 const errors: string[] = [];
 page.on('pageerror', error => errors.push(error.message));
 await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
 await page.emulateMedia({ reducedMotion: 'reduce' });
   await page.goto(`http://127.0.0.1:5174/test/fixtures/agent.html?mode=${mode}`);
   await expect(page).toHaveTitle('rustX Agent reference');
   await expect(page.getByLabel('Canonical conversation')).toBeVisible();
   if (mode === 'error') {
     await page.locator('[data-tool-call-id="bash-1"]').getByRole('button').click();
     await page.locator('[data-tool-call-id="edit-1"]').getByRole('button').click();
   }
   if (mode === 'questionnaire') {
     await page.getByRole('radio', { name: 'Keep native' }).click();
     await page.getByRole('button', { name: 'Next question' }).click();
     await page.getByRole('checkbox', { name: 'Native contracts' }).click();
   }
   if (mode === 'selectors') {
     await expect(page.getByRole('button', { name: 'Approval mode' })).toHaveCount(0);
     await page.getByRole('button', { name: 'Model and reasoning' }).click();
     await expect(page.getByText('Reading native models…')).toHaveCount(0);
     await page.getByRole('menuitem', { name: 'Reasoning profile' }).click();
     await expect(page.getByRole('menuitem', { name: 'deliberate' })).toBeVisible();
   }
   await expectStableScreenshot(page, `agent-${mode}-light.png`);
 if (mode === 'selectors') {
 await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
 await page.getByRole('button', { name: 'Settings', exact: true }).click();
 await page.getByRole('button', { name: 'Appearance', exact: true }).click();
 await page.getByLabel('Theme', { exact: true }).selectOption('dark');
 await page.getByRole('button', { name: 'Close Settings' }).click();
 await expectStableScreenshot(page, 'agent-dark-desktop.png');
 await page.setViewportSize({ width: 390, height: 844 });
 await expectStableScreenshot(page, 'agent-dark-narrow.png');
 }
 expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
 await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

const composerURL = 'http://127.0.0.1:5174/test/fixtures/agent.html?mode=composer';
test('composer geometry grows, caps, scrolls once and shrinks across desktop and narrow widths', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => { window.addEventListener('error', event => { if (event.message.includes('ResizeObserver')) document.documentElement.dataset.resizeError = event.message; }); });
  await page.goto(composerURL);
  await expect(page).toHaveTitle('rustX Agent reference');
  const input = page.getByRole('textbox', { name: 'Message', exact: true });
  const geometry = () => input.evaluate(element => {
    const css = getComputedStyle(element);
    const parents = []; let parent = element.parentElement;
    while (parent && !parent.hasAttribute('data-composer-context-stack')) {
      const style = getComputedStyle(parent);
      parents.push({ overflow: style.overflowY, height: parent.clientHeight, scroll: parent.scrollHeight }); parent = parent.parentElement;
    }
    return { height: element.getBoundingClientRect().height, cap: parseFloat(css.maxHeight), min: parseFloat(css.minHeight),
      overflow: css.overflowY, scroll: element.scrollHeight, client: element.clientHeight, parents };
  });
  for (const width of [1440, 390]) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await input.fill('');
    await expect.poll(async () => (await geometry()).height).toBe(36);
    expect((await geometry()).min).toBe(36);
    await expect(input).toHaveAttribute('rows', '1');
    await input.fill('First line\nSecond line\nThird line');
    await expect.poll(async () => (await geometry()).height).toBeGreaterThan(60);
    expect((await geometry()).height).toBeLessThan((await geometry()).cap);
    const long = Array.from({ length: 40 }, (_, i) => `Line ${i}: keep native admission and cancellation authority`).join('\n');
    await input.fill(long);
    await expect.poll(async () => { const g = await geometry(); return g.height === g.cap && g.scroll > g.client; }).toBe(true);
    const capped = await geometry(); expect(capped.overflow).toBe('auto');
    expect(capped.parents.every(parent => !['auto', 'scroll'].includes(parent.overflow))).toBe(true);
    await input.evaluate(element => { element.scrollTop = element.scrollHeight; });
    expect(await input.evaluate(element => element.scrollTop)).toBeGreaterThan(0);
    await input.fill('short');
    await expect.poll(async () => (await geometry()).height).toBe(36);
    expect(await input.evaluate(element => element.scrollTop)).toBe(0);
    await input.fill('/mdl'); await expect(page.getByRole('listbox', { name: 'Commands' })).toBeVisible();
    expect((await geometry()).height).toBe(36);
    await page.keyboard.press('Escape');
  }
  // Wrapping responds to actual editor width, including the sidebar/mobile axis.
  await page.setViewportSize({ width: 1440, height: 1000 });
  await input.fill('Content driven growth responds to a narrower composer without resetting the draft. '.repeat(4));
  const desktop = (await geometry()).height;
  await page.setViewportSize({ width: 390, height: 844 });
  await expect.poll(async () => (await geometry()).height).toBeGreaterThan(desktop);
  expect(await page.evaluate(() => document.documentElement.dataset.resizeError)).toBeUndefined();
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

for (const theme of ['light', 'dark'] as const) for (const width of [1440, 390]) {
  test(`composer primary seat, uploads and context stack ${theme} ${width}`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.addInitScript(theme => localStorage.setItem('rustx-appearance-v1', theme), theme);
    await page.goto(composerURL);
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    const stack = page.locator('[data-composer-context-stack]');
    const primary = page.locator('[data-composer-primary]');
    const shot = async (state: string) => expectStableScreenshot(stack, `composer-${state}-${theme}-${width}.png`);
    await expect(input).toBeVisible(); await expect(primary).toHaveCount(1);
    if (width === 1440) {
      await expect.poll(async () => {
        const left = (await page.getByRole('button', { name: 'Commands', exact: true }).boundingBox())!;
        const right = (await primary.boundingBox())!;
        return Math.abs((left.y + left.height / 2) - (right.y + right.height / 2));
      }).toBeLessThanOrEqual(3);
    }
    await expect(primary).toHaveAccessibleName('Send'); await expect(primary).toBeDisabled();
    await expect(page.getByLabel('Delivery', { exact: true })).toHaveCount(0);
    await shot('idle-empty');
    await input.fill('Review the composer interaction contract.');
    await expect(primary).toHaveAccessibleName('Send'); await expect(primary).toBeEnabled();
    await shot('idle-draft');
    await primary.click(); await expect(input).toHaveValue('');
    expect(await page.evaluate(() => window.composerFixture.submissions())).toEqual(['turn/start']);
    await page.evaluate(() => window.composerFixture.running(true));
    await expect(primary).toHaveAccessibleName('Stop'); await shot('running-empty');
    await input.fill('Queue the next review.');
    await expect(primary).toHaveAccessibleName('Queue'); await expect(page.getByRole('button', { name: 'Stop', exact: true })).toHaveCount(0);
    await shot('running-draft');
    await input.press('Enter'); await expect(input).toHaveValue('');
    await input.fill('Steer through the existing native operation.'); await input.press('Control+Enter'); await expect(input).toHaveValue('');
    expect(await page.evaluate(() => window.composerFixture.submissions())).toEqual(['turn/start', 'turn/start', 'turn/steer']);
    // A real picker gesture and receipt path; the toolbar accessory stays quiet.
    const chooser = page.waitForEvent('filechooser'); await page.getByRole('button', { name: 'Add attachments' }).click();
    await (await chooser).setFiles({ name: 'review.txt', mimeType: 'text/plain', buffer: Buffer.from('Review notes') });
    await expect(page.getByText('Uploaded', { exact: true })).toBeVisible();
    await input.fill('Review these notes.'); await shot('attachment');
    await page.getByRole('button', { name: 'Remove review.txt' }).click();
    await expect(input).toHaveValue('Review these notes.');
    await page.evaluate(() => window.composerFixture.docks(true));
    const order = await stack.locator(':scope > *').evaluateAll(nodes => nodes.map(node => node.getAttribute('aria-label') ?? 'Composer'));
    expect(order).toEqual(['To-dos', 'Goal', 'Queue', 'Composer']);
    await shot('context');
    await page.evaluate(() => window.composerFixture.docks(false));
    await expect(input).toHaveValue('Review these notes.');
    for (const name of ['Commands', 'Add attachments', 'Model and reasoning']) await expect(page.getByRole('button', { name, exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Approval mode', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Model and reasoning' }).click();
    await expect(page.getByText('Reading native models…')).toHaveCount(0); await page.keyboard.press('Escape');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  });
}


test('restored Session-scoped drafts are measured on mount and shrink on identity switch', async ({ page }) => {
  await page.goto('http://127.0.0.1:5174/test/fixtures/agent.html?mode=restored');
  const input = page.getByRole('textbox', { name: 'Message', exact: true });
  await expect(input).toHaveValue(/Restored line 0/);
  await expect.poll(() => input.evaluate(node => node.clientHeight === parseFloat(getComputedStyle(node).maxHeight) && node.scrollHeight > node.clientHeight)).toBe(true);
  await page.getByRole('button', { name: 'Switch Session' }).click();
  await expect(input).toHaveValue('Other Session draft');
  await expect.poll(() => input.evaluate(node => node.getBoundingClientRect().height)).toBe(36);
  await page.getByRole('button', { name: 'Switch Session' }).click();
  await expect(input).toHaveValue(/Restored line 39/);
  await expect.poll(() => input.evaluate(node => node.clientHeight === parseFloat(getComputedStyle(node).maxHeight))).toBe(true);
});

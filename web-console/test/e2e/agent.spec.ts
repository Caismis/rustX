const fixtureOrigin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
import { test, expect } from '@playwright/test';
import { expectStableScreenshot } from './screenshot';
import { compareScreenshot, decodePng } from '../screenshot-comparator';
// Each reference owns a fresh context/page, including its renderer paint caches.
for (const mode of ['settled', 'streaming', 'tools', 'error', 'approval', 'questionnaire', 'selectors']) test(`Harness Agent ${mode} reference uses native snapshots`, async ({ page }) => {
 const errors: string[] = [];
 page.on('pageerror', error => errors.push(error.message));
 await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
 await page.emulateMedia({ reducedMotion: 'reduce' });
   await page.goto(`${fixtureOrigin}/test/fixtures/agent.html?mode=${mode}`);
   await expect(page).toHaveTitle('rustX Agent reference');
   await expect(page.getByLabel('Canonical conversation')).toBeVisible();
   if (mode === 'error') {
     await page.locator('[data-tool-call-id="bash-1"]').getByRole('button').click();
     await page.locator('[data-tool-call-id="edit-1"]').getByRole('button').click();
   }
   if (mode === 'questionnaire') {
     // A single-choice answer advances to the next question.
     await page.getByRole('radio', { name: 'Keep native' }).click();
     await page.getByRole('checkbox', { name: 'Native contracts' }).click();
   }
   if (mode === 'selectors') {
     await expect(page.getByRole('button', { name: 'Approval mode' })).toHaveCount(0);
     await page.getByRole('button', { name: 'Model and reasoning' }).click();
     await expect(page.getByText('Loading models…')).toHaveCount(0);
     await page.getByRole('menuitem', { name: 'Reasoning profile' }).click();
     await expect(page.getByRole('menuitem', { name: 'deliberate' })).toBeVisible();
   }
   await expectStableScreenshot(page, `agent-${mode}-light.png`);
 if (mode === 'selectors') {
 await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
 await page.getByRole('button', { name: 'Settings', exact: true }).click();
 await page.getByRole('group', { name: 'Appearance', exact: true }).getByRole('button', { name: 'Dark', exact: true }).click();
 await page.getByRole('button', { name: 'Close Settings' }).click();
 await expectStableScreenshot(page, 'agent-dark-desktop.png');
 await page.setViewportSize({ width: 390, height: 844 });
 await expectStableScreenshot(page, 'agent-dark-narrow.png');
 }
 expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
 await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

const composerURL = `${fixtureOrigin}/test/fixtures/agent.html?mode=composer`;
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
    const shot = async (state: string) => {
      // This fixture compares dock geometry in the latest-reading pose. A
      // prior button's automatic scroll can leave a half-pixel reader offset.
      const reader = page.locator('[data-conversation-scroll]').first();
      await reader.evaluate(element => { element.scrollTop = element.scrollHeight; });
      await expect.poll(() => reader.evaluate(element => element.scrollTop === element.scrollHeight - element.clientHeight)).toBe(true);
      if (state === 'context' && theme === 'dark' && width === 390) {
        // Bind the native model/profile labels to the intended centered toolbar
        // seat as well as their exact catalog text and font metrics.
        const labels = await stack.evaluate(root => {
          const origin = root.getBoundingClientRect();
          return [...root.querySelectorAll('[data-model-select] > span')].map(node => {
            const r = node.getBoundingClientRect(), s = getComputedStyle(node);
            return { text: node.textContent, x: r.x-origin.x, y: r.y-origin.y, width:r.width, height:r.height,
              fontSize:s.fontSize, fontWeight:s.fontWeight, lineHeight:s.lineHeight, color:s.color, transform:s.transform };
          });
        });
        expect(labels).toEqual([
          { text:'native/coder', x:119, y:271, width:70.8125, height:20, fontSize:'13px', fontWeight:'500', lineHeight:'20px', color:'rgb(207, 211, 214)', transform:'none' },
          { text:'deliberate', x:193.8125, y:271, width:50.6875, height:20, fontSize:'13px', fontWeight:'500', lineHeight:'20px', color:'rgb(129, 133, 140)', transform:'none' },
        ]);
      }
      await expectStableScreenshot(stack, `composer-${state}-${theme}-${width}.png`);
    };
    await expect(input).toBeVisible(); await expect(primary).toHaveCount(1);
    const plus = page.getByRole('button', { name: 'Add', exact: true });
    expect(await plus.evaluate(el => {
      const button = el.getBoundingClientRect(), icon = el.querySelector('svg')!.getBoundingClientRect();
      return [icon.x + icon.width / 2 - button.x - button.width / 2, icon.y + icon.height / 2 - button.y - button.height / 2];
    })).toEqual([0, 0]);
    if (width === 1440) {
      await expect.poll(async () => {
        const left = (await page.getByRole('button', { name: 'Add', exact: true }).boundingBox())!;
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
    await page.getByRole('button', { name: 'Add', exact: true }).click();
    const chooser = page.waitForEvent('filechooser'); await page.locator('#command-file').click();
    await (await chooser).setFiles({ name: 'review.txt', mimeType: 'text/plain', buffer: Buffer.from('Review notes') });
    await expect(page.getByText('Uploaded', { exact: true })).toBeVisible();
    await input.fill('Review these notes.'); await shot('attachment');
    await page.getByRole('button', { name: 'Remove review.txt' }).click();
    await expect(input).toHaveValue('Review these notes.');
    await page.evaluate(() => window.composerFixture.docks(true));
    const order = await stack.locator(':scope > *').evaluateAll(nodes => nodes.map(node => (node.hasAttribute('data-context-seat') ? 'Context' : node.getAttribute('aria-label')) ?? 'Composer'));
    expect(order).toEqual(['To-dos', 'Goal', 'Queue', 'Composer']);
    // Repaint both labels while the footer is sticky, then return to the same
    // reading position. A 29px inline baseline wrapper around the 28px model
    // button used to leave a half-pixel local text origin: Chromium could keep
    // the sticky-pose glyph raster after scrolling, moving it by one pixel.
    // Compare actual renderings, with no baseline or noise allowance, so a
    // stable but history-dependent raster cannot pass this regression.
    if (width === 390) {
      const reader = page.locator('[data-conversation-scroll]').first();
      const paint = () => page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
      await reader.evaluate(element => { element.scrollTop = element.scrollHeight; });
      await paint();
      const model = page.locator('[data-model-select]');
      const geometry = await model.boundingBox();
      const before = await model.screenshot({ animations: 'disabled', caret: 'hide', scale: 'css' });
      await reader.evaluate(element => { element.scrollTop = 0; });
      await paint();
      // Controlled paint invalidation, not a timer or a retry against a reference.
      await model.locator(':scope > span').evaluateAll(nodes => nodes.forEach(node => { (node as HTMLElement).style.color = 'red'; }));
      await paint();
      await model.locator(':scope > span').evaluateAll(nodes => nodes.forEach(node => { (node as HTMLElement).style.removeProperty('color'); }));
      await paint();
      await reader.evaluate(element => { element.scrollTop = element.scrollHeight; });
      await paint();
      expect(await model.boundingBox()).toEqual(geometry);
      const after = await model.screenshot({ animations: 'disabled', caret: 'hide', scale: 'css' });
      const comparison = compareScreenshot({ expected: decodePng(before), actual: decodePng(after), referenceName: 'model-sticky-repaint', noisePolicy: [] });
      await test.info().attach('model-before-sticky-repaint', { body: before, contentType: 'image/png' });
      await test.info().attach('model-after-sticky-repaint', { body: after, contentType: 'image/png' });
      expect(comparison.totalChanged, comparison.report).toBe(0);
    }
    await shot('context');
    await page.evaluate(() => window.composerFixture.docks(false));
    await expect(input).toHaveValue('Review these notes.');
    for (const name of ['Add', 'Model and reasoning']) await expect(page.getByRole('button', { name, exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Approval mode', exact: true })).toHaveCount(0);
    await page.getByRole('button', { name: 'Model and reasoning' }).click();
    await expect(page.getByText('Loading models…')).toHaveCount(0); await page.keyboard.press('Escape');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  });
}


test('restored Session-scoped drafts are measured on mount and shrink on identity switch', async ({ page }) => {
  await page.goto(`${fixtureOrigin}/test/fixtures/agent.html?mode=restored`);
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

/** The composer's model/profile menu is the product's submenu user: on a
 * phone-width viewport both of its layers stay inside the viewport, the page
 * never scrolls sideways, and keyboard and pointer both reach the profiles. */
test('ModelSelect submenus stay usable inside a narrow viewport', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${fixtureOrigin}/test/fixtures/agent.html?mode=selectors`);
  const trigger = page.getByRole('button', { name: 'Model and reasoning' });
  const profile = page.getByRole('menuitem', { name: 'Reasoning profile' });
  const submenu = page.getByRole('menu').filter({ has: page.getByRole('menuitem', { name: 'deliberate' }) });
  /** Whether a surface lies wholly inside the viewport's 12px margin, and the page still has no horizontal scroll. */
  const contained = (locator: typeof submenu) => locator.evaluate(el => {
    const r = el.getBoundingClientRect();
    return r.left >= 11.5 && r.top >= 11.5 && r.right <= innerWidth - 11.5 && r.bottom <= innerHeight - 11.5 && document.documentElement.scrollWidth <= innerWidth;
  });

  // Keyboard: the menu opens on Model, the arrows reach the profile row, and
  // ArrowRight enters its submenu.
  await trigger.focus(); await page.keyboard.press('Enter');
  await expect(page.getByText('Loading models…')).toHaveCount(0);
  await expect(page.getByRole('menuitem', { name: 'Model', exact: true })).toBeFocused();
  await page.keyboard.press('ArrowDown'); await expect(profile).toBeFocused();
  await page.keyboard.press('ArrowRight'); await expect(page.getByRole('menuitem', { name: 'deliberate' })).toBeFocused();
  expect(await contained(submenu)).toBe(true);
  await page.keyboard.press('ArrowDown'); await expect(page.getByRole('menuitem', { name: 'brief' })).toBeFocused();
  // Escape closes the submenu first, then the menu, and focus returns to the trigger.
  await page.keyboard.press('Escape'); await expect(submenu).toHaveCount(0); await expect(profile).toBeFocused();
  await page.keyboard.press('Escape'); await expect(page.getByRole('menu')).toHaveCount(0); await expect(trigger).toBeFocused();

  // Pointer: the same submenu opens from a click, inside the viewport.
  await trigger.click(); await profile.click();
  await expect(page.getByRole('menuitem', { name: 'brief' })).toBeVisible();
  expect(await contained(submenu)).toBe(true);
  await page.getByRole('menuitem', { name: 'brief' }).hover();
  await expect(submenu).toBeVisible();
  await page.keyboard.press('Escape'); await expect(page.getByRole('menu')).toHaveCount(0); await expect(trigger).toBeFocused();
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});


for (const locale of ['en', 'zh'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 390]) {
  test(`scoped Composer keyboard policy ${locale} ${theme} ${width}`, async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.addInitScript(({ locale, theme }) => {
      localStorage.setItem('rustx-locale-v1', locale); localStorage.setItem('rustx-appearance-v1', theme);
      localStorage.removeItem('rustx-composer-busy-enter-v1');
    }, { locale, theme });
    await page.goto(composerURL); await expect(page).toHaveTitle('rustX Agent reference');
    const names = locale === 'en' ? { input: 'Message', queue: 'Queue', steer: 'Steer', send: 'Send', commands: 'Add', attach: 'Add' }
      : { input: '消息', queue: '排队', steer: '插话', send: '发送', commands: '添加', attach: '添加' };
    const input = page.getByRole('textbox', { name: names.input, exact: true });
    const primary = page.locator('[data-composer-primary]');
    // The busy-Enter preference is configured in General Settings.
    await expect(page.getByRole('button', { name: /Enter while running|运行时 Enter/ })).toHaveCount(0);
    await page.evaluate(() => window.composerFixture.running(true));
    await input.fill('Queue by default'); await expect(primary).toHaveAccessibleName(names.queue);
    await input.press('Enter'); await expect(input).toHaveValue('');
    await input.fill('Complementary steer'); await input.press('Control+Enter'); await expect(input).toHaveValue('');
    await input.fill('Queue from primary'); await primary.click(); await expect(input).toHaveValue('');
    expect(await page.evaluate(() => window.composerFixture.submissions())).toEqual(['turn/start', 'turn/steer', 'turn/start']);
    await page.getByRole('button', { name: locale === 'en' ? 'Settings' : '设置', exact: true }).click();
    const settings = page.getByRole('dialog', { name: locale === 'en' ? 'Settings' : '设置', exact: true });
    await settings.getByRole('button', { name: locale === 'en' ? /Send behavior while busy/ : /繁忙时的发送行为/ }).click();
    await page.getByRole('option', { name: locale === 'en' ? 'Immediately' : '立即', exact: true }).click();
    await page.getByRole('button', { name: locale === 'en' ? 'Close Settings' : '关闭设置', exact: true }).click();
    expect(await page.evaluate(() => localStorage.getItem('rustx-composer-busy-enter-v1'))).toBe('steer');
    await input.fill('Preferred steer'); await expect(primary).toHaveAccessibleName(names.steer);
    await input.press('Enter'); await expect(input).toHaveValue('');
    await input.fill('Complementary queue'); await input.press('Meta+Enter'); await expect(input).toHaveValue('');
    await input.fill('Steer from primary'); await primary.click(); await expect(input).toHaveValue('');
    expect(await page.evaluate(() => window.composerFixture.submissions())).toEqual(['turn/start', 'turn/steer', 'turn/start', 'turn/steer', 'turn/start', 'turn/steer']);
    await input.fill('Line one'); await input.press('Shift+Enter'); await page.keyboard.type('Line two');
    await expect(input).toHaveValue('Line one\nLine two'); await expect(primary).toHaveAccessibleName(names.steer);
    await input.focus(); await expect(input).toBeFocused();
    await page.screenshot({ path: `/tmp/rustx-439-${locale}-${theme}-${width}.png` });
    await page.getByRole('button', { name: names.commands, exact: true }).click();
    await expect(page.getByRole('listbox')).toBeVisible(); await input.press('Escape');
    await expect(page.getByRole('listbox')).toHaveCount(0);
    // A dismissed menu's Escape cannot be the first press of cancellation.
    await input.press('Escape'); expect(await page.evaluate(() => window.composerFixture.cancellations())).toEqual([]);
    // Focus outside the supported editor invalidates that arm.
    await page.getByRole('button', { name: names.commands, exact: true }).focus(); await page.keyboard.press('Escape'); await input.focus();
    await page.evaluate(() => window.composerFixture.holdCancellation());
    await input.press('Escape'); await input.press('Escape');
    await expect.poll(() => page.evaluate(() => window.composerFixture.cancellations())).toHaveLength(1);
    expect(await page.evaluate(() => window.composerFixture.cancellations())).toEqual([{ session_id: 'A', conversation_id: 'conversation-A', runtime_incarnation: '9007199254740993', attachment_id: 'A-1' }]);
    await input.press('Escape'); await input.press('Escape');
    expect(await page.evaluate(() => window.composerFixture.cancellations())).toHaveLength(1); await expect(input).toHaveValue('Line one\nLine two');
    await page.evaluate(() => window.composerFixture.acknowledgeCancellation()); await expect(primary).toHaveAccessibleName(names.send);
    await expect(input).toHaveValue('Line one\nLine two');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  });
}

test('Busy Enter preference is read from browser storage after a page reload', async ({ page }) => {
  await page.goto(composerURL);
  await page.evaluate(() => window.composerFixture.busyEnter('steer'));
  expect(await page.evaluate(() => localStorage.getItem('rustx-composer-busy-enter-v1'))).toBe('steer');
  await page.reload();
  const primary = page.locator('[data-composer-primary]'), input = page.getByRole('textbox', { name: 'Message', exact: true });
  await input.fill('Idle after reload'); await expect(primary).toHaveAccessibleName('Send');
  await page.evaluate(() => window.composerFixture.running(true)); await expect(primary).toHaveAccessibleName('Steer');
  await input.press('Enter'); await expect(input).toHaveValue('');
  expect(await page.evaluate(() => window.composerFixture.submissions())).toEqual(['turn/steer']);
});

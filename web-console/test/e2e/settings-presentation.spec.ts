/** Settings visual convergence and browser acceptance (#393).
 *
 * Every reference below is a real rendered state of the deterministic Settings
 * fixture (`test/fixtures/settings.tsx`, variants named in its query string),
 * and every one is paired with the interaction and geometry assertions that
 * make it evidence rather than a picture. axe runs against the same states as
 * supplementary evidence; it never replaces the explicit assertions. */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { expectStableScreenshot } from './screenshot';
import { choose, closeSettings, openSettingsPage, openWorkspaceSettings, selectedSettingsPage, settingsSectionMenu } from './shell-actions';

const fixture = 'http://127.0.0.1:5174/test/fixtures/settings.html';
const pages = ['General', 'Models', 'Agent', 'Tools & Permissions', 'Extensions', 'Advanced'];
const longProvider = 'enterprise-inference-gateway-eu-central-primary-with-an-exceptionally-long-provider-identity';
const longModel = 'enterprise-reasoning-model-2026-09-long-context-preview-with-an-exceptionally-long-identity';

async function start(page: Page, query = '', size = { width: 1440, height: 1000 }) {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.setViewportSize(size);
  await page.goto(fixture + query);
  await expect(page).toHaveTitle('rustX native Settings reference');
  return errors;
}
const dialog = (page: Page) => page.getByRole('dialog', { name: 'Settings', exact: true });
async function openUserSettings(page: Page) {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(dialog(page)).toBeVisible();
}
async function setTheme(page: Page, theme: 'Light' | 'Dark') {
  await openUserSettings(page);
  await choose(dialog(page), 'Theme', theme);
  await closeSettings(page);
}

/** No horizontal page overflow, and nothing inside the Settings panel is
 * pushed past its edges — `.options` clips on the inline axis, so clipped
 * overflow is checked element by element, not only by scroll width. Only a
 * control that scrolls on its own (a `pre`, a table) may be wider. */
async function expectNoHorizontalOverflow(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  if (!await dialog(page).isVisible()) return;
  const escaped = await dialog(page).evaluate(root => {
    const panel = root.getBoundingClientRect();
    // A control that scrolls on its own, or a visually hidden element (React
    // Aria's native form proxies are clipped to nothing), is not overflow.
    const exempt = (el: Element) => {
      for (let at: Element | null = el; at && at !== root; at = at.parentElement) {
        const style = getComputedStyle(at);
        if (at !== el && /auto|scroll/.test(style.overflowX)) return true;
        if (style.clipPath !== 'none' || (style.clip !== 'auto' && style.clip !== '')) return true;
      }
      return false;
    };
    return [...root.querySelectorAll('*')].filter(el => {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0 || exempt(el)) return false;
      return rect.left < panel.left - 1 || rect.right > panel.right + 1;
    }).map(el => `${el.tagName.toLowerCase()} ${el.getAttribute('aria-label') ?? el.textContent?.slice(0, 40)}`);
  });
  expect(escaped).toEqual([]);
}

/** axe over one rendered scope. Serious and critical violations are
 * regressions; no rule is disabled. */
async function expectAccessible(page: Page, scope: string) {
  const results = await new AxeBuilder({ page }).include(scope).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const serious = results.violations.filter(violation => violation.impact === 'serious' || violation.impact === 'critical');
  expect(serious.map(violation => `${violation.id}: ${violation.nodes.map(node => node.target.join(' ')).join(' | ')}`)).toEqual([]);
}
const settingsScope = '[role="dialog"][aria-label="Settings"]';

/** Measured panel geometry, in CSS pixels. */
async function panelBox(page: Page) {
  return dialog(page).evaluate(el => {
    const panel = el.parentElement!, rect = panel.getBoundingClientRect(), style = getComputedStyle(panel);
    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height, radius: style.borderTopLeftRadius };
  });
}

test('desktop: six pages share one stable Harness frame, distinct icons and one row vocabulary', async ({ page }) => {
  const errors = await start(page);
  await openUserSettings(page);
  const settings = dialog(page);
  const frame = await panelBox(page);
  // Bounded product geometry: ~1000 wide, min(820, viewport − 48) high, r24.
  expect(frame.width).toBeGreaterThanOrEqual(960); expect(frame.width).toBeLessThanOrEqual(1040);
  expect(frame.height).toBe(820); expect(frame.radius).toBe('24px');
  const rail = page.getByRole('tablist', { name: 'Settings pages' });
  await expect(rail).toHaveAttribute('aria-orientation', 'vertical');
  await expect(rail.getByRole('tab')).toHaveText(pages);
  await expect(settingsSectionMenu(page)).toBeHidden();
  const navigation = await page.getByRole('navigation', { name: 'Settings navigation' }).evaluate(nav => nav.getBoundingClientRect().width);
  expect(navigation).toBeGreaterThanOrEqual(210); expect(navigation).toBeLessThanOrEqual(224);
  const cell = await rail.getByRole('tab', { name: 'Models' }).evaluate(el => ({ height: el.getBoundingClientRect().height, radius: getComputedStyle(el).borderTopLeftRadius, font: getComputedStyle(el).fontSize }));
  expect(cell).toEqual({ height: 40, radius: '12px', font: '14px' });
  // Six distinct glyphs; the label, not the glyph, names each page.
  const glyphs = await rail.getByRole('tab').evaluateAll(tabs => tabs.map(tab => tab.querySelector('svg')!.innerHTML));
  expect(new Set(glyphs).size).toBe(6);
  for (const name of pages) await expect(rail.getByRole('tab', { name, exact: true })).toHaveAccessibleName(name);

  const references: Record<string, string> = { General: 'settings-general-light.png', Models: 'settings-models-light.png', Agent: 'settings-agent-light.png', 'Tools & Permissions': 'settings-tools-light.png', Extensions: 'settings-extensions-light.png' };
  for (const name of pages) {
    await openSettingsPage(page, name);
    // One stable modal frame: a page change never resizes or moves it.
    expect(await panelBox(page)).toEqual(frame);
    await expect(settings.getByRole('heading', { level: 3, name, exact: true })).toBeVisible();
    // The owner and observation state live in the fixed header on every page.
    await expect(settings.getByRole('heading', { level: 2, name: 'User Settings' })).toBeVisible();
    await expect(settings.locator('[data-lifecycle="ready"]')).toHaveText('Authoritative source observed');
    await expectNoHorizontalOverflow(page);
    await expectAccessible(page, settingsScope);
    if (references[name]) await expectStableScreenshot(settings, references[name]);
  }

  // Setting rows and resource rows share one geometry.
  await openSettingsPage(page, 'Agent');
  const unit = await settings.getByRole('form', { name: 'Root identity' }).evaluate(el => {
    const style = getComputedStyle(el);
    return { height: el.getBoundingClientRect().height, padding: `${style.paddingTop} ${style.paddingLeft}`, radius: style.borderTopLeftRadius };
  });
  expect(unit.height).toBeGreaterThanOrEqual(56); expect(unit.padding).toBe('12px 16px'); expect(unit.radius).toBe('16px');
  await openSettingsPage(page, 'Models');
  const row = await settings.getByRole('row', { name: 'transport', exact: true }).evaluate(el => {
    const style = getComputedStyle(el);
    return { height: el.getBoundingClientRect().height, padding: `${style.paddingTop} ${style.paddingLeft}`, radius: style.borderTopLeftRadius };
  });
  expect(row.height).toBeGreaterThanOrEqual(56); expect(row.padding).toBe('12px 16px'); expect(row.radius).toBe('14px');

  // Provider → detail keeps status and actions next to the object.
  await settings.getByRole('row', { name: 'transport', exact: true }).click();
  await expect(settings.getByRole('heading', { name: 'Provider transport', exact: true })).toBeVisible();
  await expect(settings.getByRole('button', { name: 'Remove Provider transport', exact: true })).toBeVisible();
  await expect(settings.getByRole('button', { name: '← Models', exact: true })).toBeVisible();
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(settings, 'settings-provider-detail-light.png');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('Workspace inherited and overridden units, restore versus deletion, nested Escape ownership', async ({ page }) => {
  const errors = await start(page);
  const settings = dialog(page);
  await openWorkspaceSettings(page, 'Workspace A');
  await expect(settings.getByRole('heading', { level: 2, name: 'Workspace Settings — Workspace A' })).toBeVisible();
  // Inherited: displayed, authored nothing, no draft, offers Override only.
  await openSettingsPage(page, 'Tools & Permissions');
  const approval = settings.getByRole('form', { name: 'Approval mode' });
  await expect(approval.locator('[data-authored="absent"]')).toContainText('Inherited — no Workspace override');
  await expect(approval.getByRole('button', { name: 'Save Approval mode' })).toBeDisabled();
  await expect(approval.getByRole('button', { name: 'Override Approval mode' })).toBeVisible();
  await expect(approval.getByRole('button', { name: /Use global default/ })).toHaveCount(0);
  await approval.scrollIntoViewIfNeeded();
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(settings, 'settings-workspace-inherited-light.png');

  // Overridden and dirty: the draft's Save / Discard row sticks in reach.
  await openSettingsPage(page, 'Agent');
  const identity = settings.getByRole('form', { name: 'Root identity' });
  await expect(identity.locator('[data-authored="present"]')).toBeVisible();
  await identity.getByLabel('Agent identity').fill('rustx-workspace-reviewer');
  await expect(identity).toHaveAttribute('data-draft', 'true');
  await expect(identity.getByRole('button', { name: 'Save Root identity' })).toBeEnabled();
  await expect(identity.getByRole('button', { name: 'Discard draft' })).toBeVisible();
  await expectStableScreenshot(settings, 'settings-workspace-override-light.png');
  await identity.getByRole('button', { name: 'Discard draft' }).click();
  await expect(identity).not.toHaveAttribute('data-draft', 'true');

  // Restoring inheritance is an ordinary dialog, not a deletion; Escape closes
  // only that topmost layer and returns focus to its trigger inside Settings.
  const restore = identity.getByRole('button', { name: 'Use global default Root identity', exact: true });
  await restore.click();
  const confirmation = page.getByRole('dialog', { name: 'Use the global default for Root identity?' });
  await expect(confirmation).toBeVisible();
  await expect(page.getByRole('alertdialog')).toHaveCount(0);
  await expect(confirmation.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await page.keyboard.press('Tab'); await page.keyboard.press('Tab');
  expect(await confirmation.evaluate(el => el.contains(document.activeElement))).toBe(true);
  await page.keyboard.press('Escape');
  await expect(confirmation).toHaveCount(0);
  await expect(settings).toBeVisible();
  await expect(restore).toBeFocused();
  const writes = () => page.evaluate(() => (window as unknown as { rustxNativeRequests: () => { method: string }[] }).rustxNativeRequests().filter(request => request.method === 'configuration/sourceWrite').length);
  expect(await writes()).toBe(0);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('deletion confirmation, CAS conflict review, loading and read failure are designed states', async ({ page }) => {
  let errors = await start(page, '?scenario=conflict');
  const settings = dialog(page);
  await openUserSettings(page);
  await openSettingsPage(page, 'Models');
  await settings.getByRole('row', { name: 'transport', exact: true }).click();
  // A real deletion: an alert dialog whose confirm action is destructive.
  await settings.getByRole('button', { name: 'Remove Provider transport', exact: true }).click();
  const deletion = page.getByRole('alertdialog', { name: 'Remove Provider transport from User configuration?' });
  await expect(deletion).toBeVisible();
  await expect(deletion.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await expectAccessible(page, '[role="alertdialog"]');
  await expectStableScreenshot(page, 'settings-delete-confirm-light.png');
  await deletion.getByRole('button', { name: 'Cancel' }).click();
  await expect(deletion).toHaveCount(0);

  // CAS conflict: the draft and its base survive, the review is local to the
  // unit, and Save / Discard / Use reviewed revision stay in reach.
  const endpoint = settings.getByLabel('Endpoint', { exact: true });
  await endpoint.fill('https://user.invalid/v2');
  const unit = settings.getByRole('form', { name: 'Provider transport' });
  await unit.getByRole('button', { name: 'Save Provider transport' }).click();
  await expect(unit.getByRole('alert')).toHaveText('Provider transport was not saved: the source changed. Your draft and base revision are preserved.');
  await expect(endpoint).toHaveValue('https://user.invalid/v2');
  await expect(unit.getByRole('status').filter({ hasText: 'Source revision changed.' })).toBeVisible();
  await endpoint.scrollIntoViewIfNeeded();
  for (const action of ['Save Provider transport', 'Discard draft', 'Use reviewed revision']) await expect(unit.getByRole('button', { name: action, exact: true })).toBeInViewport();
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(settings, 'settings-conflict-light.png');
  expect(errors).toEqual([]);

  // A read that never answers: loading, never an empty configuration.
  errors = await start(page, '?scenario=loading');
  await openUserSettings(page);
  await openSettingsPage(page, 'Models');
  await expect(settings.locator('[role="status"][data-lifecycle="loading"]')).toHaveText('Loading authoritative sources…');
  await expect(settings.locator('[data-lifecycle="loading"][aria-busy="true"]')).toContainText('Nothing is shown until native answers');
  await expect(settings.getByRole('row')).toHaveCount(0);
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(settings, 'settings-loading-light.png');
  expect(errors).toEqual([]);

  // A failed read: an alert with the native reason, and the lifecycle says so.
  errors = await start(page, '?scenario=read-error');
  await openUserSettings(page);
  await openSettingsPage(page, 'Models');
  await expect(settings.getByRole('alert')).toContainText('/bound/rustx.toml is not readable by the App Server process');
  await expect(settings.locator('[role="status"][data-lifecycle="failed"]')).toHaveText('Source authority unavailable');
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(settings, 'settings-read-error-light.png');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('dark desktop Advanced keeps the same family and native diagnostics', async ({ page }) => {
  const errors = await start(page);
  await setTheme(page, 'Dark');
  await openUserSettings(page);
  await openSettingsPage(page, 'Advanced');
  const settings = dialog(page);
  await expect(settings.getByText(/Revision: user-1/)).toBeVisible();
  await expect(settings.getByRole('button', { name: 'Rescan configuration files' })).toBeVisible();
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(settings, 'settings-advanced-dark.png');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('390 × 844: section menu, list → detail → back, long identities and no horizontal overflow', async ({ page }) => {
  const errors = await start(page);
  await setTheme(page, 'Dark');
  await page.setViewportSize({ width: 390, height: 844 });
  await openUserSettings(page);
  const settings = dialog(page), menu = settingsSectionMenu(page);
  // The narrow panel replaces the rail with one section menu in its header.
  await expect(page.getByRole('tablist', { name: 'Settings pages' })).toBeHidden();
  await expect(menu).toBeVisible();
  await expect(menu).toHaveAccessibleName('Settings page: General');
  await expect(page.getByRole('button', { name: 'Close Settings' })).toBeInViewport();

  // Keyboard: Enter opens on the first row, the current page is announced,
  // Escape closes only the menu and returns focus to its trigger.
  await menu.focus(); await page.keyboard.press('Enter');
  const list = page.getByRole('menu');
  await expect(list.getByRole('menuitem', { name: 'General' })).toBeFocused();
  await expect(list.getByRole('menuitem', { name: 'General' })).toHaveAttribute('aria-current', 'true');
  await expect(list.getByRole('menuitem', { name: 'Models' })).not.toHaveAttribute('aria-current');
  const bounds = (await list.boundingBox())!;
  expect(bounds.x).toBeGreaterThanOrEqual(12 - 1); expect(bounds.x + bounds.width).toBeLessThanOrEqual(390 - 12 + 1);
  expect(bounds.y + bounds.height).toBeLessThanOrEqual(844 - 12 + 1);
  await expectAccessible(page, '[role="menu"]');
  await expectStableScreenshot(page, 'settings-mobile-menu-dark.png');
  await page.keyboard.press('Escape');
  await expect(list).toHaveCount(0);
  await expect(settings).toBeVisible();
  await expect(menu).toBeFocused();

  // Every page at 390, with long identities, paths and native diagnostics.
  for (const name of pages) {
    await openSettingsPage(page, name);
    expect(await selectedSettingsPage(page)).toBe(name);
    await expect(menu).toBeFocused();
    await expectNoHorizontalOverflow(page);
    await expectAccessible(page, settingsScope);
  }
  await openSettingsPage(page, 'Models');
  await expect(settings.getByRole('row', { name: longProvider, exact: true })).toBeVisible();
  await expectStableScreenshot(page, 'settings-mobile-dark.png');

  // List → detail → back: the detail replaces the list, and the way back is
  // in reach even after the detail has scrolled.
  await settings.getByRole('row', { name: longProvider, exact: true }).click();
  const detail = settings.getByRole('region', { name: `Provider ${longProvider}` });
  await expect(detail.getByRole('heading', { level: 3 })).toHaveText(`Provider ${longProvider}`);
  await expect(settings.getByRole('row', { name: 'transport', exact: true })).toHaveCount(0);
  await expectNoHorizontalOverflow(page);
  await expectStableScreenshot(page, 'settings-provider-detail-mobile-dark.png');
  await detail.getByRole('button', { name: `Remove Provider ${longProvider}` }).scrollIntoViewIfNeeded();
  const back = settings.getByRole('button', { name: '← Models', exact: true });
  await expect(back).toBeInViewport();
  // Destructive action reachable and its confirmation inside the viewport.
  await detail.getByRole('button', { name: `Remove Provider ${longProvider}` }).click();
  const deletion = page.getByRole('alertdialog');
  await expect(deletion).toBeVisible();
  const confirm = (await deletion.boundingBox())!;
  expect(confirm.x).toBeGreaterThanOrEqual(0); expect(confirm.x + confirm.width).toBeLessThanOrEqual(390);
  await expect(deletion.getByRole('button', { name: `Remove Provider ${longProvider}` })).toBeInViewport();
  await page.keyboard.press('Escape');
  await expect(deletion).toHaveCount(0); await expect(settings).toBeVisible();
  await back.click();
  await expect(settings.getByRole('row', { name: longProvider, exact: true })).toBeVisible();

  // Model detail through the Provider, with its long identity.
  await settings.getByRole('row', { name: longProvider, exact: true }).click();
  await settings.getByRole('row', { name: longModel, exact: true }).click();
  await expect(settings.getByRole('heading', { level: 3, name: `Model ${longModel}` })).toBeVisible();
  await expectNoHorizontalOverflow(page);
  await settings.getByRole('button', { name: `← Provider ${longProvider}` }).click();
  await expect(settings.getByRole('heading', { level: 3, name: `Provider ${longProvider}` })).toBeVisible();

  // Workspace resource detail at 390: named Agent list → detail.
  await closeSettings(page);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await openWorkspaceSettings(page, 'Workspace A');
  await page.setViewportSize({ width: 390, height: 844 });
  await openSettingsPage(page, 'Extensions');
  await settings.getByRole('tab', { name: 'Agents', exact: true }).click();
  await settings.getByRole('row', { name: 'reviewer', exact: true }).click();
  await settings.getByRole('form', { name: 'Agent reviewer' }).evaluate(el => el.scrollIntoView({ block: 'start' }));
  await expect(settings.getByLabel('Description', { exact: true })).toBeVisible();
  await expect(settings.getByRole('button', { name: '← Extensions', exact: true })).toBeInViewport();
  await expectNoHorizontalOverflow(page);
  await expectStableScreenshot(page, 'settings-agent-narrow-dark.png');

  // Escape closes Settings itself when no transient layer is open; the global
  // entry then restores focus to its own trigger.
  await page.keyboard.press('Escape'); await expect(settings).toHaveCount(0);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(settings).toBeVisible();
  await page.keyboard.press('Escape'); await expect(settings).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Settings', exact: true })).toBeFocused();
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('layout follows the Settings panel width, not the window', async ({ page }) => {
  const errors = await start(page);
  await openUserSettings(page);
  const rail = page.getByRole('tablist', { name: 'Settings pages' });
  await expect(rail).toBeVisible(); await expect(settingsSectionMenu(page)).toBeHidden();
  // The same 1440px window, with the panel itself constrained: narrow layout.
  const constrain = (width: string) => dialog(page).evaluate((el, value) => { el.parentElement!.style.width = value; }, width);
  await constrain('480px');
  await expect(rail).toBeHidden(); await expect(settingsSectionMenu(page)).toBeVisible();
  await openSettingsPage(page, 'Extensions');
  expect(await selectedSettingsPage(page)).toBe('Extensions');
  await constrain('');
  await expect(rail).toBeVisible(); await expect(settingsSectionMenu(page)).toBeHidden();
  await expect(rail.getByRole('tab', { name: 'Extensions' })).toHaveAttribute('aria-selected', 'true');
  expect(errors).toEqual([]);
});

test('an open section menu settles closed when the panel widens under it', async ({ page }) => {
  const errors = await start(page);
  await openUserSettings(page);
  const settings = dialog(page), trigger = settingsSectionMenu(page);
  const rail = page.getByRole('tablist', { name: 'Settings pages' });
  const constrain = (width: string) => settings.evaluate((el, value) => { el.parentElement!.style.width = value; }, width);
  await constrain('480px');
  await expect(trigger).toBeVisible(); await expect(rail).toBeHidden();
  // Opened from the keyboard: the portaled menu holds focus on its current row.
  await trigger.focus(); await page.keyboard.press('Enter');
  const menu = page.getByRole('menu');
  await expect(menu.getByRole('menuitem', { name: 'General', exact: true })).toBeFocused();

  // The panel widens while the menu is still open. The container query moves
  // navigation to the rail and takes the trigger out of layout; the menu has
  // no anchor left, so it settles closed rather than floating over the rail.
  await constrain('');
  await expect(rail).toBeVisible();
  await expect(trigger).toBeHidden();
  await expect(menu).toHaveCount(0);
  // Its owner's open state settled with it: the hidden trigger says closed.
  await expect(settings.getByRole('button', { name: /^Settings page: /, includeHidden: true })).toHaveAttribute('aria-expanded', 'false');
  // The keyboard is on no removed row, not on the hidden trigger and not on
  // the page body: the Settings dialog, the focus owner the panel names for
  // its section menu, holds it, as when it first opened.
  await expect(settings).toBeFocused();
  const focus = await page.evaluate(() => {
    const active = document.activeElement!;
    return { body: active === document.body, menuitem: active.getAttribute('role') === 'menuitem', rendered: active.getClientRects().length > 0 };
  });
  expect(focus).toEqual({ body: false, menuitem: false, rendered: true });

  // Wide Settings keyboard navigation works from there exactly as from a
  // freshly opened dialog: Tab walks the header and reaches the current page
  // on the rail, the arrows move between pages, and Escape still closes
  // Settings and returns focus to its global entry.
  await page.keyboard.press('Tab');
  await expect(settings.getByRole('button', { name: 'Reload configuration' })).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(page.getByRole('button', { name: 'Close Settings' })).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(rail.getByRole('tab', { name: 'General' })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(rail.getByRole('tab', { name: 'Models' })).toHaveAttribute('aria-selected', 'true');
  await expect(settings.getByRole('heading', { level: 3, name: 'Models', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(settings).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Settings', exact: true })).toBeFocused();
  expect(errors).toEqual([]);
});

/** Record, on every animation frame from now on, whether the keyboard sat on
 * the page body; `bodyFrames` reads (and stops) the record. A presentation
 * change may take a focused control out of layout, but no rendered frame may
 * find the keyboard dropped to the document. */
const watchBodyFrames = (page: Page) => page.evaluate(() => {
  const record = window as unknown as { rustxBodyFrames: number; rustxBodyWatch: boolean };
  record.rustxBodyFrames = 0; record.rustxBodyWatch = true;
  const frame = () => {
    if (!record.rustxBodyWatch) return;
    if (document.activeElement === document.body) record.rustxBodyFrames++;
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
});
const bodyFrames = (page: Page) => page.evaluate(() => {
  const record = window as unknown as { rustxBodyFrames: number; rustxBodyWatch: boolean };
  record.rustxBodyWatch = false;
  return record.rustxBodyFrames;
});

// The two tests below take the focused navigation control itself out of
// layout. Nothing in the panel knows the container is narrow or wide: the
// container query alone swaps the presentations, and the panel answers only
// the fact that the control holding the keyboard is no longer rendered.
test('a focused rail tab hands the keyboard to the section trigger when the panel narrows', async ({ page }) => {
  const errors = await start(page);
  await openUserSettings(page);
  const settings = dialog(page), trigger = settingsSectionMenu(page);
  const rail = page.getByRole('tablist', { name: 'Settings pages' });
  const constrain = (width: string) => settings.evaluate((el, value) => { el.parentElement!.style.width = value; }, width);
  await openSettingsPage(page, 'Extensions');
  const extensions = rail.getByRole('tab', { name: 'Extensions' });
  await extensions.focus(); await expect(extensions).toBeFocused();
  await expect(settings.getByRole('heading', { level: 3, name: 'Extensions', exact: true })).toBeVisible();
  const requests = (await nativeRequests(page)).length;

  // The window stays 1440px wide; only the panel narrows under the rail.
  await watchBodyFrames(page);
  await constrain('480px');
  await expect(rail).toBeHidden(); await expect(trigger).toBeVisible();
  // The keyboard continues on the visible selector for the same page: not on
  // the hidden tab, and at no rendered frame on the page body.
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveAccessibleName('Settings page: Extensions');
  await expect(trigger).toHaveAttribute('aria-expanded', 'false');
  await settled(page);
  expect(await bodyFrames(page)).toBe(0);
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true });
  // Presentation only: the same page, no menu opened, no native traffic.
  expect(await selectedSettingsPage(page)).toBe('Extensions');
  await expect(page.getByRole('menu')).toHaveCount(0);
  await expect(settings.getByRole('heading', { level: 3, name: 'Extensions', exact: true })).toBeVisible();
  expect(await nativeRequests(page)).toHaveLength(requests);

  // The selector works from where the keyboard landed, and Escape closes
  // only its menu, handing the keyboard back to it inside Settings.
  await page.keyboard.press('Enter');
  const menu = page.getByRole('menu');
  await expect(menu.getByRole('menuitem', { name: 'Extensions', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(menu).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect(settings).toBeVisible();
  expect(await selectedSettingsPage(page)).toBe('Extensions');
  expect(errors).toEqual([]);
});

test('a focused closed section trigger hands the keyboard to the selected rail tab when the panel widens', async ({ page }) => {
  const errors = await start(page);
  await openUserSettings(page);
  const settings = dialog(page), trigger = settingsSectionMenu(page);
  const rail = page.getByRole('tablist', { name: 'Settings pages' });
  const constrain = (width: string) => settings.evaluate((el, value) => { el.parentElement!.style.width = value; }, width);
  await constrain('480px');
  await expect(rail).toBeHidden(); await expect(trigger).toBeVisible();
  await openSettingsPage(page, 'Extensions');
  // The menu is closed and its trigger holds the keyboard.
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByRole('menu')).toHaveCount(0);
  const requests = (await nativeRequests(page)).length;

  // With no menu open there is no floating surface to report its anchor
  // hidden: the trigger alone leaves layout as the panel widens.
  await watchBodyFrames(page);
  await constrain('');
  await expect(trigger).toBeHidden(); await expect(rail).toBeVisible();
  const extensions = rail.getByRole('tab', { name: 'Extensions' });
  await expect(extensions).toBeFocused();
  await expect(extensions).toHaveAttribute('aria-selected', 'true');
  await settled(page);
  expect(await bodyFrames(page)).toBe(0);
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true });
  expect(await nativeRequests(page)).toHaveLength(requests);

  // Rail navigation continues from there at once: the arrows move between
  // pages, and Escape, with no transient layer open, closes Settings.
  await page.keyboard.press('ArrowUp');
  await expect(rail.getByRole('tab', { name: 'Tools & Permissions' })).toBeFocused();
  await expect(rail.getByRole('tab', { name: 'Tools & Permissions' })).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('ArrowDown');
  await expect(extensions).toBeFocused();
  await expect(extensions).toHaveAttribute('aria-selected', 'true');
  await expect(settings.getByRole('heading', { level: 3, name: 'Extensions', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(settings).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Settings', exact: true })).toBeFocused();
  expect(errors).toEqual([]);
});

/** Two animation frames: every focus restoration React Aria schedules after a
 * layer unmounts has run by then. */
const settled = (page: Page) => page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
/** Where the keyboard is: never the page body, never a disabled control,
 * always inside the Settings dialog. */
const keyboard = (page: Page) => page.evaluate(() => {
  const active = document.activeElement!;
  return { body: active === document.body, disabled: active.matches(':disabled'), settings: active.closest('[role="dialog"][aria-label="Settings"]') !== null, rendered: active.getClientRects().length > 0 };
});
const releaseWrites = (page: Page) => page.evaluate(() => (window as unknown as { rustxReleaseWrites: () => void }).rustxReleaseWrites());

test('confirming a removal settles focus on its unit while the write is in flight; dismissal returns it to the trigger', async ({ page }) => {
  const errors = await start(page, '?write=held');
  const settings = dialog(page);
  await openUserSettings(page);
  await openSettingsPage(page, 'Models');
  await settings.getByRole('row', { name: 'transport', exact: true }).click();
  const unit = settings.getByRole('form', { name: 'Provider transport' });
  const remove = unit.getByRole('button', { name: 'Remove Provider transport', exact: true });
  const deletion = page.getByRole('alertdialog', { name: 'Remove Provider transport from User configuration?' });
  const cancel = deletion.getByRole('button', { name: 'Cancel', exact: true });
  const writes = async () => (await nativeRequests(page)).filter(request => request.method === 'configuration/sourceWrite').length;

  // Cancel: nothing is written and focus returns to the trigger.
  await remove.focus(); await page.keyboard.press('Enter');
  await expect(cancel).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(deletion).toHaveCount(0);
  await settled(page);
  await expect(remove).toBeFocused();
  // Escape: only the confirmation closes, and focus returns to the trigger.
  await page.keyboard.press('Enter');
  await expect(cancel).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(deletion).toHaveCount(0); await expect(settings).toBeVisible();
  await settled(page);
  await expect(remove).toBeFocused();
  expect(await writes()).toBe(0);

  // Confirm: the removal is submitted and held in flight. The unit's controls,
  // the trigger among them, are closed until native answers, so focus settles
  // on the unit itself — not the body, not the disabled trigger.
  await page.keyboard.press('Enter');
  await expect(cancel).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(deletion.getByRole('button', { name: 'Remove Provider transport', exact: true })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(deletion).toHaveCount(0);
  expect(await writes()).toBe(1);
  await expect(remove).toBeDisabled();
  await settled(page);
  await expect(unit).toBeFocused();
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true });
  // Still in flight after every scheduled restoration has run.
  await expect(remove).toBeDisabled();
  await expect(unit).toBeFocused();

  // Native answers: the removal settles normally, exactly once, and the
  // keyboard is still inside the unit's workflow.
  await releaseWrites(page);
  await expect(unit.getByText('Provider transport saved. Native coordination owns application.')).toBeVisible();
  await expect(remove).toHaveCount(0);
  expect(await writes()).toBe(1);
  await expect(unit).toBeFocused();
  // Tab continues from the unit into its now-enabled controls.
  await page.keyboard.press('Tab');
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true });
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('confirming a restore of inheritance settles focus on its unit while the write is in flight', async ({ page }) => {
  const errors = await start(page, '?write=held');
  const settings = dialog(page);
  await openWorkspaceSettings(page, 'Workspace A');
  await openSettingsPage(page, 'Agent');
  const unit = settings.getByRole('form', { name: 'Root identity' });
  const restore = unit.getByRole('button', { name: 'Use global default Root identity', exact: true });
  const confirmation = page.getByRole('dialog', { name: 'Use the global default for Root identity?' });
  await restore.focus(); await page.keyboard.press('Enter');
  await expect(confirmation.getByRole('button', { name: 'Cancel', exact: true })).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(confirmation.getByRole('button', { name: 'Use global default Root identity', exact: true })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(confirmation).toHaveCount(0);
  await expect(restore).toBeDisabled();
  await settled(page);
  await expect(unit).toBeFocused();
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true });

  await releaseWrites(page);
  await expect(unit.getByText('Root identity saved. Native coordination owns application.')).toBeVisible();
  await expect(unit.locator('[data-authored="absent"]')).toContainText('Inherited — no Workspace override');
  await expect(unit).toBeFocused();
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('reduced motion removes Settings and banner motion', async ({ page }) => {
  const errors = await start(page, '?session=preparing');
  const dot = page.getByRole('region', { name: 'Session configuration' }).locator('svg rect').first();
  await expect(dot).toHaveCSS('animation-name', 'none');
  await openUserSettings(page);
  await openSettingsPage(page, 'Extensions');
  await dialog(page).getByRole('tab', { name: 'Native', exact: true }).click();
  const track = dialog(page).getByRole('switch').first().locator('xpath=ancestor::label[1]').locator('[class*="switchTrack"]');
  await expect(track).toHaveCSS('transition-duration', '0s');
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await expect(track).toHaveCSS('transition-duration', '0.15s');
  await expect(dot).not.toHaveCSS('animation-name', 'none');
  expect(errors).toEqual([]);
});

test.describe('touch', () => {
  test.use({ hasTouch: true });
  test('the narrow section menu is operated by touch alone, no hover', async ({ page }) => {
    const errors = await start(page, '', { width: 390, height: 844 });
    await page.getByRole('button', { name: 'Settings', exact: true }).tap();
    const menu = settingsSectionMenu(page);
    await menu.tap();
    await page.getByRole('menuitem', { name: 'Tools & Permissions' }).tap();
    await expect(menu).toHaveAccessibleName('Settings page: Tools & Permissions');
    await expect(dialog(page).getByRole('heading', { level: 3, name: 'Tools & Permissions' })).toBeVisible();
    // A tap outside the open menu dismisses it without closing Settings.
    await menu.tap();
    await expect(page.getByRole('menu')).toBeVisible();
    // Well below the open list, inside the Settings panel.
    await page.touchscreen.tap(195, 780);
    await expect(page.getByRole('menu')).toHaveCount(0);
    await expect(dialog(page)).toBeVisible();
    expect(errors).toEqual([]);
  });
});

type NativeRequest = { method: string; params: Record<string, unknown> };
const nativeRequests = (page: Page) => page.evaluate(() => (window as unknown as { rustxNativeRequests: () => NativeRequest[] }).rustxNativeRequests());
const banner = (page: Page) => page.getByRole('region', { name: 'Session configuration' });

test('Session configuration banner: preparing, ready, blocked and failed, compact and owner-correct', async ({ page }) => {
  // Preparing: no adoption requirement, stated as text.
  let errors = await start(page, '?session=preparing');
  await expect(banner(page).locator('[data-state="preparing"]')).toContainText('Preparing configuration…');
  await expect(banner(page).getByRole('button', { name: 'Adopt configuration' })).toHaveCount(0);
  await expectAccessible(page, '[aria-label="Session configuration"]');
  await expectStableScreenshot(page.locator('#session-view > header'), 'session-banner-preparing-light.png');
  expect(errors).toEqual([]);

  // Blocked: the native reason, a disabled Adopt, and nothing cancelled or queued.
  errors = await start(page, '?session=blocked');
  const blocked = banner(page).locator('[data-state="blocked"]');
  await expect(blocked).toContainText('Session work must settle before adoption.');
  await expect(blocked.getByRole('button', { name: 'Adopt configuration' })).toBeDisabled();
  await expectStableScreenshot(page.locator('#session-view > header'), 'session-banner-blocked-light.png');
  expect((await nativeRequests(page)).filter(request => ['session/adoptConfiguration', 'turn/cancel'].includes(request.method))).toEqual([]);
  expect(errors).toEqual([]);

  // Failed: the native diagnostic and one action per native owner — here User
  // and Workspace B, although the focused Session lives in /workspace/A.
  errors = await start(page, '?session=failed');
  const failed = banner(page).locator('[data-state="failed"]');
  await expect(failed).toContainText('Capabilities: failed — MCP server repository-index failed to start');
  await expect(failed.getByRole('button')).toHaveText(['Open User Settings', 'Open Workspace Settings — /workspace/B']);
  await expect(banner(page).getByRole('button', { name: 'Adopt configuration' })).toHaveCount(0);
  await expectStableScreenshot(page.locator('#session-view > header'), 'session-banner-failed-light.png');
  await failed.getByRole('button', { name: 'Open Workspace Settings — /workspace/B' }).click();
  await expect(dialog(page).getByRole('heading', { level: 2, name: 'Workspace Settings — Workspace B' })).toBeVisible();
  await closeSettings(page);
  // Narrow: the reason and both owner actions wrap; nothing scrolls sideways.
  await setTheme(page, 'Dark');
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(failed.getByRole('button', { name: 'Open Workspace Settings — /workspace/B' })).toBeVisible();
  await expectNoHorizontalOverflow(page);
  await expectStableScreenshot(page.locator('#session-view > header'), 'session-banner-failed-mobile-dark.png');
  expect(errors).toEqual([]);

  // Ready: Adopt carries exactly the inspected candidate and expected binding,
  // once; the banner leaves only after native's authoritative reread.
  errors = await start(page, '?session=ready');
  const ready = banner(page).locator('[data-state="ready"]');
  await expect(ready).toContainText('Prepared configuration is waiting for this Session.');
  await expectAccessible(page, '[aria-label="Session configuration"]');
  await expectStableScreenshot(page.locator('#session-view > header'), 'session-configuration-banner-light.png');
  const before = (await nativeRequests(page)).filter(request => request.method === 'session/configuration').length;
  await ready.getByRole('button', { name: 'Adopt configuration' }).click();
  await expect(banner(page)).toHaveCount(0);
  const requests = await nativeRequests(page);
  const adoptions = requests.filter(request => request.method === 'session/adoptConfiguration');
  expect(adoptions.map(request => request.params)).toEqual([{ session_id: 'A', candidate: { input_revision: 'input-2', attempt: '2' }, expected_binding: '1' }]);
  // The adoption response is followed by the authoritative reread it owes.
  const adoptionAt = requests.indexOf(adoptions[0]);
  expect(requests.slice(adoptionAt).filter(request => request.method === 'session/configuration').length).toBeGreaterThanOrEqual(1);
  expect(requests.filter(request => request.method === 'session/configuration').length).toBeGreaterThan(before);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

const fixtureOrigin = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}`;
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
import { openResourceModule, returnHome, openSettingsPage, openWorkspaceSettings, selectedSettingsPage, settingsSectionMenu, settledSetting } from './shell-actions';
import { selectCossOption } from './coss-select';

const fixture = `${fixtureOrigin}/test/fixtures/settings.html`;
const pages = ['General', 'Models', 'Agent', 'Tools & Permissions', 'Extensions', 'Advanced'];
const longProvider = 'enterprise-inference-gateway-eu-central-primary-with-an-exceptionally-long-provider-identity';
const longModel = 'enterprise-reasoning-model-2026-09-long-context-preview-with-an-exceptionally-long-identity';

async function start(page: Page, query = '', size = { width: 1440, height: 1000 }) {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.clock.setFixedTime(new Date('2026-09-18T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.setViewportSize(size);
  await page.goto(fixture + query);
  await expect(page).toHaveTitle('Session A — rustX');
  return errors;
}
const settingsPage = (page: Page) => page.getByRole('region', { name: 'Settings', exact: true });
async function openUserSettings(page: Page) {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(settingsPage(page)).toBeVisible();
  const user = settingsPage(page).getByRole('button', { name: 'User (global)', exact: true });
  if (await user.isVisible()) await user.click();
  await openSettingsPage(page, 'General');
}
async function setTheme(page: Page, theme: 'Light' | 'Dark') {
  await openUserSettings(page);
  // General preferences are webui-local and apply the moment they are picked.
  await settingsPage(page).getByRole('group', { name: 'Appearance', exact: true }).getByRole('button', { name: theme, exact: true }).click();
  await returnHome(page);
}

/** No horizontal page overflow, and nothing inside the Settings panel is
 * pushed past its edges — `.options` clips on the inline axis, so clipped
 * overflow is checked element by element, not only by scroll width. Only a
 * control that scrolls on its own (a `pre`, a table) may be wider. */
async function expectNoHorizontalOverflow(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  if (!await settingsPage(page).isVisible()) return;
  const escaped = await settingsPage(page).evaluate(root => {
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
const settingsScope = '[data-settings-page]';

/** Measured panel geometry, in CSS pixels. */
async function panelBox(page: Page) {
  return settingsPage(page).evaluate(el => {
    const panel = el.parentElement!, rect = panel.getBoundingClientRect(), style = getComputedStyle(panel);
    return { x: rect.x, y: rect.y, width: rect.width, height: rect.height, radius: style.borderTopLeftRadius };
  });
}

test('desktop: six pages share one stable Harness frame, distinct icons and one row vocabulary', async ({ page }) => {
  const errors = await start(page);
  await openUserSettings(page);
  const settings = settingsPage(page);
  const frame = await panelBox(page);
  // Settings fills the content area beside the permanent primary rail.
  expect(frame.width).toBe(1440 - 56);
  expect(frame.height).toBe(1000); expect(frame.radius).toBe('0px');
  const rail = page.getByRole('tablist', { name: 'Settings pages' });
  await expect(rail).toHaveAttribute('aria-orientation', 'vertical');
  await expect(rail.getByRole('tab')).toHaveText(pages);
  await expect(settingsSectionMenu(page)).toBeHidden();
  const navigation = await page.getByRole('navigation', { name: 'Settings navigation' }).evaluate(nav => nav.getBoundingClientRect().width);
  // Wide enough that "Tools & Permissions" renders without truncation.
  expect(navigation).toBeGreaterThanOrEqual(208); expect(navigation).toBeLessThanOrEqual(224);
  const cell = await rail.getByRole('tab', { name: 'Models' }).evaluate(el => ({ height: el.getBoundingClientRect().height, radius: getComputedStyle(el).borderTopLeftRadius, font: getComputedStyle(el).fontSize }));
  expect(cell).toEqual({ height: 40, radius: '12px', font: '14px' });
  // Six distinct glyphs; the label, not the glyph, names each page.
  const glyphs = await rail.getByRole('tab').evaluateAll(tabs => tabs.map(tab => tab.querySelector('svg')!.innerHTML));
  expect(new Set(glyphs).size).toBe(6);
  for (const name of pages) await expect(rail.getByRole('tab', { name, exact: true })).toHaveAccessibleName(name);

  const references: Record<string, string> = { General: 'settings-general-light.png', Models: 'settings-models-light.png', Agent: 'settings-agent-light.png', 'Tools & Permissions': 'settings-tools-light.png', Extensions: 'settings-extensions-light.png' };
  for (const name of pages) {
    await openSettingsPage(page, name);
    // One stable page frame: a section change never resizes or moves it.
    expect(await panelBox(page)).toEqual(frame);
    await expect(settings.getByRole('heading', { level: 1, name: name === 'Models' ? 'Model Settings' : name, exact: true })).toBeVisible();
    // The owner and observation state live in the fixed header of every scoped
    // page; General is webui-only and carries no scope segment.
    if (name !== 'General') await expect(settings.getByRole('button', { name: 'User (global)', exact: true })).toHaveAttribute('aria-pressed', 'true');
    await expect(settings.locator('[data-lifecycle="ready"]')).toBeVisible();
    await expectNoHorizontalOverflow(page);
    await expectAccessible(page, settingsScope);
    if (references[name]) await expectStableScreenshot(settings, references[name]);
  }

  // Provider cards and the general Settings forms share the Harness spacing and radius.
  await openSettingsPage(page, 'Agent');
  const instructions = settings.getByRole('form', { name: 'Global instructions' });
  await expect(instructions.getByRole('button', { name: 'Configuration details', exact: true })).toHaveCount(0);
  await expect(instructions).toHaveAttribute('data-authored', 'absent');
  await expect(settings.getByText('/home/user/rustx/.agents/AGENTS.md', { exact: true })).toBeVisible();
  await expect(settings.getByRole('button', { name: /^Author (Global instructions|Project guidance)$/ })).toHaveCount(0);
  // A scalar unit commits itself: the draft marker appears on the edit and
  // clears when the write has settled.
  await expect(instructions).not.toHaveAttribute('data-draft', /./);
  await instructions.getByRole('textbox', { name: 'Global instructions', exact: true }).fill('Review the changes.');
  await expect(instructions).toHaveAttribute('data-draft', 'true');
  await settledSetting(instructions);
  const unit = await instructions.evaluate(el => {
    const style = getComputedStyle(el);
    return { height: el.getBoundingClientRect().height, padding: `${style.paddingTop} ${style.paddingLeft}`, radius: style.borderTopLeftRadius };
  });
  expect(unit.height).toBeGreaterThanOrEqual(56); expect(unit.padding).toBe('14px 16px'); expect(unit.radius).toBe('0px');
  await openSettingsPage(page, 'Models');
  const row = await settings.getByRole('button', { name: 'transport →', exact: true }).evaluate(el => {
    const card = el.closest('section')!;
    const style = getComputedStyle(card);
    return { height: card.getBoundingClientRect().height, padding: `${style.paddingTop} ${style.paddingLeft}`, radius: style.borderTopLeftRadius };
  });
  expect(row.height).toBeGreaterThanOrEqual(42); expect(row.padding).toBe('14px 16px'); expect(row.radius).toBe('16px');

  // Provider → detail keeps status and actions next to the object.
  await settings.getByRole('button', { name: 'transport →', exact: true }).click();
  await expect(settings.getByRole('heading', { name: 'Provider transport', exact: true }).first()).toBeVisible();
  await expect(settings.getByRole('button', { name: 'Provider actions', exact: true })).toBeVisible();
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(settings, 'settings-provider-detail-light.png');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('Workspace inherited and overridden units, restore versus deletion, nested Escape ownership', async ({ page }) => {
  const errors = await start(page);
  const settings = settingsPage(page);
  await openWorkspaceSettings(page, 'Workspace A');
  await expect(settings.getByRole('button', { name: 'Workspace A', exact: true })).toBeVisible();
  // Inherited (clean module): Follow global settings starts ON on Advanced and
  // hides sub-options; toggling OFF reveals native defaults with 0 writes.
  await openSettingsPage(page, 'Advanced');
  const followAdvanced = settings.getByRole('switch', { name: 'Follow global settings', exact: true });
  await expect(followAdvanced).toBeChecked();
  await expect(settings.getByRole('form', { name: 'Context' })).toHaveCount(0);
  await followAdvanced.click();
  await expect(followAdvanced).not.toBeChecked();
  const contextForm = settings.getByRole('form', { name: 'Context' });
  await expect(contextForm).toHaveAttribute('data-authored', 'absent');
  await expect(contextForm).not.toHaveAttribute('data-draft', /./);
  await followAdvanced.click();
  await expect(followAdvanced).toBeChecked();
  await expect(settings.getByRole('form', { name: 'Context' })).toHaveCount(0);

  // Mixed module (Tools & Permissions): Workspace A overrides Built-in tools
  // while inheriting Approval mode without per-unit Override / Use global default buttons.
  await openSettingsPage(page, 'Tools & Permissions');
  const followTools = settings.getByRole('switch', { name: 'Follow global settings', exact: true });
  await expect(followTools).not.toBeChecked();
  const approval = settings.getByRole('form', { name: 'Approval mode' });
  await expect(approval).toHaveAttribute('data-authored', 'absent');
  await expect(approval.getByRole('button', { name: /Override Approval mode/ })).toHaveCount(0);
  await expect(approval.getByRole('button', { name: /Use global default/ })).toHaveCount(0);
  await approval.scrollIntoViewIfNeeded();
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(settings, 'settings-workspace-inherited-light.png');

  // Overridden and dirty: sub-options start visible; a scalar edit stages a
  // draft that commits itself after a short debounce.
  await openSettingsPage(page, 'Agent');
  const followAgent = settings.getByRole('switch', { name: 'Follow global settings', exact: true });
  await expect(followAgent).not.toBeChecked();
  const guidance = settings.getByRole('form', { name: 'Project guidance' });
  await expect(settings.getByRole('form', { name: /^(Agent name|Description|Global instructions)$/ })).toHaveCount(0);
  await expect(guidance).toHaveAttribute('data-authored', 'present');
  // Workspace writes cross the Product Host boundary, not the User RPC socket.
  const writes = () => page.evaluate(() => (window as unknown as { rustxWorkspaceWrites: () => number }).rustxWorkspaceWrites());
  await guidance.getByRole('switch', { name: 'Automatically load project guidance' }).click();
  await expect(guidance).toHaveAttribute('data-draft', 'true');
  await settledSetting(guidance);
  expect(await writes()).toBe(1);
  await expectStableScreenshot(settings, 'settings-workspace-override-light.png');

  // Turning Follow global settings ON while overrides exist confirms the
  // destructive clear first; confirming commits it — no footer, no staging.
  await followAgent.click();
  await settings.getByRole('button', { name: 'Clear workspace overrides', exact: true }).click();
  await expect(followAgent).toBeChecked();
  await expect(guidance).toHaveCount(0);
  await expect.poll(writes).toBe(2);
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('deletion confirmation, CAS conflict review, loading and read failure are designed states', async ({ page }) => {
  let errors = await start(page, '?scenario=conflict');
  const settings = settingsPage(page);
  await openUserSettings(page);
  await openSettingsPage(page, 'Models');
  await settings.getByRole('button', { name: 'transport →', exact: true }).click();
  // A real deletion: an alert dialog whose confirm action is destructive.
  await settings.getByRole('button', { name: 'Provider actions', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
  const deletion = page.getByRole('alertdialog', { name: 'Remove Provider transport from User configuration?' });
  await expect(deletion).toBeVisible();
  await expect(deletion.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await expectAccessible(page, '[role="alertdialog"]');
  await expectStableScreenshot(page, 'settings-delete-confirm-light.png');
  await deletion.getByRole('button', { name: 'Cancel' }).click();
  await expect(deletion).toHaveCount(0);

  // CAS conflict: the draft and its base survive, the review is local to the
  // unit, and applying the retained changes requires explicit review.
  const endpoint = settings.getByLabel('Base URL', { exact: true });
  await endpoint.fill('https://user.invalid/v2');
  const unit = settings.getByRole('form', { name: 'Provider transport' });
  await settings.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(settings.getByRole('alert').filter({ hasText: 'A setting was not saved because the source changed.' })).toBeVisible();
  await expect(endpoint).toHaveValue('https://user.invalid/v2');
  await endpoint.scrollIntoViewIfNeeded();
  await expect(unit.getByRole('button', { name: 'Apply my changes', exact: true })).toBeInViewport();
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(settings, 'settings-conflict-light.png');
  expect(errors).toEqual([]);

  // A read that never answers: skeleton loading state, never an empty configuration.
  errors = await start(page, '?scenario=loading');
  await openUserSettings(page);
  await openSettingsPage(page, 'Models');
  await expect(settings.locator('[data-lifecycle="loading"][aria-busy="true"]')).toBeVisible();
  await expect(settings.getByRole('status', { name: 'Loading settings…' })).toBeVisible();
  await expect(settings.getByRole('listitem')).toHaveCount(0);
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(settings, 'settings-loading-light.png');
  expect(errors).toEqual([]);

  // A failed read: an alert with the native reason, and the lifecycle says so.
  errors = await start(page, '?scenario=read-error');
  await openUserSettings(page);
  await openSettingsPage(page, 'Models');
  await expect(page.locator('[data-toast][data-foreground]')).toContainText('/bound/rustx.toml is not readable by the App Server process');
  await expect(settings.locator('section[data-lifecycle="failed"][aria-busy="false"]')).toBeVisible();
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(settings, 'settings-read-error-light.png');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('dark desktop Advanced keeps useful controls without internal diagnostics', async ({ page }) => {
  const errors = await start(page);
  await setTheme(page, 'Dark');
  await openUserSettings(page);
  await openSettingsPage(page, 'Advanced');
  const settings = settingsPage(page);
  await expect(settings.getByText(/Revision: user-1/)).toHaveCount(0);
  await expect(settings.getByLabel('Reserved tokens', { exact: true })).toBeVisible();
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
  const settings = settingsPage(page), menu = settingsSectionMenu(page);
  // The narrow panel replaces the rail with one section menu in its header.
  await expect(page.getByRole('tablist', { name: 'Settings pages' })).toBeHidden();
  await expect(menu).toBeVisible();
  await expect(menu).toHaveAccessibleName('Settings page: General');
  await expect(page.getByRole('navigation', { name: 'Primary navigation' })).toBeInViewport();
  await expect(page.getByRole('button', { name: 'Close Settings' })).toHaveCount(0);

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
  // Workspace scope must satisfy the same side-escape check at 390px: the
  // scope toolbar (segment + workspace chooser + reload) wraps inside the
  // panel and never slides under the primary nav rail.
  await openWorkspaceSettings(page, 'Workspace A');
  for (const name of pages.filter(name => name !== 'General')) {
    await openSettingsPage(page, name);
    await expectNoHorizontalOverflow(page);
  }
  await openUserSettings(page);
  await openSettingsPage(page, 'Models');
  await expect(settings.getByRole('button', { name: `${longProvider} →`, exact: true })).toBeVisible();
  await expectStableScreenshot(page, 'settings-mobile-dark.png');

  // The detail replaces the list in-page; 'Model Settings' walks back.
  await settings.getByRole('button', { name: `${longProvider} →`, exact: true }).click();
  await expect(settings.getByRole('form', { name: `Provider ${longProvider}` })).toBeVisible();
  await expect(settings.getByRole('heading', { level: 1 })).toHaveText(longProvider);
  await expectNoHorizontalOverflow(page);
  await expectStableScreenshot(page, 'settings-provider-detail-mobile-dark.png');
  await settings.getByRole('button', { name: 'Model Settings', exact: true }).click();
  await settings.getByRole('button', { name: 'transport →', exact: true }).click();
  await settings.getByRole('button', { name: 'Provider actions', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
  const deletion = page.getByRole('alertdialog');
  await expect(deletion).toBeVisible();
  const confirm = (await deletion.boundingBox())!;
  expect(confirm.x).toBeGreaterThanOrEqual(0); expect(confirm.x + confirm.width).toBeLessThanOrEqual(390);
  await expect(deletion.getByRole('button', { name: 'Remove Provider transport' })).toBeInViewport();
  await page.keyboard.press('Escape');
  await expect(deletion).toHaveCount(0); await expect(settings).toBeVisible();
  await settings.getByRole('button', { name: 'Model Settings', exact: true }).click();
  await expect(settings.getByRole('button', { name: `${longProvider} →`, exact: true })).toBeVisible();

  // Model detail through the Provider, with its long identity.
  await settings.getByRole('button', { name: `${longProvider} →`, exact: true }).click();
  await settings.getByRole('listitem', { name: longModel, exact: true }).getByRole('button', { name: 'Edit Model', exact: true }).click();
  const modelDialog = page.getByRole('dialog', { name: `Model ${longModel}`, exact: true });
  await expect(modelDialog).toBeVisible();
  await expect(modelDialog.getByLabel('Model ID', {exact:true})).toBeVisible();
  await expectNoHorizontalOverflow(page);
  await modelDialog.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(settings.getByRole('heading', { level: 1, name: longProvider })).toBeVisible();

  await openSettingsPage(page, 'Agent');
  const guidance = settings.getByRole('form', { name: 'Project guidance' });
  const automatic = guidance.getByRole('switch', { name: 'Automatically load project guidance' });
  await expect(automatic).toBeChecked();
  await expect(guidance.getByRole('button', { name: 'Advanced options' })).toHaveCount(0);
  await automatic.press('Space');
  await expect(automatic).not.toBeChecked();
  await settledSetting(guidance);
  await expectNoHorizontalOverflow(page);
  await expectAccessible(page, settingsScope);
  await expectStableScreenshot(page, 'settings-agent-mobile-dark.png');
  await automatic.press('Space');
  await settledSetting(guidance);
  await expect(automatic).toBeChecked();

  // Subagent details now belong to their independent resource module.
  const agents=await openResourceModule(page,'subagents','Workspace A');
  await agents.getByRole('listitem',{name:'reviewer',exact:true}).getByRole('button',{name:'reviewer',exact:true}).click();
  await expect(agents.getByRole('heading', { name: 'Edit subagent', exact: true })).toBeVisible();
  await expect(agents.getByLabel('Description',{exact:true})).toBeVisible();
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  await expectAccessible(page, '[data-resource-module="subagents"]');
  await expect(agents.getByRole('heading', { name: 'Advanced options' })).toBeVisible();
  await expect(agents.getByRole('group', { name: 'Skills', exact: true }).getByRole('checkbox', { name: 'review' })).toBeChecked();
  await expectStableScreenshot(page,'subagent-narrow-dark.png');
  await expect(agents.getByText('Delegated Agents', { exact: true })).toHaveCount(0);
  await expect(agents.getByText('Project guidance files', { exact: true })).toHaveCount(0);
  await openSettingsPage(page,'Extensions');

  // Escape preserves the Settings page when no transient layer is open.
  // Leaving and returning through primary navigation restores its section.
  await page.keyboard.press('Escape'); await expect(settings).toBeVisible();
  await returnHome(page); await expect(settings).toBeHidden();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(settings).toBeVisible();
  await page.keyboard.press('Escape'); await expect(settings).toBeVisible();
  await returnHome(page); await expect(settings).toBeHidden();
  await expect(page.getByRole('button', { name: 'Home', exact: true })).toBeFocused();
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('layout follows the Settings panel width, not the window', async ({ page }) => {
  const errors = await start(page);
  await openUserSettings(page);
  const rail = page.getByRole('tablist', { name: 'Settings pages' });
  await expect(rail).toBeVisible(); await expect(settingsSectionMenu(page)).toBeHidden();
  // The same 1440px window, with the panel itself constrained: narrow layout.
  const constrain = (width: string) => settingsPage(page).evaluate((el, value) => { el.parentElement!.style.width = value; }, width);
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
  const settings = settingsPage(page), trigger = settingsSectionMenu(page);
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
  // the page body: the Settings page, the focus owner the panel names for
  // its section menu, holds it, as when it first opened.
  await expect(settings).toBeFocused();
  const focus = await page.evaluate(() => {
    const active = document.activeElement!;
    return { body: active === document.body, menuitem: active.getAttribute('role') === 'menuitem', rendered: active.getClientRects().length > 0 };
  });
  expect(focus).toEqual({ body: false, menuitem: false, rendered: true });

  // Wide Settings keyboard navigation works from there exactly as from a
  // freshly opened page: Tab walks the header and reaches the current page
  // on the rail and the arrows move between sections. Escape leaves Settings
  // visible; primary Home navigation leaves the page.
  await page.keyboard.press('Tab');
  await expect(rail.getByRole('tab', { name: 'General' })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(rail.getByRole('tab', { name: 'Models' })).toHaveAttribute('aria-selected', 'true');
  await expect(settings.getByRole('heading', { level: 1, name: 'Model Settings', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(settings).toBeVisible();
  await page.getByRole('button', { name: 'Home', exact: true }).click();
  await expect(settings).toBeHidden();
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
  const settings = settingsPage(page), trigger = settingsSectionMenu(page);
  const rail = page.getByRole('tablist', { name: 'Settings pages' });
  const constrain = (width: string) => settings.evaluate((el, value) => { el.parentElement!.style.width = value; }, width);
  await openSettingsPage(page, 'Extensions');
  const extensions = rail.getByRole('tab', { name: 'Extensions' });
  await extensions.focus(); await expect(extensions).toBeFocused();
  await expect(settings.getByRole('heading', { level: 1, name: 'Extensions', exact: true })).toBeVisible();
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
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true, connected: true });
  // Presentation only: the same page, no menu opened, no native traffic.
  expect(await selectedSettingsPage(page)).toBe('Extensions');
  await expect(page.getByRole('menu')).toHaveCount(0);
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
  const settings = settingsPage(page), trigger = settingsSectionMenu(page);
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
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true, connected: true });
  expect(await nativeRequests(page)).toHaveLength(requests);

  // Rail navigation continues from there at once: the arrows move between
  // pages, and Escape, with no transient layer open, closes Settings.
  await page.keyboard.press('ArrowUp');
  await expect(rail.getByRole('tab', { name: 'Tools & Permissions' })).toBeFocused();
  await expect(rail.getByRole('tab', { name: 'Tools & Permissions' })).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('ArrowDown');
  await expect(extensions).toBeFocused();
  await expect(extensions).toHaveAttribute('aria-selected', 'true');
  await expect(settings.getByRole('heading', { level: 1, name: 'Extensions', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(settings).toBeVisible();
  await page.getByRole('button', { name: 'Home', exact: true }).click();
  await expect(settings).toBeHidden();
  expect(errors).toEqual([]);
});

/** Observe rendered frames after a responsive layout transition. */
const settled = (page: Page) => page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
/** Where the keyboard is: never the page body, never a disabled control,
 * always inside the Settings page. */
const keyboard = (page: Page) => page.evaluate(() => {
  const active = document.activeElement!;
  return { body: active === document.body, disabled: active.matches(':disabled'), settings: active.closest('[data-settings-page]') !== null, rendered: active.getClientRects().length > 0, connected: active.isConnected };
});
const releaseWrites = (page: Page) => page.evaluate(() => (window as unknown as { rustxReleaseWrites: () => void }).rustxReleaseWrites());

test('confirming a removal settles focus on its unit while the write is in flight; dismissal returns it to the trigger', async ({ page }) => {
  const errors = await start(page, '?write=held');
  const settings = settingsPage(page);
  await openUserSettings(page);
  await openSettingsPage(page, 'Models');
  await settings.getByRole('button', { name: 'transport →', exact: true }).click();
  const unit = settings.getByRole('form', { name: 'Provider transport' });
  const remove = unit.getByRole('button', { name: 'Provider actions', exact: true });
  const deletion = page.getByRole('alertdialog', { name: 'Remove Provider transport from User configuration?' });
  const openDeletion = async () => {
    await page.keyboard.press('Enter');
    await expect(page.getByRole('menuitem', { name: 'Delete', exact: true })).toBeVisible();
    await page.getByRole('menuitem', { name: 'Delete', exact: true }).focus();
    await page.keyboard.press('Enter');
  };
  const cancel = deletion.getByRole('button', { name: 'Cancel', exact: true });
  const writes = async () => (await nativeRequests(page)).filter(request => request.method === 'configuration/sourceWrite').length;

  // Cancel: nothing is written and focus returns to the trigger.
  await expect(remove).toBeEnabled(); await remove.focus(); await expect(remove).toBeFocused();
  await openDeletion();
  await expect(cancel).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(deletion).toHaveCount(0);
  await expect(remove).toBeFocused();
  await expect(settings).toBeVisible();
  expect(await writes()).toBe(0);
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true, connected: true });
  // Escape: only the confirmation closes, and focus returns to the trigger.
  await openDeletion();
  await expect(cancel).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(deletion).toHaveCount(0);
  await expect(remove).toBeFocused();
  await expect(settings).toBeVisible();
  expect(await writes()).toBe(0);
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true, connected: true });

  // With the transient layer gone, Escape leaves the Settings page intact.
  await page.keyboard.press('Escape');
  await expect(settings).toBeVisible();
  await page.getByRole('button', { name: 'Home', exact: true }).click();
  await expect(settings).toBeHidden();
  expect(await writes()).toBe(0);
  await openUserSettings(page);
  await openSettingsPage(page, 'Models');
  const listBack = settings.getByRole('button', { name: 'Model Settings', exact: true });
  if (await listBack.isVisible()) await listBack.click();
  await settings.getByRole('button', { name: 'transport →', exact: true }).click();
  await expect(remove).toBeEnabled(); await remove.focus(); await expect(remove).toBeFocused();

  // Confirm: the deletion modal submits immediately and holds the write in
  // flight. The unit's controls, the trigger among them, are closed until
  // native answers, so focus settles on the unit itself — not the body.
  await openDeletion();
  await expect(cancel).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(deletion.getByRole('button', { name: 'Remove Provider transport', exact: true })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(deletion).toHaveCount(0);
  await expect.poll(writes).toBe(1);
  await expect(remove).toBeDisabled();
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true, connected: true });
  // The native response is still held; the trigger is still closed.
  await expect(remove).toBeDisabled();

  // Native answers: the removal settles normally, exactly once, and the
  // keyboard is still inside the unit's workflow.
  await releaseWrites(page);
  await expect(unit).toHaveCount(0);
  await expect(remove).toHaveCount(0);
  await expect.poll(writes).toBe(1);
  await expect(settings.getByRole('region', { name: 'Model Settings', exact: true })).toBeFocused();
  await expect(settings.getByRole('button', { name: 'transport →', exact: true })).toHaveCount(0);
  // Tab continues from Models after its removed provider editor unmounts.
  await page.keyboard.press('Tab');
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true, connected: true });
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('toggling Follow global settings commits the restore of inheritance while the write is in flight', async ({ page }) => {
  const errors = await start(page, '?write=held');
  const settings = settingsPage(page);
  await openWorkspaceSettings(page, 'Workspace A');
  await openSettingsPage(page, 'Agent');
  const follow = settings.getByRole('switch', { name: 'Follow global settings', exact: true });
  const unit = settings.getByRole('form', { name: 'Project guidance' });
  // This Workspace mutation travels through the native Product Host boundary.
  const writes = () => page.evaluate(() => (window as unknown as { rustxHeldWorkspaceWrites: () => number }).rustxHeldWorkspaceWrites());

  // Toggling Follow global settings ON submits the removal immediately; the
  // write is held in flight while the switch already reads as following.
  await expect(follow).not.toBeChecked(); await follow.focus(); await expect(follow).toBeFocused();
  await page.keyboard.press('Space');
  // Restoring inheritance clears workspace overrides, which confirms first.
  await settings.getByRole('button', { name: 'Clear workspace overrides', exact: true }).click();
  await expect(follow).toBeChecked();
  await expect(unit).toHaveCount(0);
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true, connected: true });
  await expect.poll(writes).toBe(1);

  await releaseWrites(page);
  await expect(follow).toBeChecked();
  await expect(unit).toHaveCount(0);
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true, connected: true });
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

for (const unavailable of ['disabled', 'hidden', 'removed'] as const) test(`dismissal settles on the unit when its trigger becomes ${unavailable}`, async ({ page }) => {
  const errors = await start(page, '?write=held');
  await openUserSettings(page);
  const settings = settingsPage(page);
  await openSettingsPage(page, 'Models');
  await settings.getByRole('button', { name: 'transport →', exact: true }).click();
  const unit = settings.getByRole('form', { name: 'Provider transport' });
  const trigger = page.locator('form[aria-label="Provider transport"] button[aria-label="Provider actions"]');
  await trigger.click();
  await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
  const confirmation = page.getByRole('alertdialog');
  await expect(confirmation.getByRole('button', { name: 'Cancel' })).toBeFocused();
  // Simulate an invoking control becoming unavailable while the modal owns
  // focus. The other unit controls remain usable, so settlement must be the
  // form itself, not an arbitrary descendant selected by a focus heuristic.
  await trigger.evaluate((node, reason) => {
    if (reason === 'removed') node.remove();
    else if (reason === 'hidden') (node as HTMLElement).style.display = 'none';
    else (node as HTMLButtonElement).disabled = true;
  }, unavailable);
  await confirmation.getByRole('button', { name: 'Cancel' }).click();
  await expect(confirmation).toHaveCount(0);
  await expect(settings).toBeVisible();
  await expect(unit).toBeFocused();
  await expect(unit.getByLabel('Base URL', { exact: true })).toBeEnabled();
  expect(await keyboard(page)).toEqual({ body: false, disabled: false, settings: true, rendered: true, connected: true });
  expect((await nativeRequests(page)).filter(request => request.method === 'configuration/sourceWrite')).toHaveLength(0);
  expect(errors).toEqual([]);
});

test('Keep editing a keyboard navigation request does not reopen the discard prompt', async ({ page }) => {
  const errors = await start(page, '?write=held');
  await openUserSettings(page);
  const settings = settingsPage(page);
  await openSettingsPage(page, 'Agent');
  // Stage a draft, then request another page from the rail by keyboard: the
  // guard asks, Keep editing answers, and the keyboard landing on the
  // still-selected tab must not read as another navigation request.
  const instructions = settings.getByRole('form', { name: 'Global instructions' });
  await instructions.getByRole('textbox', { name: 'Global instructions', exact: true }).fill('Review the changes.');
  await expect(settings.getByRole('button', { name: 'Keep editing', exact: true })).toHaveCount(0);
  const rail = page.getByRole('tablist', { name: 'Settings pages' });
  await rail.getByRole('tab', { name: 'Agent', exact: true }).focus();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('button', { name: 'Keep editing', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Keep editing', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Keep editing', exact: true })).toHaveCount(0);
  // The prompt stays closed: focus settled on the still-selected Agent tab,
  // and the draft remains editable.
  await expect(rail.getByRole('tab', { name: 'Agent', exact: true })).toBeFocused();
  await expect(instructions.getByRole('textbox', { name: 'Global instructions', exact: true })).toHaveValue('Review the changes.');
  await new Promise(resolve => setTimeout(resolve, 400));
  await expect(page.getByRole('button', { name: 'Keep editing', exact: true })).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('reduced motion removes Settings transitions', async ({ page }) => {
  const errors = await start(page, '?session=preparing');
  await expect(banner(page)).toHaveCount(0);
  await openUserSettings(page);
  await openSettingsPage(page, 'Extensions');
  await settingsPage(page).getByRole('tab', { name: 'Built-in', exact: true }).click();
  // The coss switch root is the track; duration-200 is the design motion.
  const track = settingsPage(page).getByRole('switch').first();
  await expect(track).toHaveCSS('transition-duration', '0s');
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await expect(track).toHaveCSS('transition-duration', '0.2s');
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
    expect(await selectedSettingsPage(page)).toBe('Tools & Permissions');
    // A tap outside the open menu dismisses it without closing Settings.
    await menu.tap();
    await expect(page.getByRole('menu')).toBeVisible();
    // Well below the open list, inside the Settings panel.
    await page.touchscreen.tap(195, 780);
    await expect(page.getByRole('menu')).toHaveCount(0);
    await expect(settingsPage(page)).toBeVisible();
    expect(errors).toEqual([]);
  });
});

type NativeRequest = { method: string; params: Record<string, unknown> };
const nativeRequests = (page: Page) => page.evaluate(() => (window as unknown as { rustxNativeRequests: () => NativeRequest[] }).rustxNativeRequests());
const banner = (page: Page) => page.getByRole('region', { name: 'Session configuration' });

test('Session configuration banner: preparing, waiting and failed, compact and owner-correct', async ({ page }) => {
  let errors = await start(page, '?session=preparing');
  await expect(banner(page)).toHaveCount(0);
  expect(errors).toEqual([]);

  // Waiting: the prepared change applies by itself after current work; the
  // browser confirms, schedules and cancels nothing.
  errors = await start(page, '?session=ready');
  const waiting = banner(page).locator('[data-state="waiting"]');
  await expect(waiting).toContainText('Configuration changes will apply automatically after the current work settles.');
  await expect(banner(page).getByRole('button')).toHaveCount(0);
  await expectAccessible(page, '[aria-label="Session configuration"]');
  await expectStableScreenshot(page.locator('#session-view > header'), 'session-banner-waiting-light.png');
  expect((await nativeRequests(page)).filter(request => ['configuration/reconcile', 'turn/interrupt'].includes(request.method))).toEqual([]);
  expect(errors).toEqual([]);

  // Failed: the native diagnostic, one action per native owner — here User
  // and Workspace B, although the focused Session lives in /workspace/A — and
  // a native Retry.
  errors = await start(page, '?session=failed');
  const failed = banner(page).locator('[data-state="failed"]');
  await expect(failed).toContainText('previous configuration');
  await expect(failed).toContainText('Capabilities: failed — MCP server repository-index failed to start');
  await expect(failed.getByRole('button')).toHaveText(['Open User Settings', 'Open Workspace Settings — /workspace/B', 'Retry']);
  await expectStableScreenshot(page.locator('#session-view > header'), 'session-banner-failed-light.png');
  await failed.getByRole('button', { name: 'Open Workspace Settings — /workspace/B' }).click();
  await expect(settingsPage(page).getByRole('button', { name: 'Workspace B', exact: true })).toBeVisible();
  await returnHome(page);
  // Narrow: the reason and the actions wrap; nothing scrolls sideways.
  await setTheme(page, 'Dark');
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(failed.getByRole('button', { name: 'Open Workspace Settings — /workspace/B' })).toBeVisible();
  await expectNoHorizontalOverflow(page);
  await expectStableScreenshot(page.locator('#session-view > header'), 'session-banner-failed-mobile-dark.png');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
});

test('Session configuration banner neither flickers nor rereads configuration while an answer streams', async ({ page }) => {
  const errors = await start(page, '?session=ready');
  const waiting = banner(page).locator('[data-state="waiting"]');
  await expect(waiting).toContainText('Configuration changes will apply automatically after the current work settles.');
  // Record every DOM mutation that ever shows the unavailable line, so a single
  // painted frame of it fails the test.
  await page.evaluate(() => {
    const record = window as unknown as { rustxUnavailableFrames: number };
    record.rustxUnavailableFrames = 0;
    new MutationObserver(() => {
      if (document.querySelector('[aria-label="Session configuration"] [data-state="unavailable"]')) record.rustxUnavailableFrames++;
    }).observe(document.body, { subtree: true, childList: true, characterData: true, attributes: true });
  });
  const reads = async () => (await nativeRequests(page)).filter(request => request.method === 'session/configuration').length;
  const before = await reads();
  await page.evaluate(() => (window as unknown as { rustxStream: (deltas: number) => Promise<void> }).rustxStream(200));
  expect(await page.evaluate(() => (window as unknown as { rustxUnavailableFrames: number }).rustxUnavailableFrames)).toBe(0);
  expect(await reads()).toBe(before);
  await expect(waiting).toBeVisible();
  expect(errors).toEqual([]);
});

test('Provider split-panel deletion preserves exact unit CAS and updates the navigation after observation', async ({ page }) => {
  const errors = await start(page, '?write=held');
  await openUserSettings(page); await openSettingsPage(page, 'Models');
  const settings = settingsPage(page);
  await settings.getByRole('button', { name: 'transport →', exact: true }).click();
  const remove = settings.getByRole('button', { name: 'Provider actions', exact: true });
  const openDeletion = async () => {
    await remove.click();
    await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
  };
  await openDeletion();
  const confirmation = page.getByRole('alertdialog', { name: 'Remove Provider transport from User configuration?', exact: true });
  await expect(confirmation).toBeVisible();
  await page.keyboard.press('Escape'); await expect(remove).toBeFocused();
  await openDeletion();
  await confirmation.getByRole('button', { name: 'Remove Provider transport', exact: true }).click();
  await expect(confirmation).toHaveCount(0);
  // Confirming deletion inside the alertdialog immediately submits the CAS write.
  await expect.poll(async () => (await nativeRequests(page)).filter(request => request.method === 'configuration/sourceWrite')).toHaveLength(1);
  const write = (await nativeRequests(page)).find(request => request.method === 'configuration/sourceWrite')!;
  expect(write.params).toMatchObject({ target: { kind: 'user' }, expected_revision: 'user-1', mutation: { kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: null } } });
  await releaseWrites(page);
  await expect(settings.getByRole('button', { name: 'transport →', exact: true })).toHaveCount(0);
  await expect(settings.getByRole('button', { name: `${longProvider} →`, exact: true })).toBeVisible();
  await expectAccessible(page, settingsScope); expect(errors).toEqual([]);
});

for (const viewport of [{ width: 1440, height: 1000 }, { width: 390, height: 844 }]) {
  test(`native JSON Schema capability is one editable control at ${viewport.width}px`, async ({ page }) => {
    const errors = await start(page, '', viewport);
    await openUserSettings(page);
    await openSettingsPage(page, 'Models');
    const settings = settingsPage(page);
    await settings.getByRole('button', { name: `${longProvider} →`, exact: true }).click();
    await settings.getByRole('listitem', { name: longModel, exact: true }).getByRole('button', { name: 'Edit Model', exact: true }).click();
    const model = page.getByRole('dialog', { name: `Model ${longModel}`, exact: true });
    await expect(model.getByRole('heading', { name: 'Edit Model', exact: true })).toBeVisible();
    await model.getByRole('button', { name: 'Advanced configuration', exact: true }).click();
    const capability = model.getByRole('switch', { name: 'Structured output', exact: true });
    await expect(capability).toHaveCount(1);
    await expect(capability).not.toBeChecked();
    // The settings row itself is the click target that actuates the toggle.
    await capability.locator('xpath=..').click();
    await expect(capability).toBeChecked();
    await expect(model.getByRole('button', { name: `Save Model ${longModel}`, exact: true })).toBeEnabled();
    await expectNoHorizontalOverflow(page);
    await model.screenshot({ path: test.info().outputPath(`native-json-schema-${viewport.width}.png`) });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

test('scalar pages have no footer; the structured provider editor keeps one flush at viewport bottom, and Connection controls render cleanly', async ({ page }) => {
  const errors = await start(page);
  await openUserSettings(page);
  const settings = settingsPage(page);

  for (const name of ['General', 'Models', 'Agent', 'Advanced'] as const) {
    await openSettingsPage(page, name);
    await expect(settings.locator('[data-settings-footer]')).toHaveCount(0);
  }
  await openSettingsPage(page, 'Models');
  await settings.getByRole('button', { name: 'transport →', exact: true }).click();
  const footer = settings.locator('[data-settings-footer]');
  await expect(footer).toBeVisible();
  const box = (await footer.boundingBox())!;
  expect(Math.abs(box.y + box.height - 1000)).toBeLessThanOrEqual(2);

  await openSettingsPage(page, 'Advanced');
  await settings.getByRole('button', { name: 'Connection', exact: true }).click();
  await expect(settings.getByRole('region', { name: 'Connection Settings', exact: true })).toBeVisible();
  const mode = settings.getByLabel('Connection mode', { exact: true });
  await expect(mode).toBeVisible();
  await selectCossOption(mode, 'remote');
  await expect(settings.getByLabel('WebSocket endpoint', { exact: true })).toBeVisible();
  await expect(settings.getByLabel('Access token', { exact: true })).toBeVisible();
  // Single clean border comes from the Input primitive wrapper; inner input has border 0.
  const endpointBorder = await settings.getByLabel('WebSocket endpoint', { exact: true }).evaluate(el => getComputedStyle(el).borderTopWidth);
  expect(endpointBorder).toBe('0px');
  await settings.screenshot({ path: test.info().outputPath('connection-settings-light.png') });
  await expectAccessible(page, settingsScope);
  expect(errors).toEqual([]);
});

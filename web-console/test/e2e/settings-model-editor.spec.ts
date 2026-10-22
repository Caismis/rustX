import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { openSettingsPage } from './shell-actions';
import { expectStableScreenshot } from './screenshot';

const fixture = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/settings.html`;
const provider = 'enterprise-inference-gateway-eu-central-primary-with-an-exceptionally-long-provider-identity';
const model = 'enterprise-reasoning-model-2026-09-long-context-preview-with-an-exceptionally-long-identity';
const settings = (page: Page) => page.getByRole('dialog', { name: 'Settings', exact: true });
const editor = (page: Page) => page.getByRole('dialog', { name: `Model ${model}`, exact: true });
const writes = (page: Page) => page.evaluate(() => (window as unknown as {rustxNativeRequests: () => {method:string}[]}).rustxNativeRequests().filter(request => request.method === 'configuration/sourceWrite').length);

function runtimeErrors(page: Page) {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', event => { if (['error','warning'].includes(event.type())) errors.push(event.text()); });
  return errors;
}
async function start(page: Page, theme: 'Light' | 'Dark', width: number) {
  const errors = runtimeErrors(page);
  await page.clock.setFixedTime(new Date('2026-10-21T12:00:00Z'));
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
  await page.goto(fixture);
  await expect(page).toHaveTitle('rustX native Settings reference');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await settings(page).getByRole('group', { name: 'Appearance' }).getByRole('button', { name: theme, exact: true }).click();
  return errors;
}
async function accessible(page: Page) {
  const result = await new AxeBuilder({page}).include('[role="dialog"][aria-label^="Model "], [role="dialog"][aria-label^="模型 "]').withTags(['wcag2a','wcag2aa','wcag21a','wcag21aa']).analyze();
  expect(result.violations.filter(item => item.impact === 'serious' || item.impact === 'critical')).toEqual([]);
}
async function noOverflow(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const box = (await editor(page).boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual((page.viewportSize()!).width);
  expect(box.y + box.height).toBeLessThanOrEqual((page.viewportSize()!).height);
}

for (const theme of ['Light', 'Dark'] as const) for (const width of [1440,390]) {
  test(`${theme} ${width}: appearance stays on one row, model basic/advanced and footer remain usable`, async ({page}) => {
    const errors = await start(page, theme, width);
    const appearance = settings(page).getByRole('group', {name:'Appearance'});
    const choices = await appearance.getByRole('button').evaluateAll(nodes => nodes.map(node => {
      const {y,width} = node.getBoundingClientRect(); return {y,width};
    }));
    expect(choices).toHaveLength(3);
    expect(new Set(choices.map(item => item.y)).size).toBe(1);
    expect(Math.max(...choices.map(item=>item.width))-Math.min(...choices.map(item=>item.width))).toBeLessThan(1);
    await expectStableScreenshot(appearance, `appearance-${theme.toLowerCase()}-${width}.png`);
    await openSettingsPage(page, 'Models');
    await settings(page).getByRole('row', {name:provider, exact:true}).click();
    await expectStableScreenshot(settings(page), `providers-${theme.toLowerCase()}-${width}.png`);
    await settings(page).getByRole('row', {name:model, exact:true}).click();
    const modal = editor(page);
    const context = modal.getByLabel('Context window', {exact:true});
    await expect(context).toBeFocused();
    await expect(modal.getByText(/Smart configuration|智能配置/)).toHaveCount(0);
    await expect(modal.getByRole('button', {name:'Save Model '+model, exact:true})).toBeDisabled();
    await expect(modal.getByRole('button', {name:'Advanced configuration', exact:true})).toHaveAttribute('aria-expanded','false');
    await noOverflow(page); await accessible(page);
    await expectStableScreenshot(page, `model-basic-${theme.toLowerCase()}-${width}.png`);
    await modal.getByRole('button', {name:'Advanced configuration', exact:true}).click();
    await expect(modal.getByRole('button', {name:'Request defaults and protocol compatibility', exact:true})).toHaveCount(0);
    for (const name of ['Chat reasoning replay', 'Chat output field', 'Chat stream usage', 'Chat tool protocol', 'Responses storage']) {
      await expect(modal.getByLabel(name, {exact:true})).toHaveCount(0);
    }
    await expect(modal.getByRole('textbox', {name:'Model request parameters', exact:true})).toHaveCount(1);
    await expect(modal.getByRole('checkbox')).toHaveCount(2);
    await expect(modal.getByRole('checkbox', {name:'Tool calls', exact:true})).toHaveCount(0);
    await expect(modal.getByRole('checkbox', {name:'File', exact:true})).toHaveCount(0);
    await expect(modal.getByText('Output types', {exact:true})).toHaveCount(0);
    await modal.getByRole('checkbox', {name:'Image', exact:true}).first().uncheck();
    await context.fill('256000');
    await expect(modal.getByRole('button', {name:'Reset form', exact:true})).toBeEnabled();
    // Scrolling the body never moves its title or footer outside the dialog.
    await modal.getByLabel('Provider identity', {exact:true}).scrollIntoViewIfNeeded();
    await expect(modal.getByRole('heading', {name:'Edit Model', exact:true})).toBeInViewport();
    for (const name of ['Cancel','Reset form','Save Model '+model]) await expect(modal.getByRole('button', {name,exact:true})).toBeInViewport();
    await expectStableScreenshot(page, `model-advanced-${theme.toLowerCase()}-${width}.png`);
    await modal.getByRole('button', {name:'Reset form',exact:true}).click();
    await expect(context).toHaveValue('1048576');
    await expect(modal.getByRole('checkbox', {name:'Image',exact:true}).first()).toBeChecked();
    const addProfile = modal.getByRole('button', {name:'Add profile',exact:true});
    const identity = modal.getByRole('textbox', {name:'New profile identity',exact:true});
    const params = modal.getByRole('textbox', {name:'Profile parameters (JSON)',exact:true});
    await addProfile.click();
    await expect(identity).toBeFocused();
    await identity.fill('cancelled');
    await identity.press('Escape');
    await expect(modal.getByRole('button', {name:'Profile cancelled',exact:true})).toHaveCount(0);
    await expect(addProfile).toBeFocused();
    await expect(modal.getByRole('button', {name:'Save Model '+model,exact:true})).toBeDisabled();
    await addProfile.click();
    await identity.fill('low');
    await identity.press('Enter');
    await expect(modal.getByRole('button', {name:'Profile low',exact:true})).toHaveAttribute('aria-pressed','true');
    const lowValue = {request_params:{reasoning_effort:'low'}};
    await params.fill(JSON.stringify(lowValue));
    await addProfile.click();
    await identity.fill('low');
    await identity.press('Enter');
    await expect(modal.getByRole('alert')).toHaveText('low is already listed.');
    await identity.press('Escape');
    await addProfile.click();
    const high = 'high-with-an-exceptionally-long-profile-identity';
    await identity.fill(high);
    await identity.press('Enter');
    await expect(params).toHaveText(JSON.stringify({request_params:{}},null,2), {useInnerText:true});
    await params.fill('{"request_params":');
    await modal.getByRole('button', {name:'Profile low',exact:true}).click();
    await expect(params).toHaveText('{"request_params":', {useInnerText:true});
    await expect(params).toBeFocused();
    await expect(modal.getByRole('button', {name:'Profile '+high,exact:true})).toHaveAttribute('aria-pressed','true');
    const highValue = {max_output_tokens:16384,request_params:{reasoning_effort:'high'}};
    await params.fill(JSON.stringify(highValue));
    await modal.getByRole('button', {name:'Profile low',exact:true}).click();
    await expect(params).toHaveText(JSON.stringify(lowValue,null,2), {useInnerText:true});
    await modal.getByRole('button', {name:'Profile '+high,exact:true}).click();
    await expect(params).toHaveText(JSON.stringify(highValue,null,2), {useInnerText:true});
    const profile = modal.getByRole('group',{name:'Profile '+high,exact:true});
    await expect(profile.getByRole('textbox')).toHaveCount(1);
    await expect(profile.getByRole('button', {name:'Format JSON: Profile parameters (JSON)',exact:true})).toHaveCount(1);
    await expect(profile.getByRole('spinbutton')).toHaveCount(0);
    await modal.getByText('Default profile',{exact:true}).scrollIntoViewIfNeeded();
    await noOverflow(page); await accessible(page);
    await expectStableScreenshot(modal, `model-profiles-${theme.toLowerCase()}-${width}.png`);
    await modal.getByRole('button', {name:'Reset form',exact:true}).click();
    await expect(params).toHaveCount(0);
    await context.fill('256000');
    await modal.getByRole('button', {name:'Cancel',exact:true}).click();
    await expect(modal).toHaveCount(0);
    await settings(page).getByRole('row', {name:model, exact:true}).click();
    await expect(context).toHaveValue('256000'); // The native unit's actor owns the draft.
    await modal.getByRole('button', {name:'Reset form',exact:true}).click();
    await expect(context).toHaveValue('1048576');
    expect(await writes(page)).toBe(0);
    await noOverflow(page);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

test('provider creation validates the native name; API Key reveal is explicit and no navigation writes configuration', async ({page}) => {
  const errors = await start(page,'Light',1440);
  await openSettingsPage(page,'Models');
  await settings(page).getByRole('button',{name:'+ Add Provider',exact:true}).click();
  const add = page.getByRole('dialog',{name:'Add Provider',exact:true});
  await add.getByLabel('New Provider identity').fill('transport');
  await expect(add.getByRole('button',{name:'Continue'})).toBeDisabled();
  await expect(add.getByRole('alert')).toHaveText('This configuration name already exists.');
  await add.getByRole('button',{name:'Cancel'}).click();
  await settings(page).getByRole('row',{name:'transport',exact:true}).click();
  await settings(page).getByRole('button',{name:/Credential source$/}).click();
  await page.getByRole('option',{name:'Enter a literal secret'}).click();
  const key = settings(page).getByLabel('API Key',{exact:true});
  await key.fill('fixture-only-value');
  await expect(key).toHaveAttribute('type','password');
  await settings(page).getByRole('button',{name:'Show',exact:true}).click();
  await expect(key).toHaveAttribute('type','text');
  await settings(page).getByRole('button',{name:'Hide',exact:true}).click();
  await expect(key).toHaveAttribute('type','password');
  expect(await writes(page)).toBe(0);
  expect(errors).toEqual([]);
});

test('Chinese settings use the same model dialog without Smart configuration', async ({page}) => {
  const errors = runtimeErrors(page);
  await page.addInitScript(() => localStorage.setItem('rustx-locale-v1','zh'));
  await page.goto(fixture);
  await page.getByRole('button',{name:'设置',exact:true}).click();
  await page.getByRole('group',{name:'外观',exact:true}).getByRole('button',{name:'深色',exact:true}).click();
  await page.getByRole('tab',{name:'模型',exact:true}).click();
  await page.getByRole('row',{name:provider,exact:true}).click();
  await page.getByRole('row',{name:model,exact:true}).click();
  const modal = page.getByRole('dialog',{name:`模型 ${model}`,exact:true});
  await expect(modal.getByLabel('模型 ID',{exact:true})).toHaveValue(model);
  await modal.getByRole('button',{name:'高级配置',exact:true}).click();
  await expect(modal.getByText('智能配置',{exact:true})).toHaveCount(0);
  await expect(modal.getByRole('button',{name:'请求默认值与协议兼容性',exact:true})).toHaveCount(0);
  await expect(modal.getByRole('button',{name:'取消',exact:true})).toBeInViewport();
  await accessible(page);
  await expect(modal.getByText('输出类型',{exact:true})).toHaveCount(0);
  await expect(modal.getByRole('checkbox')).toHaveCount(2);
    await expect(modal.getByRole('checkbox', {name:'工具调用', exact:true})).toHaveCount(0);
  await expect(modal.getByRole('checkbox', {name:'文件', exact:true})).toHaveCount(0);
  await modal.getByText('输入类型',{exact:true}).scrollIntoViewIfNeeded();
  await expectStableScreenshot(modal,'model-advanced-zh-dark.png');
  for (const name of ['low', 'high']) {
    await modal.getByRole('button',{name:'添加预设',exact:true}).click();
    const identity = modal.getByRole('textbox',{name:'新预设标识',exact:true});
    await identity.fill(name);
    await identity.press('Enter');
  }
  await modal.getByRole('textbox',{name:'预设参数（JSON）',exact:true}).fill(JSON.stringify({max_output_tokens:16384,request_params:{reasoning_effort:'high'}},null,2));
  const profile = modal.getByRole('group',{name:'预设 high',exact:true});
  await expect(profile.getByRole('button', {name:'格式化 JSON：预设参数（JSON）',exact:true})).toHaveCount(1);
  await expect(profile.getByRole('spinbutton')).toHaveCount(0);
  await modal.getByText('默认预设',{exact:true}).scrollIntoViewIfNeeded();
  await accessible(page);
  await expectStableScreenshot(modal,'model-profiles-zh-dark.png');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
});

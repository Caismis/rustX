import { test,expect } from '@playwright/test';
import { captureEvidence, expectStableScreenshot } from './screenshot';
const origin=`http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT??5174}`;
for(const width of [1440,390])test(`completed turn changes and sampled back-to-bottom at ${width}px`,async({page})=>{
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message));
  await page.setViewportSize({width,height:1000});
  await page.emulateMedia({reducedMotion:'reduce'});
  await page.goto(`${origin}/test/fixtures/agent.html?mode=workspace-changes`);
  await expect(page).toHaveTitle('Session A — rustX');
  await expect(page.getByLabel('Canonical conversation')).toBeVisible();
  await expect(page.locator('[data-changed-files]')).toHaveCount(0);
  await page.evaluate(()=>window.workspaceChangesFixture.settle());
  const card=page.locator('[data-changed-files]');
  await expect(card).toBeVisible();await expect(card).toContainText('Edited 6 files');
  await expect(card.getByRole('listitem')).toHaveCount(4);
  await expect(page.locator('[data-chat-latest]')).toHaveCount(0);
  const scroller=page.locator('[data-conversation-scroll]').first();
  await scroller.evaluate(element=>{element.scrollTop-=10;});
  await expect(page.locator('[data-chat-latest]')).toHaveCount(0);
  await scroller.evaluate(element=>{element.scrollTop=0;});
  await expect(page.locator('[data-chat-latest]')).toBeVisible();
  await page.locator('[data-chat-latest]').click();
  await expect(page.locator('[data-chat-latest]')).toHaveCount(0);
  await expect(card).toBeInViewport();
  await expectStableScreenshot(card,`turn-changes-${width}.png`);
  await card.getByRole('button',{name:'All 6 files'}).click();
  await expect(card.getByRole('listitem')).toHaveCount(6);
  await card.getByRole('button',{name:'View changes to src/file-2.ts',exact:true}).click();
  const review=page.locator('[data-changes-review]');
  await expect(review).toBeVisible();
  await expect(review.getByRole('combobox')).toHaveValue('1');
  await expect(review.locator('[data-change-comparison]')).toContainText('export const enabled = true;');
  await expectStableScreenshot(review,`turn-changes-review-${width}.png`);
  await review.getByRole('combobox').selectOption('4');
  await expect(review.locator('[data-change-comparison]')).toContainText('src/file-5.ts');
  if(width===1440) {
    await page.getByRole('tab',{name:'Trajectory',exact:true}).click();
    await expect(page.getByRole('tab',{name:'Trajectory',exact:true})).toHaveAttribute('aria-selected','true');
    await review.getByRole('combobox').selectOption('2');
    await expect(review.locator('[data-change-comparison]')).toContainText('export const enabled = true;');
    await page.getByRole('tab',{name:'Chat',exact:true}).click();
    await expect(page.getByLabel('Canonical conversation')).toBeVisible();
    await review.getByRole('combobox').selectOption('0');
    await expect(review.locator('[data-change-comparison]')).toContainText('src/file-1.ts');
    await expect(review.locator('[data-change-comparison]')).toContainText('export const enabled = true;');
  }
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  expect(errors).toEqual([]);
});

test('a single changed file uses one compact card',async({page})=>{
  await page.goto(`${origin}/test/fixtures/agent.html?mode=workspace-changes`);
  await expect(page.getByLabel('Canonical conversation')).toBeVisible();
  await page.evaluate(()=>window.workspaceChangesFixture.settle(1));
  const card=page.locator('[data-changed-files]');
  await expect(card).toContainText('Edited file-1.ts');
  await expect(card.getByRole('list')).toHaveCount(0);
  await card.getByRole('button').click();
  await expect(page.locator('[data-changes-review]')).toBeVisible();
});

for (const dark of [false, true]) test(`long changed-file previews fit the viewport and scroll independently (dark=${dark})`, async ({page}) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.goto(`${origin}/test/fixtures/agent.html?mode=workspace-changes&long-diff`);
  await page.evaluate(dark => document.body.toggleAttribute('data-ds-dark-theme', dark), dark);
  await expect(page).toHaveTitle('Session A — rustX');
  await expect(page.getByLabel('Canonical conversation')).toBeVisible();
  await page.evaluate(() => window.workspaceChangesFixture.settle());
  const card = page.locator('[data-changed-files]');
  const row = card.getByRole('button', {name: /View changes to src\/nested-workspace-folder/});
  const preview = page.locator('[data-changes-hover-preview]');
  const path = preview.locator('[data-changes-preview-path]');
  const diff = preview.locator('[data-review-view="unified"]');
  await expect(row).toBeVisible();
  expect(await page.evaluate(() => window.workspaceChangesFixture.requests())).toEqual([
    expect.objectContaining({method:'conversation/changes'}),
  ]);
  for (const width of [1214, 390]) {
    await page.setViewportSize({width, height: 660});
    await row.hover({position: {x: 60, y: 12}});
    await expect(diff.locator('[data-diff-line="del"]')).toContainText('19-export const old');
    await expect(diff.locator('[data-diff-line="add"]').first()).toContainText('27+export const path');
    await expect(diff.locator('[data-diff-line="add"]').last()).toContainText('67+export const value39');
    await expect(row).toHaveAccessibleDescription(await path.textContent() ?? '');
    await expect(row).not.toHaveAttribute('title');
    const anchor = (await row.boundingBox())!;
    const bounds = (await preview.boundingBox())!;
    const cardBounds = (await card.boundingBox())!;
    expect(bounds.x - cardBounds.x).toBeCloseTo(24, 0);
    expect(bounds.width).toBeCloseTo(cardBounds.width - 48, 0);
    expect(bounds.height).toBeLessThanOrEqual(420);
    expect(bounds.y).toBeGreaterThanOrEqual(8);
    expect(bounds.y + bounds.height).toBeLessThanOrEqual(652);
    expect(bounds.y + bounds.height <= anchor.y || bounds.y >= anchor.y + anchor.height).toBe(true);
    await expect(path).toHaveCSS('white-space', 'nowrap');
    expect(await path.evaluate(element => { element.scrollLeft = 50; return element.scrollLeft; })).toBe(50);
    await path.evaluate(element => { element.scrollLeft = 0; });
    await diff.hover();
    expect(await diff.evaluate(element => {
      element.scrollTo(80, 90);
      return [element.scrollLeft, element.scrollTop];
    })).toEqual([80, 90]);
    await diff.evaluate(element => { element.scrollTo(0, 0); });
    await captureEvidence(page, `changed-file-preview-${width}`);
    if (width === 1214) await page.keyboard.press('Escape');
    else await row.click();
    await expect(preview).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  }
  await expect(page.locator('[data-changes-review] [data-change-comparison]')).toContainText('export const path');
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('changed-file previews dismiss when their anchor becomes hidden, inert or scrolled away', async ({page}) => {
  await page.goto(`${origin}/test/fixtures/agent.html?mode=workspace-changes&long-diff`);
  await expect(page.getByLabel('Canonical conversation')).toBeVisible();
  await page.evaluate(() => window.workspaceChangesFixture.settle());
  const card = page.locator('[data-changed-files]');
  const row = card.locator('ul button').first();
  const preview = page.locator('[data-changes-hover-preview]');
  for (const attribute of ['hidden', 'inert']) {
    await page.mouse.move(0, 0);
    await row.hover();
    await expect(preview).toBeVisible();
    await card.evaluate((element, name) => element.setAttribute(name, ''), attribute);
    await expect(preview).toHaveCount(0);
    await card.evaluate((element, name) => element.removeAttribute(name), attribute);
  }
  await page.mouse.move(0, 0);
  await row.hover();
  await expect(preview).toBeVisible();
  await page.locator('[data-conversation-scroll]').first().evaluate(element => { element.scrollTop = 0; });
  await expect(row).not.toBeInViewport();
  await expect(preview).toHaveCount(0);
});

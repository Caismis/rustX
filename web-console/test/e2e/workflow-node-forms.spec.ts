import { test, expect, type Locator, type Page, type TestInfo } from '@playwright/test';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { captureEvidence } from './screenshot';
import { emptyBlock } from '../../src/app/workflows/document/commands';
import { setNodePresentation } from '../../src/app/workflows/document/geometry';
import type { NodeKind, WorkflowDocument } from '../../src/app/workflows/document/model';

const canvas = (page: Page) => page.getByRole('region', { name: 'Workflow canvas' });
const inspector = (page: Page) => page.getByRole('complementary', { name: 'Inspector' });

/** Every ordinary card grows and shrinks with its visible content, retaining
 * only its normal bottom padding. This also covers Start inside a Loop. */
async function checkCardHeight(page: Page, card: Locator, evidence: string, info: TestInfo) {
  const description = inspector(page).getByRole('textbox', { name: 'Description', exact: true });
  const original = await description.inputValue();
  const height = () => card.evaluate(element => (element as HTMLElement).offsetHeight);
  const initialHeight = await height();
  await description.fill(''); await description.blur();
  await expect(card.locator('[data-node-description]')).toHaveCount(0);
  if (original.trim()) await expect.poll(height).toBeLessThan(initialHeight);
  const baseline = await height();
  const contentGap = () => card.locator('[data-node-description]').evaluate(element => {
    const card = element.parentElement!, bounds = card.getBoundingClientRect(), scale = bounds.width / card.offsetWidth;
    return (bounds.bottom - element.getBoundingClientRect().bottom) / scale;
  });
  await description.fill('Short description.'); await description.blur();
  await expect(card.locator('[data-node-description]')).toBeVisible();
  await expect.poll(height).toBe(baseline + 26);
  expect(await contentGap()).toBeGreaterThanOrEqual(11);
  expect(await contentGap()).toBeLessThan(15);
  await captureEvidence(page, `${evidence}-short-description`, info);
  await description.fill('First line.\nSecond line.\nThird line.'); await description.blur();
  await expect(card.locator('[data-node-description]')).toHaveCSS('-webkit-line-clamp', '2');
  await expect.poll(height).toBe(baseline + 42);
  expect(await contentGap()).toBeLessThan(15);
  await description.fill('A long description that wraps naturally as more text is entered into the node description field.'); await description.blur();
  await expect.poll(height).toBe(baseline + 42);
  await description.fill(''); await description.blur();
  await expect(card.locator('[data-node-description]')).toHaveCount(0);
  await expect.poll(height).toBe(baseline);
  await description.fill(original); await description.blur();
  await expect.poll(height).toBe(initialHeight);
}

test('shows each native node in the Dify-style card and grouped settings panel', async ({ page }, info) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    let document: WorkflowDocument = { description: 'Node surfaces', block: emptyBlock() };
    document.block.input = { type: 'object', properties: { topic: { type: 'string' } }, required: ['topic'], additionalProperties: false };
    document.block.nodes.agent = { type: 'agent', profile: 'reviewer', task: [{ type: 'literal', value: 'Research the requested topic' }], output: { type: 'json_schema', schema: { type: 'object', properties: { summary: { type: 'string' } }, additionalProperties: false } } };
    document.block.nodes.model = { type: 'model', model: { mode: 'session' }, messages: [{ role: 'system', content: [{ type: 'literal', value: 'Write a concise summary.' }] }, { role: 'user', content: [{ type: 'literal', value: 'Go.' }] }], output: { type: 'text' } };
    document.block.nodes.tool = { type: 'tool', selector: { origin: 'builtin', name: 'read' }, arguments: { type: 'object', fields: { path: { type: 'reference', path: ['args', 'topic'] }, token: { type: 'literal', value: 'super-secret' } } }, result: { type: 'text', part: 0 } };
    document.block.nodes.review = { type: 'review', subject: { type: 'plan', value: { type: 'reference', path: ['model'] } }, context: [{ type: 'reference', path: ['args', 'topic'] }] };
    document.block.nodes.loop = { type: 'loop', input: { type: 'reference', path: ['args'] }, body: emptyBlock(), until: { type: 'boolean', value: { type: 'literal', value: true } }, carry: { type: 'reference', path: ['result'] }, max_iterations: 3 };
    document.block.nodes.done = { type: 'return', output: { type: 'object', fields: { summary: { type: 'reference', path: ['model', 'text'] }, status: { type: 'literal', value: 'ready' } } } };
    document.block.edges = [{ from: 'start', to: 'agent' }, { from: 'agent', to: 'model' }, { from: 'model', to: 'tool' }, { from: 'tool', to: 'review' }, { from: 'review', to: 'loop' }, { from: 'loop', to: 'done', port: 'satisfied' }];
    const positions: [string, number, number][] = [['agent', 340, 80], ['model', 690, 80], ['tool', 1040, 80], ['review', 340, 400], ['loop', 690, 400], ['done', 1040, 400]];
    for (const [id, x, y] of positions) document = setNodePresentation(document, [], id, { title: id, description: id === 'model' ? 'Configure model here.\nWrite a concise response.' : `Configure ${id} here.`, position: { x, y } });
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    writeFileSync(join(directory, 'node_surfaces.yaml'), JSON.stringify(document));
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await page.getByRole('button', { name: 'node_surfaces', exact: true }).click();
    await expect(page).toHaveTitle(/rustX$/);
    const start = canvas(page).getByLabel('Workflow Start', { exact: true }); await start.focus(); await start.press('Enter');
    await expect(inspector(page).getByRole('heading', { name: 'Input fields' })).toBeVisible();
    await checkCardHeight(page, start, 'start', info);
    await captureEvidence(page, 'start-panel', info);
    const cases: { id: string; kind: NodeKind; row: string; section: string }[] = [
      { id: 'agent', kind: 'agent', row: 'reviewer', section: 'Agent configuration' },
      { id: 'model', kind: 'model', row: 'Follow Attempt model', section: 'Output variables' },
      { id: 'tool', kind: 'tool', row: 'path', section: 'Tool configuration' },
      { id: 'review', kind: 'review', row: 'model', section: 'Review request' },
      { id: 'loop', kind: 'loop', row: '3', section: 'Loop body' },
      { id: 'done', kind: 'return', row: 'summary', section: 'Output contract' },
    ];
    for (const { id, kind, row, section } of cases) {
      const card = canvas(page).getByLabel(`${kind[0].toUpperCase()}${kind.slice(1)} node ${id}`, { exact: true });
      await card.focus(); await card.press('Enter');
      if (id === 'loop') await card.getByRole('button', { name: 'Collapse loop' }).click();
      await expect(card.locator('[data-node-preview]')).toContainText(row);
      if (id === 'tool') await expect(card).not.toContainText('super-secret');
      await expect(inspector(page).getByRole(id === 'model' ? 'region' : 'heading', { name: section, exact: true }).first()).toBeVisible();
      await expect(inspector(page).getByLabel('Node ID', { exact: true })).toHaveCount(0);
      await expect(inspector(page).getByText('Native identity and rename', { exact: true })).toHaveCount(0);
      await checkCardHeight(page, card, id, info);
      await expect(card).toContainText(`Configure ${id} here.`);
      const layout = await card.evaluate(element => {
        const preview = element.querySelector('[data-node-preview]')!, description = element.querySelector('[data-node-description]')!;
        return { card: element.getBoundingClientRect().bottom, preview: preview.getBoundingClientRect().bottom, description: description.getBoundingClientRect().bottom };
      });
      expect(layout.preview).toBeLessThan(layout.card - 8);
      expect(layout.description).toBeLessThanOrEqual(layout.card - 8);
      await captureEvidence(page, `${id}-panel`, info);
      if (id === 'agent') {
        await inspector(page).getByRole('button', { name: 'Agent profile', exact: true }).click();
        await expect(page.getByRole('menuitemradio', { name: 'reviewer', exact: true })).toHaveAttribute('aria-checked', 'true');
        await page.keyboard.press('Escape');
        const height = () => card.evaluate(element => (element as HTMLElement).offsetHeight);
        const before = await height(), task = inspector(page).getByRole('textbox', { name: 'Task', exact: true });
        await task.fill(''); await task.blur();
        await expect.poll(height).toBe(before - 27);
        await task.fill('Research the requested topic'); await task.blur();
        await expect.poll(height).toBe(before);
      }
      if (id === 'model') {
        const height = () => card.evaluate(element => (element as HTMLElement).offsetHeight), baseline = await height();
        const strategy = inspector(page).getByRole('button', { name: 'Error handling strategy', exact: true });
        for (const label of ['Default value', 'Fail branch']) {
          await strategy.click(); await page.getByRole('menuitem', { name: new RegExp(`^${label}`) }).click();
          await expect.poll(height).toBe(baseline + 32);
          await checkCardHeight(page, card, `model-${label.toLowerCase().replaceAll(' ', '-')}`, info);
          if (label === 'Fail branch') {
            const port = card.locator('[data-handleid="fail"]');
            await expect(port).toBeVisible();
            const row = card.getByText('Fail branch', { exact: true });
            const portBounds = (await port.boundingBox())!, rowBounds = (await row.boundingBox())!;
            expect(Math.abs(portBounds.y + portBounds.height / 2 - rowBounds.y - rowBounds.height / 2)).toBeLessThan(2);
          }
        }
        await strategy.click(); await page.getByRole('menuitem', { name: /^None/ }).click();
        await expect.poll(height).toBe(baseline);
      }
      if (id === 'loop') {
        const limit = inspector(page).getByRole('textbox', { name: 'Maximum iterations' });
        await limit.fill('4'); await limit.blur();
        await expect(card.locator('[data-node-preview]')).toContainText(/Limit\s*4/);
        await card.getByRole('button', { name: 'Expand loop', exact: true }).click();
        const container = await card.evaluate(element => ({ outer: (element as HTMLElement).offsetHeight, inner: (element.firstElementChild as HTMLElement).offsetHeight }));
        expect(container.outer).toBeGreaterThanOrEqual(260); expect(container.inner).toBe(container.outer);
        const nestedStart = canvas(page).getByLabel(/^Block inputs ·/);
        await nestedStart.focus(); await nestedStart.press('Enter');
        await checkCardHeight(page, nestedStart, 'nested-start', info);
        await card.focus(); await card.press('Enter');
        await card.getByRole('button', { name: 'Collapse loop', exact: true }).click();
      }
    }
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(inspector(page)).toHaveCSS('position', 'absolute');
    await captureEvidence(page, 'return-panel-mobile', info);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

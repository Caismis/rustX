import { test, expect, type Page } from '@playwright/test';
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

test('shows each native node in the Dify-style card and grouped settings panel', async ({ page }, info) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    let document: WorkflowDocument = { description: 'Node surfaces', block: emptyBlock() };
    document.block.input = { type: 'object', properties: { topic: { type: 'string' } }, required: ['topic'], additionalProperties: false };
    document.block.nodes.agent = { type: 'agent', profile: 'reviewer', task: 'Research the requested topic', input: {}, output: { type: 'object', properties: { summary: { type: 'string' } }, additionalProperties: false } };
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
      await expect(inspector(page).getByRole('heading', { name: section, exact: true }).first()).toBeVisible();
      await expect(card).toContainText(`Configure ${id} here.`);
      const layout = await card.evaluate(element => {
        const preview = element.querySelector('[data-node-preview]')!, description = element.querySelector('[data-node-description]')!;
        return { card: element.getBoundingClientRect().bottom, preview: preview.getBoundingClientRect().bottom, description: description.getBoundingClientRect().bottom };
      });
      expect(layout.preview).toBeLessThan(layout.card - 8);
      expect(layout.description).toBeLessThanOrEqual(layout.card - 8);
      await captureEvidence(page, `${id}-panel`, info);
      if (id === 'agent') {
        await inspector(page).getByRole('combobox', { name: 'Agent profile' }).fill('planner');
        await expect(card.locator('[data-node-preview]')).toContainText('planner');
      }
      if (id === 'loop') {
        const limit = inspector(page).getByRole('textbox', { name: 'Maximum iterations' });
        await limit.fill('4'); await limit.blur();
        await expect(card.locator('[data-node-preview]')).toContainText(/Limit\s*4/);
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

import { test, expect, type Locator, type Page } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse } from 'yaml';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { rootBlock } from '../workflow-root';
import { setNodePresentation } from '../../src/app/workflows/document/geometry';
import { portsOf } from '../../src/app/workflows/document/ports';
import type { BlockPath, NodeDefinition, WorkflowDocument } from '../../src/app/workflows/document/model';
import { viewId } from '../../src/app/workflows/ui/editor/selection';

const canvas = (page: Page) => page.getByRole('region', { name: 'Workflow canvas' });
const panel = (page: Page) => page.getByRole('complementary', { name: 'Inspector' });
const card = (page: Page, path: BlockPath, kind: 'node' | 'start', id = '') => canvas(page).locator(`.react-flow__node[data-id="${viewId(path, kind, id)}"]`);
async function activate(node: Locator) { await node.focus(); await node.press('Enter'); }
async function title(page: Page, node: Locator, value: string) {
  await activate(node);
  const field = panel(page).getByRole('textbox', { name: 'Node title', exact: true });
  await field.fill(value); await field.press('Enter');
  await expect(node).toContainText(value);
}
async function drag(page: Page, from: Locator, port: string, to: Locator) {
  const source = from.locator(`[data-handleid="${port}"]`).first(), target = to.locator('[data-handleid="in"]').first();
  const a = (await source.boundingBox())!, b = (await target.boundingBox())!;
  await page.mouse.move(a.x + a.width / 2, a.y + a.height / 2);
  await page.mouse.down();
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 16 });
  await expect(target).toHaveClass(/connectingto/);
  await expect(target).toHaveClass(/\bvalid\b/);
  await page.mouse.up();
}
async function open(page: Page, fixture: Awaited<ReturnType<typeof startDogfood>>, document: WorkflowDocument) {
  page.setDefaultTimeout(10_000);
  await page.setViewportSize({ width: 2000, height: 1100 });
  const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
  const file = join(directory, 'connections.yaml'); writeFileSync(file, JSON.stringify(document));
  mkdirSync(join(fixture.workspaceA, '.agents/agents'), { recursive: true });
  writeFileSync(join(fixture.workspaceA, '.agents/agents/reviewer.toml'), 'description = "Reviewer"\ninstructions = "Check the request."\n');
  await routeWorkspaceHost(page, fixture); await page.goto('/');
  await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
  await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
  await page.getByRole('button', { name: 'connections', exact: true }).click();
  await expect(page).toHaveTitle(/rustX$/);
  return file;
}
async function save(page: Page, file: string) {
  await page.getByRole('button', { name: 'Save File', exact: true }).click();
  await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
  await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  return parse(readFileSync(file, 'utf8')) as WorkflowDocument;
}
const definitions: NodeDefinition[] = [
  { type: 'agent', profile: 'reviewer', task: [{ type: 'literal', value: 'Check.' }], output: { type: 'text' }, on_error: { type: 'fail_branch' } },
  { type: 'model', model: { mode: 'session' }, messages: [{ role: 'user', content: [{ type: 'literal', value: 'Check.' }] }], output: { type: 'text' }, on_error: { type: 'fail_branch' } },
  { type: 'tool', selector: { origin: 'builtin', name: 'bash' }, arguments: { type: 'literal', value: { command: 'printf check' } }, result: { type: 'text' }, on_error: { type: 'fail_branch' } },
  { type: 'branch', cases: ['true', 'case_2'].map((id, index) => ({ id, condition: { type: 'boolean', value: { type: 'literal', value: !index } } })) },
  { type: 'loop', loop_count: 1, body: { nodes: { inner_start: { type: 'start' } }, edges: [] } },
  { type: 'iteration', iterator: { type: 'literal', value: [1] }, output: { type: 'literal', value: 'done' }, parallel_nums: 1, body: { nodes: { inner_start: { type: 'start' } }, edges: [] } },
];

for (const definition of definitions) test(`renamed ${definition.type}, Start and Output retain draggable connections`, async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood'), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    let exits = [...portsOf(definition)];
    let document: WorkflowDocument = { description: 'Renamed connection endpoints', block: rootBlock() };
    document.block.nodes.subject = definition;
    document.block.edges = [{ from: 'start', to: 'subject' }, ...exits.map(port => ({ from: 'subject', to: 'done', ...(port === 'next' ? {} : { port }) }))];
    document = setNodePresentation(document, [], 'subject', { position: { x: 340, y: 100 } });
    document = setNodePresentation(document, [], 'done', { position: { x: 920, y: 100 } });
    const file = await open(page, fixture, document), start = card(page, [], 'start'), subject = card(page, [], 'node', 'subject'), done = card(page, [], 'node', 'done');
    await title(page, start, 'Renamed entry');
    await panel(page).getByRole('button', { name: 'Disconnect subject', exact: true }).click();
    await title(page, subject, `Renamed ${definition.type}`);
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(subject).not.toContainText(`Renamed ${definition.type}`);
    await page.getByRole('button', { name: 'Redo', exact: true }).click();
    await expect(subject).toContainText(`Renamed ${definition.type}`);
    for (const _ of exits) await panel(page).getByRole('button', { name: 'Disconnect done', exact: true }).first().click();
    if (definition.type === 'branch') {
      // Removing the first CASE makes a surviving exit move into the IF row.
      await panel(page).getByRole('button', { name: 'Remove CASE 1', exact: true }).click();
      await expect(subject.locator('[data-handleid="true"]')).toHaveCount(0);
      exits = ['case_2', 'false'];
    }
    await title(page, done, 'Renamed output');
    await drag(page, start, 'next', subject);
    for (const port of exits) await drag(page, subject, port, done);
    const saved = await save(page, file);
    expect(saved.block.edges).toEqual([{ from: 'start', to: 'subject' }, ...exits.map(port => ({ from: 'subject', to: 'done', ...(port === 'next' ? {} : { port }) }))]);
    await page.screenshot({ animations: 'disabled', path: `/tmp/rustx-node-connections-${definition.type}.png` });
    expect(errors).toEqual([]);
  } finally { await stopDogfoodPage(page, fixture, false); }
});

test('renamed Agent route retains its identity through drag, Undo/Redo and save', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood'), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    let document: WorkflowDocument = { description: 'Renamed Agent route', block: rootBlock() };
    document.block.nodes.subject = { type: 'agent', profile: 'reviewer', task: [{ type: 'literal', value: 'Choose a route.' }], output: { type: 'text' },
      routes: [{ id: 'approve', description: 'Ready.' }, { id: 'route_2', description: 'Needs work.' }], on_error: { type: 'fail_branch' } };
    document.block.edges = [{ from: 'start', to: 'subject' }, ...['approve', 'route_2', 'fail'].map(port => ({ from: 'subject', to: 'done', port }))];
    document = setNodePresentation(document, [], 'subject', { position: { x: 340, y: 100 } });
    document = setNodePresentation(document, [], 'done', { position: { x: 920, y: 100 } });
    const file = await open(page, fixture, document), subject = card(page, [], 'node', 'subject'), done = card(page, [], 'node', 'done');
    await activate(subject);
    await panel(page).getByRole('button', { name: 'Disconnect done', exact: true }).nth(1).click();
    const height = (await subject.boundingBox())!.height;
    const id = panel(page).getByRole('textbox', { name: 'Route ID', exact: true }).nth(1);
    await id.fill('rework'); await id.press('Enter');
    await expect(subject.locator('[data-handleid="rework"]')).toHaveCount(1);
    expect((await subject.boundingBox())!.height).toBe(height);
    await drag(page, subject, 'rework', done);
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(subject.locator('[data-handleid="route_2"]')).toHaveCount(1);
    await page.getByRole('button', { name: 'Redo', exact: true }).click();
    await drag(page, subject, 'rework', done);
    const saved = await save(page, file);
    expect(saved.block.edges).toEqual([{ from: 'start', to: 'subject' }, { from: 'subject', to: 'done', port: 'approve' }, { from: 'subject', to: 'done', port: 'fail' }, { from: 'subject', to: 'done', port: 'rework' }]);
    expect(errors).toEqual([]);
    await page.screenshot({ animations: 'disabled', path: '/tmp/rustx-node-connections-agent-route.png' });
  } finally { await stopDogfoodPage(page, fixture, false); }
});

test('renamed private Start, Assigner and Exit Loop retain their connection points', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood'), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    const path: BlockPath = [{ node: 'repeat' }];
    let document: WorkflowDocument = { description: 'Private connection endpoints', block: rootBlock() };
    document.block.nodes.repeat = { type: 'loop', loop_count: 1, loop_variables: [{ label: 'count', var_type: 'number', value: { type: 'literal', value: 0 } }], body: {
      nodes: { inner_start: { type: 'start' }, assign: { type: 'assigner', items: [{ variable: ['repeat', 'count'], operation: 'increment', value: { type: 'literal', value: 1 } }] }, finish: { type: 'loop_end' } },
      edges: [{ from: 'inner_start', to: 'assign' }, { from: 'assign', to: 'finish' }],
    } };
    document.block.edges = [{ from: 'start', to: 'repeat' }, { from: 'repeat', to: 'done' }];
    document = setNodePresentation(document, [], 'repeat', { position: { x: 100, y: 100 } });
    document = setNodePresentation(document, [], 'done', { position: { x: 1200, y: 100 } });
    document = setNodePresentation(document, path, 'assign', { position: { x: 140, y: 68 } });
    document = setNodePresentation(document, path, 'finish', { position: { x: 500, y: 68 } });
    const file = await open(page, fixture, document), start = card(page, path, 'start'), assign = card(page, path, 'node', 'assign'), finish = card(page, path, 'node', 'finish');
    // Private Start has no inspector Next step list, so disconnect its edge.
    const edge = canvas(page).getByRole('group', { name: `Edge from ${viewId(path, 'start')} to ${viewId(path, 'node', 'assign')}`, exact: true });
    await edge.focus(); await edge.press('Enter'); await edge.press('Delete');
    // The private Start displays an icon, with its authored title in a tooltip.
    await activate(start);
    const field = panel(page).getByRole('textbox', { name: 'Node title', exact: true });
    await field.fill('Renamed private entry'); await field.press('Enter');
    await expect(start.getByTitle('Renamed private entry', { exact: true })).toBeVisible();
    await title(page, assign, 'Renamed assigner');
    await panel(page).getByRole('button', { name: 'Disconnect finish', exact: true }).click();
    await title(page, finish, 'Renamed exit');
    await drag(page, start, 'next', assign);
    await drag(page, assign, 'next', finish);
    const saved = await save(page, file), repeat = saved.block.nodes.repeat;
    expect(repeat.type).toBe('loop');
    if (repeat.type === 'loop') expect(repeat.body.edges).toEqual([{ from: 'inner_start', to: 'assign' }, { from: 'assign', to: 'finish' }]);
    await page.screenshot({ animations: 'disabled', path: '/tmp/rustx-node-connections-private.png' });
    expect(errors).toEqual([]);
  } finally { await stopDogfoodPage(page, fixture, false); }
});

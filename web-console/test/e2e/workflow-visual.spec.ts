/** Complete native-backed visual authoring. Fixtures exist only in this test. */
import { test, expect, type Locator, type Page, type TestInfo } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse } from 'yaml';
import { startDogfood } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { captureEvidence } from './screenshot';
import { addBranch, addNode, connect, emptyBlock, reconnect, renameBranch, renameNode, setEntry } from '../../src/app/workflows/document/commands';
import { putNote, setLane, setNodePresentation } from '../../src/app/workflows/document/geometry';
import { blockAt, blockKey, type BlockPath, type WorkflowDocument } from '../../src/app/workflows/document/model';
import { viewId } from '../../src/app/workflows/ui/editor/selection';
import renameCases from '../../../tests/fixtures/workflow/schema-rename.json' with { type: 'json' };
import collisionCases from '../../../tests/fixtures/workflow/schema-rename-collisions.json' with { type: 'json' };
import constructionCollision from '../../../tests/fixtures/workflow/schema-construction-collision.json' with { type: 'json' };

const canvas = (page: Page) => page.getByRole('region', { name: 'Workflow canvas' });
const panel = (page: Page) => page.getByRole('complementary', { name: 'Inspector' });
async function activate(page: Page, label: string) { const node = canvas(page).getByLabel(label, { exact: true }); await node.focus(); await node.press('Enter'); }
async function add(page: Page, name: string, after?: { node: string; port: string }) {
  if (after) await canvas(page).getByRole('button', { name: `Add node after ${after.node} (${after.port})`, exact: true }).click();
  else await canvas(page).getByRole('toolbar').getByRole('button', { name: 'Add node', exact: true }).click();
  await page.getByRole('dialog', { name: 'Add node', exact: true }).getByRole('option', { name, exact: true }).click();
}
const schema = (page: Page, label: string) => panel(page).locator(`[data-schema-label="${label}"]`).first();
async function property(page: Page, label: string, name: string, type = 'string') {
  return propertyIn(page, schema(page, label), name, type);
}
async function propertyIn(page: Page, editor: Locator, name: string, type = 'string') {
  await editor.getByPlaceholder('Field name', { exact: true }).last().fill(name);
  await editor.getByRole('button', { name: 'Add field', exact: true }).last().click();
  // Form values are live DOM properties, so locate this property's summary.
  const item = editor.locator('details').filter({ has: page.locator('summary').filter({ hasText: new RegExp(`^${name}`) }) }).first();
  if (!(await item.evaluate(element => (element as HTMLDetailsElement).open))) await item.locator(':scope > summary').click();
  await item.getByLabel('Type', { exact: true }).selectOption(type);
  await item.getByRole('checkbox', { name: 'Required', exact: true }).check();
  return item;
}
async function addBinding(group: Locator, name: string) { await group.getByPlaceholder('Field name', { exact: true }).last().fill(name); await group.getByRole('button', { name: 'Add field', exact: true }).last().click(); }
async function reference(page: Page, group: Locator, path: string) {
  const radio = group.getByRole('radio', { name: 'Reference', exact: true }).first();
  if (await radio.getAttribute('aria-checked') !== 'true') await radio.click();
  await group.getByRole('button', { name: 'Choose a variable', exact: true }).first().click();
  const choice = panel(page).getByRole('dialog', { name: 'Choose a variable' }).getByRole('button', { name: new RegExp(`^${path.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')} ·`) });
  await expect(choice).toBeEnabled(); await choice.click();
}
async function capture(page: Page, info: TestInfo, name: string) {
  // Wait for the selected projection and one committed viewport frame, not a timer.
  const selected = canvas(page).locator('.react-flow__node.selected');
  if (await selected.count() === 1) {
    await expect.poll(async () => {
      const node = await selected.boundingBox(), bounds = await canvas(page).boundingBox();
      return !!node && !!bounds && node.x >= bounds.x && node.y >= bounds.y && node.x < bounds.x + bounds.width - 80;
    }).toBe(true);
  }
  await captureEvidence(page, name, info); }
async function openLibrary(page: Page, fixture: Awaited<ReturnType<typeof startDogfood>>) {
  await routeWorkspaceHost(page, fixture); await page.goto('/');
  await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
  await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
}

async function dragConnector(page: Page, source: Locator, target: Locator, valid: boolean) {
  // Fit view animates. Wait for the actual SVG handle to stop moving before
  // pressing it, then exercise the library's pointer reconnection gesture.
  await expect.poll(async () => {
    const before = await source.boundingBox();
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    const after = await source.boundingBox();
    return !!before && !!after && Math.abs(before.x - after.x) + Math.abs(before.y - after.y) < 0.1;
  }).toBe(true);
  const from = (await source.boundingBox())!, to = (await target.boundingBox())!;
  const point = { x: from.x + from.width / 2, y: from.y + from.height / 2 };
  const hit = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.getAttribute('class'), point);
  expect(hit).toContain('react-flow__edgeupdater');
  await page.mouse.move(point.x, point.y);
  await page.mouse.down();
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 16 });
  await expect(target).toHaveClass(/connectingto/);
  if (valid) await expect(target).toHaveClass(/\bvalid\b/);
  else await expect(target).not.toHaveClass(/\bvalid\b/);
  await page.mouse.up();
}

test.use({ trace: 'on' });
test('keeps executable-edge reconnection separate from the synthetic entry connector', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood'), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    let document: WorkflowDocument = { description: 'Entry and executable edge ownership', block: emptyBlock() };
    document = addNode(document, [], 'branch', 'decision');
    document = setEntry(document, [], 'decision');
    document = connect(document, [], 'decision', 'true', 'done');
    document = connect(document, [], 'decision', 'false', 'done');
    document = setNodePresentation(document, [], 'decision', { position: { x: 360, y: 80 } });
    document = setNodePresentation(document, [], 'done', { position: { x: 1000, y: 400 } });
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'entry_reconnection.yaml'); writeFileSync(file, JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'entry_reconnection', exact: true }).click();
    await expect(canvas(page).getByLabel('Branch node decision', { exact: true })).toBeVisible();
    await canvas(page).getByRole('button', { name: 'Fit view', exact: true }).click();
    const edge = (kind: 'edge' | 'entry', local?: string) => canvas(page).locator(`.react-flow__edge[data-id=${JSON.stringify(viewId([], kind, local))}]`);
    const start = canvas(page).getByLabel('Workflow Start', { exact: true });
    await dragConnector(page, edge('edge', '0').locator('.react-flow__edgeupdater-source'), start.locator('[data-handleid="entry"]'), false);
    await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeDisabled();
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    await activate(page, 'Workflow Start');
    await expect(panel(page).getByLabel('Entry node')).toHaveValue('decision');
    expect(parse(readFileSync(file, 'utf8'))).toEqual(document);
    expect(errors).toEqual([]);
    await panel(page).getByRole('button', { name: 'Close inspector' }).click();
    await canvas(page).getByRole('button', { name: 'Fit view', exact: true }).click();
    await dragConnector(page, edge('entry').locator('.react-flow__edgeupdater-target'), canvas(page).getByLabel('Return node done', { exact: true }).locator('[data-handleid="in"]'), true);
    await activate(page, 'Workflow Start');
    await expect(panel(page).getByLabel('Entry node')).toHaveValue('done');
    await expect(canvas(page).locator('.react-flow__edge')).toHaveCount(3);
    expect(errors).toEqual([]);
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(panel(page).getByLabel('Entry node')).toHaveValue('decision');
    await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeDisabled();
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

test('renames finite schema contracts and explicitly edits optional array items through Settings', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    const document: WorkflowDocument = { description: 'Finite input rename and optional items', block: emptyBlock() };
    document.block.input = renameCases[0].before;
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'finite_schema.yaml'); writeFileSync(file, JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'finite_schema', exact: true }).click();
    await activate(page, 'Workflow Start');
    const input = schema(page, 'Input schema');
    const name = input.getByLabel('Field name', { exact: true }).first();
    await name.fill('after'); await name.blur();
    await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(panel(page)).toContainText('Structure check passed');
    await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    expect(parse(readFileSync(file, 'utf8')).block.input).toEqual(renameCases[0].after);
    await activate(page, 'Workflow Start');
    await input.getByPlaceholder('Field name', { exact: true }).last().fill('list'); await input.getByRole('button', { name: 'Add field', exact: true }).last().click();
    const list = input.locator('details').filter({ has: page.locator('summary').filter({ hasText: /^list/ }) }).first();
    await list.locator(':scope > summary').click(); await list.getByLabel('Type', { exact: true }).selectOption('array');
    await list.getByRole('checkbox', { name: 'Constrain array items' }).uncheck();
    await expect(list).toContainText('Array items may have any type or structure.');
    await expect(list.locator('[data-schema-label="Array item contract"]')).toHaveCount(0);
    await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    expect(parse(readFileSync(file, 'utf8')).block.input.properties.list).toEqual({ type: 'array' });
    await list.getByRole('checkbox', { name: 'Constrain array items' }).check();
    await list.locator('[data-schema-label="Array item contract"]').getByLabel('Type', { exact: true }).selectOption('boolean');
    await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    expect(parse(readFileSync(file, 'utf8')).block.input.properties.list).toEqual({ type: 'array', items: { type: 'boolean' } });
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

test('refuses destination-only finite schema collisions without adopting extra properties or losing rename text', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    for (const [index, example] of collisionCases.entries()) writeFileSync(join(directory, `finite_collision_${index}.yaml`), JSON.stringify({ description: example.name, block: { ...emptyBlock(), input: example.schema } }));
    await openLibrary(page, fixture);
    for (const [index, example] of collisionCases.entries()) {
      const file = join(directory, `finite_collision_${index}.yaml`), before = parse(readFileSync(file, 'utf8'));
      await page.getByRole('button', { name: `finite_collision_${index}`, exact: true }).click();
      await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(panel(page)).toContainText('Structure check passed');
      await activate(page, 'Workflow Start');
      const owner = schema(page, 'Input schema').locator('details').filter({ has: page.locator('summary').filter({ hasText: /^before/ }) }).last();
      const name = owner.getByLabel('Field name', { exact: true }).first();
      await name.fill('after'); await name.blur();
      await expect(name).toHaveValue('after'); await expect(name).toHaveAttribute('aria-invalid', 'true');
      await expect(panel(page).getByRole('alert')).toContainText('That ID is already used');
      await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeDisabled();
      await page.getByRole('button', { name: 'Save File', exact: true }).click();
      await expect(page.getByRole('alert').filter({ hasText: 'Nothing was sent.' })).toBeVisible();
      expect(parse(readFileSync(file, 'utf8'))).toEqual(before); expect(before.block.input).toEqual(example.schema);
      await name.press('Escape'); await expect(name).toHaveValue('before');
      await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
      await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();
    }
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

test('rejects construction collisions and opaque mappings before a successful Check can conceal a different downstream value', async ({ page }, info) => {
  const fixture = await startDogfood('web_console_dogfood'), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const cases = ['object', 'literal', 'reference'].map(kind => {
      const document = structuredClone(constructionCollision) as WorkflowDocument, fork = document.block.nodes.fork;
      if (fork.type !== 'parallel') throw new Error('Parallel fixture');
      const branch = fork.branches.main;
      if (kind === 'literal') branch.block.nodes.done = { type: 'return', output: { type: 'literal', value: { before: 'wanted', after: 'unrelated' } } };
      if (kind === 'reference') {
        branch.block.input = { type: 'object', properties: { before: { type: 'string' }, after: { type: 'string' } }, required: ['before', 'after'], additionalProperties: false };
        branch.input = { type: 'object', fields: { before: { type: 'literal', value: 'wanted' }, after: { type: 'literal', value: 'unrelated' } } };
        branch.block.nodes.done = { type: 'return', output: { type: 'reference', path: ['args'] } };
      }
      const id = `construction_${kind}`, file = join(directory, `${id}.yaml`); writeFileSync(file, JSON.stringify(document));
      return { kind, id, file, document };
    });
    await openLibrary(page, fixture);
    for (const example of cases) {
      await page.getByRole('button', { name: example.id, exact: true }).click();
      await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(panel(page)).toContainText('Structure check passed');
      await activate(page, 'Parallel node fork');
      const name = schema(page, 'Output schema').getByLabel('Field name', { exact: true }).first();
      await name.fill('after'); await name.blur();
      await expect(name).toHaveValue('after'); await expect(name).toHaveAttribute('aria-invalid', 'true');
      await expect(panel(page).getByRole('alert')).toContainText(example.kind === 'reference' ? 'explicit Object fields' : 'That ID is already used');
      await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeDisabled();
      await page.getByRole('button', { name: 'Save File', exact: true }).click();
      await expect(page.getByRole('alert').filter({ hasText: 'Nothing was sent.' })).toBeVisible();
      expect(parse(readFileSync(example.file, 'utf8'))).toEqual(example.document);
      await page.screenshot({ path: info.outputPath(`construction-${example.kind}.png`) });
      await name.press('Escape'); await expect(name).toHaveValue('before');
      await activate(page, 'Return node done');
      await expect(panel(page).getByRole('button', { name: 'Choose a variable', exact: true })).toHaveText('fork / main / before');
      await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(panel(page)).toContainText('Structure check passed');
      await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
      await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();
    }
    expect(errors).toEqual([]); expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

test('copies pastes and duplicates a legal args branch and its private nodes through real clipboard and all duplicate controls', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood'), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  try {
    let document = renameBranch(structuredClone(constructionCollision) as WorkflowDocument, [], 'fork', 'main', 'args');
    const privatePath = [{ node: 'fork', branch: 'args' }];
    document = setLane(document, { address: { block: [], node: 'fork' }, branch: 'args', position: { x: 24, y: 72 }, dimensions: { width: 640, height: 256 } });
    document = setNodePresentation(document, privatePath, 'done', { title: 'Args return', position: { x: 402, y: 120 } });
    // Orphan presentation comes from the native file, rather than the UI's
    // live-node title command, which correctly requires an executable owner.
    document = { ...document, editor: { ...document.editor, nodes: [...document.editor?.nodes ?? [], { address: { block: privatePath, node: 'args' }, title: 'Retained orphan' }] } };
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'args_clipboard.yaml'); writeFileSync(file, JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'args_clipboard', exact: true }).click();
    await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(panel(page)).toContainText('Structure check passed');
    for (const child of [false, true]) {
      const path = child ? privatePath : [], id = child ? 'done' : 'fork';
      const label = child ? 'Return node done · fork/args' : 'Parallel node fork';
      const copy = canvas(page).locator(`.react-flow__node[data-id=${JSON.stringify(viewId(path, 'node', `${id}_copy`))}]`);
      const undo = async () => { await page.getByRole('button', { name: 'Undo', exact: true }).click(); await expect(copy).toHaveCount(0); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible(); };
      await activate(page, label); await canvas(page).focus(); await page.keyboard.press('Control+c'); await page.keyboard.press('Control+v');
      await expect(copy).toHaveCount(1); await undo();
      await activate(page, label); await panel(page).getByRole('button', { name: 'Duplicate', exact: true }).click();
      await expect(copy).toHaveClass(/selected/); await undo();
      await activate(page, label); await canvas(page).focus(); await page.keyboard.press('Control+d');
      await expect(copy).toHaveClass(/selected/); await undo();
      await activate(page, label); await canvas(page).getByRole('button', { name: 'Fit view', exact: true }).click();
      await canvas(page).getByLabel(label, { exact: true }).getByText(child ? 'Args return' : 'fork', { exact: true }).click({ button: 'right' });
      await canvas(page).getByRole('dialog').getByRole('button', { name: 'Duplicate', exact: true }).click();
      await expect(copy).toHaveClass(/selected/); await undo();
    }
    expect(parse(readFileSync(file, 'utf8'))).toEqual(document);
    expect(errors).toEqual([]); expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

test('shows real result producer titles in ordinary consumers and the committed result only in Loop post-body fields', async ({ page }, info) => {
  const fixture = await startDogfood('web_console_dogfood'), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    let document = addNode(renameNode(structuredClone(constructionCollision) as WorkflowDocument, [], 'fork', 'result'), [], 'loop', 'repeat');
    document = reconnect(document, [], 0, 'result', 'next', 'repeat');
    document = connect(connect(document, [], 'repeat', 'satisfied', 'done'), [], 'repeat', 'exhausted', 'done');
    const result = document.block.nodes.result, repeat = document.block.nodes.repeat;
    if (result.type !== 'parallel' || repeat.type !== 'loop') throw new Error('Parallel and Loop fixture');
    repeat.body.input = structuredClone(result.branches.main.block.output); repeat.body.output = structuredClone(repeat.body.input);
    repeat.body.nodes.done = { type: 'return', output: { type: 'reference', path: ['args'] } };
    repeat.input = { type: 'reference', path: ['result', 'main'] };
    repeat.until = { type: 'equal', left: { type: 'reference', path: ['result', 'before'] }, right: { type: 'literal', value: 'wanted' } };
    repeat.carry = { type: 'reference', path: ['result'] };
    document = setNodePresentation(document, [], 'result', { title: 'Actual result producer' });
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    writeFileSync(join(directory, 'result_title.yaml'), JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'result_title', exact: true }).click();
    await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(panel(page)).toContainText('Structure check passed');
    await activate(page, 'Return node done');
    const selected = panel(page).getByRole('button', { name: 'Choose a variable', exact: true });
    await expect(selected).toHaveText('Actual result producer / main / before'); await selected.click();
    await expect(panel(page).getByRole('dialog', { name: 'Choose a variable' }).locator('legend').filter({ hasText: 'Actual result producer' })).toBeVisible();
    await page.screenshot({ path: info.outputPath('actual-result-producer.png') });
    await panel(page).getByRole('button', { name: 'Close variable picker', exact: true }).click();
    await activate(page, 'Loop node repeat');
    await expect(panel(page).getByRole('group', { name: 'Initial input', exact: true }).getByRole('button', { name: 'Choose a variable', exact: true })).toHaveText('Actual result producer / main');
    await expect(panel(page).getByRole('group', { name: 'Until', exact: true }).getByRole('button', { name: 'Choose a variable', exact: true })).toHaveText('Committed body result / before');
    await expect(panel(page).getByRole('group', { name: 'Carry', exact: true }).getByRole('button', { name: 'Choose a variable', exact: true })).toHaveText('Committed body result');
    await page.screenshot({ path: info.outputPath('loop-result-scope.png') });
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    expect(errors).toEqual([]); expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

test('duplicates an unpositioned sibling beside its currently visible collapsed container scene', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    let document: WorkflowDocument = addNode(addNode({ description: 'Visible collapsed duplicate geometry', block: emptyBlock() }, [], 'parallel', 'fork'), [], 'parallel', 'other');
    document = setEntry(document, [], 'fork'); document = connect(document, [], 'fork', 'next', 'other'); document = connect(document, [], 'other', 'next', 'done');
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true }); writeFileSync(join(directory, 'collapsed_duplicate.yaml'), JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'collapsed_duplicate', exact: true }).click();
    await activate(page, 'Parallel node other');
    await canvas(page).getByLabel('Parallel node other', { exact: true }).getByRole('button', { name: 'Collapse other', exact: true }).click();
    await activate(page, 'Parallel node fork');
    await canvas(page).getByLabel('Parallel node fork', { exact: true }).getByRole('button', { name: 'Collapse fork', exact: true }).click();
    await expect(canvas(page).locator(`.react-flow__node[data-id=${JSON.stringify(viewId([{ node: 'fork', branch: 'main' }], 'start'))}]`)).toHaveCount(0);
    await activate(page, 'Return node done');
    const position = (label: string) => canvas(page).getByLabel(label, { exact: true }).evaluate(element => {
      const values = (element as HTMLElement).style.transform.match(/translate\(([-\d.]+)px,\s*([-\d.]+)px\)/)!;
      return { x: Number(values[1]), y: Number(values[2]) };
    });
    const before = await position('Return node done');
    await panel(page).getByRole('button', { name: 'Duplicate', exact: true }).click();
    const copied = canvas(page).getByLabel('Return node done_copy', { exact: true });
    await expect(copied).toHaveClass(/selected/); await expect(copied).toBeInViewport();
    expect(await position('Return node done_copy')).toEqual({ x: before.x + 32, y: before.y + 32 });
    await page.getByRole('button', { name: 'Undo', exact: true }).click(); await expect(copied).toHaveCount(0);
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    // A selected private Inspector can remain open while its ancestor closes.
    // Duplicating it must reveal its owner without expanding unrelated frames.
    await activate(page, 'Parallel node fork');
    await canvas(page).getByLabel('Parallel node fork', { exact: true }).getByRole('button', { name: 'Expand fork', exact: true }).click();
    const path = [{ node: 'fork', branch: 'main' }], original = `Return node done · ${blockKey(path)}`;
    await activate(page, original); const childPosition = await position(original);
    // Focus can locate the ancestor without replacing the private Inspector.
    await canvas(page).getByLabel('Parallel node fork', { exact: true }).focus();
    await canvas(page).getByLabel('Parallel node fork', { exact: true }).getByRole('button', { name: 'Collapse fork', exact: true }).click();
    await expect(canvas(page).getByLabel(original, { exact: true })).toHaveCount(0);
    await panel(page).getByRole('button', { name: 'Duplicate', exact: true }).click();
    const copy = `Return node done_copy · ${blockKey(path)}`;
    await expect(canvas(page).locator(`.react-flow__node[data-id=${JSON.stringify(viewId([{ node: 'other', branch: 'main' }], 'start'))}]`)).toHaveCount(0);
    await expect(canvas(page).getByLabel(copy, { exact: true })).toHaveClass(/selected/);
    expect(await position(copy)).toEqual({ x: childPosition.x + 32, y: childPosition.y + 32 });
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(canvas(page).getByLabel(copy, { exact: true })).toHaveCount(0);
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

test('discovers every matching display title together with original field paths', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    let document: WorkflowDocument = { description: 'All matching titles and paths', block: emptyBlock() };
    document.block.input = { type: 'object', properties: { Draft: { type: 'string' } }, required: ['Draft'], additionalProperties: false };
    document.block.nodes.done = { type: 'return', output: { type: 'reference', path: ['args'] } };
    document = addNode(addNode(document, [], 'model', 'first'), [], 'model', 'second');
    document = connect(connect(document, [], 'first', 'next', 'second'), [], 'second', 'next', 'done');
    document = setEntry(document, [], 'first');
    document = setNodePresentation(setNodePresentation(document, [], 'first', { title: 'First Draft' }), [], 'second', { title: 'Second Draft' });
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true }); writeFileSync(join(directory, 'title_search.yaml'), JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'title_search', exact: true }).click();
    await activate(page, 'Return node done'); await panel(page).getByRole('button', { name: 'Choose a variable', exact: true }).click();
    const picker = panel(page).getByRole('dialog', { name: 'Choose a variable' });
    await picker.getByRole('textbox', { name: 'Search reference paths…' }).fill('Draft');
    for (const path of ['args / Draft', 'first', 'second']) await expect(picker.getByRole('button', { name: new RegExp(`^${path} ·`) })).toBeVisible();
    await expect(picker.locator('legend').filter({ hasText: 'First Draft' })).toBeVisible();
    await expect(picker.locator('legend').filter({ hasText: 'Second Draft' })).toBeVisible();
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

test('uses the same visible duplicate transaction from Inspector and canvas for nodes and private containers', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    let document: WorkflowDocument = addNode({ description: 'Unified visible duplicate', block: emptyBlock() }, [], 'loop', 'repeat');
    document = setNodePresentation(document, [], 'done', { title: 'Finish', position: { x: 500, y: 240 } });
    document = setNodePresentation(document, [], 'repeat', { position: { x: 900, y: 240 } });
    const child = [{ node: 'repeat' }];
    document = setNodePresentation(document, child, 'done', { position: { x: 360, y: 90 } });
    document = putNote(document, { id: 'private', block: child, text: 'Keep relative', theme: 'blue', position: { x: 32, y: 320 }, dimensions: { width: 220, height: 100 } });
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true }); writeFileSync(join(directory, 'visible_duplicate.yaml'), JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'visible_duplicate', exact: true }).click();
    const node = (path: BlockPath, kind: 'node' | 'note', id: string) => canvas(page).locator(`.react-flow__node[data-id=${JSON.stringify(viewId(path, kind, id))}]`);
    const position = (item: Locator) => item.evaluate(element => {
      const values = (element as HTMLElement).style.transform.match(/translate\(([-\d.]+)px,\s*([-\d.]+)px\)/)!;
      return { x: Number(values[1]), y: Number(values[2]) };
    });
    await activate(page, 'Return node done'); await panel(page).getByRole('button', { name: 'Duplicate', exact: true }).click();
    await expect(node([], 'node', 'done_copy')).toHaveClass(/selected/); await expect(node([], 'node', 'done_copy')).toBeInViewport();
    expect(await position(node([], 'node', 'done_copy'))).toEqual({ x: 532, y: 272 });
    await page.getByRole('button', { name: 'Undo', exact: true }).click(); await expect(node([], 'node', 'done_copy')).toHaveCount(0);
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Redo', exact: true }).click(); await expect(node([], 'node', 'done_copy')).toBeVisible();
    await activate(page, 'Return node done'); await node([], 'node', 'done').click({ button: 'right' });
    await canvas(page).getByRole('dialog').getByRole('button', { name: 'Duplicate', exact: true }).click();
    await expect(node([], 'node', 'done_copy_2')).toHaveClass(/selected/);
    expect(await position(node([], 'node', 'done_copy_2'))).toEqual({ x: 564, y: 304 });
    await page.getByRole('button', { name: 'Undo', exact: true }).click(); await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await activate(page, 'Loop node repeat'); await panel(page).getByRole('button', { name: 'Duplicate', exact: true }).click();
    const copied = [{ node: 'repeat_copy' }];
    await expect(node([], 'node', 'repeat_copy')).toHaveClass(/selected/); await expect(node([], 'node', 'repeat_copy')).toBeInViewport();
    expect(await position(node([], 'node', 'repeat_copy'))).toEqual({ x: 932, y: 272 });
    const origin = await position(node([], 'node', 'repeat_copy'));
    expect(await position(node(copied, 'node', 'done'))).toEqual({ x: origin.x + 360, y: origin.y + 90 });
    expect(await position(node(copied, 'note', 'private_2'))).toEqual({ x: origin.x + 32, y: origin.y + 320 });
    await page.getByRole('button', { name: 'Undo', exact: true }).click(); await expect(node([], 'node', 'repeat_copy')).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeDisabled(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    await activate(page, 'Return node done');
    let copiedId = 'done';
    for (let index = 0; index < 8; index++) {
      await page.keyboard.press('Control+d'); copiedId += '_copy';
      const selected = node([], 'node', copiedId);
      await expect(selected).toHaveClass(/selected/); await expect(canvas(page)).toBeFocused();
      await expect.poll(async () => {
        const card = await selected.boundingBox(), bounds = await canvas(page).boundingBox();
        return !!card && !!bounds && card.x >= bounds.x && card.y >= bounds.y && card.x + card.width <= bounds.x + bounds.width && card.y + card.height <= bounds.y + bounds.height;
      }).toBe(true);
      // Ordinary hit testing must reach the newly selected card after panning.
      await selected.click(); await expect(selected).toBeFocused();
    }
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

test.describe('complete visual authoring', () => {
  test('creates, binds, nests, saves, reopens and copies one native file without editing YAML or reference paths', async ({ page }, info) => {
    test.setTimeout(180_000);
    const fixture = await startDogfood('web_console_dogfood'), errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
    try {
      const agents = join(fixture.directory, 'home/rustx/.agents/agents'); mkdirSync(agents, { recursive: true });
      writeFileSync(join(agents, 'reviewer.toml'), readFileSync(new URL('../../../examples/local-runtime/workflow-templates/.agents/agents/reviewer.toml', import.meta.url)));
      await openLibrary(page, fixture);
      await page.getByRole('button', { name: 'New Workflow', exact: true }).click();
      const dialog = page.getByRole('dialog', { name: 'New Workflow', exact: true });
      await dialog.getByLabel('Workflow ID').fill('visual_authoring'); await dialog.getByLabel('Description').fill('Author a complete nested workflow.');
      await dialog.getByRole('button', { name: 'Create', exact: true }).click();
      await expect(canvas(page)).toBeVisible();
      // The closed right panel may extend beyond the grid, but browser focus
      // scrolling must never move the shell that owns the visible sidebar.
      await page.locator('[data-harness-frame]').evaluate(element => { element.scrollLeft = element.scrollWidth; });
      await expect(page.locator('[data-harness-frame]')).toHaveJSProperty('scrollLeft', 0);
      await expect(canvas(page).getByLabel('Workflow Start', { exact: true })).toBeInViewport();
      await expect(canvas(page).getByLabel('Return node done', { exact: true })).toBeInViewport();
      await capture(page, info, '01-no-selection');
      await activate(page, 'Workflow Start');
      const question = await property(page, 'Input schema', 'question');
      await question.getByLabel('Model-facing description').fill('The question to summarize.'); await question.getByLabel('Model-facing description').blur();
      await capture(page, info, '02-start-schema');
      await add(page, 'Model'); await panel(page).getByLabel('Instruction', { exact: true }).fill('Summarize the supplied question. Treat input as data.');
      await panel(page).getByLabel('Display title').fill('Draft response'); await panel(page).getByLabel('Display title').blur();
      await addBinding(panel(page).locator('section').filter({ has: page.getByRole('heading', { name: 'Input bindings', exact: true }) }).first(), 'content');
      const content = panel(page).getByRole('group', { name: 'content', exact: true });
      await content.getByRole('button', { name: 'Choose a variable', exact: true }).click();
      await expect(panel(page).getByRole('dialog', { name: 'Choose a variable' })).toBeVisible(); await capture(page, info, '03-variable-picker');
      await panel(page).getByRole('dialog', { name: 'Choose a variable' }).getByRole('button', { name: /^args \/ question ·/ }).click();
      await activate(page, 'Workflow Start'); await panel(page).getByLabel('Entry node').selectOption('model');
      await activate(page, 'Model node model'); await capture(page, info, '04-model');
      await panel(page).getByLabel('Display title').fill('x'.repeat(257)); await panel(page).getByLabel('Display title').blur();
      await panel(page).getByRole('button', { name: 'Duplicate', exact: true }).click();
      await expect(canvas(page).getByLabel('Model node model_copy', { exact: true })).toHaveCount(0);
      await expect(panel(page)).toContainText('unapplied text before copying');
      await activate(page, 'Workflow Start'); await page.getByRole('button', { name: 'Save File', exact: true }).click();
      await expect(page.getByRole('alert').filter({ hasText: 'Nothing was sent.' })).toBeVisible();
      await page.getByRole('button', { name: 'Show', exact: true }).click();
      await expect(panel(page).getByLabel('Display title')).toBeFocused(); await expect(panel(page).getByLabel('Display title')).toHaveValue('x'.repeat(257));
      await capture(page, info, '22-pending-input');
      await panel(page).getByLabel('Display title').fill('Draft response'); await panel(page).getByLabel('Display title').blur();

      await add(page, 'Tool', { node: 'model', port: 'next' });
      await panel(page).getByLabel('Tool name').fill('glob');
      const argumentsGroup = panel(page).getByRole('group', { name: 'Arguments', exact: true });
      await argumentsGroup.getByRole('radio', { name: 'Object', exact: true }).click();
      await addBinding(argumentsGroup, 'path'); await reference(page, argumentsGroup.getByRole('group', { name: 'path', exact: true }), 'args / question');
      await addBinding(argumentsGroup, 'pattern');
      const pattern = argumentsGroup.getByRole('group', { name: 'pattern', exact: true });
      await pattern.getByRole('radio', { name: 'Literal', exact: true }).click(); await pattern.getByLabel('Literal JSON').fill('"*.md"'); await pattern.getByLabel('Literal JSON').blur();
      await capture(page, info, '05-tool');
      await add(page, 'Agent', { node: 'tool', port: 'next' });
      await panel(page).getByLabel('Agent profile').fill('reviewer'); await panel(page).getByLabel('Task', { exact: true }).fill('Review the summary and return an empty object.');
      await addBinding(panel(page).locator('section').filter({ has: page.getByRole('heading', { name: 'Input bindings', exact: true }) }).first(), 'summary');
      await reference(page, panel(page).getByRole('group', { name: 'summary', exact: true }), 'model'); await capture(page, info, '06-agent');
      await add(page, 'Review', { node: 'agent', port: 'next' }); await capture(page, info, '07-review');
      await add(page, 'Parallel', { node: 'review', port: 'next' });
      await panel(page).getByLabel('New branch key').fill('other'); await panel(page).getByRole('button', { name: 'Add branch', exact: true }).click();
      await capture(page, info, '08-parallel-expanded');
      await activate(page, 'Block inputs · parallel/main'); await property(page, 'Input schema', 'message');
      await activate(page, 'Return node done · parallel/main');
      await property(page, 'Output schema', 'echo');
      const privateReturn = panel(page).getByRole('group', { name: 'Output value', exact: true });
      await privateReturn.getByRole('radio', { name: 'Object', exact: true }).first().click(); await addBinding(privateReturn, 'echo');
      await reference(page, privateReturn.getByRole('group', { name: 'echo', exact: true }), 'args / message');
      await activate(page, 'Parallel node parallel');
      const branchInput = panel(page).getByRole('group', { name: 'Branch input mapping', exact: true }).first();
      await branchInput.getByRole('radio', { name: 'Object', exact: true }).click(); await addBinding(branchInput, 'message');
      await reference(page, branchInput.getByRole('group', { name: 'message', exact: true }), 'args / question');
      await panel(page).getByRole('button', { name: 'Collapse parallel', exact: true }).click(); await capture(page, info, '09-parallel-collapsed');
      await panel(page).getByRole('button', { name: 'Expand parallel', exact: true }).click();

      await add(page, 'Loop', { node: 'parallel', port: 'next' }); await capture(page, info, '10-loop-expanded');
      await activate(page, 'Return node done · loop/body'); await property(page, 'Output schema', 'accepted', 'boolean');
      const bodyReturn = panel(page).getByRole('group', { name: 'Output value', exact: true });
      await bodyReturn.getByRole('radio', { name: 'Object', exact: true }).first().click(); await addBinding(bodyReturn, 'accepted');
      const accepted = bodyReturn.getByRole('group', { name: 'accepted', exact: true });
      await accepted.getByRole('radio', { name: 'Literal', exact: true }).click(); await accepted.getByLabel('Literal JSON').fill('true'); await accepted.getByLabel('Literal JSON').blur();
      await activate(page, 'Loop node loop');
      await reference(page, panel(page).getByRole('group', { name: 'Boolean value', exact: true }), 'result / accepted');
      await panel(page).getByRole('group', { name: 'Carry', exact: true }).getByRole('radio', { name: 'Object', exact: true }).click();
      await panel(page).getByLabel('Successor on exhausted').selectOption('done');
      await panel(page).getByRole('button', { name: 'Collapse loop', exact: true }).click(); await capture(page, info, '11-loop-collapsed');
      await add(page, 'Branch', { node: 'loop', port: 'satisfied' });
      await panel(page).getByLabel('Successor on true').selectOption('done'); await panel(page).getByLabel('Successor on false').selectOption('done'); await capture(page, info, '12-branch');
      await activate(page, 'Return node done');
      await property(page, 'Output schema', 'summary'); await property(page, 'Output schema', 'echo'); await property(page, 'Output schema', 'iterations', 'integer');
      const returned = panel(page).getByRole('group', { name: 'Output value', exact: true });
      await returned.getByRole('radio', { name: 'Object', exact: true }).first().click();
      for (const [name, path] of [['summary', 'model'], ['echo', 'parallel / main / echo'], ['iterations', 'loop / iterations']]) { await addBinding(returned, name); await reference(page, returned.getByRole('group', { name, exact: true }), path); }
      await capture(page, info, '13-return');

      await activate(page, 'Loop node loop'); await canvas(page).focus(); await page.keyboard.press('Control+d');
      await expect(canvas(page).getByLabel('Loop node loop_copy', { exact: true })).toHaveCount(1);
      await page.keyboard.press('Control+z'); await expect(canvas(page).getByLabel('Loop node loop_copy', { exact: true })).toHaveCount(0);
      await page.keyboard.press('Control+Shift+z'); await expect(canvas(page).getByLabel('Loop node loop_copy', { exact: true })).toHaveCount(1);
      await page.keyboard.press('Control+z');
      await activate(page, 'Loop node loop');
      await page.keyboard.press('Control+c'); await page.keyboard.press('Control+v');
      await expect(canvas(page).getByLabel('Loop node loop_copy', { exact: true })).toHaveCount(1);
      await page.keyboard.press('Control+z'); await expect(canvas(page).getByLabel('Loop node loop_copy', { exact: true })).toHaveCount(0);
      await canvas(page).getByRole('toolbar').getByRole('button', { name: 'Add note', exact: true }).click();
      await panel(page).getByLabel('Markdown note').fill('## Author guidance\n\nA nested Return completes only its Block.\n\n[Unsafe](javascript:alert(1)) <script>alert(1)</script>');
      await panel(page).getByLabel('Markdown note').blur();
      await panel(page).getByLabel('Width', { exact: true }).fill('340'); await panel(page).getByLabel('Width', { exact: true }).blur();
      await expect(canvas(page).locator('script')).toHaveCount(0); await expect(canvas(page).locator('a[href^="javascript:"]')).toHaveCount(0); await capture(page, info, '14-note');
      await page.getByRole('button', { name: 'Check', exact: true }).first().click();
      await expect(panel(page)).toContainText('Structure check passed'); await capture(page, info, '15-checked');
      await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
      const file = join(fixture.workspaceA, '.agents/workflows/visual_authoring.yaml'), saved = parse(readFileSync(file, 'utf8'));
      expect(saved.editor.notes[0].dimensions.width).toBe(340); expect(saved.block.nodes.loop.until).toMatchObject({ value: { path: ['result', 'accepted'] } });
      expect(saved.block.nodes.done.output.fields.summary.path).toEqual(['model']);
      expect((await fixture.control('requests')).requests).toHaveLength(0); expect(errors).toEqual([]);

      const fresh = await page.context().browser()!.newContext({ viewport: { width: 1440, height: 1000 }, locale: 'en-US' });
      const reopened = await fresh.newPage();
      await reopened.addInitScript(() => localStorage.setItem('rustx-locale-v1', 'en'));
      await openLibrary(reopened, fixture);
      await reopened.getByRole('button', { name: 'visual_authoring', exact: true }).click();
      await activate(reopened, 'Note · note'); await expect(panel(reopened).getByLabel('Markdown note')).toHaveValue(saved.editor.notes[0].text); await capture(reopened, info, '16-fresh-profile');
      await reopened.setViewportSize({ width: 880, height: 900 }); await activate(reopened, 'Model node model');
      await expect(panel(reopened)).toHaveCSS('position', 'absolute'); await expect(canvas(reopened).getByRole('img', { name: 'Minimap', exact: true })).toBeHidden();
      await capture(reopened, info, '17-narrow');
      const sheet = await panel(reopened).boundingBox(), drawing = await canvas(reopened).boundingBox();
      expect(sheet!.width).toBeGreaterThan(drawing!.width * 0.9); expect(sheet!.y).toBeGreaterThan(drawing!.y + 100);
      await expect(panel(reopened).getByLabel('Instruction', { exact: true })).toBeVisible();
      await reopened.getByRole('button', { name: 'Back to Workflows', exact: true }).click();
      const row = reopened.getByRole('listitem').filter({ has: reopened.getByRole('button', { name: 'visual_authoring', exact: true }) });
      await row.getByRole('button', { name: 'Actions for visual_authoring' }).click(); await reopened.getByRole('menuitem', { name: 'Copy File' }).click();
      const copy = reopened.getByRole('dialog'); await copy.getByLabel('Workflow ID').fill('visual_copy'); await copy.getByRole('button', { name: 'Copy', exact: true }).click();
      await expect(reopened.getByRole('button', { name: 'visual_copy', exact: true })).toBeVisible();
      const copied = parse(readFileSync(join(fixture.workspaceA, '.agents/workflows/visual_copy.yaml'), 'utf8')); expect(copied.editor).toEqual(saved.editor);
      await fresh.close();
      expect((await fixture.control('requests')).requests).toHaveLength(0);
    } finally { await fixture.stop(false); }
  });
});

test('authors structured required output, nested contracts and repeated downstream bindings using visual controls', async ({ page }, info) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    await openLibrary(page, fixture);
    await page.getByRole('button', { name: 'New Workflow', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'New Workflow', exact: true });
    await dialog.getByLabel('Workflow ID').fill('structured_authoring'); await dialog.getByLabel('Description').fill('Declare and reuse structured model output.');
    await dialog.getByRole('button', { name: 'Create', exact: true }).click();
    await add(page, 'Model'); await panel(page).getByLabel('Instruction', { exact: true }).fill('Classify input and return the declared object.');
    await panel(page).getByLabel('Output mode').selectOption('json_schema');
    const risk = await property(page, 'Output JSON Schema', 'risk');
    await risk.getByLabel('Model-facing description').fill('Risk classification.'); await risk.getByLabel('Model-facing description').blur();
    await risk.getByText('Enum and const constraints', { exact: true }).click();
    await risk.getByRole('checkbox', { name: 'enum', exact: true }).check();
    await risk.getByRole('textbox', { name: 'enum', exact: true }).fill('["low", "high"]'); await risk.getByRole('textbox', { name: 'enum', exact: true }).blur();
    await property(page, 'Output JSON Schema', 'tags', 'array');
    const details = await property(page, 'Output JSON Schema', 'details', 'object');
    await propertyIn(page, details.locator('[data-schema-label="details"]'), 'accepted', 'boolean');
    await capture(page, info, '18-structured-schema');
    await activate(page, 'Workflow Start'); await panel(page).getByLabel('Entry node').selectOption('model');
    await activate(page, 'Model node model'); await add(page, 'Model', { node: 'model', port: 'next' });
    await panel(page).getByLabel('Instruction', { exact: true }).fill('Compare two bindings of the same producer.');
    const bindings = panel(page).locator('section').filter({ has: page.getByRole('heading', { name: 'Input bindings', exact: true }) }).first();
    for (const name of ['first', 'second']) { await addBinding(bindings, name); await reference(page, bindings.getByRole('group', { name, exact: true }), 'model / risk'); }
    await panel(page).getByLabel('Successor on next').selectOption('done');
    await capture(page, info, '19-structured-reuse');
    await activate(page, 'Return node done');
    await property(page, 'Output schema', 'risk'); await property(page, 'Output schema', 'tags', 'array'); await property(page, 'Output schema', 'accepted', 'boolean');
    const output = panel(page).getByRole('group', { name: 'Output value', exact: true });
    await output.getByRole('radio', { name: 'Object', exact: true }).first().click();
    for (const [name, path] of [['risk', 'model / risk'], ['tags', 'model / tags'], ['accepted', 'model / details / accepted']]) { await addBinding(output, name); await reference(page, output.getByRole('group', { name, exact: true }), path); }
    await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(panel(page)).toContainText('Structure check passed');
    await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    const file = join(fixture.workspaceA, '.agents/workflows/structured_authoring.yaml'), saved = parse(readFileSync(file, 'utf8'));
    expect(saved.block.nodes.model.output.schema).toMatchObject({ type: 'object', required: ['risk', 'tags', 'details'], properties: { risk: { type: 'string', enum: ['low', 'high'], description: 'Risk classification.' }, tags: { type: 'array', items: { type: 'string' } }, details: { required: ['accepted'], properties: { accepted: { type: 'boolean' } } } } });
    expect(saved.block.nodes.model_2.input).toEqual({ first: { type: 'reference', path: ['model', 'risk'] }, second: { type: 'reference', path: ['model', 'risk'] } });
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click(); await page.getByRole('button', { name: 'structured_authoring', exact: true }).click();
    await activate(page, 'Model node model');
    await expect(schema(page, 'Output JSON Schema').getByRole('button', { name: 'Advanced JSON', exact: true }).first()).toBeVisible();
    await expect(schema(page, 'Output JSON Schema')).toContainText('risk');
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

import { scaleWorkflow } from '../fixtures/workflow-scale';
test('measures the native 256-node nested fixture and one metadata-only organize transaction', async ({ page }, info) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'bounded_scene.yaml'), original = scaleWorkflow(); writeFileSync(file, JSON.stringify(original));
    await openLibrary(page, fixture);
    const start = await page.evaluate(() => performance.now());
    await page.getByRole('button', { name: 'bounded_scene', exact: true }).click();
    await expect(canvas(page).locator('.react-flow__node[aria-label*=" node "]')).toHaveCount(256);
    const renderMs = await page.evaluate(start => performance.now() - start, start);
    expect(renderMs).toBeLessThan(10_000);
    await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(panel(page)).toContainText('Structure check passed');
    await panel(page).getByRole('button', { name: 'Close inspector' }).click();
    const organize = canvas(page).getByRole('button', { name: 'Organize layout', exact: true });
    const before = await page.evaluate(() => performance.now()); await organize.click();
    await expect(organize).toBeEnabled(); await expect(page.getByText('Unsaved changes', { exact: true }).first()).toBeVisible();
    const layoutMs = await page.evaluate(before => performance.now() - before, before); expect(layoutMs).toBeLessThan(10_000);
    await canvas(page).focus(); await page.keyboard.press('Control+z'); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    await page.keyboard.press('Control+Shift+z'); await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    const saved = parse(readFileSync(file, 'utf8')); expect(saved.block).toEqual(original.block); expect(saved.editor.notes).toEqual(original.editor?.notes);
    const measurement = { fixture: 'test/fixtures/workflow-scale.ts', executableNodes: 256, blocks: 3, viewport: '1440x1000', renderMs, layoutMs, thresholdsMs: { render: 10000, layout: 10000 } };
    const measurements = info.outputPath('bounded-scene-measurements.json'); writeFileSync(measurements, JSON.stringify(measurement, null, 2) + '\n');
    await info.attach('bounded-scene-measurements', { path: measurements, contentType: 'application/json' });
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

for (const kind of ['loop', 'parallel'] as const) test(`inserts at a moved ${kind} private edge and keeps lane additions in child coordinates`, async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    let document: WorkflowDocument = addNode({ description: 'Private insertion coordinate ownership', block: emptyBlock() }, [], kind, 'container');
    document = setEntry(document, [], 'container');
    document = connect(document, [], 'container', kind === 'loop' ? 'satisfied' : 'next', 'done');
    if (kind === 'loop') document = connect(document, [], 'container', 'exhausted', 'done');
    else document = addBranch(document, [], 'container', 'other');
    const path: BlockPath = [{ node: 'container', ...(kind === 'parallel' ? { branch: 'other' } : {}) }];
    document = addNode(document, path, 'model', 'reader');
    const reader = blockAt(document, path)!.nodes.reader;
    if (reader.type !== 'model') throw new Error('model fixture');
    reader.instruction = 'Return text without changing the Block contract.';
    document = setEntry(document, path, 'reader'); document = connect(document, path, 'reader', 'next', 'done');
    document = setNodePresentation(document, [], 'container', { position: { x: 600, y: 300 } });
    document = setNodePresentation(document, path, 'reader', { position: { x: 360, y: 80 } });
    document = setNodePresentation(document, path, 'done', { position: { x: 780, y: 80 } });
    if (kind === 'parallel') document = setLane(document, { address: { block: [], node: 'container' }, branch: 'other', position: { x: 500, y: 1000 }, dimensions: { width: 1100, height: 400 } });
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'private_insertion.yaml'); writeFileSync(file, JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'private_insertion', exact: true }).click();
    await activate(page, `Return node done · ${blockKey(path)}`);
    const frameId = kind === 'loop' ? viewId([], 'node', 'container') : viewId(path, 'lane');
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const frame = await canvas(page).locator('.react-flow__node').evaluateAll((nodes, id) => {
      const node = nodes.find(node => node.getAttribute('data-id') === id)!;
      const rect = node.getBoundingClientRect(); return { x: rect.x, y: rect.y };
    }, frameId);
    const edge = canvas(page).locator('.react-flow__edge').filter({ has: page.locator('path.react-flow__edge-interaction') });
    const point = await edge.evaluateAll((edges, id) => {
      const path = edges.find(edge => edge.getAttribute('data-id') === id)!.querySelector<SVGPathElement>('path.react-flow__edge-interaction')!;
      const point = path.getPointAtLength(path.getTotalLength() / 2), screen = point.matrixTransform(path.getScreenCTM()!);
      return { x: screen.x, y: screen.y };
    }, viewId(path, 'edge', '0'));
    const zoom = await canvas(page).locator('.react-flow__viewport').evaluate(element => new DOMMatrix(getComputedStyle(element).transform).a);
    await expect.poll(() => page.evaluate(point => globalThis.document.elementFromPoint(point.x, point.y)?.closest('.react-flow__edge')?.getAttribute('data-id'), point)).toBe(viewId(path, 'edge', '0'));
    await page.mouse.dblclick(point.x, point.y);
    await expect(page.getByRole('dialog', { name: 'Add node', exact: true })).toBeVisible();
    await page.getByRole('dialog', { name: 'Add node', exact: true }).getByRole('option', { name: 'Model', exact: true }).click();
    await panel(page).getByLabel('Instruction', { exact: true }).fill('Inserted on the private edge.');
    await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    let saved = parse(readFileSync(file, 'utf8')) as WorkflowDocument;
    const position = saved.editor!.nodes!.find(entry => entry.address.node === 'model')!.position!;
    expect(Math.abs(position.x - (point.x - frame.x) / zoom)).toBeLessThan(2);
    expect(Math.abs(position.y - (point.y - frame.y) / zoom)).toBeLessThan(2);
    expect(blockAt(saved, path)!.edges).toEqual([{ from: 'reader', to: 'model' }, { from: 'model', to: 'done' }]);
    if (kind === 'parallel') {
      await activate(page, `Block inputs · ${blockKey(path)}`);
      await panel(page).getByRole('button', { name: 'Close inspector' }).click();
      await canvas(page).getByRole('button', { name: 'Fit view', exact: true }).click();
      await canvas(page).getByRole('button', { name: 'Add in other', exact: true }).click();
      await page.getByRole('dialog', { name: 'Add node', exact: true }).getByRole('option', { name: 'Model', exact: true }).click();
      await panel(page).getByLabel('Instruction', { exact: true }).fill('Added inside the moved lane.');
      await panel(page).getByLabel('Successor on next').selectOption('reader');
      await activate(page, `Block inputs · ${blockKey(path)}`); await panel(page).getByLabel('Entry node').selectOption('model_2');
      await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
      saved = parse(readFileSync(file, 'utf8')) as WorkflowDocument;
      const added = saved.editor!.nodes!.find(entry => entry.address.node === 'model_2')!.position!;
      expect(added.x).toBe(360); expect(added.y).toBeLessThan(700);
    }
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

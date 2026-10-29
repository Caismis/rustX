import { connectStart } from '../workflow-graph';
/** Complete native-backed visual authoring. Fixtures exist only in this test. */
import { test, expect, type Locator, type Page, type TestInfo } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse } from 'yaml';
import { startDogfood } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { captureEvidence } from './screenshot';
import { addNode, connect, emptyBlock } from '../../src/app/workflows/document/commands';
import { putNote, setNodePresentation } from '../../src/app/workflows/document/geometry';
import { type BlockPath, type WorkflowDocument } from '../../src/app/workflows/document/model';
import { viewId } from '../../src/app/workflows/ui/editor/selection';
import renameCases from '../../../tests/fixtures/workflow/schema-rename.json' with { type: 'json' };
import collisionCases from '../../../tests/fixtures/workflow/schema-rename-collisions.json' with { type: 'json' };

const canvas = (page: Page) => page.getByRole('region', { name: 'Workflow canvas' });
const panel = (page: Page) => page.getByRole('complementary', { name: 'Inspector' });
async function activate(page: Page, label: string) { const node = canvas(page).getByLabel(label, { exact: true }); await node.focus(); await node.press('Enter'); }
async function chooseExistingNext(page: Page, id: string, port = 'Next step') {
  await panel(page).getByRole('button', { name: new RegExp(`^(Choose next node for|Add parallel node on) ${port}$`) }).click();
  const picker = page.getByRole('dialog', { name: 'Choose a workflow node' });
  await picker.getByRole('tab', { name: 'Existing' }).click();
  await picker.locator(`[data-node-id="${id}"]`).click();
}
async function add(page: Page, name: string, after?: { node: string; port: string }) {
  if (after) {
    const port = after.port === 'next' ? 'Next step' : after.port;
    await panel(page).getByRole('button', { name: new RegExp(`^(Choose next node for|Add parallel node on) ${port}$`) }).click();
    const picker = page.getByRole('dialog', { name: 'Choose a workflow node' });
    await picker.getByRole('tab', { name: name === 'Tool' ? 'Tools' : 'Nodes' }).click();
    await picker.getByRole('option', { name, exact: true }).click();
  } else {
    await canvas(page).getByRole('toolbar').getByRole('button', { name: 'Add node', exact: true }).click();
    await page.getByRole('dialog', { name: 'Add node', exact: true }).getByRole('option', { name, exact: true }).click();
  }
}
const schema = (page: Page, label: string) => panel(page).locator(`[data-schema-label="${label}"]`).first();
async function property(page: Page, label: string, name: string, type = 'string') {
  return propertyIn(page, schema(page, label), name, type);
}
/** Add one Start-node input field through the Add input field dialog. */
async function chooseFieldType(page: Page, dialog: Locator, type: string) {
  await dialog.getByRole('button', { name: /^Field type/ }).click();
  await page.getByRole('menuitem', { name: new RegExp(`${type}$`) }).click();
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

test.use({ trace: 'on', actionTimeout: 15_000 });
test('adds parallel next nodes from the inspector and keeps every existing connection', async ({ page }, info) => {
  const fixture = await startDogfood('web_console_dogfood'), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  try {
    let document: WorkflowDocument = { description: 'Implicit parallel next steps', block: emptyBlock() };
    document.block.input = { type: 'object', properties: { flag: { type: 'boolean' } }, required: ['flag'], additionalProperties: false };
    document = addNode(document, [], 'model', 'model'); document = connectStart(document, [], 'model');
    document.block.nodes.model = { ...document.block.nodes.model, instruction: 'Return a short string.' } as typeof document.block.nodes.model;
    document = setNodePresentation(document, [], 'model', { position: { x: 380, y: 80 } });
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'next_step.yaml'); writeFileSync(file, JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'next_step', exact: true }).click();
    await expect(page).toHaveTitle(/rustX$/); await activate(page, 'Model node model');
    const next = panel(page).getByRole('region', { name: 'Next step' });
    await next.getByRole('button', { name: 'Choose next node for Next step' }).click();
    const picker = page.getByRole('dialog', { name: 'Choose a workflow node' });
    const search = picker.getByRole('textbox', { name: 'Search nodes and tools' });
    await expect(search).toBeFocused(); await expect(picker.getByRole('option', { name: 'Parallel', exact: true })).toHaveCount(0);
    await search.fill('review'); await search.press('Enter'); await chooseExistingNext(page, 'done');
    await activate(page, 'Model node model'); await next.getByRole('button', { name: 'Add parallel node on Next step' }).click();
    await search.fill('review'); await search.press('Enter'); await chooseExistingNext(page, 'done');
    await activate(page, 'Model node model');
    await expect(next.getByRole('button', { name: 'Review', exact: true })).toBeVisible();
    await expect(next.getByRole('button', { name: 'Review 2', exact: true })).toBeVisible();
    await next.scrollIntoViewIfNeeded(); await page.screenshot({ path: info.outputPath('parallel-next-steps.png') });
    await next.getByRole('button', { name: 'Disconnect Review', exact: true }).click();
    await expect(next.getByRole('button', { name: 'Review', exact: true })).toHaveCount(0);
    await expect(next.getByRole('button', { name: 'Review 2', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(next.getByRole('button', { name: 'Review', exact: true })).toBeVisible();
    await activate(page, 'Workflow Start'); await next.getByRole('button', { name: 'Add parallel node on Next step' }).click();
    await search.fill('branch'); await search.press('Enter');
    await panel(page).getByRole('button', { name: 'Add condition', exact: true }).click();
    await page.getByRole('dialog', { name: 'Choose a variable' }).getByRole('button', { name: /^args \/ flag ·/ }).click();
    await chooseExistingNext(page, 'done', 'IF'); await chooseExistingNext(page, 'done', 'ELSE');
    await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(panel(page)).toContainText('Structure check passed');
    await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    const saved = parse(readFileSync(file, 'utf8'));
    expect(saved.block).not.toHaveProperty('entry'); expect(saved.block.nodes.start.type).toBe('start');
    expect(Object.values(saved.block.nodes).some((node: any) => node.type === 'parallel')).toBe(false);
    expect(saved.block.edges.filter((edge: any) => edge.from === 'model').map((edge: any) => edge.to).sort()).toEqual(['review', 'review_2']);
    expect(saved.block.edges.filter((edge: any) => edge.from === 'start').map((edge: any) => edge.to).sort()).toEqual(['branch', 'model']);
    await activate(page, 'Workflow Start'); await next.getByRole('button', { name: 'Add parallel node on Next step' }).click();
    await search.press('Escape'); await expect(picker).toHaveCount(0);
    await expect(next.getByRole('button', { name: 'Add parallel node on Next step' })).toBeFocused();
    await next.getByRole('button', { name: 'Add parallel node on Next step' }).click();
    await page.screenshot({ path: info.outputPath('parallel-node-picker.png') });
    await page.setViewportSize({ width: 390, height: 844 }); await expect(picker).toBeVisible();
    const bounds = (await picker.boundingBox())!;
    expect(bounds.x).toBeGreaterThanOrEqual(0); expect(bounds.x + bounds.width).toBeLessThanOrEqual(390);
    expect(bounds.y).toBeGreaterThanOrEqual(0); expect(bounds.y + bounds.height).toBeLessThanOrEqual(844);
    await page.screenshot({ path: info.outputPath('parallel-node-picker-mobile.png') });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

test('reconnects ordinary edges to the real Start without replacing its other successor', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    let document: WorkflowDocument = { description: 'Real Start fanout', block: emptyBlock() };
    document = addNode(document, [], 'model', 'model'); document = addNode(document, [], 'return', 'other');
    document = connectStart(document, [], 'model'); document = connect(document, [], 'model', 'next', 'done'); document = connect(document, [], 'model', 'next', 'other');
    document = setNodePresentation(document, [], 'model', { position: { x: 380, y: 80 } });
    document = setNodePresentation(document, [], 'done', { position: { x: 900, y: 80 } }); document = setNodePresentation(document, [], 'other', { position: { x: 900, y: 400 } });
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    writeFileSync(join(directory, 'start_fanout.yaml'), JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'start_fanout', exact: true }).click();
    await canvas(page).getByRole('button', { name: 'Fit view', exact: true }).click();
    const edge = canvas(page).getByRole('group', { name: `Edge from ${viewId([], 'node', 'model')} to ${viewId([], 'node', 'done')}`, exact: true });
    const start = canvas(page).getByLabel('Workflow Start', { exact: true });
    await edge.focus(); await edge.press('Enter'); await expect(edge).toHaveClass(/selected/);
    await dragConnector(page, edge.locator('.react-flow__edgeupdater-source'), start.locator('[data-handleid="next"]'), true);
    await activate(page, 'Workflow Start');
    await expect(panel(page).getByRole('button', { name: 'model', exact: true })).toBeVisible();
    await expect(panel(page).getByRole('button', { name: 'done', exact: true })).toBeVisible();
    await expect(canvas(page).locator('.react-flow__edge')).toHaveCount(3);
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(panel(page).getByRole('button', { name: 'done', exact: true })).toHaveCount(0);
    await expect(panel(page).getByRole('button', { name: 'model', exact: true })).toBeVisible();
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
  } finally { await fixture.stop(false); }
});

test('renames input fields and edits optional array items through the field dialog', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    const document: WorkflowDocument = { description: 'Finite input rename and optional items', block: emptyBlock() };
    document.block.input = renameCases[0].before;
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'finite_schema.yaml'); writeFileSync(file, JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'finite_schema', exact: true }).click();
    await activate(page, 'Workflow Start');
    const input = schema(page, 'Input fields');
    const edit = page.getByRole('dialog', { name: 'Edit input field', exact: true });
    await input.getByRole('button', { name: 'Edit before', exact: true }).first().click();
    await edit.getByLabel('Variable name', { exact: true }).fill('after'); await edit.getByRole('button', { name: 'Save', exact: true }).click();
    await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(panel(page)).toContainText('Structure check passed');
    await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    expect(parse(readFileSync(file, 'utf8')).block.input).toEqual(renameCases[0].after);
    await activate(page, 'Workflow Start');
    await input.getByRole('button', { name: 'Add input field', exact: true }).click();
    const add = page.getByRole('dialog', { name: 'Add input field', exact: true });
    await chooseFieldType(page, add, 'array'); await add.getByLabel('Variable name', { exact: true }).fill('list');
    await add.getByLabel('Item type', { exact: true }).selectOption('');
    await add.getByRole('button', { name: 'Save', exact: true }).click();
    await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    expect(parse(readFileSync(file, 'utf8')).block.input.properties.list).toEqual({ type: 'array' });
    expect(parse(readFileSync(file, 'utf8')).block.input.required).toContain('list');
    await input.getByRole('button', { name: 'Edit list', exact: true }).first().click();
    await edit.getByLabel('Item type', { exact: true }).selectOption('boolean'); await edit.getByRole('button', { name: 'Save', exact: true }).click();
    await page.getByRole('button', { name: 'Save File', exact: true }).click(); await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    expect(parse(readFileSync(file, 'utf8')).block.input.properties.list).toEqual({ type: 'array', items: { type: 'boolean' } });
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

test('refuses destination-only finite schema collisions without adopting extra properties or losing rename text', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    // Nested contracts are edited as JSON; the dialog renames top-level fields.
    const topLevel = collisionCases.filter(example => Object.hasOwn(example.schema.properties, 'before'));
    for (const [index, example] of topLevel.entries()) writeFileSync(join(directory, `finite_collision_${index}.yaml`), JSON.stringify({ description: example.name, block: { ...emptyBlock(), input: example.schema } }));
    await openLibrary(page, fixture);
    for (const [index, example] of topLevel.entries()) {
      const file = join(directory, `finite_collision_${index}.yaml`), before = parse(readFileSync(file, 'utf8'));
      await page.getByRole('button', { name: `finite_collision_${index}`, exact: true }).click();
      await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(panel(page)).toContainText('Structure check passed');
      await activate(page, 'Workflow Start');
      await schema(page, 'Input fields').getByRole('button', { name: 'Edit before', exact: true }).first().click();
      const dialog = page.getByRole('dialog', { name: 'Edit input field', exact: true });
      const name = dialog.getByLabel('Variable name', { exact: true });
      await name.fill('after'); await dialog.getByRole('button', { name: 'Save', exact: true }).click();
      await expect(name).toHaveValue('after'); await expect(name).toHaveAttribute('aria-invalid', 'true');
      await expect(dialog.getByRole('alert')).toContainText('That ID is already used');
      await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeDisabled();
      await name.press('Escape'); await expect(dialog).toHaveCount(0);
      expect(parse(readFileSync(file, 'utf8'))).toEqual(before); expect(before.block.input).toEqual(example.schema);
      await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
      await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();
    }
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
    document = connectStart(document, [], 'first');
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

test('authors structured required output, nested contracts and repeated downstream bindings using visual controls', async ({ page }, info) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    await openLibrary(page, fixture);
    await page.getByRole('button', { name: 'New Workflow', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'New Workflow', exact: true });
    await dialog.getByLabel('Workflow ID').fill('structured_authoring'); await dialog.getByRole('textbox', { name: /^Name/ }).fill('Declare and reuse structured model output.'); await dialog.getByLabel('Model description').fill('Declare and reuse structured model output.');
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
    await activate(page, 'Workflow Start'); await chooseExistingNext(page, 'model');
    await activate(page, 'Model node model'); await add(page, 'Model', { node: 'model', port: 'next' });
    await panel(page).getByLabel('Instruction', { exact: true }).fill('Compare two bindings of the same producer.');
    const bindings = panel(page).locator('section').filter({ has: page.getByRole('heading', { name: 'Input bindings', exact: true }) }).first();
    for (const name of ['first', 'second']) { await addBinding(bindings, name); await reference(page, bindings.getByRole('group', { name, exact: true }), 'model / risk'); }
    await chooseExistingNext(page, 'done');
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
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();
    await page.getByRole('button', { name: 'Declare and reuse structured model output.', exact: true }).click();
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
    await expect(canvas(page).locator('.react-flow__node')).toHaveCount(257);
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
    const measurement = { fixture: 'test/fixtures/workflow-scale.ts', executableNodes: 256, blocks: 2, viewport: '1440x1000', renderMs, layoutMs, thresholdsMs: { render: 10000, layout: 10000 } };
    const measurements = info.outputPath('bounded-scene-measurements.json'); writeFileSync(measurements, JSON.stringify(measurement, null, 2) + '\n');
    await info.attach('bounded-scene-measurements', { path: measurements, contentType: 'application/json' });
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});



test('appends siblings from connected canvas handles instead of inserting into the first edge', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    let document: WorkflowDocument = { description: 'Canvas handle fanout', block: emptyBlock() };
    document = addNode(document, [], 'model', 'model'); document = connectStart(document, [], 'model'); document = connect(document, [], 'model', 'next', 'done');
    document = setNodePresentation(document, [], 'model', { position: { x: 380, y: 80 } });
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    writeFileSync(join(directory, 'canvas_fanout.yaml'), JSON.stringify(document));
    await openLibrary(page, fixture); await page.getByRole('button', { name: 'canvas_fanout', exact: true }).click();
    await activate(page, 'Model node model');
    await canvas(page).getByRole('button', { name: /^Add node after model \(/ }).click();
    const picker = page.getByRole('dialog', { name: 'Add node', exact: true });
    await expect(picker.getByRole('option', { name: 'Parallel', exact: true })).toHaveCount(0);
    await picker.getByRole('option', { name: 'Branch', exact: true }).click();
    await chooseExistingNext(page, 'done', 'IF'); await chooseExistingNext(page, 'done', 'ELSE');
    await activate(page, 'Model node model');
    const next = panel(page).getByRole('region', { name: 'Next step' });
    await expect(next.getByRole('button', { name: 'done', exact: true })).toBeVisible();
    await expect(next.getByRole('button', { name: 'Branch', exact: true })).toBeVisible();
    await expect(canvas(page).locator('.react-flow__edge')).toHaveCount(5);
  } finally { await fixture.stop(false); }
});

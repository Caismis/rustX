import { test, expect, type Page } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
import { startDogfood } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';

async function editor(page: Page, fixture: Awaited<ReturnType<typeof startDogfood>>, id: string) {
  await routeWorkspaceHost(page, fixture); await page.goto('/');
  await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
  await page.getByRole('navigation', { name: 'Primary navigation' }).getByRole('button', { name: 'Workflows', exact: true }).click();
  await page.getByRole('button', { name: id, exact: true }).click();
  await expect(page).toHaveTitle(/rustX$/);
}

for (const width of [1440, 390]) test(`focused Human Input fields save and follow history at ${width}px`, async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'review_fields.yaml');
    writeFileSync(file, JSON.stringify({ description: 'Focused review fields', block: { input: { type: 'object', properties: {}, additionalProperties: false },
      nodes: { start: { type: 'start' }, human: { type: 'review', content: [{ type: 'literal', value: 'Choose' }], actions: [{ id: 'approve', title: 'First' }], timeout_ms: 60000 }, done: { type: 'return', outputs: [] } },
      edges: [{ from: 'start', to: 'human' }, { from: 'human', to: 'done', port: 'approve' }, { from: 'human', to: 'done', port: 'timeout' }] } }));
    await editor(page, fixture, 'review_fields'); await page.setViewportSize({ width, height: 1000 });
    const human = page.getByRole('region', { name: 'Workflow canvas' }).getByLabel('Human Input node human', { exact: true });
    await human.focus(); await human.press('Enter');
    const panel = page.getByRole('complementary', { name: 'Inspector' });
    const title = panel.getByRole('textbox', { name: 'Button text', exact: true });
    await title.fill('Replacement'); await title.press('ControlOrMeta+s');
    await expect.poll(() => parse(readFileSync(file, 'utf8')).block.nodes.human.actions[0].title).toBe('Replacement');
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    await page.getByRole('button', { name: 'Undo', exact: true }).click(); await expect(title).toHaveValue('First');
    await title.focus(); await title.blur(); await expect(title).toHaveValue('First');
    await page.getByRole('button', { name: 'Redo', exact: true }).click(); await expect(title).toHaveValue('Replacement');
    const id = panel.getByRole('textbox', { name: 'Action ID', exact: true });
    await id.fill('renamed'); await id.press('ControlOrMeta+s');
    await expect.poll(() => parse(readFileSync(file, 'utf8')).block.nodes.human.actions[0].id).toBe('renamed');
    expect(parse(readFileSync(file, 'utf8')).block.edges.some((edge: { port?: string }) => edge.port === 'renamed')).toBe(true);
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    await page.screenshot({ path: `/tmp/rustx-review-focused-fields-${width}.png` });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  } finally { await fixture.stop(false); }
});

test('a catalog larger than one RPC resolves a tool on the final page', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const server = fileURLToPath(new URL('./workflow-json-mcp.py', import.meta.url));
    writeFileSync(join(fixture.workspaceA, '.agents/mcp.toml'), `[mcp_servers.large]\ntype = "stdio"\ncommand = "python3"\nargs = [${JSON.stringify(server)}, "--large-catalog"]\n`);
    const file = join(directory, 'large_catalog.yaml');
    writeFileSync(file, JSON.stringify({ description: 'Large catalog', block: { input: { type: 'object', properties: {}, additionalProperties: false },
      nodes: { start: { type: 'start' }, last: { type: 'tool', selector: { origin: 'source', source_id: 'large', name: 'tool_127' }, arguments: { type: 'object', fields: {} }, result: { type: 'text' } }, done: { type: 'return', outputs: [] } },
      edges: [{ from: 'start', to: 'last' }, { from: 'last', to: 'done' }] } }));
    await editor(page, fixture, 'large_catalog');
    const tool = page.getByRole('region', { name: 'Workflow canvas' }).getByLabel('Tool node last', { exact: true });
    await tool.focus(); await tool.press('Enter');
    const panel = page.getByRole('complementary', { name: 'Inspector' });
    await expect(panel.getByRole('region', { name: 'Input variables', exact: true })).toBeVisible();
    const last = panel.getByRole('textbox', { name: 'field_63', exact: true });
    await last.fill('Final page'); await last.press('ControlOrMeta+s');
    await expect.poll(() => parse(readFileSync(file, 'utf8')).block.nodes.last.arguments.fields.field_63).toEqual({ type: 'literal', value: 'Final page' });
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    await page.screenshot({ path: '/tmp/rustx-large-tool-catalog.png' });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  } finally { await fixture.stop(false); }
});

for (const operation of ['add', 'remove'] as const) test(`End name drafts survive ${operation}, Undo/Redo and correction before save`, async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'end_drafts.yaml');
    const first = { variable: 'first', value_selector: ['args', 'first'] };
    const second = { variable: 'second', value_selector: ['args', 'second'] };
    writeFileSync(file, JSON.stringify({ description: 'End draft identity', block: {
      input: { type: 'object', properties: { first: { type: 'string' }, second: { type: 'string' } }, required: ['first', 'second'], additionalProperties: false },
      nodes: { start: { type: 'start' }, done: { type: 'return', outputs: [first, second] } }, edges: [{ from: 'start', to: 'done' }],
    } }));
    await editor(page, fixture, 'end_drafts');
    const end = page.getByRole('region', { name: 'Workflow canvas' }).getByLabel('Output node done', { exact: true });
    await end.focus(); await end.press('Enter');
    const panel = page.getByRole('complementary', { name: 'Inspector' });
    const names = panel.getByRole('textbox', { name: 'Variable name', exact: true });
    await names.nth(1).fill(''); await names.nth(1).blur();
    await expect(names.nth(1)).toHaveAttribute('aria-invalid', 'true');
    await panel.getByRole('button', { name: operation === 'add' ? 'Add output variable' : 'Remove first', exact: true }).click();
    const index = operation === 'add' ? 1 : 0;
    await expect(names.nth(index)).toHaveValue('');
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(names).toHaveCount(2); await expect(names.nth(1)).toHaveValue('');
    await page.getByRole('button', { name: 'Redo', exact: true }).click();
    await expect(names.nth(index)).toHaveValue('');
    if (operation === 'add') await panel.getByRole('button', { name: 'Remove 3', exact: true }).click();
    await names.nth(index).fill('fixed'); await names.nth(index).press('Enter');
    await names.nth(index).press('ControlOrMeta+s');
    const fixed = { variable: 'fixed', value_selector: ['args', 'second'] };
    await expect.poll(() => parse(readFileSync(file, 'utf8')).block.nodes.done.outputs).toEqual(operation === 'add' ? [first, fixed] : [fixed]);
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    await page.screenshot({ path: `/tmp/rustx-end-draft-${operation}.png` });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
  } finally { await fixture.stop(false); }
});

for (const width of [1440, 390]) test(`Loop variable rename saves all references and follows Undo/Redo at ${width}px`, async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  const fixture = await startDogfood('web_console_dogfood');
  try {
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'loop_rename.yaml');
    writeFileSync(file, JSON.stringify({ description: 'Loop declaration identity', block: { input: { type: 'object', properties: {}, additionalProperties: false },
      nodes: { start: { type: 'start' }, repeat: { type: 'loop', loop_count: 1, loop_variables: [{ label: 'ready', var_type: 'boolean', value: { type: 'literal', value: false } }],
        break_condition: { type: 'boolean', value: { type: 'reference', path: ['repeat', 'ready'] } }, body: { nodes: { start: { type: 'start' }, assign: { type: 'assigner', items: [{ variable: ['repeat', 'ready'], operation: 'set', value: { type: 'literal', value: true } }] } }, edges: [{ from: 'start', to: 'assign' }] } },
        done: { type: 'return', outputs: [{ variable: 'ready', value_selector: ['repeat', 'ready'] }] } }, edges: [{ from: 'start', to: 'repeat' }, { from: 'repeat', to: 'done' }] } }));
    await editor(page, fixture, 'loop_rename'); await page.setViewportSize({ width, height: 1000 });
    const loop = page.getByRole('region', { name: 'Workflow canvas' }).getByLabel('Loop node repeat', { exact: true });
    await loop.focus(); await loop.press('Enter');
    const input = page.getByRole('complementary', { name: 'Inspector' }).getByRole('textbox', { name: 'Variable Name', exact: true });
    await input.fill('finished'); await input.press('ControlOrMeta+s');
    await expect.poll(() => parse(readFileSync(file, 'utf8')).block.nodes.repeat.loop_variables[0].label).toBe('finished');
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    const document = parse(readFileSync(file, 'utf8'));
    expect(document.block.nodes.repeat.body.nodes.assign.items[0].variable).toEqual(['repeat', 'finished']);
    expect(document.block.nodes.repeat.break_condition.value.path).toEqual(['repeat', 'finished']);
    expect(document.block.nodes.done.outputs[0].value_selector).toEqual(['repeat', 'finished']);
    await page.getByRole('button', { name: 'Undo', exact: true }).click(); await expect(input).toHaveValue('ready');
    await page.getByRole('button', { name: 'Redo', exact: true }).click(); await expect(input).toHaveValue('finished');
    await input.press('ControlOrMeta+s');
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    await page.screenshot({ path: `/tmp/rustx-loop-variable-rename-${width}.png` });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

for (const kind of ['required', 'optional']) test(`catalog ${kind} enum saves an explicit empty string`, async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  const fixture = await startDogfood('web_console_dogfood');
  try {
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const server = fileURLToPath(new URL('./workflow-json-mcp.py', import.meta.url));
    writeFileSync(join(fixture.workspaceA, '.agents/mcp.toml'), `[mcp_servers.enums]\ntype = "stdio"\ncommand = "python3"\nargs = [${JSON.stringify(server)}, "--empty-enum"]\n`);
    const file = join(directory, 'empty_enum.yaml');
    writeFileSync(file, JSON.stringify({ description: 'Empty enum business value', block: { input: { type: 'object', properties: {}, additionalProperties: false },
      nodes: { start: { type: 'start' }, call: { type: 'tool', selector: { origin: 'source', source_id: 'enums', name: `empty_${kind}` }, arguments: { type: 'object', fields: { mode: { type: 'literal', value: 'explicit' } } }, result: { type: 'text' } }, done: { type: 'return', outputs: [] } }, edges: [{ from: 'start', to: 'call' }, { from: 'call', to: 'done' }] } }));
    await editor(page, fixture, 'empty_enum');
    const tool = page.getByRole('region', { name: 'Workflow canvas' }).getByLabel('Tool node call', { exact: true });
    await tool.focus(); await tool.press('Enter');
    const mode = page.getByRole('complementary', { name: 'Inspector' }).getByRole('combobox', { name: 'mode', exact: true });
    await mode.selectOption({ label: '""' });
    await page.getByRole('button', { name: 'Undo', exact: true }).click(); await expect(mode).toHaveValue('1');
    await page.getByRole('button', { name: 'Redo', exact: true }).click(); await expect(mode).toHaveValue('0');
    await mode.selectOption(''); await expect(mode).toHaveValue('');
    await page.getByRole('button', { name: 'Undo', exact: true }).click(); await expect(mode).toHaveValue('0');
    await mode.press('ControlOrMeta+s');
    await expect.poll(() => parse(readFileSync(file, 'utf8')).block.nodes.call.arguments.fields.mode).toEqual({ type: 'literal', value: '' });
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    await page.screenshot({ path: `/tmp/rustx-empty-enum-${kind}.png` });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

for (const operation of ['add', 'remove_other', 'remove_before'] as const) test(`Assigner unfinished number keeps its field through ${operation}, history and native save`, async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood'), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  try {
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const file = join(directory, 'assigner_fields.yaml');
    const items = [{ variable: ['repeat', 'x'], operation: 'overwrite', value: { type: 'literal', value: 10 } }, { variable: ['repeat', 'y'], operation: 'overwrite', value: { type: 'literal', value: 20 } }];
    writeFileSync(file, JSON.stringify({ description: 'Assigner field identities', block: { input: { type: 'object', properties: {}, additionalProperties: false }, nodes: {
      start: { type: 'start' }, repeat: { type: 'loop', loop_count: 1, loop_variables: ['x', 'y'].map(label => ({ label, var_type: 'number', value: { type: 'literal', value: 0 } })),
        body: { nodes: { start: { type: 'start' }, assign: { type: 'assigner', items } }, edges: [{ from: 'start', to: 'assign' }] } }, done: { type: 'return', outputs: [] } },
      edges: [{ from: 'start', to: 'repeat' }, { from: 'repeat', to: 'done' }] } }));
    await editor(page, fixture, 'assigner_fields');
    const assign = page.getByRole('region', { name: 'Workflow canvas' }).getByLabel('Variable Assigner node assign · repeat/body', { exact: true });
    await assign.focus(); await assign.press('Enter');
    const panel = page.getByRole('complementary', { name: 'Inspector' });
    const fields = panel.getByRole('textbox', { name: 'Value', exact: true });
    const initial = operation === 'remove_before' ? 1 : 0;
    await fields.nth(initial).fill('1e'); await fields.nth(initial).blur();
    await expect(fields.nth(initial)).toHaveAttribute('aria-invalid', 'true');
    await panel.getByRole('button', { name: operation === 'add' ? 'Add variable' : operation === 'remove_other' ? 'Remove write 2' : 'Remove write 1', exact: true }).click();
    await expect(panel.getByRole('combobox', { name: 'Assigned Variable', exact: true })).toHaveCount(operation === 'add' ? 3 : 1);
    await expect(fields.nth(0)).toHaveValue('1e');
    await page.getByRole('button', { name: 'Undo', exact: true }).click(); await expect(fields.nth(initial)).toHaveValue('1e');
    await page.getByRole('button', { name: 'Redo', exact: true }).click(); await expect(fields.nth(0)).toHaveValue('1e');
    if (operation === 'add') await panel.getByRole('button', { name: 'Remove write 3', exact: true }).click();
    await fields.nth(0).press('ControlOrMeta+s');
    expect(parse(readFileSync(file, 'utf8')).block.nodes.repeat.body.nodes.assign.items).toEqual(items);
    await expect(fields.nth(0)).toHaveValue('1e');
    await fields.nth(0).fill('42'); await fields.nth(0).press('ControlOrMeta+s');
    const expected = operation === 'add' ? [{ ...items[0], value: { type: 'literal', value: 42 } }, items[1]] : [{ ...items[initial], value: { type: 'literal', value: 42 } }];
    await expect.poll(() => parse(readFileSync(file, 'utf8')).block.nodes.repeat.body.nodes.assign.items).toEqual(expected);
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    await page.screenshot({ path: `/tmp/rustx-assigner-draft-${operation}.png` });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
  } finally { await fixture.stop(false); }
});

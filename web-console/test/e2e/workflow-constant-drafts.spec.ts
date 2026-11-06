import { test, expect } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
import { startDogfood, stopDogfoodPage } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';

for (const width of [1440, 390]) test(`catalog Tool constants keep unsafe drafts across selection and save at ${width}px`, async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  try {
    const directory = join(fixture.workspaceA, '.agents/workflows');
    mkdirSync(directory, { recursive: true });
    const file = join(directory, 'constant_drafts.yaml');
    writeFileSync(file, JSON.stringify({ description: 'Catalog constant drafts', block: {
      input: { type: 'object', properties: {}, additionalProperties: false },
      nodes: { start: { type: 'start' }, read_file: { type: 'tool', selector: { origin: 'builtin', name: 'read' },
        arguments: { type: 'object', fields: { path: { type: 'literal', value: 'sample.txt' } } }, result: { type: 'text' } },
      done: { type: 'return', outputs: [] } }, edges: [{ from: 'start', to: 'read_file' }, { from: 'read_file', to: 'done' }],
    } }));
    await page.setViewportSize({ width: 1440, height: 1000 });
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await page.getByRole('button', { name: 'constant_drafts', exact: true }).click();
    await expect(page).toHaveTitle(/rustX$/);
    await page.setViewportSize({ width, height: 1000 });
    const canvas = page.getByRole('region', { name: 'Workflow canvas' });
    const inspector = page.getByRole('complementary', { name: 'Inspector' });
    const tool = canvas.getByLabel('Tool node read_file', { exact: true });
    await tool.focus(); await tool.press('Enter');
    const offset = inspector.getByRole('textbox', { name: 'offset', exact: true });
    await offset.fill('9007199254740993'); await offset.blur();
    await expect(offset).toHaveAttribute('aria-invalid', 'true');
    const start = canvas.getByLabel('Workflow Start', { exact: true });
    await start.focus(); await start.press('Enter');
    await tool.focus(); await tool.press('Enter');
    await expect(offset).toHaveValue('9007199254740993');
    await offset.press('ControlOrMeta+s');
    expect(parse(readFileSync(file, 'utf8')).block.nodes.read_file.arguments.fields.offset).toBeUndefined();
    await offset.scrollIntoViewIfNeeded();
    await page.screenshot({ path: `/tmp/rustx-workflow-constant-draft-${width}.png` });
    await offset.fill('12'); await offset.press('Enter'); await offset.press('ControlOrMeta+s');
    await expect.poll(() => parse(readFileSync(file, 'utf8')).block.nodes.read_file.arguments.fields.offset).toEqual({ type: 'literal', value: 12 });
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    await offset.fill(''); await offset.press('Enter'); await offset.press('ControlOrMeta+s');
    await expect.poll(() => parse(readFileSync(file, 'utf8')).block.nodes.read_file.arguments.fields.offset).toBeUndefined();
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    expect(errors).toEqual([]);
  } finally { await stopDogfoodPage(page, fixture, false); }
});

for (const kind of ['object', 'literal'] as const) test(`catalog JSON payloads use their own draft base in ${kind} arguments`, async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  try {
    const original = { x: 1 }, wrapper = { type: 'literal', value: original };
    const directory = join(fixture.workspaceA, '.agents/workflows');
    mkdirSync(directory, { recursive: true });
    const file = join(directory, 'json_payload.yaml');
    const server = fileURLToPath(new URL('./workflow-json-mcp.py', import.meta.url));
    writeFileSync(join(fixture.workspaceA, '.agents/mcp.toml'), `[mcp_servers.json_fixture]\ntype = "stdio"\ncommand = "python3"\nargs = [${JSON.stringify(server)}]\n`);
    writeFileSync(file, JSON.stringify({ description: 'Catalog payload projection', block: {
      input: { type: 'object', properties: {}, additionalProperties: false },
      nodes: { start: { type: 'start' }, payload: { type: 'tool', selector: { origin: 'source', source_id: 'json_fixture', name: 'accept_payload' },
        arguments: kind === 'object' ? { type: kind, fields: { metadata: wrapper } }
          : { type: kind, value: { metadata: original } }, result: { type: 'text' } },
      done: { type: 'return', outputs: [] } }, edges: [{ from: 'start', to: 'payload' }, { from: 'payload', to: 'done' }],
    } }));
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await page.getByRole('button', { name: 'json_payload', exact: true }).click();
    await expect(page).toHaveTitle(/rustX$/);
    const tool = page.getByRole('region', { name: 'Workflow canvas' }).getByLabel('Tool node payload', { exact: true });
    await tool.focus(); await tool.press('Enter');
    const panel = page.getByRole('complementary', { name: 'Inspector' });
    const code = panel.getByText('metadata', { exact: true }).locator('../..').getByRole('textbox', { name: 'JSON', exact: true });
    const replace = async (value: unknown) => {
      await code.press('ControlOrMeta+a'); await page.keyboard.insertText(JSON.stringify(value, null, 2));
    };
    await replace(wrapper);
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeEnabled();
    await replace(original);
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    await replace(wrapper); await code.blur();
    await page.screenshot({ path: `/tmp/rustx-workflow-json-payload-${kind}.png` });
    await page.getByRole('button', { name: 'Save File', exact: true }).click();
    await expect.poll(() => {
      const args = parse(readFileSync(file, 'utf8')).block.nodes.payload.arguments;
      return kind === 'object' ? args.fields.metadata.value : args.value.metadata;
    }).toEqual(wrapper);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally { await stopDogfoodPage(page, fixture, false); }
});

test('a resolved catalog preserves constructed object and array parameters with references', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  try {
    const ref = { type: 'reference', path: ['args', 'token'] };
    const metadata = { type: 'object', fields: { x: ref, values: { type: 'array', items: [ref] } } };
    const directory = join(fixture.workspaceA, '.agents/workflows'); mkdirSync(directory, { recursive: true });
    const server = fileURLToPath(new URL('./workflow-json-mcp.py', import.meta.url));
    writeFileSync(join(fixture.workspaceA, '.agents/mcp.toml'), `[mcp_servers.json_fixture]\ntype = "stdio"\ncommand = "python3"\nargs = [${JSON.stringify(server)}]\n`);
    const file = join(directory, 'constructed_params.yaml');
    writeFileSync(file, JSON.stringify({ description: 'Constructed Tool parameters', block: {
      input: { type: 'object', properties: { token: { type: 'string' } }, required: ['token'], additionalProperties: false },
      nodes: { start: { type: 'start' }, payload: { type: 'tool', selector: { origin: 'source', source_id: 'json_fixture', name: 'accept_payload' },
        arguments: { type: 'object', fields: { metadata } }, result: { type: 'text' } }, done: { type: 'return', outputs: [] } },
      edges: [{ from: 'start', to: 'payload' }, { from: 'payload', to: 'done' }],
    } }));
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await page.getByRole('button', { name: 'constructed_params', exact: true }).click();
    await expect(page).toHaveTitle(/rustX$/);
    const tool = page.getByRole('region', { name: 'Workflow canvas' }).getByLabel('Tool node payload', { exact: true });
    await tool.focus(); await tool.press('Enter');
    const panel = page.getByRole('complementary', { name: 'Inspector' });
    await expect(panel.getByRole('region', { name: 'Input variables', exact: true })).toBeVisible();
    const parameter = panel.getByRole('group', { name: 'metadata', exact: true });
    await expect(parameter.getByRole('radio', { name: 'Object', exact: true, checked: true })).toHaveAttribute('aria-checked', 'true');
    await expect(parameter.getByRole('group', { name: 'values', exact: true }).getByRole('radio', { name: 'Array', exact: true, checked: true })).toHaveAttribute('aria-checked', 'true');
    expect(parse(readFileSync(file, 'utf8')).block.nodes.payload.arguments.fields.metadata).toEqual(metadata);
    const slot = parameter.getByRole('group', { name: 'x', exact: true });
    await slot.getByRole('radio', { name: 'Literal', exact: true }).click();
    const code = slot.getByRole('textbox', { name: 'Literal JSON', exact: true });
    await code.fill('"replacement"'); await code.blur();
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await page.getByRole('button', { name: 'Undo', exact: true }).click();
    await expect(slot.getByRole('radio', { name: 'Reference', exact: true })).toHaveAttribute('aria-checked', 'true');
    await page.getByRole('button', { name: 'Redo', exact: true }).click();
    await page.getByRole('button', { name: 'Redo', exact: true }).click();
    await expect(code).toHaveValue('"replacement"');
    await page.getByRole('button', { name: 'Save File', exact: true }).click();
    await expect.poll(() => parse(readFileSync(file, 'utf8')).block.nodes.payload.arguments.fields.metadata)
      .toEqual({ ...metadata, fields: { ...metadata.fields, x: { type: 'literal', value: 'replacement' } } });
    await page.screenshot({ path: '/tmp/rustx-workflow-constructed-parameters.png' });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0); expect(errors).toEqual([]);
  } finally { await stopDogfoodPage(page, fixture, false); }
});

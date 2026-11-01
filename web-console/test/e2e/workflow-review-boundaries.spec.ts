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
  await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
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

import { test, expect } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse } from 'yaml';
import { startDogfood } from './dogfood-server';
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
  } finally { await fixture.stop(false); }
});

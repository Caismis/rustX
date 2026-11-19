import { expect, test, type Page } from '@playwright/test';
import type { Request } from '../../../protocol/app-server/v80';
import { choose, editMcpConnection, openResourceModule } from './shell-actions';

const fixture = `http://127.0.0.1:${process.env.RUSTX_E2E_FIXTURE_PORT ?? 5174}/test/fixtures/settings.html?scenario=mcp-details`;
const nativeRequests = (page: Page) => page.evaluate(() =>
  (window as unknown as { rustxNativeRequests: () => Request[] }).rustxNativeRequests());
const listRequests = async (page: Page) => (await nativeRequests(page)).flatMap(request =>
  request.method === 'workflow/toolCatalog' ? [request.params] : []);

for (const width of [1440, 390]) test(`MCP server cards and exact tool access at ${width}px`, async ({ page }) => {
  await page.setViewportSize({ width, height: 950 });
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(`${fixture}&hold-tools`);
  if (width === 390) {
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByRole('button', { name: 'Dark', exact: true }).click();
  }
  const surface = await openResourceModule(page, 'mcp', 'Global');
  const back = surface.getByRole('button', { name: '← MCP servers', exact: true });
  expect(await listRequests(page)).toEqual([]);

  await editMcpConnection(surface, 'notes');
  await expect(surface.getByLabel('Command', { exact: true })).toHaveValue('notes-fixture');
  expect(await listRequests(page)).toEqual([]);
  await back.click();

  await surface.getByRole('button', { name: 'MCP notes', exact: true }).click();
  const tools = surface.getByRole('tab', { name: 'Tools', exact: true });
  await expect(tools).toHaveAttribute('aria-selected', 'true');
  await expect(surface.getByRole('status').filter({ hasText: /loading.*tools/i })).toBeVisible();
  await expect.poll(() => listRequests(page)).toMatchObject([{ target: { kind: 'user' }, source: 'notes', expected_scope: 'user' }]);
  await page.evaluate(() => (window as unknown as { rustxReleaseTools: () => void }).rustxReleaseTools());
  await expect(surface.getByText('Read the selected note.', { exact: true })).toBeVisible();
  await expect(surface.getByText('Find notes that match a text query.', { exact: true })).toBeVisible();
  expect(await surface.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.screenshot({ path: `/tmp/rustx-mcp-tools-${width}.png` });

  await surface.getByRole('tab', { name: 'Permissions', exact: true }).click();
  const selection = surface.getByRole('form', { name: 'Tools: notes' });
  await expect(selection.getByRole('checkbox', { name: 'previous_tool', exact: true })).toBeChecked();
  const readNote = selection.getByRole('checkbox', { name: 'read_note', exact: true });
  await readNote.check();
  await page.screenshot({ path: `/tmp/rustx-mcp-permissions-${width}.png` });
  await selection.getByRole('button', { name: /save/i }).click();
  await expect.poll(async () => (await nativeRequests(page)).flatMap(request =>
    request.method === 'configuration/sourceWrite' ? [request.params] : [])).toMatchObject([{
    target: { kind: 'user' },
    mutation: { kind: 'config', mutation: { unit: 'source_tools', id: 'notes', authored: ['previous_tool', 'read_note'] } },
  }]);
  await expect(readNote).toBeChecked();
  await tools.click();
  await expect(surface.getByText('Read the selected note.', { exact: true })).toBeVisible();
  expect(await listRequests(page)).toHaveLength(1);
  await back.click();
  await surface.getByRole('button', { name: 'Permissions notes', exact: true }).click();
  await expect(surface.getByRole('tab', { name: 'Permissions', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(selection.getByRole('checkbox', { name: 'read_note', exact: true })).toBeChecked();
  const approval = surface.getByRole('form', { name: 'Permissions', exact: true });
  await choose(approval, 'Plan', 'Ask');
  await approval.getByRole('button', { name: 'Save approval rules' }).click();
  await expect.poll(async () => (await nativeRequests(page)).flatMap(request =>
    request.method === 'configuration/sourceWrite' && request.params.mutation.kind === 'mcp' ? [request.params.mutation] : [])).toMatchObject([{
    kind: 'mcp', id: 'notes', authored: { definition: { command: 'notes-fixture', permissions: { plan: 'ask', automatic: 'default', full_access: 'default' } } },
  }]);
  expect(errors).toEqual([]);
});

test('MCP tools distinguish an empty server from a failed listing and retry only that server', async ({ page }) => {
  await page.goto(fixture);
  const surface = await openResourceModule(page, 'mcp', 'Global');
  await surface.getByRole('button', { name: 'MCP empty', exact: true }).click();
  await expect(surface.getByText(/no tools/i)).toBeVisible();
  await surface.getByRole('button', { name: '← MCP servers', exact: true }).click();
  await surface.getByRole('button', { name: 'MCP retry', exact: true }).click();
  await surface.getByRole('button', { name: /retry/i }).click();
  await expect(surface.getByText('Read the selected note.', { exact: true })).toBeVisible();
  expect((await listRequests(page)).map(request => request.source)).toEqual(['empty', 'retry', 'retry']);
  await surface.getByRole('tab', { name: 'Permissions', exact: true }).click();
  const selection = surface.getByRole('form', { name: 'Tools: retry' });
  await choose(selection, 'Selection', 'Selected tools');
  await expect(selection.getByRole('checkbox', { name: 'read_note', exact: true })).not.toBeChecked();
  await expect(selection.getByRole('button', { name: 'Save tool availability' })).toBeDisabled();
  expect((await nativeRequests(page)).filter(request => request.method === 'configuration/sourceWrite')).toEqual([]);
});

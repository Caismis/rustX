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

test('MCP tool rules survive reopening, rebase after server edits, and restore inheritance', async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(`${fixture}&tool-rules`);
  const surface = await openResourceModule(page, 'mcp', 'Global');
  await surface.getByRole('button', { name: 'MCP notes', exact: true }).click();
  const tools = surface.getByRole('tab', { name: 'Tools', exact: true });
  const permissions = surface.getByRole('tab', { name: 'Permissions', exact: true });
  const tool = surface.getByRole('form', { name: /read_note/ });
  const policy = tool.getByRole('combobox', { name: 'Tool approval', exact: true });
  const save = tool.getByRole('button', { name: /save/i });
  const writes = async () => (await nativeRequests(page)).flatMap(request =>
    request.method === 'configuration/sourceWrite' ? [request.params] : []);

  await expect(policy).toHaveValue('inherit');
  await policy.selectOption('allow');
  expect(await writes()).toHaveLength(0);
  await permissions.click();
  const defaults = surface.getByRole('form', { name: 'Permissions', exact: true });
  await choose(defaults, 'Plan', 'Ask');
  await defaults.getByRole('button', { name: 'Save approval rules', exact: true }).click();
  await expect.poll(writes).toHaveLength(1);

  await tools.click();
  await expect(policy).toHaveValue('allow');
  await tool.getByRole('button', { name: 'Use latest setting' }).click();
  await save.click();
  await expect.poll(writes).toHaveLength(2);
  const lostSibling = { permissions: { plan: 'deny', automatic: 'deny', full_access: 'deny' } };
  const authored = {
    definition: {
      command: 'notes-fixture',
      permissions: { plan: 'ask', automatic: 'ask', full_access: 'allow' },
      tools: { previous_tool: lostSibling, read_note: { permissions: { plan: 'allow', automatic: 'allow', full_access: 'allow' } } },
    },
    retained_env: ['NOTES_TOKEN'],
  };
  expect((await writes())[1]).toMatchObject({ target: { kind: 'user' }, mutation: { kind: 'mcp', id: 'notes', authored } });
  await expect(save).toBeDisabled();
  await surface.getByRole('button', { name: '← MCP servers', exact: true }).click();
  await surface.getByRole('button', { name: 'MCP notes', exact: true }).click();
  await expect(policy).toHaveValue('allow');
  await page.screenshot({ path: info.outputPath('tool-approvals-desktop.png') });

  for (const choice of ['ask', 'deny', 'inherit']) {
    const count = (await writes()).length;
    await policy.selectOption(choice);
    await save.click();
    await expect.poll(writes).toHaveLength(count + 1);
    const write = (await writes()).at(-1)!;
    expect(write.mutation).toMatchObject({ kind: 'mcp', id: 'notes', authored: {
      ...authored, definition: { ...authored.definition, tools: { previous_tool: lostSibling } },
    } });
    if (write.mutation.kind !== 'mcp') throw new Error('Expected MCP mutation');
    expect(write.mutation.authored?.definition.tools?.read_note?.permissions).toEqual(
      choice === 'inherit' ? undefined : { plan: choice, automatic: choice, full_access: choice },
    );
    await expect(save).toBeDisabled();
  }
  await expect(policy).toHaveValue('inherit');
  const search = surface.getByRole('tabpanel', { name: 'Tools' }).getByRole('searchbox');
  await search.fill('text query');
  await expect(tool).toBeHidden();
  await expect(surface.getByText('Find notes that match a text query.', { exact: true })).toBeVisible();
  await search.clear();
  await expect(policy).toHaveValue('inherit');
  const advanced = tool.locator('details');
  await advanced.getByText('Rules by mode', { exact: true }).click();
  await choose(advanced, 'Plan', 'Mode default');
  await expect(policy).toHaveValue('custom');
  const count = (await writes()).length;
  await save.click();
  await expect.poll(writes).toHaveLength(count + 1);
  expect((await writes()).at(-1)?.mutation).toMatchObject({
    kind: 'mcp', id: 'notes', authored: { definition: { tools: {
      read_note: { permissions: { plan: 'default', automatic: 'ask', full_access: 'allow' } },
    } } },
  });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(tool).toBeVisible();
  expect(await surface.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.screenshot({ path: info.outputPath('tool-approvals-narrow.png') });
  expect(errors).toEqual([]);
});

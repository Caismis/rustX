/** Issue #491 acceptance: the scoped Workflow file library and the React
 * Flow editor over the real native App Server and Product Host, with
 * isolated User and Workspace roots. No model request is ever made. */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page, type Locator } from '@playwright/test';
import { mkdirSync, readFileSync, readdirSync, utimesSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startDogfood } from './dogfood-server';
import { captureEvidence, expectStableScreenshot } from './screenshot';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { parse } from 'yaml';
import { COLLIDING_FIELDS } from '../workflow-colliding-document';

const examples = fileURLToPath(new URL('../../../examples/local-runtime/', import.meta.url));
const example = (path: string) => readFileSync(resolve(examples, path));

async function seed(fixture: Awaited<ReturnType<typeof startDogfood>>) {
  const global = join(fixture.directory, 'home/rustx/.agents');
  for (const directory of [join(global, 'workflows'), join(global, 'agents'), join(fixture.workspaceA, '.agents/workflows'), join(fixture.workspaceB, '.agents/workflows')]) mkdirSync(directory, { recursive: true });
  writeFileSync(join(global, 'agents/reviewer.toml'), example('workflow-templates/.agents/agents/reviewer.toml'));
  // The same identity in Global and both Workspaces stays three files.
  for (const root of [join(global, 'workflows'), join(fixture.workspaceA, '.agents/workflows'), join(fixture.workspaceB, '.agents/workflows')]) {
    writeFileSync(join(root, 'typed_agent.yaml'), example('workflow-templates/.agents/workflows/typed_agent.yaml'));
  }
  writeFileSync(join(fixture.workspaceA, '.agents/workflows/parallel_review.yaml'), example('.agents/workflows/parallel_review.yaml'));
  writeFileSync(join(fixture.workspaceA, '.agents/workflows/implement_and_review.yaml'), example('.agents/workflows/implement_and_review.yaml'));
  // Modification times are shown; references need one fixed instant.
  const instant = new Date('2026-10-21T12:42:00Z');
  for (const root of [join(global, 'workflows'), join(fixture.workspaceA, '.agents/workflows'), join(fixture.workspaceB, '.agents/workflows')]) {
    for (const name of readdirSync(root)) utimesSync(join(root, name), instant, instant);
  }
}

async function accessible(page: Page, scope: string) {
  const results = await new AxeBuilder({ page }).include(scope).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const serious = results.violations.filter(violation => violation.impact === 'serious' || violation.impact === 'critical');
  expect(serious.map(violation => `${violation.id}: ${violation.nodes.map(node => node.target.join(' ')).join(' | ')}`)).toEqual([]);
}

const editor = (page: Page) => page.getByRole('region', { name: 'Workflow canvas' });
const card = (page: Page, id: string) => page.getByRole('listitem').filter({ has: page.getByRole('button', { name: id, exact: true }) });

async function showIdentity(inspector: Locator) {
  const details = inspector.locator('details').filter({ has: inspector.page().getByText('Native identity and rename', { exact: true }) });
  if (!(await details.evaluate(element => (element as HTMLDetailsElement).open))) await details.locator('summary').click();
}
async function showSchemaJson(inspector: Locator, label: string) {
  const section = inspector.locator(`[data-schema-label="${label}"]`).first();
  const button = section.locator(':scope > h3').getByRole('button', { name: 'Advanced JSON', exact: true });
  if (await button.count()) await button.click();
}
async function showAdvanced(inspector: Locator) {
  const summary = inspector.locator('summary').filter({ hasText: /^Advanced settings$/ });
  if (await summary.count() && !(await summary.evaluate(element => (element.parentElement as HTMLDetailsElement).open))) await summary.click();
}


test('unknown output contracts hide out-of-scope variables and keep pending upstream choices', async ({ page }, testInfo) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  try {
    await seed(fixture);
    const empty = { type: 'object', properties: {}, additionalProperties: false };
    const tool = (schema: unknown) => ({ type: 'tool', selector: { origin: 'builtin', name: 'read' }, arguments: { type: 'reference', path: ['args'] }, result: { type: 'json', part: 0, schema } });
    const unknown = { type: 'object', unknown: true };
    writeFileSync(join(fixture.workspaceA, '.agents/workflows/scope_facts.yaml'), JSON.stringify({ description: 'Unknown schemas keep independent scope facts', block: {
      input: empty, output: empty,  nodes: {workflow_start:{type:'start'}, ...{
        common: { type: 'model', instruction: 'Return a value.', input: { value: { type: 'reference', path: ['args'] } }, output: { type: 'json_schema', schema: unknown } },
        gate: { type: 'branch', cases: [{ id: 'true', condition: { type: 'boolean', value: { type: 'literal', value: true } } }] },
        broken: tool(unknown), skip: { type: 'model', instruction: 'Return text.', output: { type: 'text' } }, join: tool({ type: 'string' }),
        done: { type: 'return', output: { type: 'reference', path: ['args'] } },
      }}, edges: [{from:'workflow_start',to:'common'}, ...[
        { from: 'common', to: 'gate' }, { from: 'gate', to: 'broken', port: 'true' }, { from: 'gate', to: 'skip', port: 'false' },
        { from: 'broken', to: 'join' }, { from: 'skip', to: 'join' }, { from: 'join', to: 'done' },
      ]],
    } }));
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await card(page, 'scope_facts').getByRole('button', { name: 'scope_facts', exact: true }).click();
    await expect(page).toHaveTitle(/rustX$/);
    const inspector = page.getByRole('complementary', { name: 'Inspector' });
    await editor(page).getByLabel('Model node common', { exact: true }).focus(); await editor(page).getByLabel('Model node common', { exact: true }).press('Enter');
    await inspector.getByRole('button', { name: 'Choose a variable', exact: true }).click();
    for (const producer of ['common', 'gate', 'broken', 'skip', 'join', 'done'])
      await expect(inspector.getByRole('button', { name: new RegExp(`^${producer} ·`) })).toHaveCount(0);
    await editor(page).getByLabel('Tool node join', { exact: true }).focus(); await editor(page).getByLabel('Tool node join', { exact: true }).press('Enter');
    await inspector.getByRole('button', { name: 'Choose a variable', exact: true }).click();
    for (const producer of ['broken', 'skip', 'join', 'done'])
      await expect(inspector.getByRole('button', { name: new RegExp(`^${producer} ·`) })).toHaveCount(0);
    const pending = inspector.getByRole('button', { name: /^common ·/ });
    await expect(pending).toBeEnabled(); await expect(pending).toContainText('Availability pending');
    await pending.click();
    await expect(inspector.getByRole('button', { name: 'Choose a variable', exact: true })).toHaveAttribute('aria-expanded', 'false');
    await expect(page.getByText('Unsaved changes', { exact: true }).first()).toBeVisible();
    await editor(page).getByLabel('Model node common', { exact: true }).focus(); await editor(page).getByLabel('Model node common', { exact: true }).press('Enter');
    await expect(inspector.getByRole('status')).toHaveText('The output declaration is incomplete or violates the Workflow schema contract.');
    await page.screenshot({ path: testInfo.outputPath('incomplete-output.png') });
    await editor(page).getByLabel('Model node skip', { exact: true }).focus(); await editor(page).getByLabel('Model node skip', { exact: true }).press('Enter');
    await expect(inspector).toContainText('Declared or derived by native candidate analysis.');
    await expect(inspector.locator('pre')).toContainText('"type": "string"');
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

test('unfinished input schemas hide self, forward and alternate branch outputs', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    await seed(fixture);
    const empty = { type: 'object', properties: {}, additionalProperties: false };
    const tool = () => ({ type: 'tool', selector: { origin: 'builtin', name: 'read' }, arguments: { type: 'reference', path: ['args'] }, result: { type: 'json', part: 0, schema: empty } });
    writeFileSync(join(fixture.workspaceA, '.agents/workflows/unfinished_input.yaml'), JSON.stringify({ description: 'Input types are separate from producer presence', block: {
      input: {}, output: empty,  nodes: {workflow_start:{type:'start'}, ...{ common: tool(), left: tool(), right: tool(), join: tool(),
        gate: { type: 'branch', cases: [{ id: 'true', condition: { type: 'boolean', value: { type: 'literal', value: true } } }] }, done: { type: 'return', output: { type: 'literal', value: {} } } }},
      edges: [{from:'workflow_start',to:'common'}, ...[{ from: 'common', to: 'gate' }, { from: 'gate', to: 'left', port: 'true' }, { from: 'gate', to: 'right', port: 'false' }, { from: 'left', to: 'join' }, { from: 'right', to: 'join' }, { from: 'join', to: 'done' }]],
    } }));
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await card(page, 'unfinished_input').getByRole('button', { name: 'unfinished_input', exact: true }).click();
    const inspector = page.getByRole('complementary', { name: 'Inspector' });
    await editor(page).getByLabel('Tool node common', { exact: true }).focus(); await editor(page).getByLabel('Tool node common', { exact: true }).press('Enter');
    await inspector.getByRole('button', { name: 'Choose a variable', exact: true }).click();
    for (const producer of ['common', 'left', 'right', 'join'])
      await expect(inspector.getByRole('button', { name: new RegExp(`^${producer} ·`) })).toHaveCount(0);
    await editor(page).getByLabel('Tool node join', { exact: true }).focus(); await editor(page).getByLabel('Tool node join', { exact: true }).press('Enter');
    await inspector.getByRole('button', { name: 'Choose a variable', exact: true }).click();
    for (const producer of ['left', 'right', 'join'])
      await expect(inspector.getByRole('button', { name: new RegExp(`^${producer} ·`) })).toHaveCount(0);
    await expect(inspector.getByRole('button', { name: /^common ·/ })).toBeEnabled();
    await expect(inspector.getByRole('button', { name: /^common ·/ })).toContainText('Available here');
    expect((await fixture.control('requests')).requests).toHaveLength(0); expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

test('equality picker rejects numeric operands and admits a matching string binding after editing the literal', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    await seed(fixture);
    const empty = { type: 'object', properties: {}, additionalProperties: false };
    writeFileSync(join(fixture.workspaceA, '.agents/workflows/equality_types.yaml'), JSON.stringify({ description: 'Native equality type necessities', block: {
      input: { type: 'object', properties: { count: { type: 'integer' }, text: { type: 'string' } }, required: ['count', 'text'], additionalProperties: false }, output: empty,
      nodes: {workflow_start:{type:'start'}, ...{ gate: { type: 'branch', cases: [{ id: 'true', condition: { type: 'equal', left: { type: 'reference', path: ['args', 'count'] }, right: { type: 'literal', value: 1 } } }] }, done: { type: 'return', output: { type: 'literal', value: {} } } }},
      edges: [{from:'workflow_start',to:'gate'}, ...[{ from: 'gate', to: 'done', port: 'true' }, { from: 'gate', to: 'done', port: 'false' }]],
    } }));
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await card(page, 'equality_types').getByRole('button', { name: 'equality_types', exact: true }).click();
    await editor(page).getByLabel('Branch node gate', { exact: true }).focus(); await editor(page).getByLabel('Branch node gate', { exact: true }).press('Enter');
    const inspector = page.getByRole('complementary', { name: 'Inspector' }), pick = inspector.getByRole('button', { name: 'Choose a variable', exact: true });
    await inspector.getByRole('button', { name: 'Advanced conditions', exact: true }).click();
    await pick.click();
    await expect(inspector.getByRole('button', { name: /^args \/ count ·/ })).toBeDisabled();
    await expect(inspector.getByRole('button', { name: /^args \/ text ·/ })).toBeDisabled();
    await pick.click();
    await inspector.getByLabel('Literal JSON', { exact: true }).fill('"known"'); await inspector.getByLabel('Literal JSON', { exact: true }).blur();
    await pick.click();
    await expect(inspector.getByRole('button', { name: /^args \/ count ·/ })).toBeDisabled();
    const text = inspector.getByRole('button', { name: /^args \/ text ·/ });
    await expect(text).toBeEnabled(); await expect(text).toContainText('Compatible type'); await text.click();
    await page.getByRole('button', { name: 'Check', exact: true }).first().click();
    await expect(inspector).toContainText('Structure check passed');
    expect((await fixture.control('requests')).requests).toHaveLength(0); expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

test('native array item choices preserve a common element contract and admit broader objects', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    await seed(fixture);
    const closed = (key: string) => ({ type: 'object', properties: { [key]: { type: 'string' } }, required: [key], additionalProperties: false });
    const ref = { type: 'reference', path: ['args', 'a'] };
    writeFileSync(join(fixture.workspaceA, '.agents/workflows/array_contract.yaml'), JSON.stringify({ description: 'Native array common element proof', block: {
      input: { type: 'object', properties: { a: closed('left'), b: closed('right'), open: { type: 'object' } }, required: ['a', 'b', 'open'], additionalProperties: false },
      output: { type: 'object', properties: { values: { type: 'array', items: { type: 'object' } } }, required: ['values'], additionalProperties: false },
      nodes: {workflow_start:{type:'start'}, ...{ done: { type: 'return', output: { type: 'object', fields: { values: { type: 'array', items: [ref, ref] } } } } }}, edges: [{from:'workflow_start',to:'done'}, ...[]],
    } }));
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await card(page, 'array_contract').getByRole('button', { name: 'array_contract', exact: true }).click();
    await editor(page).getByLabel('Return node done', { exact: true }).focus(); await editor(page).getByLabel('Return node done', { exact: true }).press('Enter');
    const inspector = page.getByRole('complementary', { name: 'Inspector' });
    await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(inspector).toContainText('Structure check passed');
    await editor(page).getByLabel('Return node done', { exact: true }).focus(); await editor(page).getByLabel('Return node done', { exact: true }).press('Enter');
    await inspector.getByRole('button', { name: 'Choose a variable', exact: true }).nth(1).click();
    await expect(inspector.getByRole('button', { name: /^args \/ a ·/ })).toBeEnabled();
    const incompatible = inspector.getByRole('button', { name: /^args \/ b ·/ });
    await expect(incompatible).toBeDisabled(); await expect(incompatible).toHaveAttribute('title', /constructed array element contracts are incompatible/);
    await inspector.getByRole('button', { name: /^args \/ open ·/ }).click();
    await page.getByRole('button', { name: 'Check', exact: true }).first().click(); await expect(inspector).toContainText('Structure check passed');
    expect((await fixture.control('requests')).requests).toHaveLength(0); expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

test('node rename cannot adopt a lone orphan title when the source has no metadata', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    await seed(fixture);
    const empty = { type: 'object', properties: {}, additionalProperties: false };
    const bytes = JSON.stringify({ description: 'Keep orphan authorship', block: { input: empty, output: empty,  nodes: {workflow_start:{type:'start'}, ...{ done: { type: 'return', output: { type: 'literal', value: {} } } }}, edges: [{from:'workflow_start',to:'done'}, ...[]] }, editor: { nodes: [{ address: { block: [], node: 'renamed' }, title: 'old deleted node title' }] } });
    const file = join(fixture.workspaceA, '.agents/workflows/orphan_node.yaml'); writeFileSync(file, bytes);
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await card(page, 'orphan_node').getByRole('button', { name: 'orphan_node', exact: true }).click();
    await editor(page).getByLabel('Return node done', { exact: true }).focus(); await editor(page).getByLabel('Return node done', { exact: true }).press('Enter');
    const inspector = page.getByRole('complementary', { name: 'Inspector' });
    await showIdentity(inspector);
    await inspector.getByLabel('Node ID', { exact: true }).fill('renamed'); await inspector.getByLabel('Node ID', { exact: true }).press('Enter');
    await expect(inspector.getByText('That ID is already used in this block.', { exact: true }).first()).toBeVisible();
    await expect(editor(page).getByLabel('Return node done', { exact: true })).toBeVisible();
    await expect(editor(page).getByLabel('Return node renamed', { exact: true })).toHaveCount(0);
    await expect(editor(page).getByText('old deleted node title', { exact: true })).toHaveCount(0);
    await showIdentity(inspector);
    await inspector.getByLabel('Node ID', { exact: true }).fill('done'); await inspector.getByLabel('Node ID', { exact: true }).press('Enter');
    await page.getByRole('button', { name: 'Check', exact: true }).first().click();
    await expect(inspector).toContainText('Structure check passed');
    await expect(inspector.getByText('editor.nodes.0', { exact: true })).toBeVisible();
    expect(readFileSync(file, 'utf8')).toBe(bytes);
    expect((await fixture.control('requests')).requests).toHaveLength(0); expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

test('closed, open and declared native destination fields keep distinct picker states', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    await seed(fixture);
    const input = { type: 'object', properties: { x: { type: 'string' } }, required: ['x'], additionalProperties: false };
    const files = (['closed', 'open', 'declared'] as const).map(kind => {
      const id = `field_${kind}`, file = join(fixture.workspaceA, `.agents/workflows/${id}.yaml`);
      const output = kind === 'declared' ? input : { type: 'object', properties: {}, additionalProperties: kind === 'open' };
      const bytes = JSON.stringify({ description: 'Destination property constraints', block: { input, output,  nodes: {workflow_start:{type:'start'}, ...{ done: { type: 'return', output: { type: 'object', fields: { x: { type: 'reference', path: ['args', 'x'] } } } } }}, edges: [{from:'workflow_start',to:'done'}, ...[]] } });
      writeFileSync(file, bytes); return { kind, id, file, bytes };
    });
    await routeWorkspaceHost(page, fixture); await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    for (const { kind, id, file, bytes } of files) {
      await card(page, id).getByRole('button', { name: id, exact: true }).click();
      await editor(page).getByLabel('Return node done', { exact: true }).focus(); await editor(page).getByLabel('Return node done', { exact: true }).press('Enter');
      const inspector = page.getByRole('complementary', { name: 'Inspector' });
      await inspector.getByRole('button', { name: 'Choose a variable', exact: true }).click();
      const choice = inspector.getByRole('button', { name: /^args \/ x ·/ });
      if (kind === 'closed') { await expect(choice).toBeDisabled(); await expect(choice).toHaveAttribute('title', /additionalProperties: false/); }
      else { await expect(choice).toBeEnabled(); await expect(choice).toContainText(kind === 'open' ? 'Compatibility unknown' : 'Compatible type'); }
      expect(readFileSync(file, 'utf8')).toBe(bytes);
      await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();
    }
    expect((await fixture.control('requests')).requests).toHaveLength(0); expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

test('incomplete candidate variable picking and file-owned keyboard positions preserve notes through save and copy', async ({ page }, info) => {
  const fixture = await startDogfood('web_console_dogfood');
  const rpcErrors: unknown[] = [];
  page.on('websocket', socket => socket.on('framereceived', event => {
    try { const message = JSON.parse(event.payload.toString()); if (message.error) rpcErrors.push(message.error); } catch {}
  }));
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    await seed(fixture);
    const file = join(fixture.workspaceA, '.agents/workflows/metadata_flow.yaml');
    const notes = [{ id: 'note', block: [], text: '# Author note\n\n**Keep this**', theme: 'blue', position: { x: 0, y: 100 }, dimensions: { width: 240, height: 180 } }];
    const empty = { type: 'object', properties: {}, additionalProperties: false };
    writeFileSync(file, JSON.stringify({ description: 'Candidate with file metadata', block: {
      input: empty, output: { type: 'object', properties: { summary: { type: 'string', description: 'Model-visible summary' } }, required: ['summary'], additionalProperties: false },
       nodes: {workflow_start:{type:'start'}, ...{
        text: { type: 'model', instruction: '', output: { type: 'text' } },
        done: { type: 'return', output: { type: 'object', fields: { summary: { type: 'reference', path: ['text'] } } } },
      }}, edges: [{from:'workflow_start',to:'text'}, ...[{ from: 'text', to: 'done' }]],
    }, editor: { notes, nodes: [{ address: { block: [], node: 'text' }, position: { x: 0, y: 0 } }] } }));
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token); await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await card(page, 'metadata_flow').getByRole('button', { name: 'metadata_flow', exact: true }).click();
    await editor(page).getByLabel('Return node done', { exact: true }).focus(); await editor(page).getByLabel('Return node done', { exact: true }).press('Enter');
    const inspector = page.getByRole('complementary', { name: 'Inspector' });
    await inspector.getByRole('button', { name: 'Choose a variable', exact: true }).click();
    await inspector.getByRole('textbox', { name: 'Search reference paths…' }).fill('text');
    await expect(inspector.getByRole('button', { name: 'text · Available here · Compatible type', exact: true })).toBeEnabled();
    await inspector.getByRole('button', { name: 'text · Available here · Compatible type', exact: true }).click();
    // Declarations and lexical proof were usable before the Model instruction
    // was complete. Save still requires the ordinary native compiler rules.
    const text = editor(page).getByLabel('Model node text', { exact: true });
    // The Return inspector pans that card into view. Locate the Model through
    // keyboard focus before proving its ordinary pointer interaction again.
    await text.focus(); await text.press('Enter');
    await text.click(); await inspector.getByLabel('Instruction', { exact: true }).fill('Produce the summary.');
    await text.focus(); await page.keyboard.press('ArrowRight');
    await expect(page.getByText('Unsaved changes', { exact: true }).first()).toBeVisible();
    await page.getByRole('button', { name: 'Save File', exact: true }).click();
    await expect.poll(() => parse(readFileSync(file, 'utf8')).editor.nodes.find((node: any) => node.address.node === 'text')?.position.x).toBe(5);
    // A committed file can precede the save acknowledgement and observed
    // reread. Navigation must wait for the editor to settle that receipt.
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    const saved = parse(readFileSync(file, 'utf8'));
    expect(saved.editor.notes).toEqual(notes);
    expect(saved.block.output.properties.summary.description).toBe('Model-visible summary');
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();
    await page.getByRole('button', { name: 'Actions for metadata_flow.yaml' }).click();
    await page.getByRole('menuitem', { name: 'Copy File' }).click();
    const dialog = page.getByRole('dialog', { name: 'Copy Workflow file' });
    await dialog.getByLabel('Workflow ID').fill('metadata_copy'); await dialog.getByRole('button', { name: 'Copy', exact: true }).click();
    await expect(card(page, 'metadata_copy')).toBeVisible();
    expect(parse(readFileSync(join(fixture.workspaceA, '.agents/workflows/metadata_copy.yaml'), 'utf8')).editor).toEqual(saved.editor);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    expect(errors).toEqual([]);
  } finally {
    if (rpcErrors.length) await info.attach('workflow-rpc-errors', { body: JSON.stringify(rpcErrors, null, 2), contentType: 'application/json' });
    await fixture.stop(false);
  }
});

test('conflicts keep edits, invalid files stay preserved, and leaving asks first', async ({ page }, info) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    await seed(fixture);
    const broken = join(fixture.workspaceA, '.agents/workflows/broken.yaml');
    writeFileSync(broken, 'description: [');
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    // An invalid file is listed with its native diagnostic and never replaced.
    await expect(card(page, 'broken')).toContainText('Invalid file');
    await card(page, 'broken').getByRole('button', { name: 'broken', exact: true }).click();
    await expect(page.getByText('This file is preserved as is and cannot be edited here.')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    expect(readFileSync(broken, 'utf8')).toBe('description: [');
    await captureEvidence(page, '20-preserved-invalid-file', info);
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();

    // The editor screen shows the real Start and its concurrent next steps.
    await card(page, 'parallel_review').getByRole('button', { name: 'parallel_review', exact: true }).click();
    await editor(page).getByLabel('Workflow Start', { exact: true }).focus(); await editor(page).getByLabel('Workflow Start', { exact: true }).press('Enter');
    await expect(page.getByRole('complementary', { name: 'Inspector' })).toContainText('Add parallel node');
    await expectStableScreenshot(page.locator('[data-workflows-level="editor"]'), 'workflow-editor.png');
    await accessible(page, '[data-workflows-level="editor"] header');
    await accessible(page, '[aria-label="Inspector"]');

    // A producer rename rewrites the structured references at the join.
    await editor(page).getByLabel('Agent node reviews_quality_refine', { exact: true }).focus();
    await editor(page).getByLabel('Agent node reviews_quality_refine', { exact: true }).press('Enter');
    const inspector = page.getByRole('complementary', { name: 'Inspector' });
    await showIdentity(inspector);
    await inspector.getByLabel('Node ID', { exact: true }).fill('clarity');
    await inspector.getByLabel('Node ID', { exact: true }).press('Enter');
    await expect(editor(page).getByLabel('Branch node decision')).toContainText('clarity.acceptable');

    // An external edit since opening is a conflict; local edits are kept.
    const path = join(fixture.workspaceA, '.agents/workflows/parallel_review.yaml');
    writeFileSync(path, `${readFileSync(path, 'utf8')}\n# external edit\n`);
    await page.getByRole('button', { name: 'Save File', exact: true }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'The file changed on disk since it was opened' })).toBeVisible();
    await expect(editor(page).getByLabel('Branch node decision')).toContainText('clarity.acceptable');
    expect(readFileSync(path, 'utf8')).toContain('# external edit');
    await captureEvidence(page, '21-source-conflict', info);

    // Leaving with unsaved edits asks first; Keep editing stays on the file.
    await page.getByRole('button', { name: 'New Conversation', exact: true }).first().click();
    const leave = page.getByRole('dialog', { name: 'Discard unsaved changes?' });
    await expect(leave).toBeVisible();
    await leave.getByRole('button', { name: 'Keep editing', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'parallel_review', exact: true })).toBeVisible();
    // Closing every conversation view is also a navigation away from this editor.
    await page.getByRole('button', { name: 'View options', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Close all views', exact: true }).click();
    await expect(leave).toBeVisible();
    await leave.getByRole('button', { name: 'Keep editing', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'parallel_review', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'View options', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Close all views', exact: true }).click();
    await leave.getByRole('button', { name: 'Discard and leave', exact: true }).click();
    await expect(page.getByLabel('Message', { exact: true })).toBeVisible();
    expect(readFileSync(path, 'utf8')).toContain('# external edit');

    // Narrow viewport: the inspector becomes a bottom sheet over the canvas.
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await card(page, 'parallel_review').getByRole('button', { name: 'parallel_review', exact: true }).click();
    await editor(page).getByLabel('Branch node decision').focus(); await editor(page).getByLabel('Branch node decision').press('Enter');
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole('complementary', { name: 'Inspector' })).toBeVisible();
    await expect(page.getByRole('complementary', { name: 'Inspector' })).toHaveCSS('position', 'absolute');
    await expectStableScreenshot(page, 'workflow-editor-narrow.png');
  } finally { await fixture.stop(false); }
});

const FLOATS = `description: Floats a browser would retype.
block:
  input: {type: object, properties: {}, additionalProperties: false}
  output: {type: object, properties: {values: {type: array, items: {type: number}}}, required: [values], additionalProperties: false}
  nodes:
    start: {type: start}
    done:
      type: return
      output: {type: object, fields: {values: {type: array, items: [{type: literal, value: 1.0}, {type: literal, value: 1.5}]}}}
  edges: [{from: start, to: done}]
`;

test('keyboard selection, node-move undo, focused-field save and the browser number contract', async ({ page }, testInfo) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  try {
    await seed(fixture);
    const floats = join(fixture.workspaceA, '.agents/workflows/floats.yaml');
    writeFileSync(floats, FLOATS);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();

    // A natively valid file whose numbers the browser would retype (1.0
    // becomes 1) opens read-only, located at the exact number.
    await expect(card(page, 'floats')).toContainText('cannot preserve this number exactly');
    await expect(card(page, 'floats')).toContainText('block.nodes.done.output.fields.values.items.0.value');
    await card(page, 'floats').getByRole('button', { name: 'floats', exact: true }).click();
    await expect(page.getByText('This file is preserved as is and cannot be edited here.')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    expect(readFileSync(floats, 'utf8')).toBe(FLOATS);
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();

    // Keyboard: focus an existing node, activate it, edit it in the inspector.
    await card(page, 'parallel_review').getByRole('button', { name: 'parallel_review', exact: true }).click();
    const ready = editor(page).getByLabel('Return node ready');
    await ready.focus();
    await page.keyboard.press('Enter');
    const inspector = page.getByRole('complementary', { name: 'Inspector' });
    await showIdentity(inspector);
    await expect(inspector.getByLabel('Node ID')).toHaveValue('ready');
    // Move before renaming: the new identity must keep the authored layout,
    // and Undo/Redo must carry it back and forward instead of relaying out.
    const initial = (await ready.boundingBox())!;
    await page.mouse.move(initial.x + 40, initial.y + 20);
    await page.mouse.down();
    await page.mouse.move(initial.x - 20, initial.y + 100, { steps: 8 });
    await page.mouse.up();
    const authored = (await ready.boundingBox())!;
    expect(Math.round(authored.y - initial.y)).toBeGreaterThan(40);
    await showIdentity(inspector);
    await inspector.getByLabel('Node ID').fill('accepted');
    await showIdentity(inspector);
    await inspector.getByLabel('Node ID').press('Enter');
    await expect(editor(page).getByLabel('Return node accepted')).toBeVisible();
    await expect.poll(async () => Math.round((await editor(page).getByLabel('Return node accepted').boundingBox())!.y)).toBe(Math.round(authored.y));

    // One completed drag is one document step: Undo reverts it before the
    // older rename. The complete document reaches the file only on Save.
    const decision = editor(page).getByLabel('Branch node decision');
    const start = (await decision.boundingBox())!;
    await page.mouse.move(start.x + 40, start.y + 20);
    await page.mouse.down();
    await page.mouse.move(start.x + 140, start.y + 140, { steps: 8 });
    await page.mouse.up();
    const moved = (await decision.boundingBox())!;
    expect(Math.round(moved.y - start.y)).toBeGreaterThan(60);
    await page.keyboard.press('Control+z');
    await expect.poll(async () => Math.round((await decision.boundingBox())!.y)).toBe(Math.round(start.y));
    await expect(editor(page).getByLabel('Return node accepted')).toBeVisible();
    await page.keyboard.press('Control+z');
    await expect(editor(page).getByLabel('Return node ready')).toBeVisible();
    await expect.poll(async () => Math.round((await ready.boundingBox())!.y)).toBe(Math.round(authored.y));
    await page.keyboard.press('Control+Shift+z');
    await expect(editor(page).getByLabel('Return node accepted')).toBeVisible();
    await expect.poll(async () => Math.round((await editor(page).getByLabel('Return node accepted').boundingBox())!.y)).toBe(Math.round(authored.y));
    await page.keyboard.press('Control+Shift+z');
    await expect.poll(async () => Math.round((await decision.boundingBox())!.y)).toBe(Math.round(moved.y));
    await editor(page).getByRole('toolbar', { name: 'Canvas tools' }).getByRole('button', { name: 'Fit view', exact: true }).click();
    await expect.poll(async () => {
      const node = (await editor(page).getByLabel('Return node accepted').boundingBox())!;
      const canvas = (await editor(page).boundingBox())!;
      return node.x >= canvas.x && node.y >= canvas.y
        && node.x + node.width <= canvas.x + canvas.width && node.y + node.height <= canvas.y + canvas.height;
    }).toBe(true);
    await page.screenshot({ path: testInfo.outputPath('workflow-renamed-layout.png') });

    // Ctrl/Cmd+S in a focused JSON field saves exactly the visible text.
    const path = join(fixture.workspaceA, '.agents/workflows/parallel_review.yaml');
    const original = readFileSync(path, 'utf8');
    await page.getByRole('button', { name: 'Block settings', exact: true }).click();
    await showSchemaJson(inspector, 'Output schema');
    const input = inspector.getByLabel('Output schema');
    const contract = parse(original).block.output;
    await input.fill(JSON.stringify({ ...contract, properties: { ...contract.properties, rubric: { type: 'string' } } }, null, 2));
    await expect(input).toBeFocused();
    await page.keyboard.press('Control+s');
    await expect.poll(() => readFileSync(path, 'utf8')).toContain('rubric');
    expect(readFileSync(path, 'utf8')).toContain('accepted');
    await expect(page.getByRole('status').filter({ hasText: 'No unsaved changes' })).toBeVisible();

    // A number the browser would round is refused in the field; nothing is
    // written and the visible text stays protected.
    const saved = readFileSync(path, 'utf8');
    expect(saved).not.toBe(original);
    await input.fill('{"type": "object", "properties": {"proposal": {"type": "string", "const": 9007199254740993}}, "additionalProperties": false}');
    await page.keyboard.press('Control+s');
    await expect(inspector.getByText('This number would change in the browser editor', { exact: false })).toBeVisible();
    await expect(input).toHaveValue(/9007199254740993/);
    expect(readFileSync(path, 'utf8')).toBe(saved);
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Discard unsaved changes?' })).toBeVisible();
    expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

test('object keys spelled like paths stay separate fields, and rejected field text is kept unsent', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  try {
    await seed(fixture);
    // JSON is YAML: the very document the component tests use.
    const path = join(fixture.workspaceA, '.agents/workflows/colliding.yaml');
    writeFileSync(path, JSON.stringify(COLLIDING_FIELDS, null, 2));
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await card(page, 'colliding').getByRole('button', { name: 'colliding', exact: true }).click();

    // Native accepts the structure; dependencies are a separate fact.
    await page.getByRole('button', { name: 'Check', exact: true }).click();
    const inspector = page.getByRole('complementary', { name: 'Inspector' });
    await expect(inspector.getByText('Structure check passed').first()).toBeVisible();

    const done = editor(page).getByLabel('Return node done');
    await done.focus();
    await page.keyboard.press('Enter');
    // Each value editor is the group its authored key labels.
    await expect(inspector.getByLabel('Literal JSON')).toHaveCount(2);
    const direct = inspector.getByRole('group', { name: 'a.fields.b', exact: true }).getByLabel('Literal JSON');
    const nested = inspector.getByRole('group', { name: 'a', exact: true }).getByRole('group', { name: 'b', exact: true }).getByLabel('Literal JSON');
    await expect(direct).toHaveValue('1');
    await expect(nested).toHaveValue('2');
    // Negative zero is refused in the field before anything is written.
    await direct.fill('-0');
    await page.keyboard.press('Control+s');
    await expect(inspector.getByText('This number would change in the browser editor', { exact: false })).toBeVisible();
    await expect(nested).toHaveValue('2');
    expect(parse(readFileSync(path, 'utf8'))).toEqual(COLLIDING_FIELDS);
    await direct.fill('3');
    await expect(nested).toHaveValue('2');
    await page.keyboard.press('Control+s');
    await expect.poll(() => parse(readFileSync(path, 'utf8')).block.nodes.done.output.fields).toEqual({
      'a.fields.b': { type: 'literal', value: 3 },
      a: { type: 'object', fields: { b: { type: 'literal', value: 2 } } },
    });
    await expect(page.getByRole('status').filter({ hasText: 'No unsaved changes' })).toBeVisible();
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();

    // A rename to a key the node already has is refused; its text and error
    // are kept, and Save sends nothing.
    await card(page, 'parallel_review').getByRole('button', { name: 'parallel_review', exact: true }).click();
    const reviews = editor(page).getByLabel('Agent node reviews_quality_refine', { exact: true });
    await reviews.focus();
    await page.keyboard.press('Enter');
    await showIdentity(inspector);
    const key = inspector.getByLabel('Node ID', { exact: true });
    const review = join(fixture.workspaceA, '.agents/workflows/parallel_review.yaml');
    const original = readFileSync(review, 'utf8');
    await key.fill('reviews_security_review');
    await page.keyboard.press('Control+s');
    await expect(inspector.getByText('That ID is already used in this block.')).toBeVisible();
    await expect(key).toHaveValue('reviews_security_review');
    await expect(page.getByRole('list', { name: 'Fields with unapplied text' })).toContainText('reviews_quality_refine');
    expect(readFileSync(review, 'utf8')).toBe(original);
    expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

/** Issue #496: one Model node, one native output union, real source CAS and
 * Rust checking. Model generation itself is covered by native adapter tests. */
test('new Model → Text → check/save/reopen → default and custom JSON Schema → check/save/reopen', async ({ page }) => {
  const fixture = await startDogfood('tui_multi_session');
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  const inspector = page.getByRole('complementary', { name: 'Inspector' });
  const file = join(fixture.workspaceA, '.agents/workflows/model_call.yaml');
  const connectNext = async (target: string) => {
    await inspector.getByRole('button', { name: /^(Choose next node for|Add parallel node on) Next step$/ }).click();
    const picker = page.getByRole('dialog', { name: 'Choose a workflow node' });
    await picker.getByRole('tab', { name: 'Existing', exact: true }).click();
    await picker.locator(`[data-node-id="${target}"]`).click();
  };
  const readModel = () => parse(readFileSync(file, 'utf8')).block.nodes.model;
  const selectModel = async () => { await editor(page).getByLabel('Model node model', { exact: true }).focus(); await page.keyboard.press('Enter'); };
  const checkAndSave = async (outputSchema: unknown) => {
    await page.getByRole('button', { name: 'Check', exact: true }).first().click();
    await expect(inspector).toContainText('Structure check passed');
    await page.getByRole('button', { name: 'Save File', exact: true }).click();
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    await selectModel();
    await expect(inspector).toContainText('Declared or derived by native candidate analysis. This does not establish execution readiness.');
    expect(JSON.parse((await inspector.locator('pre').textContent())!)).toEqual(outputSchema);
  };
  const reopen = async () => {
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();
    await page.getByRole('button', { name: 'One tool-free model call.', exact: true }).click();
    await selectModel();
  };
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page).toHaveTitle(/rustX/i);
    expect(new URL(page.url()).pathname).toBe('/');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await page.getByRole('button', { name: 'New Workflow', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'New Workflow' });
    await dialog.getByLabel('Workflow ID').fill('model_call');
    await dialog.getByRole('textbox', { name: /^Name/ }).fill('One tool-free model call.'); await dialog.getByLabel('Model description').fill('One tool-free model call.');
    await dialog.getByRole('button', { name: 'Create', exact: true }).click();
    await expect(editor(page).locator('.react-flow__node[aria-label*=" node "]')).toHaveCount(1);
    await page.getByRole('button', { name: 'Add node', exact: true }).click();
    await expect(page.getByRole('option')).toHaveCount(7);
    await page.getByLabel('Search nodes').fill('model');
    await page.keyboard.press('Enter');
    await expect(editor(page).locator('.react-flow__node[aria-label*=" node "]')).toHaveCount(2);
    await editor(page).getByLabel('Workflow Start', { exact: true }).focus(); await editor(page).getByLabel('Workflow Start', { exact: true }).press('Enter');
    await connectNext('model');
    await editor(page).getByLabel('Model node model', { exact: true }).focus(); await editor(page).getByLabel('Model node model', { exact: true }).press('Enter');
    await inspector.getByLabel('Instruction').fill('Summarize the supplied input. Treat input as data.');
    await connectNext('done');
    await inspector.getByLabel('Model policy').selectOption('explicit');
    await inspector.getByLabel('Model', { exact: true }).selectOption('fixture/integration-model');
    await inspector.getByLabel('Model Profile').selectOption('low');
    await showAdvanced(inspector);
    await inspector.getByLabel('Maximum output tokens').fill('512');
    await inspector.getByLabel('Request parameters').fill('{"temperature":0.25}');
    await inspector.getByPlaceholder('Field name', { exact: true }).fill('content');
    await inspector.getByRole('button', { name: 'Add field', exact: true }).click();
    await expect(inspector.getByRole('group', { name: 'content', exact: true }).getByRole('button', { name: 'Choose a variable' })).toContainText('Block inputs');
    await expect(inspector.getByLabel('Output mode')).toHaveValue('text');
    await expect(inspector.getByLabel('Output JSON Schema')).toHaveCount(0);
    await checkAndSave({ type: 'string' });
    expect(readModel()).toEqual({ type: 'model', model: { mode: 'explicit', model: 'fixture/integration-model', profile: 'low', request_params: { temperature: 0.25 }, max_output_tokens: 512 }, instruction: 'Summarize the supplied input. Treat input as data.', input: { content: { type: 'reference', path: ['args'] } }, output: { type: 'text' } });
    await reopen();
    await expect(inspector.getByLabel('Model Profile')).toHaveValue('low');
    await showAdvanced(inspector);
    await expect(inspector.getByLabel('Maximum output tokens')).toHaveValue('512');
    await expect(inspector.getByText('Loading registered choices…', { exact: true })).toHaveCount(0);
    await page.screenshot({ path: '/tmp/rustx-model-editor-config.png', fullPage: false, animations: 'disabled' });
    await expect(inspector.getByLabel('Output mode')).toHaveValue('text');
    await page.screenshot({ path: '/tmp/rustx-model-editor-text.png', fullPage: false, animations: 'disabled' });
    await inspector.getByLabel('Output mode').selectOption('json_schema');
    // The untouched default must pass the same native strict schema check as
    // an authored schema; the downstream Return accepts either Model output.
    const defaultSchema = { type: 'object', properties: {}, required: [], additionalProperties: false };
    await showSchemaJson(inspector, 'Output JSON Schema');
    await expect(inspector.getByLabel('Output JSON Schema')).toHaveValue(JSON.stringify(defaultSchema, null, 2));
    await checkAndSave(defaultSchema);
    expect(readModel().output).toEqual({ type: 'json_schema', schema: defaultSchema });
    await page.screenshot({ path: '/tmp/rustx-model-editor-default-json-schema.png', fullPage: false, animations: 'disabled' });
    await reopen();
    await showSchemaJson(inspector, 'Output JSON Schema');
    await expect(inspector.getByLabel('Output JSON Schema')).toHaveValue(JSON.stringify(defaultSchema, null, 2));
    const schema = { type: 'object', properties: { risk: { type: 'string', enum: ['low', 'high'] } }, required: ['risk'], additionalProperties: false };
    await showSchemaJson(inspector, 'Output JSON Schema');
    await inspector.getByLabel('Output JSON Schema').fill(JSON.stringify(schema, null, 2));
    // Check commits the focused JSON draft before dispatching its exact
    // candidate; save/reopen must preserve that same declared output type.
    await checkAndSave(schema);
    expect(readModel().output).toEqual({ type: 'json_schema', schema });
    await reopen();
    await expect(inspector.getByLabel('Output mode')).toHaveValue('json_schema');
    await showSchemaJson(inspector, 'Output JSON Schema');
    await expect(inspector.getByLabel('Output JSON Schema')).toHaveValue(JSON.stringify(schema, null, 2));
    await accessible(page, '[data-workflows-level="editor"]');
    await page.screenshot({ path: '/tmp/rustx-model-editor-json-schema.png', fullPage: false, animations: 'disabled' });
    await page.setViewportSize({ width: 900, height: 900 });
    await expect(inspector.getByLabel('Output mode')).toBeVisible();
    await showSchemaJson(inspector, 'Output JSON Schema');
    await expect(inspector.getByLabel('Output JSON Schema')).toBeVisible();
    await page.screenshot({ path: '/tmp/rustx-model-editor-narrow.png', fullPage: false, animations: 'disabled' });
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect((await fixture.control('requests')).requests).toHaveLength(0);
    expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

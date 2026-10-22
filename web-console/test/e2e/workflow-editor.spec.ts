/** Issue #491 acceptance: the scoped Workflow file library and the React
 * Flow editor over the real native App Server and Product Host, with
 * isolated User and Workspace roots. No model request is ever made. */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { existsSync, mkdirSync, readFileSync, readdirSync, utimesSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startDogfood } from './dogfood-server';
import { expectStableScreenshot } from './screenshot';
import { connectRemote } from './shell-actions';
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

test('library → new → nested editor → check → save → reopen → copy → delete', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    await seed(fixture);
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    // One shell entry; conversation navigation is otherwise unchanged.
    const nav = page.getByRole('navigation', { name: 'Global panels' });
    await expect(nav.getByRole('button')).toHaveCount(1);
    await nav.getByRole('button', { name: 'Workflows', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Workflows', exact: true })).toBeVisible();
    await expect(page.getByRole('tab', { name: 'Workspace', exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(card(page, 'parallel_review')).toContainText('.agents/workflows/parallel_review.yaml');
    await expect(card(page, 'typed_agent')).toContainText('Overrides Global');
    await expect(page.getByText('Workspace A · 3 Workflow files')).toBeVisible();
    await expectStableScreenshot(page.locator('[data-workflows-level="library"]'), 'workflow-library.png');
    await accessible(page, '[data-workflows-level="library"]');

    // Exact-scope listing: Workspace B and Global each own one file.
    await page.getByRole('button', { name: 'Choose workspace (current: Workspace A)' }).click();
    await page.getByRole('option', { name: /Workspace B/ }).click();
    await expect(page.getByText('Workspace B · 1 Workflow file')).toBeVisible();
    await page.getByRole('tab', { name: 'Global', exact: true }).click();
    await expect(page.getByText('Global Workflows · 1 Workflow file')).toBeVisible();
    await expect(card(page, 'typed_agent')).not.toContainText('Overrides Global');
    await page.getByRole('tab', { name: 'Workspace', exact: true }).click();
    await expect(page.getByText('Workspace B · 1 Workflow file')).toBeVisible();
    await page.getByRole('button', { name: 'Choose workspace (current: Workspace B)' }).click();
    await page.getByRole('option', { name: /Workspace A/ }).click();

    // New shows scope and target first; Cancel writes nothing.
    await page.getByRole('button', { name: 'New Workflow', exact: true }).click();
    let dialog = page.getByRole('dialog', { name: 'New Workflow' });
    await dialog.getByLabel('Workflow ID').fill('triage');
    await dialog.getByLabel('Description').fill('Triage one issue.');
    await expect(dialog).toContainText('.agents/workflows/triage.yaml');
    await expect(dialog).toContainText('Workspace A');
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    expect(existsSync(join(fixture.workspaceA, '.agents/workflows/triage.yaml'))).toBe(false);
    await page.getByRole('button', { name: 'New Workflow', exact: true }).click();
    dialog = page.getByRole('dialog', { name: 'New Workflow' });
    await dialog.getByLabel('Workflow ID').fill('parallel_review');
    await expect(dialog).toContainText('A file with this ID already exists in this scope.');
    await dialog.getByLabel('Workflow ID').fill('triage');
    await dialog.getByLabel('Description').fill('Triage one issue.');
    await dialog.getByRole('button', { name: 'Create', exact: true }).click();
    // The exact new file opens in the editor.
    await expect(page.getByRole('heading', { name: 'triage', exact: true })).toBeVisible();
    await expect(editor(page).locator('.react-flow__node')).toHaveCount(1);
    expect(readFileSync(join(fixture.workspaceA, '.agents/workflows/triage.yaml'), 'utf8')).toContain('Triage one issue.');

    // Keyboard: the contextual palette adds a Parallel node.
    await page.getByRole('button', { name: 'Add node', exact: true }).focus();
    await page.keyboard.press('Enter');
    await page.getByLabel('Search nodes').fill('par');
    await page.keyboard.press('Enter');
    await expect(editor(page).locator('.react-flow__node')).toHaveCount(2);
    const inspector = page.getByRole('complementary', { name: 'Inspector' });
    await expect(inspector).toContainText('parallel');
    // Nested lexical block: edit one block at a time, keep the parent.
    await inspector.getByRole('button', { name: /Edit subgraph/ }).click();
    const crumbs = page.getByRole('navigation', { name: 'Block path' });
    await expect(crumbs).toContainText('parallel / main');
    await expect(editor(page).locator('.react-flow__node')).toHaveCount(1);
    await crumbs.getByRole('button', { name: 'Root block', exact: true }).click();
    await expect(editor(page).locator('.react-flow__node')).toHaveCount(2);

    // Native check reports the unconnected node; nothing is rewired.
    await page.getByRole('button', { name: 'Check', exact: true }).click();
    await expect(inspector).toContainText('Structure check failed');
    // Undo the addition, edit the description, check, save.
    await page.locator('section[aria-labelledby="workflow-editor-title"] .react-flow__pane').click();
    await page.keyboard.press('Control+z');
    await expect(editor(page).locator('.react-flow__node')).toHaveCount(1);
    await page.getByRole('button', { name: 'Block settings', exact: true }).click();
    await page.getByRole('complementary', { name: 'Inspector' }).getByLabel('Description').fill('Triage one incoming issue.');
    await expect(page.getByText('Unsaved changes', { exact: true }).first()).toBeVisible();
    await page.getByRole('button', { name: 'Check', exact: true }).click();
    await expect(page.getByRole('complementary', { name: 'Inspector' })).toContainText('Structure check passed');
    await page.getByRole('button', { name: 'Save File', exact: true }).click();
    await expect(page.getByText('No unsaved changes', { exact: true })).toBeVisible();
    expect(readFileSync(join(fixture.workspaceA, '.agents/workflows/triage.yaml'), 'utf8')).toContain('Triage one incoming issue.');

    // Reopen from the library reads the saved file.
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();
    await expect(card(page, 'triage')).toContainText('Triage one incoming issue.');
    await card(page, 'triage').getByRole('button', { name: 'triage', exact: true }).click();
    await page.getByRole('button', { name: 'Block settings', exact: true }).click();
    await expect(page.getByRole('complementary', { name: 'Inspector' }).getByLabel('Description')).toHaveValue('Triage one incoming issue.');
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();

    // Copy File: same scope, new ID, source revision guarded.
    await page.getByRole('button', { name: 'Actions for triage.yaml' }).click();
    await page.getByRole('menuitem', { name: 'Copy File' }).click();
    dialog = page.getByRole('dialog', { name: 'Copy Workflow file' });
    await dialog.getByLabel('Workflow ID').fill('triage_copy');
    await dialog.getByRole('button', { name: 'Copy', exact: true }).click();
    await expect(card(page, 'triage_copy')).toBeVisible();
    expect(readFileSync(join(fixture.workspaceA, '.agents/workflows/triage_copy.yaml'), 'utf8')).toContain('Triage one incoming issue.');

    // Delete: exact file at its revision; the Workspace override warns.
    await page.getByRole('button', { name: 'Actions for typed_agent.yaml' }).click();
    await page.getByRole('menuitem', { name: 'Delete' }).click();
    dialog = page.getByRole('dialog', { name: 'Delete Workflow file' });
    await expect(dialog).toContainText('After deletion the Global Workflow applies in this Workspace');
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await page.getByRole('button', { name: 'Actions for triage_copy.yaml' }).click();
    await page.getByRole('menuitem', { name: 'Delete' }).click();
    await page.getByRole('dialog', { name: 'Delete Workflow file' }).getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(card(page, 'triage_copy')).toHaveCount(0);
    expect(existsSync(join(fixture.workspaceA, '.agents/workflows/triage_copy.yaml'))).toBe(false);
    expect(existsSync(join(fixture.directory, 'home/rustx/.agents/workflows/typed_agent.yaml'))).toBe(true);
    expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

test('conflicts keep edits, invalid files stay preserved, and leaving asks first', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  try {
    await seed(fixture);
    const broken = join(fixture.workspaceA, '.agents/workflows/broken.yaml');
    writeFileSync(broken, 'description: [');
    await routeWorkspaceHost(page, fixture);
    await page.goto('/'); await connectRemote(page, fixture.endpoint, fixture.token);
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    // An invalid file is listed with its native diagnostic and never replaced.
    await expect(card(page, 'broken')).toContainText('Invalid file');
    await card(page, 'broken').getByRole('button', { name: 'broken', exact: true }).click();
    await expect(page.getByText('This file is preserved as is and cannot be edited here.')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Save File', exact: true })).toBeDisabled();
    expect(readFileSync(broken, 'utf8')).toBe('description: [');
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).click();

    // The approved editor screen: parallel_review, Parallel node selected.
    await card(page, 'parallel_review').getByRole('button', { name: 'parallel_review', exact: true }).click();
    await editor(page).getByLabel('Parallel node reviews').click();
    await expect(page.getByRole('complementary', { name: 'Inspector' })).toContainText('Parallel branches');
    await expectStableScreenshot(page.locator('[data-workflows-level="editor"]'), 'workflow-editor.png');
    await accessible(page, '[data-workflows-level="editor"] header');
    await accessible(page, '[aria-label="Inspector"]');

    // Branch rename rewrites the structured references into its output.
    await page.getByLabel('Rename branch quality').fill('clarity');
    await page.getByLabel('Rename branch quality').press('Enter');
    await expect(editor(page).getByLabel('Branch node decision')).toContainText('reviews.clarity.acceptable');

    // An external edit since opening is a conflict; local edits are kept.
    const path = join(fixture.workspaceA, '.agents/workflows/parallel_review.yaml');
    writeFileSync(path, `${readFileSync(path, 'utf8')}\n# external edit\n`);
    await page.getByRole('button', { name: 'Save File', exact: true }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'The file changed on disk since it was opened' })).toBeVisible();
    await expect(editor(page).getByLabel('Branch node decision')).toContainText('reviews.clarity.acceptable');
    expect(readFileSync(path, 'utf8')).toContain('# external edit');

    // Leaving with unsaved edits asks first; Keep editing stays on the file.
    await page.getByRole('button', { name: 'New Conversation', exact: true }).first().click();
    const leave = page.getByRole('dialog', { name: 'Discard unsaved changes?' });
    await expect(leave).toBeVisible();
    await leave.getByRole('button', { name: 'Keep editing', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'parallel_review', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'New Conversation', exact: true }).first().click();
    await page.getByRole('dialog', { name: 'Discard unsaved changes?' }).getByRole('button', { name: 'Discard and leave', exact: true }).click();
    await expect(page.getByLabel('Message', { exact: true })).toBeVisible();
    expect(readFileSync(path, 'utf8')).toContain('# external edit');

    // Narrow viewport: the inspector becomes a bottom sheet over the canvas.
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await card(page, 'parallel_review').getByRole('button', { name: 'parallel_review', exact: true }).click();
    await editor(page).getByLabel('Branch node decision').click();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole('complementary', { name: 'Inspector' })).toBeVisible();
    await expectStableScreenshot(page, 'workflow-editor-narrow.png');
  } finally { await fixture.stop(false); }
});

const FLOATS = `description: Floats a browser would retype.
block:
  input: {type: object, properties: {}, additionalProperties: false}
  output: {type: object, properties: {values: {type: array, items: {type: number}}}, required: [values], additionalProperties: false}
  entry: done
  nodes:
    done:
      type: return
      output: {type: object, fields: {values: {type: array, items: [{type: literal, value: 1.0}, {type: literal, value: 1.5}]}}}
`;

test('keyboard selection, node-move undo, focused-field save and the browser number contract', async ({ page }) => {
  const fixture = await startDogfood('web_console_dogfood');
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
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
    await expect(inspector.getByLabel('Node ID')).toHaveValue('ready');
    await inspector.getByLabel('Node ID').fill('accepted');
    await inspector.getByLabel('Node ID').press('Enter');
    await expect(editor(page).getByLabel('Return node accepted')).toBeVisible();

    // One completed drag is one view step: Undo reverts it before the
    // older rename, without touching the file.
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
    await page.keyboard.press('Control+Shift+z');
    await expect(editor(page).getByLabel('Return node accepted')).toBeVisible();
    await page.keyboard.press('Control+Shift+z');
    await expect.poll(async () => Math.round((await decision.boundingBox())!.y)).toBe(Math.round(moved.y));

    // Ctrl/Cmd+S in a focused JSON field saves exactly the visible text.
    const path = join(fixture.workspaceA, '.agents/workflows/parallel_review.yaml');
    const original = readFileSync(path, 'utf8');
    await page.getByRole('button', { name: 'Block settings', exact: true }).click();
    const input = inspector.getByLabel('Input schema');
    await input.fill('{"type": "object", "properties": {"proposal": {"type": "string"}, "rubric": {"type": "string"}}, "required": ["proposal"], "additionalProperties": false}');
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
    const reviews = editor(page).getByLabel('Parallel node reviews');
    await reviews.focus();
    await page.keyboard.press('Enter');
    const key = inspector.getByLabel('Rename branch quality');
    const review = join(fixture.workspaceA, '.agents/workflows/parallel_review.yaml');
    const original = readFileSync(review, 'utf8');
    await key.fill('security');
    await page.keyboard.press('Control+s');
    await expect(inspector.getByText('That ID is already used in this block.')).toBeVisible();
    await expect(key).toHaveValue('security');
    await expect(page.getByRole('list', { name: 'Fields with unapplied text' })).toContainText('reviews · branches["quality"]');
    expect(readFileSync(review, 'utf8')).toBe(original);
    expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

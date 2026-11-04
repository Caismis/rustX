import { test, expect, type Page } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse } from 'yaml';
import { nodeTemplate } from '../../src/app/workflows/document/commands';
import { setNodePresentation } from '../../src/app/workflows/document/geometry';
import type { BlockPath, WorkflowDocument } from '../../src/app/workflows/document/model';
import { viewId } from '../../src/app/workflows/ui/editor/selection';
import { rootBlock } from '../workflow-root';
import { startDogfood } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';

const panel = (page: Page) => page.getByRole('complementary', { name: 'Inspector' });
function auditDocument(): WorkflowDocument {
  let document: WorkflowDocument = { description: 'All native workflow nodes', block: rootBlock() };
  document.block.input = {
    type: 'object',
    properties: {
      text: { type: 'string' },
      threshold: { type: 'number' },
      count: { type: 'integer' },
      flag: { type: 'boolean' },
      numbers: { type: 'array', items: { type: 'number' } },
    },
    required: ['text', 'threshold', 'count', 'flag', 'numbers'],
    additionalProperties: false,
  };
  for (const kind of [
    'model',
    'agent',
    'tool',
    'branch',
    'review',
    'template',
    'code',
    'list',
    'classifier',
    'aggregate',
    'loop',
    'iteration',
  ] as const)
    document.block.nodes[kind] = nodeTemplate(kind, kind);
  Object.assign(document.block.nodes, {
    model: {
      type: 'model',
      model: { mode: 'session' },
      messages: [
        { role: 'system', content: [{ type: 'literal', value: 'Explain the input.' }] },
        { role: 'user', content: [{ type: 'reference', path: ['args', 'text'] }] },
      ],
      output: { type: 'text' },
    },
    agent: {
      type: 'agent',
      profile: 'reviewer',
      task: [{ type: 'literal', value: 'Review the response.' }],
      output: { type: 'text' },
    },
    tool: {
      type: 'tool',
      selector: { origin: 'builtin', name: 'read' },
      arguments: { type: 'object', fields: { path: { type: 'reference', path: ['args', 'text'] } } },
      result: { type: 'text' },
    },
    branch: {
      type: 'branch',
      cases: [
        {
          id: 'true',
          condition: {
            type: 'equal',
            left: { type: 'reference', path: ['args', 'count'] },
            right: { type: 'literal', value: 0 },
          },
        },
      ],
    },
    review: {
      type: 'review',
      content: [{ type: 'literal', value: 'Approve this result?' }],
      actions: [{ id: 'approve', title: 'Approve' }],
      timeout_ms: '3600000',
    },
    template: {
      type: 'template',
      variables: { arg1: { type: 'reference', path: ['args', 'text'] } },
      template: '{{ arg1 }}',
    },
    list: { type: 'list', input: { type: 'reference', path: ['args', 'numbers'] } },
    classifier: {
      type: 'classifier',
      model: { mode: 'session' },
      input: { type: 'reference', path: ['args', 'text'] },
      classes: [
        { id: 'class_1', label: 'CLASS 1', description: [{ type: 'literal', value: 'First topic' }] },
        { id: 'class_2', label: 'CLASS 2', description: [{ type: 'literal', value: 'Other topic' }] },
      ],
    },
    aggregate: {
      type: 'aggregate',
      grouped: false,
      groups: [
        {
          name: 'Group1',
          output_type: 'string',
          variables: [
            ['model', 'text'],
            ['agent', 'text'],
          ],
        },
      ],
    },
    loop: {
      type: 'loop',
      loop_count: 10,
      loop_variables: [{ label: 'count', var_type: 'number', value: { type: 'literal', value: 0 } }],
      body: {
        nodes: {
          loop_start: { type: 'start' },
          assigner: {
            type: 'assigner',
            items: [{ variable: ['loop', 'count'], operation: 'increment', value: { type: 'literal', value: 1 } }],
          },
          loop_end: { type: 'loop_end' },
        },
        edges: [
          { from: 'loop_start', to: 'assigner' },
          { from: 'assigner', to: 'loop_end' },
        ],
      },
    },
    iteration: {
      type: 'iteration',
      iterator: { type: 'reference', path: ['args', 'numbers'] },
      output: { type: 'reference', path: ['format', 'output'] },
      body: {
        nodes: {
          iteration_start: { type: 'start' },
          format: {
            type: 'template',
            variables: { arg1: { type: 'reference', path: ['iteration', 'item'] } },
            template: '{{ arg1 }}',
          },
        },
        edges: [{ from: 'iteration_start', to: 'format' }],
      },
    },
    done: { type: 'return', outputs: [{ variable: 'answer', value_selector: ['aggregate', 'output'] }] },
  });
  document.block.edges = [
    { from: 'start', to: 'model' },
    { from: 'model', to: 'agent' },
    { from: 'agent', to: 'tool' },
    { from: 'tool', to: 'branch' },
    ...['true', 'false'].map((port) => ({ from: 'branch', to: 'review', port })),
    ...['approve', 'timeout'].map((port) => ({ from: 'review', to: 'template', port })),
    { from: 'template', to: 'code' },
    { from: 'code', to: 'list' },
    { from: 'list', to: 'classifier' },
    ...['class_1', 'class_2'].map((port) => ({ from: 'classifier', to: 'aggregate', port })),
    { from: 'aggregate', to: 'loop' },
    { from: 'loop', to: 'iteration' },
    { from: 'iteration', to: 'done' },
  ];
  const code = document.block.nodes.code;
  if (code.type === 'code')
    document.block.nodes.code = {
      ...code,
      variables: {
        arg1: { type: 'reference', path: ['args', 'text'] },
        arg2: { type: 'reference', path: ['args', 'text'] },
      },
    };
  let index = 0;
  for (const id of Object.keys(document.block.nodes))
    document = setNodePresentation(document, [], id, {
      position: { x: 320 * (index++ % 5), y: Math.floor((index - 1) / 5) * 340 },
    });
  document = setNodePresentation(document, [{ node: 'loop' }], 'assigner', { position: { x: 320, y: 90 } });
  document = setNodePresentation(document, [{ node: 'loop' }], 'loop_end', { position: { x: 640, y: 90 } });
  return document;
}
for (const width of [1440, 390])
  test.describe(`All node audit ${width}`, () => {
    test.use({ viewport: { width, height: 1000 }, hasTouch: width === 390 });
    test('all settings fit; typed branch, prompt expansion and output names persist', async ({ page }) => {
      page.setDefaultTimeout(15000);
      const fixture = await startDogfood('web_console_dogfood'),
        errors: string[] = [];
      page.on('pageerror', (error) => errors.push(error.message));
      try {
        const directory = join(fixture.workspaceA, '.agents/workflows');
        mkdirSync(directory, { recursive: true });
        const file = join(directory, 'all_nodes.yaml');
        writeFileSync(file, JSON.stringify(auditDocument()).replace('"timeout_ms":"3600000"', '"timeout_ms":3600000'));
        mkdirSync(join(fixture.workspaceA, '.agents/agents'), { recursive: true });
        writeFileSync(
          join(fixture.workspaceA, '.agents/agents/reviewer.toml'),
          'description = "Reviewer"\ninstructions = "Review carefully."\n',
        );
        await page.addInitScript(
          (dark) => localStorage.setItem('rustx-appearance-v1', dark ? 'dark' : 'light'),
          width === 390,
        );
        await routeWorkspaceHost(page, fixture);
        await page.goto('/');
        await connectRemote(page, fixture.endpoint, fixture.token);
        await openEmptySession(page, fixture, 'Workspace A');
        await page
          .getByRole('navigation', { name: 'Global panels' })
          .getByRole('button', { name: 'Workflows', exact: true })
          .click();
        await page.getByRole('button', { name: 'all_nodes', exact: true }).click();
        const open = async (id: string, path: BlockPath = [], start = false) => {
          const card = page.locator(
            `.react-flow__node[data-id="${viewId(path, start ? 'start' : 'node', start ? '' : id)}"]`,
          );
          await card.focus();
          await card.press('Enter');
          await expect(panel(page)).toBeVisible();
        };
        for (const [id, path, start] of [
          ['start', [], true],
          ...[
            'model',
            'agent',
            'tool',
            'branch',
            'review',
            'template',
            'code',
            'list',
            'classifier',
            'aggregate',
            'loop',
            'iteration',
            'done',
          ].map((id) => [id, [], false]),
          ['loop_start', [{ node: 'loop' }], true],
          ['assigner', [{ node: 'loop' }], false],
          ['loop_end', [{ node: 'loop' }], false],
          ['iteration_start', [{ node: 'iteration' }], true],
          ['format', [{ node: 'iteration' }], false],
        ] as [string, BlockPath, boolean][]) {
          await open(id, path, start);
          await expect(panel(page).locator('[data-workflow-inspector-body]')).toBeVisible();
          if (['code', 'template', 'format'].includes(id))
            await expect(panel(page).locator('.cm-editor')).toBeVisible();
          expect(
            await panel(page)
              .locator('[data-workflow-inspector-body]')
              .evaluate((element) => element.scrollWidth - element.clientWidth),
            id,
          ).toBeLessThanOrEqual(1);
          await expect(panel(page).getByRole('switch', { name: /vision/i })).toHaveCount(0);
          await expect(panel(page).getByRole('button', { name: /generate.*(prompt|code)/i })).toHaveCount(0);
          if (['model', 'agent', 'review', 'classifier'].includes(id)) {
            const expand = panel(page).getByRole('button', { name: 'Expand', exact: true }).first();
            const editor = panel(page).locator('[contenteditable="true"]').first(),
              handle = await editor.elementHandle();
            await editor.press('Escape');
            await expect(panel(page)).toBeVisible();
            await expand.click();
            const expanded = panel(page).locator('[data-prompt-expanded]');
            await expect(expanded).toHaveCSS('position', 'fixed');
            expect(await editor.evaluate((element, original) => element === original, handle)).toBe(true);
            const bounds = (await expanded.boundingBox())!;
            expect(bounds.x).toBeGreaterThanOrEqual(0);
            expect(bounds.x + bounds.width).toBeLessThanOrEqual(width);
            expect(bounds.y + bounds.height).toBeLessThanOrEqual(1001);
            await page.screenshot({ path: `/tmp/rustx-all-nodes-${id}-expanded-${width}.png` });
            await editor.press('Escape');
            await expect(expanded).toHaveCount(0);
            await expect(panel(page)).toBeVisible();
          }
          if (id === 'loop')
            expect(
              (await panel(page).getByRole('textbox', { name: 'Variable Name', exact: true }).boundingBox())!.width,
            ).toBeGreaterThan(90);
          if (id === 'loop_end')
            await expect(panel(page).getByRole('region', { name: 'Next step', exact: true })).toHaveCount(0);
          await page.screenshot({ path: `/tmp/rustx-all-nodes-${id}-${width}.png` });
        }
        await open('branch');
        await panel(page).getByRole('button', { name: 'Operator', exact: true }).click();
        for (const name of ['greater than', 'greater than or equal to', 'less than', 'less than or equal to'])
          await expect(page.getByRole('menuitem', { name, exact: true })).toBeVisible();
        await page.getByRole('menuitem', { name: 'greater than or equal to', exact: true }).click();
        const number = panel(page).getByRole('textbox', { name: 'Value', exact: true });
        await number.fill('3');
        await number.press('Enter');
        await panel(page).getByRole('radio', { name: 'Variable', exact: true }).click();
        await panel(page).getByRole('button', { name: 'Choose a variable', exact: true }).last().click();
        const picker = page.getByRole('dialog', { name: 'Choose a variable', exact: true });
        await expect(picker.getByRole('button', { name: /^args \/ text ·/ })).toHaveCount(0);
        await picker.getByRole('button', { name: /^args \/ threshold ·/ }).click();
        await expect(panel(page).getByRole('button', { name: 'Operator', exact: true })).toHaveText('≥');

        expect(
          await panel(page)
            .getByRole('button', { name: 'Choose a variable', exact: true })
            .first()
            .locator('b')
            .evaluate((element) => element.scrollWidth - element.clientWidth),
        ).toBeLessThanOrEqual(1);
        await panel(page).screenshot({ path: `/tmp/rustx-node-audit-branch-panel-${width}.png` });
        await panel(page).getByRole('button', { name: 'ELIF', exact: true }).click();
        const elif = panel(page).getByRole('region', { name: 'ELIF CASE 2', exact: true });
        await elif.getByRole('button', { name: 'Add condition', exact: true }).click();
        await picker.getByRole('button', { name: /^args \/ numbers ·/ }).click();
        await elif.getByRole('button', { name: 'Operator', exact: true }).click();
        await page.getByRole('menuitem', { name: 'contains', exact: true }).click();
        await elif.getByRole('textbox', { name: 'Value', exact: true }).fill('2');
        await elif.getByRole('textbox', { name: 'Value', exact: true }).press('Enter');
        await elif.getByRole('button', { name: 'Operator', exact: true }).click();
        await page.getByRole('menuitem', { name: 'is not empty', exact: true }).click();
        await expect(elif.getByRole('textbox', { name: 'Value', exact: true })).toHaveCount(0);
        await panel(page).getByRole('button', { name: 'Remove CASE 2', exact: true }).click();
        await open('loop');
        await panel(page).getByRole('button', { name: 'Add condition', exact: true }).click();
        await picker.getByRole('button', { name: /^loop \/ count ·/ }).click();
        await panel(page).getByRole('button', { name: 'Operator', exact: true }).click();
        await page.getByRole('menuitem', { name: 'greater than or equal to', exact: true }).click();
        await panel(page).getByRole('textbox', { name: 'Value', exact: true }).last().fill('3');
        await panel(page).getByRole('textbox', { name: 'Value', exact: true }).last().press('Enter');
        await panel(page).screenshot({ path: `/tmp/rustx-node-audit-loop-panel-${width}.png` });
        await open('done');
        for (const source of ['model', 'agent']) {
          await panel(page).getByRole('button', { name: 'Add output variable', exact: true }).click();
          await panel(page).getByRole('button', { name: 'Choose a variable', exact: true }).last().click();
          await picker.getByRole('button', { name: new RegExp(`^${source} / text ·`) }).click();
        }
        await page.getByRole('button', { name: 'Save File', exact: true }).click();
        await expect(page.getByText('No unsaved changes', { exact: true })).toHaveText('No unsaved changes');
        const saved = parse(readFileSync(file, 'utf8'));
        expect(saved.block.nodes.branch.cases[0].condition).toEqual({
          type: 'greater_equal',
          left: { type: 'reference', path: ['args', 'count'] },
          right: { type: 'reference', path: ['args', 'threshold'] },
        });
        expect(saved.block.nodes.done.outputs.map((output: { variable: string }) => output.variable)).toEqual([
          'answer',
          'text',
          'text_2',
        ]);
        expect(errors).toEqual([]);
        expect((await fixture.control('requests')).requests).toHaveLength(0);
      } finally {
        await fixture.stop(false);
      }
    });
  });

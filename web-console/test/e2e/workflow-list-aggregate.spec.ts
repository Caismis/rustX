import { test, expect } from '@playwright/test';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse } from 'yaml';
import { dataNodesDocument } from '../workflow-data-document';
import { viewId } from '../../src/app/workflows/ui/editor/selection';
import { startDogfood } from './dogfood-server';
import { connectRemote, openEmptySession, choose } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import type { WorkflowCheckReport } from '../../../protocol/app-server/v72';
import type { WorkflowDocument } from '../../src/app/workflows/document/model';
import { rootBlock } from '../workflow-root';

for (const width of [1440, 390])
  test.describe(`List and Aggregate ${width}`, () => {
    test.use({ viewport: { width, height: 1000 }, hasTouch: width === 390 });
    test('typed conditions, serial references, grouping and downstream reference ownership', async ({ page }) => {
      page.setDefaultTimeout(15000);
      const fixture = await startDogfood('web_console_dogfood'),
        errors: string[] = [];
      page.on('pageerror', (error) => errors.push(error.message));
      page.on('console', (message) => {
        if (message.type() === 'error' || message.type() === 'warning') errors.push(message.text());
      });
      try {
        const directory = join(fixture.workspaceA, '.agents/workflows');
        mkdirSync(directory, { recursive: true });
        const file = join(directory, 'data_align.yaml'),
          workflow = dataNodesDocument();
        workflow.block.input = {
          type: 'object',
          properties: {
            query: { type: 'string' },
            priority: { type: 'number' },
            memo: { type: 'object', properties: {}, additionalProperties: false },
            numbers: { type: 'array', items: { type: 'integer' } },
            words: { type: 'array', items: { type: 'string' } },
            flags: { type: 'array', items: { type: 'boolean' } },
            serial: { type: 'number' },
            threshold: { type: 'number' },
          },
          required: ['query', 'priority', 'memo', 'numbers', 'words', 'flags', 'serial', 'threshold'],
          additionalProperties: false,
        };
        writeFileSync(file, JSON.stringify(workflow));
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
        await page.getByRole('button', { name: 'data_align', exact: true }).click();
        await expect(page).toHaveTitle(/rustX$/);
        const activate = async (id: string) => {
          const card = page.locator(`.react-flow__node[data-id='${viewId([], 'node', id)}']`);
          await card.focus();
          await card.press('Enter');
        };
        const panel = page.getByRole('complementary', { name: 'Inspector', exact: true }),
          picker = page.getByRole('dialog', { name: 'Choose a variable', exact: true });
        await activate('list');
        await panel.getByRole('switch', { name: 'Filter', exact: true }).click();
        const operator = panel.getByRole('combobox', { name: 'Operator', exact: true });
        await expect(operator.locator('option[value="contains"]')).toHaveCount(0);
        await operator.selectOption('greater_equal');
        const comparison = panel.getByRole('textbox', { name: 'Comparison value', exact: true });
        await comparison.fill('');
        await comparison.press('/');
        await expect(picker).toBeVisible();
        const threshold = picker.getByRole('button', { name: /^args \/ threshold / });
        await expect(threshold).toBeEnabled();
        // Native numeric List operands accept numbers and numeric text; objects
        // remain incompatible. Await a real choice before checking exclusions.
        await expect(picker.getByRole('button', { name: /^args \/ query / })).toBeEnabled();
        await expect(picker.getByRole('button', { name: /^args \/ memo / })).toHaveCount(0);
        await threshold.click();
        await panel.getByRole('switch', { name: 'Extract by serial number', exact: true }).click();
        const serial = panel.getByRole('textbox', { name: 'Serial number', exact: true });
        await serial.fill('');
        await serial.press('/');
        await picker.getByRole('button', { name: /^args \/ serial / }).click();
        await panel.getByRole('switch', { name: 'Limit', exact: true }).click();
        const slider = panel.getByRole('slider', { name: 'Maximum results', exact: true });
        await slider.focus();
        await slider.press('Home');
        await slider.press('ArrowRight');
        await slider.press('ArrowRight');
        await expect(slider).toHaveValue('3');
        await panel.getByRole('switch', { name: 'Sort', exact: true }).click();
        await panel.getByRole('radio', { name: 'Descending', exact: true }).click();
        // Switching input resets type-specific conditions; no user-controlled value-type selector.
        await panel.getByRole('button', { name: 'Choose a variable', exact: true }).first().click();
        await picker.getByRole('button', { name: /^args \/ words / }).click();
        await operator.selectOption('empty');
        await expect(comparison).toHaveCount(0);
        await expect(operator.locator('option[value="greater"]')).toHaveCount(0);
        await panel.getByRole('button', { name: 'Choose a variable', exact: true }).first().click();
        await picker.getByRole('button', { name: /^args \/ flags / }).click();
        await expect(operator.locator('option')).toHaveCount(2);
        await panel.getByRole('combobox', { name: 'Comparison value', exact: true }).selectOption('true');
        await panel.getByRole('button', { name: 'Choose a variable', exact: true }).first().click();
        await picker.getByRole('button', { name: /^args \/ numbers / }).click();
        await operator.selectOption('greater_equal');
        await comparison.fill('2');
        await comparison.blur();
        const heights = await panel
          .locator('[data-list-scalar]')
          .evaluateAll((elements) => elements.map((element) => element.getBoundingClientRect().height));
        expect(heights.length).toBe(2);
        for (const height of heights) {
          expect(height).toBeGreaterThanOrEqual(32);
          expect(height).toBeLessThanOrEqual(34);
        }

        await panel.getByRole('button', { name: 'Output variables', exact: true }).click();
        await expect(
          panel.getByText('The first record, or null when the list is empty.', { exact: true }),
        ).toBeVisible();
        await activate('merge');
        await expect(panel.getByRole('switch', { name: 'Aggregation groups', exact: true })).not.toBeChecked();
        await expect(panel.getByRole('combobox', { name: 'Output type', exact: true })).toHaveCount(0);
        await panel.getByRole('switch', { name: 'Aggregation groups', exact: true }).click();
        const groupName = panel.getByRole('textbox', { name: 'Output name', exact: true });
        await groupName.fill('answer');
        await groupName.press('Enter');
        await panel.getByRole('button', { name: 'Add output group', exact: true }).click();
        await panel.getByRole('button', { name: 'Add variable to Group2', exact: true }).click();
        await picker.getByRole('button', { name: /^args \/ priority / }).click();
        await panel.getByRole('button', { name: 'Add variable to Group2', exact: true }).click();
        await expect(picker.getByRole('button', { name: /^args \/ priority / })).toBeEnabled();
        await expect(picker.getByRole('button', { name: /^args \/ query / })).toHaveCount(0);
        await picker.getByRole('button', { name: /^args \/ priority / }).click();
        await expect(panel.getByRole('button', { name: 'Choose a variable', exact: true })).toHaveCount(3);
        const addBox = (await panel
            .getByRole('button', { name: 'Add variable to Group2', exact: true })
            .boundingBox())!,
          removeBox = (await panel
            .getByRole('button', { name: 'Remove output group Group2', exact: true })
            .boundingBox())!;
        expect(removeBox.x - addBox.x - addBox.width).toBeGreaterThanOrEqual(0);
        expect(removeBox.x - addBox.x - addBox.width).toBeLessThanOrEqual(8);
        const lastSource=(await panel.getByRole('button',{name:'Choose a variable',exact:true}).last().boundingBox())!,addGroup=(await panel.getByRole('button',{name:'Add output group',exact:true}).boundingBox())!;
        expect(addGroup.y-lastSource.y-lastSource.height).toBeCloseTo(8,0);
        await activate('done');
        await panel.getByRole('button', { name: 'Choose a variable', exact: true }).nth(1).click();
        await picker.getByRole('button', { name: /^merge \/ Group2 \/ output / }).click();
        await activate('merge');
        await panel.getByRole('switch', { name: 'Aggregation groups', exact: true }).click();
        const confirmation = page.getByRole('dialog', { name: 'Remove referenced outputs?', exact: true });
        await expect(confirmation).toBeVisible();
        await confirmation.getByRole('button', { name: 'Cancel', exact: true }).click();
        await panel.getByRole('button', { name: 'Remove output group Group2', exact: true }).click();
        await confirmation.getByRole('button', { name: 'Remove and clear references', exact: true }).click();
        await expect(panel.getByRole('textbox', { name: 'Output name', exact: true })).toHaveCount(1);
        await page.getByRole('button', { name: 'Undo', exact: true }).click();
        await expect(panel.getByRole('textbox', { name: 'Output name', exact: true })).toHaveCount(2);
        const save = page.getByRole('button', { name: 'Save File', exact: true });
        await save.click();
        await expect(save).toBeDisabled({ timeout: 15000 });
        const saved = parse(readFileSync(file, 'utf8'));
        expect(saved.block.nodes.list).toMatchObject({
          filter: { operator: 'greater_equal', value: { type: 'literal', value: 2 } },
          extract: { type: 'template', parts: [{ type: 'reference', path: ['args', 'serial'] }] },
          limit: 3,
          sort: { order: 'desc' },
        });
        expect(saved.block.nodes.merge).toMatchObject({
          grouped: true,
          groups: [
            { name: 'answer', output_type: 'string' },
            { name: 'Group2', output_type: 'number', variables: [['args', 'priority']] },
          ],
        });
        expect(saved.block.nodes.done.outputs.map((item: any) => item.value_selector)).toEqual([
          ['merge', 'answer', 'output'],
          ['merge', 'Group2', 'output'],
        ]);
        await page.getByRole('button', { name: 'Back to Workflows', exact: true }).first().click();
        await page.getByRole('button', { name: 'data_align', exact: true }).click();
        await activate('merge');
        await expect(panel.getByRole('textbox', { name: 'Output name', exact: true })).toHaveCount(2);
        await page.getByRole('button', { name: 'Settings', exact: true }).click();
        await choose(page.getByRole('dialog', { name: 'Settings', exact: true }), 'Language', '中文');
        await page.getByRole('button', { name: '关闭设置', exact: true }).click();
        const zhPanel = page.getByRole('complementary', { name: '检查器', exact: true });
        await expect(zhPanel.getByRole('switch', { name: '聚合分组', exact: true })).toBeVisible();
        await zhPanel.screenshot({ path: `/tmp/rustx-aggregate-aligned-${width}.png` });
        await activate('list');
        await expect(zhPanel.getByRole('switch', { name: '按序号提取', exact: true })).toBeVisible();
        await zhPanel.screenshot({ path: `/tmp/rustx-list-aligned-${width}.png` });
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        await expect(page.locator('vite-error-overlay')).toHaveCount(0);
        expect(errors).toEqual([]);
      } finally {
        await fixture.stop(false);
      }
    });
  });

for (const width of [1440, 390]) test(`Boolean List references remain dynamic until an explicit constant switch at ${width}px`, async ({ page }) => {
  page.setDefaultTimeout(15000);
  await page.setViewportSize({ width, height: 844 });
  const fixture = await startDogfood('web_console_dogfood'), errors: string[] = [];
  const checks: { value: unknown; report: WorkflowCheckReport }[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  try {
    const document: WorkflowDocument = { description: 'Boolean reference filter', block: rootBlock() };
    document.block.input = { type: 'object', properties: {
      flags: { type: 'array', items: { type: 'boolean' } }, keep: { type: 'boolean' }, other: { type: 'boolean' },
    }, required: ['flags', 'keep', 'other'], additionalProperties: false };
    document.block.nodes.list = { type: 'list', input: { type: 'reference', path: ['args', 'flags'] },
      filter: { path: [], operator: 'equal', value: { type: 'reference', path: ['args', 'keep'] } } };
    document.block.nodes.done = { type: 'return', outputs: [{ variable: 'flags', value_selector: ['list', 'result'] }] };
    document.block.edges = [{ from: 'start', to: 'list' }, { from: 'list', to: 'done' }];
    const directory = join(fixture.workspaceA, '.agents/workflows');
    mkdirSync(directory, { recursive: true });
    const file = join(directory, 'boolean_filter.yaml');
    writeFileSync(file, JSON.stringify(document));
    await routeWorkspaceHost(page, fixture, undefined, (request, response) => {
      if (!request.url().endsWith('/workflow') || response.status !== 200) return;
      const operation = request.postDataJSON().operation;
      if (operation.kind !== 'check') return;
      const result = JSON.parse(response.body.toString());
      if (result.kind !== 'check') throw Error('Unexpected native Workflow check response');
      checks.push({ value: operation.document.block.nodes.list.filter.value, report: result.report });
    });
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await openEmptySession(page, fixture, 'Workspace A');
    await page.getByRole('navigation', { name: 'Global panels' }).getByRole('button', { name: 'Workflows', exact: true }).click();
    await page.getByRole('button', { name: 'boolean_filter', exact: true }).click();
    await expect(page).toHaveTitle(/rustX$/);
    const activate = async () => {
      const card = page.locator(`.react-flow__node[data-id='${viewId([], 'node', 'list')}']`);
      await card.focus(); await card.press('Enter');
    };
    await activate();
    const panel = page.getByRole('complementary', { name: 'Inspector', exact: true });
    const comparison = panel.getByRole('group', { name: 'Comparison value', exact: true });
    await expect(comparison.getByRole('radio', { name: 'Variable', exact: true })).toBeChecked();
    await expect(comparison.getByRole('button', { name: 'Choose a variable', exact: true })).toContainText('keep');
    await expect(comparison.getByRole('combobox')).toHaveCount(0);
    await comparison.getByRole('radio', { name: 'Variable', exact: true }).click();
    await comparison.getByRole('button', { name: 'Choose a variable', exact: true }).click();
    const picker = page.getByRole('dialog', { name: 'Choose a variable', exact: true });
    await expect(picker.getByRole('button', { name: /^args \/ other / })).toBeEnabled();
    await expect(picker.getByRole('button', { name: /^args \/ flags / })).toHaveCount(0);
    await picker.getByRole('button', { name: /^args \/ other / }).click();
    const reference = { type: 'reference', path: ['args', 'other'] };
    await expect.poll(() => [...checks].reverse().find(check => JSON.stringify(check.value) === JSON.stringify(reference))?.report)
      .toMatchObject({ structure: { status: 'valid' } });
    const save = page.getByRole('button', { name: 'Save File', exact: true });
    await save.click(); await expect(save).toBeDisabled();
    expect(parse(readFileSync(file, 'utf8')).block.nodes.list.filter.value).toEqual(reference);
    await page.getByRole('button', { name: 'Back to Workflows', exact: true }).first().click();
    await page.getByRole('button', { name: 'boolean_filter', exact: true }).click(); await activate();
    await expect(comparison.getByRole('radio', { name: 'Variable', exact: true })).toBeChecked();
    await expect(comparison.getByRole('button', { name: 'Choose a variable', exact: true })).toContainText('other');
    await expect(comparison.getByRole('combobox')).toHaveCount(0);
    await panel.screenshot({ path: `/tmp/rustx-list-boolean-reference-${width}.png` });
    await comparison.getByRole('radio', { name: 'Constant', exact: true }).click();
    await expect(comparison.getByRole('combobox', { name: 'Comparison value', exact: true })).toHaveValue('false');
    await comparison.getByRole('combobox', { name: 'Comparison value', exact: true }).selectOption('true');
    await expect.poll(() => [...checks].reverse().find(check => JSON.stringify(check.value) === JSON.stringify({ type: 'literal', value: true }))?.report)
      .toMatchObject({ structure: { status: 'valid' } });
    await save.click(); await expect(save).toBeDisabled();
    expect(parse(readFileSync(file, 'utf8')).block.nodes.list.filter.value).toEqual({ type: 'literal', value: true });
    expect(await page.evaluate(() => window.document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('vite-error-overlay')).toHaveCount(0);
    expect(errors).toEqual([]);
  } finally { await fixture.stop(false); }
});

import { expect, test, type Locator, type Page } from '@playwright/test';
import { existsSync, mkdirSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { materializeEffective } from '../../../protocol/app-server/effective-observation';
import { startDogfood } from './dogfood-server';
import { connectRemote, openEmptySession } from './shell-actions';
import { routeWorkspaceHost } from './workspace-host';
import { wireProbe } from './wire-probe';

const toolResult = 'HISTORY_TOOL_RESULT_v1';
const workflowInput = 'HISTORY_WORKFLOW_INPUT_v1';
const workflowResult = `Saved ${workflowInput}`;

async function inspect(page: Page, row: Locator, fields: { tab: string; contains: string }[]) {
  await row.click();
  const details = page.getByRole('complementary', { name: 'Event details', exact: true });
  const values: Record<string, string> = {};
  for (const { tab, contains } of fields) {
    await details.getByRole('tab', { name: tab, exact: true }).click();
    const body = details.getByRole('tabpanel');
    await expect(body).toContainText(contains);
    await expect(details.getByRole('alert')).toHaveCount(0);
    values[tab] = await body.innerText();
  }
  await details.getByRole('button', { name: 'Close details', exact: true }).click();
  return values;
}

async function historicalTrace(page: Page) {
  await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
  const trajectory = page.getByRole('region', { name: 'Trajectory', exact: true });
  const ledger = trajectory.getByRole('table', { name: 'Trace ledger', exact: true });
  const read = ledger.locator('[data-kind="tool"]').filter({ has: page.getByText('read', { exact: true }) });
  const workflow = ledger.locator('[data-workflow-entry]').filter({ has: page.getByText('history_flow', { exact: true }) });
  await expect(read).toHaveCount(1);
  await expect(workflow).toHaveCount(1);
  const tool = await inspect(page, read, [
    { tab: 'Payload', contains: 'history-evidence.txt' },
    { tab: 'Result', contains: toolResult },
    { tab: 'Schema', contains: 'path' },
  ]);
  const parent = await inspect(page, workflow, [
    { tab: 'Payload', contains: workflowInput },
    { tab: 'Result', contains: workflowResult },
    { tab: 'Schema', contains: 'message' },
  ]);
  await workflow.getByRole('button', { name: 'Expand workflow nodes', exact: true }).click();
  const nodes = ledger.locator('[data-workflow-node]');
  await expect(nodes).toHaveCount(3);
  expect(await nodes.evaluateAll(rows => rows.map(row => row.getAttribute('data-workflow-instance')))).toEqual(['start', 'render', 'done']);
  const visits = [];
  for (const name of ['start', 'render', 'done']) {
    const row = ledger.locator(`[data-workflow-instance="${name}"]`);
    await expect(row).toHaveAttribute('data-state', 'completed');
    visits.push({ id: await row.getAttribute('data-workflow-node'), name, ...await inspect(page, row, [
      { tab: 'Payload', contains: workflowInput },
      { tab: 'Result', contains: name === 'start' ? 'port' : workflowResult },
    ]) });
  }
  await workflow.getByRole('button', { name: 'Collapse workflow nodes', exact: true }).click();
  await expect(nodes).toHaveCount(0);
  await workflow.getByRole('button', { name: 'Expand workflow nodes', exact: true }).click();
  await expect(nodes).toHaveCount(3);
  await expect(trajectory.getByRole('alert')).toHaveCount(0);
  return { tool, parent, visits };
}

for (const width of [1440, 390]) {
  test(`removed tool and deleted workflow retain historical Trace after reload at ${width}px`, async ({ page }) => {
    const fixture = await startDogfood('web_removed_capability_history');
    const wire = await wireProbe(page);
    const errors: string[] = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (['error', 'warning'].includes(message.type())) errors.push(message.text()); });
    let passed = false;
    try {
      const workflowFile = join(fixture.workspaceA, '.agents/workflows/history_flow.yaml');
      const source = join(fixture.workspaceA, 'rustx.toml');
      mkdirSync(join(fixture.workspaceA, '.agents/workflows'), { recursive: true });
      writeFileSync(join(fixture.workspaceA, 'history-evidence.txt'), toolResult);
      writeFileSync(workflowFile, JSON.stringify({ description: 'Original historical workflow', block: {
        input: { type: 'object', properties: { message: { type: 'string' } }, required: ['message'], additionalProperties: false },
        nodes: {
          start: { type: 'start' },
          render: { type: 'template', variables: { message: { type: 'reference', path: ['args', 'message'] } }, template: 'Saved {{ message }}' },
          done: { type: 'return', outputs: [{ variable: 'result', value_selector: ['render', 'output'] }] },
        },
        edges: [{ from: 'start', to: 'render' }, { from: 'render', to: 'done' }],
      } }));
      writeFileSync(source, '[agent]\nworkflows = ["history_flow"]\n[agent.tools]\nbuiltin = ["read"]\n');
      await page.setViewportSize({ width, height: 1000 });
      await routeWorkspaceHost(page, fixture);
      await page.goto('/');
      await connectRemote(page, fixture.endpoint, fixture.token);
      const session = await openEmptySession(page, fixture, 'Workspace A');
      const effective = async () => {
        const target = await wire.attachedTarget(session);
        const { projection } = await wire.read('session/effectiveConfiguration', target);
        return materializeEffective(projection, (reference, offset) => wire.effectiveValueRead(target, reference, offset), () => true);
      };
      const original = await effective();
      expect(original.root_agent.tools?.builtin).toEqual(['read']);
      expect(original.root_agent.workflows).toEqual(['history_flow']);
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Execute capabilities before removal');
      await page.getByRole('button', { name: 'Send', exact: true }).click();
      await expect(page.getByText('Original capabilities executed.', { exact: true })).toBeVisible();
      const before = await historicalTrace(page);

      // Remove the actual enabled capabilities and physically delete the YAML.
      // Change the read source too, so an accidental re-execution cannot pass.
      writeFileSync(source, '[agent]\nworkflows = []\n[agent.tools]\nbuiltin = []\n');
      unlinkSync(workflowFile);
      writeFileSync(join(fixture.workspaceA, 'history-evidence.txt'), 'CHANGED_AFTER_EXECUTION');
      expect(existsSync(workflowFile)).toBe(false);
      // External file edits enter the native configuration system through its
      // explicit reconcile operation, just as a Settings source reload does.
      const workspace = (await fixture.workspaceHost.host.listWorkspaces()).workspaces.find(workspace => workspace.displayName === 'Workspace A')!;
      await fixture.workspaceHost.host.configureWorkspace(workspace.id, fixture.endpoint, { kind: 'reconcile' });
      await page.getByRole('tab', { name: 'Chat', exact: true }).click();
      const adopt = page.getByRole('button', { name: 'Adopt configuration', exact: true });
      await expect(adopt).toBeEnabled();
      await adopt.click();
      await expect.poll(async () => {
        const current = await effective();
        return { tools: current.root_agent.tools?.builtin, workflows: current.root_agent.workflows };
      }).toEqual({ tools: [], workflows: [] });
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Continue after capability removal');
      await page.getByRole('button', { name: 'Send', exact: true }).click();
      await expect(page.getByText('Removed capabilities are absent from this request.', { exact: true })).toBeVisible();
      const requests = (await fixture.control('requests')).requests;
      expect(requests).toHaveLength(4);
      expect(requests.at(-1).body.tools ?? []).toEqual([]);

      // A full document reload discards all React/Trace caches. Historical
      // detail bodies have to be fetched again from their original owners.
      await page.reload();
      await expect(page).toHaveTitle(/rustX$/);
      // Remote credentials are deliberately memory-only. Reconnect and open
      // the existing Session rather than creating or re-executing anything.
      await connectRemote(page, fixture.endpoint, fixture.token);
      const expand = page.getByRole('button', { name: 'Expand Sidebar', exact: true });
      if (await expand.isVisible()) await expand.click();
      await page.locator(`button[data-session-id="${session}"]`).click();
      await expect(page.getByText('Removed capabilities are absent from this request.', { exact: true })).toBeVisible();
      expect(await historicalTrace(page)).toEqual(before);
      await page.screenshot({ path: `/tmp/rustx-removed-capability-history-${width}.png` });
      const node = page.locator('[data-workflow-instance="render"]');
      await node.click();
      const details = page.getByRole('complementary', { name: 'Event details', exact: true });
      await details.getByRole('tab', { name: 'Result', exact: true }).click();
      await expect(details.getByRole('tabpanel')).toContainText(workflowResult);
      await page.screenshot({ path: `/tmp/rustx-removed-capability-result-${width}.png` });
      await expect(page.locator('vite-error-overlay')).toHaveCount(0);
      expect(errors).toEqual([]);
      passed = true;
    } finally {
      await fixture.stop(passed);
    }
  });
}

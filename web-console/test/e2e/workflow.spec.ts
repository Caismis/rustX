import { expect, test } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';

test('native Workflow Agent child composes with Chat, Trace, reload and resource inventory', async ({ page }) => {
  const fixture = await startDogfood('web_workflow_conformance'); let passed = false;
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const workflowPath = join(fixture.workspaceA, '.agents/workflows/review_pr.yaml');
  const agentPath = join(fixture.workspaceA, '.agents/agents/reviewer.toml');
  const workflow = readFileSync(workflowPath, 'utf8'), agent = readFileSync(agentPath, 'utf8');
  const connect = async () => {
    await page.getByLabel('WebSocket endpoint').fill(fixture.endpoint);
    await page.getByLabel('Transport token').fill(fixture.token);
    await page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(page.locator('.status strong')).toHaveText('connected');
  };
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/'); await connect();
    await page.getByLabel('Choose Workspace').selectOption({ label: 'Workspace A' });
    await page.getByRole('button', { name: 'Create Session', exact: true }).click();
    await page.getByLabel('Message', { exact: true }).fill('workflow conformance request');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await fixture.gate('workflow-child-admitted');
    const child = page.locator('[data-subagent-id]');
    const run = page.locator('[data-workflow-run-id]');
    await expect(child).toHaveCount(1); await expect(run).toHaveCount(1);
    const childId = await child.getAttribute('data-subagent-id');
    const runId = await run.getAttribute('data-workflow-run-id');
    await expect(run).toContainText('review_pr');
    await page.reload(); await connect();
    await expect(child).toHaveAttribute('data-subagent-id', childId!);
    await expect(run).toHaveAttribute('data-workflow-run-id', runId!);
    await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
    const trajectory = page.getByRole('region', { name: 'Trajectory', exact: true });
    await trajectory.getByLabel('Trace category').selectOption('workflow');
    await expect(trajectory.locator('[data-trace-id]')).toHaveCount(1);
    await trajectory.locator('[data-trace-id] button').click();
    const inspector = trajectory.getByLabel('Trace record inspector');
    await inspector.getByRole('tab', { name: 'Timing', exact: true }).click();
    await expect(inspector).toContainText('Unavailable');
    await fixture.release('workflow-child-admitted');
    await page.getByRole('tab', { name: 'Chat', exact: true }).click();
    await expect(page.getByText('workflow conformance complete', { exact: true })).toBeVisible();
    await expect(run).toContainText('completed');
    await page.getByRole('tab', { name: 'Settings', exact: true }).click();
    await page.getByRole('button', { name: 'Integrations', exact: true }).click();
    const integrations = page.getByRole('region', { name: 'Integrations', exact: true });
    await expect(integrations.getByRole('group', { name: 'Named Agents · root selection' })).toContainText('reviewer');
    await expect(integrations.getByRole('group', { name: 'Workflows · root selection' })).toContainText('review_pr');
    await expect(integrations).toContainText('acceptance');
    await integrations.getByLabel('Integration scope').selectOption('workspace');
    await integrations.getByRole('checkbox', { name: /review_pr/ }).click();
    await expect(page.getByRole('status').filter({ hasText: 'Source committed' })).toBeVisible();
    await expect(integrations.getByRole('checkbox', { name: /review_pr/ })).not.toBeChecked();
    expect(readFileSync(workflowPath, 'utf8')).toBe(workflow);
    expect(readFileSync(agentPath, 'utf8')).toBe(agent);
    await page.screenshot({ path: 'test-results/workflow-inventory.png', fullPage: true });
    expect((await fixture.control('requests')).requests).toHaveLength(3);
    expect(errors).toEqual([]); passed = true;
  } catch (error) { console.error(fixture.diagnostics()); throw error; }
  finally { await page.close(); await fixture.stop(passed); }
});

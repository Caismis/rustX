import { connectRemote } from './shell-actions';
import { chooseWorkspace, closeSettings } from './shell-actions';
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
    await connectRemote(page, fixture.endpoint, fixture.token);
    await expect(page.getByLabel('Transport token')).toHaveCount(0);
  };
  try {
    await routeWorkspaceHost(page, fixture); await page.goto('/'); await connect();
    await chooseWorkspace(page, 'Workspace A');
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
    await page.screenshot({ path: test.info().outputPath('native-workflow-subagent.png') });
    await page.reload(); await connect();
    await expect(child).toHaveAttribute('data-subagent-id', childId!);
    await expect(run).toHaveAttribute('data-workflow-run-id', runId!);
    await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
    const trajectory = page.getByRole('region', { name: 'Trajectory', exact: true });
    await trajectory.getByLabel('Search loaded Trace').fill('workflow');
    await expect(trajectory.locator('[data-trace-id][data-kind="workflow"]')).toHaveCount(1);
    await trajectory.locator('[data-trace-id][data-kind="workflow"]').click();
    const inspector = trajectory.getByLabel('Trace record inspector');
    await inspector.getByRole('tab', { name: 'Timing', exact: true }).click();
    await expect(inspector).toContainText('Unavailable');
    await fixture.release('workflow-child-admitted');
    await closeSettings(page); await page.getByRole('tab', { name: 'Chat', exact: true }).click();
    await expect(page.getByText('workflow conformance complete', { exact: true })).toBeVisible();
    await expect(run).toContainText('completed');
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
    await settings.getByLabel('Configuration owner').selectOption({ label: 'Workspace A' });
    await settings.getByRole('button', { name: 'Agents', exact: true }).click();
    await expect(settings).toContainText('reviewer');
    await settings.getByRole('button', { name: 'Workflows', exact: true }).click();
    await expect(settings).toContainText('review_pr');
    await settings.screenshot({ path: test.info().outputPath('native-workflow-inventory.png') });
    await settings.getByRole('button', { name: 'Skills', exact: true }).click();
    await expect(settings).toContainText('acceptance');
    await settings.getByLabel('Configuration owner').selectOption({ label: 'Workspace A' });
    await settings.getByRole('button', { name: 'Agents & Workflows', exact: true }).click();
    await settings.getByRole('button', { name: 'Remove workflows 1', exact: true }).click();
    await settings.getByRole('button', { name: 'Save Workflow allowlist', exact: true }).click();
    await expect(settings.getByText(/Source saved. Native coordination/)).toBeVisible();
    expect(readFileSync(workflowPath, 'utf8')).toBe(workflow);
    expect(readFileSync(agentPath, 'utf8')).toBe(agent);
    expect((await fixture.control('requests')).requests).toHaveLength(3);
    expect(errors).toEqual([]); passed = true;
  } catch (error) { console.error(fixture.diagnostics()); throw error; }
  finally { await page.close(); await fixture.stop(passed); }
});

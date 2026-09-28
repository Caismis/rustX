import { expect, test } from '@playwright/test';
import { connectRemote, chooseWorkspace } from './shell-actions';
import { startDogfood } from './dogfood-server';
import { routeWorkspaceHost } from './workspace-host';

test('native interrupt and wait capture one activation; the same Agent resumes with canonical history', async ({ page }) => {
  const fixture = await startDogfood('web_agent_continuation');
  let passed = false;
  const errors: string[] = [];
  const requests = new Map<string | number, string>();
  const settlements: { method: string; result: Record<string, unknown> }[] = [];
  let capturedInitialSettlements!: () => void;
  const initialSettlements = new Promise<void>(resolve => { capturedInitialSettlements = resolve; });
  let capturedResume!: () => void;
  const resumeAcknowledged = new Promise<void>(resolve => { capturedResume = resolve; });
  page.on('pageerror', error => errors.push(error.message));
  page.on('websocket', socket => {
    socket.on('framesent', frame => {
      const request = JSON.parse(String(frame.payload));
      if (request.method === 'agent/wait' || request.method === 'agent/interrupt' || request.method === 'agent/sendMessage') requests.set(request.id, request.method);
    });
    socket.on('framereceived', frame => {
      const response = JSON.parse(String(frame.payload));
      const method = requests.get(response.id);
      if (method === 'agent/sendMessage' && response.result?.type === 'agent_message') { capturedResume(); return; }
      if (method && response.result) { settlements.push({ method, result: response.result }); if (settlements.length === 2) capturedInitialSettlements(); }
    });
  });
  try {
    await routeWorkspaceHost(page, fixture);
    await page.goto('/');
    await connectRemote(page, fixture.endpoint, fixture.token);
    await chooseWorkspace(page, 'Workspace A');
    await page.getByLabel('Message', { exact: true }).fill('WEB_AGENT_PARENT: delegate review');
    await page.getByRole('button', { name: 'Send', exact: true }).click();
    await Promise.all([fixture.gate('agent-initial-0'), fixture.gate('agent-initial-1')]);
    const recorded = (await fixture.control('requests')).requests;
    const parentIndex = [1, 2].find(index => JSON.stringify(recorded[index]).includes('WEB_AGENT_PARENT'))!;
    expect(parentIndex).toBeDefined();
    const parentGate = `agent-initial-${parentIndex - 1}`;
    const childGate = `agent-initial-${2 - parentIndex}`;
    const agent = page.locator('[data-agent-id]');
    await expect(agent).toHaveCount(1);
    await expect(agent).toHaveAttribute('data-agent-state', 'active');
    const agentId = await agent.getAttribute('data-agent-id');
    const first = await agent.getAttribute('data-activation-id');
    await agent.getByRole('button', { name: 'Transcript', exact: true }).click();
    await expect(agent.getByText('WEB_AGENT_CHILD: review the workspace', { exact: true })).toBeVisible();
    await agent.getByRole('button', { name: 'Wait for activation', exact: true }).click();
    await agent.getByRole('button', { name: 'Interrupt', exact: true }).click();
    await expect(agent).toHaveAttribute('data-agent-state', 'inactive');
    await expect(agent.getByText(`Activation ${first}: cancelled.`, { exact: true })).toBeVisible();
    await initialSettlements;
    expect(settlements.filter(row => row.result.activation_id === first)).toHaveLength(2);
    expect(settlements).toEqual(expect.arrayContaining(['agent/wait', 'agent/interrupt'].map(method => ({
      method, result: expect.objectContaining({ type: 'agent_wait', agent_id: agentId, activation_id: first, outcome: 'cancelled', agent: expect.objectContaining({ agent_id: agentId, state: 'inactive' }) }),
    }))));
    await expect(agent.getByRole('alert')).toHaveCount(0);
    await fixture.release(childGate);
    await agent.getByRole('textbox', { name: 'Message Agent reviewer' }).fill('WEB_AGENT_RESUME: continue the same review');
    await agent.getByRole('button', { name: 'Send message', exact: true }).click();
    await fixture.gate('agent-resumed');
    await resumeAcknowledged;
    await agent.getByRole('button', { name: 'Transcript', exact: true }).click();
    await expect(agent.getByText('WEB_AGENT_RESUME: continue the same review', { exact: true })).toBeVisible();
    await expect(agent).toHaveAttribute('data-agent-id', agentId!);
    await expect(agent).toHaveAttribute('data-agent-state', 'active');
    const second = await agent.getAttribute('data-activation-id');
    expect(second).not.toBe(first);
    await agent.getByRole('button', { name: 'Wait for activation', exact: true }).click();
    await fixture.release('agent-resumed');
    await expect(agent.getByText(`Activation ${second}: succeeded.`, { exact: true })).toBeVisible();
    await expect(agent.getByText('Resumed canonical child report.', { exact: true })).toBeVisible();
    await expect(agent).toHaveCount(1);
    await expect(agent.getByRole('alert')).toHaveCount(0);
    await fixture.release(parentGate);
    await expect(page.getByText('Parent received the resumed child report.', { exact: true })).toBeVisible();
    await page.getByRole('tab', { name: 'Trajectory', exact: true }).click();
    const trajectory = page.getByRole('region', { name: 'Trajectory', exact: true });
    const activations = trajectory.locator('[data-display-type="RecordRow"][data-kind="subagent"]');
    await expect(activations).toHaveCount(2);
    const inspector = trajectory.getByLabel('Trace record inspector');
    for (let index = 0; index < 2; index++) {
      await activations.nth(index).click();
      await inspector.getByRole('tab', { name: 'Native', exact: true }).click();
      await expect(inspector.getByText(agentId!, { exact: true })).toBeVisible();
      await expect(inspector.getByText(index === 0 ? 'Creation Tool' : 'Client control', { exact: true })).toBeVisible();
    }
    expect(errors).toEqual([]);
    passed = true;
  } catch (error) { console.error(fixture.diagnostics()); throw error; }
  finally { await page.close(); await fixture.stop(passed); }
});

import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { RuntimeClientAgent, RuntimeClientJob } from '../../protocol/app-server/v24';
import { RpcFailure } from '../src/client/app-server';
import { RuntimeFacts } from '../src/app/agent/Activity';
import { Server, snapshot } from './fixture';
import { Tool } from '../src/app/agent/Tool';
afterEach(cleanup);
const servers: Server[] = [];
afterEach(() => { for (const server of servers.splice(0)) server.client.disconnect(); });
export const agentFixture = (): RuntimeClientAgent => ({
  agent_id: 'agent-worker', parent_agent_id: 'parent-agent', child_conversation_id: 'conversation-worker', agent: 'Worker',
  activation_id: 'activation-a', current_activation: 'activation-a', state: 'active', activation_state: 'running',
  definition_digest: 'definition', profile_digest: 'profile', started_at: '2026-09-15T00:00:00Z',
  observation: { revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 0, model_retries: 0, tool_executions: 0 } },
  workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' }, resource_state: 'none' },
});
it('Active and Inactive send use one owner operation; wait remains interruptible and captures a native activation', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const agent = agentFixture(); s.agents = [agent]; server.snapshots.set('A', s);
  server.handlers.set('agent/sendMessage', () => ({ type: 'agent_message', agent_id: agent.agent_id, activation_id: 'activation-a', resumed: false }));
  server.handlers.set('agent/interrupt', () => ({ type: 'agent', agent: { ...agent, state: 'inactive', current_activation: null, activation_state: 'cancelled' } }));
  server.held.add('agent/wait');
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Wait for activation' })); await server.waitFor('agent/wait', 1); });
  expect((ui.getByRole('button', { name: 'Interrupt' }) as HTMLButtonElement).disabled).toBe(false);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Interrupt' })); await server.waitFor('agent/interrupt', 1); });
  await act(async () => {
    fireEvent.change(ui.getByRole('textbox', { name: 'Message Agent Worker' }), { target: { value: 'Active input' } });
    fireEvent.click(ui.getByRole('button', { name: 'Send message' })); await server.waitFor('agent/sendMessage', 1);
  });
  const inactive = { ...agent, state: 'inactive' as const, current_activation: null, activation_state: 'cancelled' as const };
  ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [inactive] }} client={server.client} sessionId="A"/>);
  await act(async () => {
    fireEvent.change(ui.getByRole('textbox', { name: 'Message Agent Worker' }), { target: { value: 'Later input' } });
    fireEvent.click(ui.getByRole('button', { name: 'Send message' })); await server.waitFor('agent/sendMessage', 2);
  });
  expect(server.requests.filter(row => row.request.method === 'agent/sendMessage').map(row => row.request)).toMatchObject([
    { params: { agent_id: 'agent-worker', message: 'Active input' } }, { params: { agent_id: 'agent-worker', message: 'Later input' } },
  ]);
  expect(server.requests.filter(row => row.request.method === 'agent/wait')).toHaveLength(1);
});

it('Job wait does not disable cancellation and terminal projections cannot resume or receive messages', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const job: RuntimeClientJob = { job_id: 'job-a', tool_id: 'tool-bash', tool_name: 'bash', state: 'running' }; s.jobs = [job]; server.snapshots.set('A', s);
  server.held.add('job/wait');
  server.handlers.set('job/cancel', () => ({ type: 'job', job: { ...job, state: 'cancelled' } }));
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Wait for Job' })); await server.waitFor('job/wait', 1); });
  expect((ui.getByRole('button', { name: 'Cancel Job' }) as HTMLButtonElement).disabled).toBe(false);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Cancel Job' })); await server.waitFor('job/cancel', 1); });
  ui.rerender(<RuntimeFacts snapshot={{ ...s, jobs: [{ ...job, state: 'cancelled' }] }} client={server.client} sessionId="A"/>);
  expect((ui.getByRole('button', { name: 'Cancel Job' }) as HTMLButtonElement).disabled).toBe(true);
  expect(ui.queryByRole('textbox')).toBeNull();
});

it('native Tool renderers explicitly distinguish Jobs and durable Agents', () => {
  for (const [name, domain] of [['job_wait', 'job'], ['send_message', 'agent'], ['subagent', 'agent']] as const) {
    const ui = render(<Tool tool={{ message_id: 'assistant', block_index: 0, call_id: `call-${name}`, tool_id: `tool-${name}`, name,
      state: { type: 'settled', arguments: JSON.stringify(domain === 'job' ? { job_id: 'job-a' } : { agent_id: 'agent-a' }), result: { status: { type: 'success' }, duration_ms: 0 } } }}/>);
    expect(ui.container.querySelector(`[data-activity-domain="${domain}"]`)).toBeTruthy();
    ui.unmount();
  }
});


it('a completed wait response cannot replace a newer resumed Agent projection', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const agent = agentFixture(); s.agents = [agent]; server.snapshots.set('A', s);
  await server.client.refresh('A');
  server.held.add('agent/wait');
  server.handlers.set('agent/wait', () => ({ type: 'agent_wait', agent_id: agent.agent_id, activation_id: 'activation-a', agent: { ...agent, state: 'inactive', current_activation: null, activation_state: 'succeeded' } }));
  function Harness() {
    const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
    return <RuntimeFacts snapshot={state.views.A.snapshot!} client={server.client} sessionId="A"/>;
  }
  const ui = render(<Harness/>);
  let waited: Awaited<ReturnType<Server['waitFor']>>;
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Wait for activation' })); waited = await server.waitFor('agent/wait', 1); });
  const row = ui.container.querySelector('[data-agent-id="agent-worker"]');
  server.snapshots.set('A', { ...s, agents: [{ ...agent, activation_id: 'activation-b', current_activation: 'activation-b' }] });
  await act(async () => { server.reply(waited!); await server.waitFor('session/snapshot', 2); });
  expect(ui.container.querySelector('[data-agent-id="agent-worker"]')).toBe(row);
  expect(row?.getAttribute('data-activation-id')).toBe('activation-b');
  expect(row?.textContent).not.toContain('Inactive');
  expect(ui.getByText('Activation activation-a settled.')).toBeTruthy();
});

it('selected child transcript refreshes canonical final content at settlement and stays open on resume', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const agent = agentFixture(); s.agents = [agent]; server.snapshots.set('A', s);
  let report = 'First committed child message';
  server.handlers.set('agent/transcript', () => ({ type: 'transcript', page: { entries: [{ cursor: '1', item: { type: 'message', message: { role: 'assistant', id: 'child-report', content: [{ type: 'text', text: report }] } } }] } }));
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Transcript' })); await server.waitFor('agent/transcript', 1); });
  expect(ui.getByText(report)).toBeTruthy();
  report = 'Final report from canonical child history';
  await act(async () => { ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [{ ...agent, state: 'inactive', current_activation: null, activation_state: 'succeeded' }] }} client={server.client} sessionId="A"/>); });
  await server.waitFor('agent/transcript', 2);
  expect(ui.getByText(report)).toBeTruthy();
  await act(async () => { ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [{ ...agent, activation_id: 'activation-b', current_activation: 'activation-b' }] }} client={server.client} sessionId="A"/>); });
  await server.waitFor('agent/transcript', 3);
  expect(ui.getByText(report)).toBeTruthy();
  expect(ui.container.querySelector('details')?.open).toBe(true);
});


it('native Stopping refusal preserves the draft and never falls back to a resume request', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); s.agents = [agentFixture()]; server.snapshots.set('A', s);
  server.handlers.set('agent/sendMessage', () => { throw new RpcFailure({ code: -32000, message: 'Agent is stopping; retry after settlement.' }); });
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  await act(async () => {
    fireEvent.change(ui.getByRole('textbox', { name: 'Message Agent Worker' }), { target: { value: 'Keep this input' } });
    fireEvent.click(ui.getByRole('button', { name: 'Send message' }));
    await server.waitFor('agent/sendMessage', 1);
  });
  expect(ui.getByRole('alert').textContent).toContain('Agent is stopping');
  expect((ui.getByRole('textbox', { name: 'Message Agent Worker' }) as HTMLInputElement).value).toBe('Keep this input');
  expect(server.requests.filter(row => row.request.method === 'agent/sendMessage')).toHaveLength(1);
  ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [{ ...agentFixture(), state: 'stopping', activation_state: 'stopping' }] }} client={server.client} sessionId="A"/>);
  expect((ui.getByRole('button', { name: 'Send message' }) as HTMLButtonElement).disabled).toBe(true);
  expect(ui.getByText('Stopping…')).toBeTruthy();
});

import { JobItemHarness } from './job-item-harness';
import { domainReceipt } from './fixture';
import { localeController } from '../src/locale/controller';
import { translator } from '../src/locale/translation';
import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import type { RuntimeClientAgent, RuntimeClientJob } from '../../protocol/app-server/v57';
import { RpcFailure } from '../src/client/errors';
import { RuntimeFacts as OtherActivity } from './runtime-facts';
import { AgentCard } from '../src/app/components/ActivityCards';
function RuntimeFacts(props: { snapshot: ReturnType<typeof snapshot>; client?: Server['client']; sessionId?: string }) { return <><OtherActivity snapshot={props.snapshot}/>{props.snapshot.jobs?.map(job => <JobItemHarness key={job.job_id} job={job} client={props.client} sessionId={props.sessionId}/>)}{props.snapshot.agents?.map(agent => <AgentCard key={agent.agent_id} agent={agent} client={props.client} sessionId={props.sessionId}/>)}</>; }
import { Server, snapshot, childConversation } from './fixture';
import { cfg3Effective } from './cfg3-data';
import { Tool } from '../src/app/agent/Tool';
beforeEach(() => { const host = document.createElement('div'); host.id = 'subagent-header-actions'; document.body.append(host); });
afterEach(() => { cleanup(); document.getElementById('subagent-header-actions')?.remove(); });
const servers: Server[] = [];
afterEach(() => { for (const server of servers.splice(0)) server.client.disconnect(); });
export const agentFixture = (): RuntimeClientAgent => ({ title: 'Worker',
  agent_id: 'agent-worker', parent_agent_id: 'parent-agent', child_conversation_id: 'conversation-worker', agent: 'Worker',
  activation_id: 'activation-a', current_activation: 'activation-a', state: 'active', activation_state: 'running',
  definition_digest: 'definition', profile_digest: 'profile', started_at: '2026-09-15T00:00:00Z',
  observation: { attempt_id: null, revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 0, model_retries: 0, tool_executions: 0 } },
  workspace: '/workspace',
});

it('intent receipts leave Agent and Job execution unchanged until their complete Root settlement publication', async () => {
  const server = new Server(); servers.push(server);
  const agent = agentFixture(), job: RuntimeClientJob = { job_id: 'build', tool_id: 'tool-bash', tool_name: 'Build', state: 'running' };
  const source = { ...snapshot(), agents: [agent], jobs: [job] }; server.snapshots.set('A', source);
  server.handlers.set('agent/observe', () => childConversation({ entries: [] }, agent.agent_id, agent.activation_id, agent.child_conversation_id));
  server.handlers.set('agent/interrupt', () => ({ type: 'agent_interruption_accepted', agent_id: agent.agent_id, activation_id: agent.activation_id }));
  server.handlers.set('job/cancel', () => ({ type: 'job_cancellation_accepted', job_id: job.job_id }));
  await server.attached('A');
  function Harness() {
    const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
    return <RuntimeFacts snapshot={state.views.A.snapshot!} client={server.client} sessionId="A"/>;
  }
  const ui = render(<Harness/>), row = ui.container.querySelector('[data-agent-id="agent-worker"]');
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'Stop' })));
  fireEvent.click(ui.getByRole('button', { name: /^Stop task / }));
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'Click again to confirm' })));
  expect(row?.getAttribute('data-agent-state')).toBe('active');
  expect(ui.container.querySelector('[data-job-state="running"]')).not.toBeNull();
  expect(ui.queryByText('cancelled', { exact: true })).toBeNull();
  await act(async () => server.update('A', { ...source,
    agents: [{ ...agent, state: 'inactive', current_activation: null, activation_state: 'cancelled' }],
    jobs: [{ ...job, state: 'cancelled', result: { status: { type: 'cancelled', reason: 'user_requested', phase: 'during_execution' }, duration_ms: '1', content: [{ type: 'text', text: 'Process settled' }] } }],
  }));
  expect(ui.container.querySelector('[data-agent-id="agent-worker"]')).toBe(row);
  expect(row?.getAttribute('data-agent-state')).toBe('inactive');
  expect(within(document.getElementById('subagent-header-actions')!).getByRole('status').textContent).toBe('cancelled');
  expect(ui.queryByRole('button', { name: /^Stop task / })).toBeNull();
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'Show output of Build' })));
  expect(ui.getByText('Process settled')).toBeTruthy();
  expect(server.requests.filter(({ request }) => request.method === 'agent/interrupt')).toHaveLength(1);
  expect(server.requests.filter(({ request }) => request.method === 'job/cancel')).toHaveLength(1);
});
it('Active and Inactive send use one owner operation; wait remains interruptible and captures a native activation', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const agent = agentFixture(); s.agents = [agent]; server.snapshots.set('A', s);
  server.handlers.set('agent/sendMessage', () => ({ type: 'agent_message', agent_id: agent.agent_id, activation_id: 'activation-a', resumed: false }));
  server.handlers.set('agent/interrupt', () => ({ type: 'agent_interruption_accepted', agent_id: agent.agent_id, activation_id: agent.activation_id }));
  server.held.add('agent/wait');
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Wait for activation' })); await server.waitFor('agent/wait', 1); });
  expect((ui.getByRole('button', { name: 'Stop' }) as HTMLButtonElement).disabled).toBe(false);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Stop' })); await server.waitFor('agent/interrupt', 1); });
  expect(ui.queryByText('cancelled')).toBeNull();
  await act(async () => server.socket.success(await server.waitFor('agent/wait', 1), { type: 'agent_wait', agent_id: agent.agent_id, activation_id: agent.activation_id, outcome: 'cancelled' }));
  expect(ui.getByText('cancelled')).toBeTruthy();
  expect(ui.queryByRole('alert')).toBeNull();
  await act(async () => {
    fireEvent.change(ui.getByRole('textbox', { name: 'Message Agent Worker' }), { target: { value: 'Active input' } });
    fireEvent.click(ui.getByRole('button', { name: /^(Send|Steer)$/ })); await server.waitFor('agent/sendMessage', 1);
  });
  const inactive = { ...agent, state: 'inactive' as const, current_activation: null, activation_state: 'cancelled' as const };
  ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [inactive] }} client={server.client} sessionId="A"/>);
  await act(async () => {
    fireEvent.change(ui.getByRole('textbox', { name: 'Message Agent Worker' }), { target: { value: 'Later input' } });
    fireEvent.click(ui.getByRole('button', { name: /^(Send|Steer)$/ })); await server.waitFor('agent/sendMessage', 2);
  });
  expect(server.requests.filter(row => row.request.method === 'agent/sendMessage').map(row => row.request)).toMatchObject([
    { params: { agent_id: 'agent-worker', message: 'Active input' } }, { params: { agent_id: 'agent-worker', message: 'Later input' } },
  ]);
  expect(server.requests.filter(row => row.request.method === 'agent/wait')).toHaveLength(1);
});

it('Job stopping needs a second press, stays pending until native publication, and offers no terminal controls', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(), job: RuntimeClientJob = { job_id: 'job-a', tool_id: 'tool-bash', tool_name: 'bash', state: 'running' };
  s.jobs = [job]; server.snapshots.set('A', s);
  server.handlers.set('job/cancel', () => ({ type: 'job_cancellation_accepted', job_id: job.job_id }));
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  fireEvent.click(ui.getByRole('button', { name: 'Stop task bash' }));
  expect(server.requests.some(row => row.request.method === 'job/cancel')).toBe(false);
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'Click again to confirm' })));
  expect((ui.getByRole('button', { name: 'Stop task bash' }) as HTMLButtonElement).disabled).toBe(true);
  expect(ui.container.querySelector('[data-job-state]')?.getAttribute('data-job-state')).toBe('running');
  ui.rerender(<RuntimeFacts snapshot={{ ...s, jobs: [{ ...job, state: 'cancelled' }] }} client={server.client} sessionId="A"/>);
  expect(ui.queryByRole('button', { name: /^Stop task / })).toBeNull();
  expect(ui.queryByRole('textbox')).toBeNull();
});

it('native Tool renderers explicitly distinguish Jobs and durable Agents', () => {
  for (const [name, domain] of [['job_wait', 'job'], ['send_message', 'agent'], ['subagent', 'agent']] as const) {
    const ui = render(<Tool tool={{committed_result_deferred:false,occurrence:{locator:{type:'by_message_id' as const,message_id:'assistant'},block_index:0},assembly_complete:true, message_id: 'assistant', block_index: 0, call_id: `call-${name}`, tool_id: `tool-${name}`, name,
      state: { type: 'settled', arguments: JSON.stringify(domain === 'job' ? { job_id: 'job-a' } : { agent_id: 'agent-a' }), result: { status: { type: 'success' }, duration_ms: "0" } } }}/>);
    expect(ui.container.querySelector(`[data-activity-domain="${domain}"]`)).toBeTruthy();
    ui.unmount();
  }
});


it.each(['agent/wait', 'agent/interrupt'] as const)('a completed %s response cannot replace a newer resumed Agent projection', async method => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const agent = agentFixture(); s.agents = [agent]; server.snapshots.set('A', s);
  await server.client.refresh('A');
  server.held.add(method);
  server.handlers.set(method, () => method === 'agent/interrupt' ? { type: 'agent_interruption_accepted', agent_id: agent.agent_id, activation_id: 'activation-a' } : { type: 'agent_wait', agent_id: agent.agent_id, activation_id: 'activation-a', outcome: 'succeeded' });
  if (method === 'agent/interrupt') {
    server.held.add('agent/wait');
    server.handlers.set('agent/wait', () => ({ type: 'agent_wait', agent_id: agent.agent_id, activation_id: 'activation-a', outcome: 'succeeded' }));
  }
  function Harness() {
    const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
    return <RuntimeFacts snapshot={state.views.A.snapshot!} client={server.client} sessionId="A"/>;
  }
  const ui = render(<Harness/>);
  let waited: Awaited<ReturnType<Server['waitFor']>>;
  if (method === 'agent/interrupt') await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Wait for activation' })); await server.waitFor('agent/wait', 1); });
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: method === 'agent/wait' ? 'Wait for activation' : 'Stop' })); waited = await server.waitFor(method, 1); });
  const row = ui.container.querySelector('[data-agent-id="agent-worker"]');
  server.snapshots.set('A', { ...s, agents: [{ ...agent, activation_id: 'activation-b', current_activation: 'activation-b' }] });
  await act(async () => {
    server.reply(waited!);
    if (method === 'agent/interrupt') server.reply(await server.waitFor('agent/wait', 1));
    await server.waitFor('session/checkpoint', 2);
  });
  expect(ui.container.querySelector('[data-agent-id="agent-worker"]')).toBe(row);
  expect(row?.getAttribute('data-activation-id')).toBe('activation-b');
  expect(row?.textContent).not.toContain('Inactive');
  expect(ui.getByText('succeeded')).toBeTruthy();
});

it('selected child transcript refreshes canonical final content at settlement and stays open on resume', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const agent = agentFixture(); s.agents = [agent]; server.snapshots.set('A', s);
  let report='First committed child message',activation=agent.activation_id;
  server.handlers.set('agent/observe', () => childConversation( { entries: [{ cursor: '1', item: { type: 'message', message: { role: 'assistant', id: 'child-report', content: [{ type: 'text', text: report }] } } }] },agent.agent_id,activation));
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  await act(async () => { await server.waitFor('agent/observe', 1); });
  expect(ui.getByText(report)).toBeTruthy();
  report = 'Final report from canonical child history';
  await act(async () => { ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [{ ...agent, state: 'inactive', current_activation: null, activation_state: 'succeeded' }] }} client={server.client} sessionId="A"/>); });
  await server.waitFor('agent/observe', 2);
  expect(ui.getByText(report)).toBeTruthy();
  activation='activation-b';
  await act(async () => { ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [{ ...agent, activation_id: 'activation-b', current_activation: 'activation-b' }] }} client={server.client} sessionId="A"/>); });
  await server.waitFor('agent/observe', 3);
  expect(ui.getByText(report)).toBeTruthy();
  expect(ui.getByText(report)).toBeTruthy();
});


it('native Stopping refusal preserves the draft and never falls back to a resume request', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); s.agents = [agentFixture()]; server.snapshots.set('A', s);
  server.handlers.set('agent/sendMessage', () => { throw new RpcFailure({ code: -32000, message: 'Agent is stopping or admitting an activation; retry after settlement', data: { kind: 'agent_stopping', agent_id: 'agent-worker' } }); });
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  await act(async () => {
    fireEvent.change(ui.getByRole('textbox', { name: 'Message Agent Worker' }), { target: { value: 'Keep this input' } });
    fireEvent.click(ui.getByRole('button', { name: /^(Send|Steer)$/ }));
    await server.waitFor('agent/sendMessage', 1);
  });
  expect(ui.getByRole('alert').textContent).toContain('Agent is stopping');
  expect((ui.getByRole('textbox', { name: 'Message Agent Worker' }) as HTMLInputElement).value).toBe('Keep this input');
  expect(server.requests.filter(row => row.request.method === 'agent/sendMessage')).toHaveLength(1);
  ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [{ ...agentFixture(), state: 'stopping', activation_state: 'stopping' }] }} client={server.client} sessionId="A"/>);
  expect((ui.getByRole('textbox', {name:'Message Agent Worker'}) as HTMLTextAreaElement).disabled).toBe(true);
  expect(ui.getByText('Stopping…')).toBeTruthy();
});

it('native admission remains waitable and interruptible while its send is pending', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const agent = { ...agentFixture(), state: 'inactive' as const, current_activation: null, activation_state: 'succeeded' as const };
  s.agents = [agent]; server.snapshots.set('A', s);
  server.held.add('agent/sendMessage'); server.held.add('agent/wait');
  server.handlers.set('agent/interrupt', () => ({ type: 'agent_interruption_accepted', agent_id: agent.agent_id, activation_id: 'admission-b' }));
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  await act(async () => {
    fireEvent.change(ui.getByRole('textbox', { name: 'Message Agent Worker' }), { target: { value: 'Resume input' } });
    fireEvent.click(ui.getByRole('button', { name: /^(Send|Steer)$/ })); await server.waitFor('agent/sendMessage', 1);
  });
  const row = ui.container.querySelector('[data-agent-id="agent-worker"]');
  ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [{ ...agent, state: 'admitting', current_activation: 'admission-b' }] }} client={server.client} sessionId="A"/>);
  expect(ui.getByText('Admitting…')).toBeTruthy();
  expect(row?.getAttribute('data-activation-id')).toBe('admission-b');
  expect((ui.getByRole('textbox', {name:'Message Agent Worker'}) as HTMLTextAreaElement).disabled).toBe(true);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Wait for activation' })); await server.waitFor('agent/wait', 1); });
  expect((ui.getByRole('button', { name: 'Stop' }) as HTMLButtonElement).disabled).toBe(false);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Stop' })); await server.waitFor('agent/interrupt', 1); });
  expect(ui.queryByText('Agent was inactive when the operation observed it.')).toBeNull();
  await act(async () => server.socket.success(await server.waitFor('agent/wait', 1), { type: 'agent_wait', agent_id: agent.agent_id, activation_id: 'admission-b', outcome: null }));
  expect(ui.getByText('Agent was inactive when the operation observed it.')).toBeTruthy();
  expect((ui.getByRole('textbox', { name: 'Message Agent Worker' }) as HTMLInputElement).value).toBe('Resume input');
  expect(ui.queryByRole('alert')).toBeNull();
});

it('native seal reopening restores Active controls on the same Agent and activation', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const agent = agentFixture(); s.agents = [agent]; server.snapshots.set('A', s);
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  fireEvent.change(ui.getByRole('textbox', { name: 'Message Agent Worker' }), { target: { value: 'Keep draft across sealing' } });
  const row = ui.container.querySelector('[data-agent-id="agent-worker"]');
  ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [{ ...agent, state: 'stopping', activation_state: 'stopping' }] }} client={server.client} sessionId="A"/>);
  expect((ui.getByRole('textbox', {name:'Message Agent Worker'}) as HTMLTextAreaElement).disabled).toBe(true);
  ui.rerender(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  expect(ui.container.querySelector('[data-agent-id="agent-worker"]')).toBe(row);
  expect(row?.getAttribute('data-activation-id')).toBe('activation-a');
  expect(ui.getByText('Working')).toBeTruthy();
  expect((ui.getByRole('button', { name: /^(Send|Steer)$/ }) as HTMLButtonElement).disabled).toBe(false);
  expect((ui.getByRole('textbox', { name: 'Message Agent Worker' }) as HTMLInputElement).value).toBe('Keep draft across sealing');
});

it('owner Unavailable differs from terminal Inactive and disables impossible settlement controls', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const agent = agentFixture(); s.agents = [agent]; server.snapshots.set('A', s);
  server.handlers.set('agent/sendMessage', () => { throw new RpcFailure({ code: -32000, message: 'Agent is unavailable; physical settlement, publication, or workspace authority requires explicit repair', data: { kind: 'agent_settlement', agent_id: agent.agent_id } }); });
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  await act(async () => {
    fireEvent.change(ui.getByRole('textbox', { name: 'Message Agent Worker' }), { target: { value: 'Preserved draft' } });
    fireEvent.click(ui.getByRole('button', { name: /^(Send|Steer)$/ })); await server.waitFor('agent/sendMessage', 1);
  });
  expect(ui.getByRole('alert').textContent).toContain('remains unresolved');
  expect(ui.getByRole('alert').textContent).not.toContain('retry');
  const unavailable = { ...agent, state: 'unavailable' as const, activation_state: 'interrupted' as const };
  ui.rerender(<RuntimeFacts snapshot={{ ...s, agents: [unavailable] }} client={server.client} sessionId="A"/>);
  expect(ui.getByText('Unavailable')).toBeTruthy();
  expect(ui.queryByText('Inactive')).toBeNull();
  expect(ui.queryByRole('button', { name: 'Stop' })).toBeNull();
  expect(ui.queryByRole('button', { name: 'Send' })).toBeNull();
  expect(ui.getByText('This agent is unavailable. Its conversation is read-only.')).toBeTruthy();
  expect((ui.getByRole('button', { name: 'Wait for activation' }) as HTMLButtonElement).disabled).toBe(true);
  expect((ui.getByRole('button', { name: 'Transcript' }) as HTMLButtonElement).disabled).toBe(false);
  expect((ui.getByLabelText('Message Agent Worker') as HTMLInputElement).value).toBe('Preserved draft');
  expect(server.requests.filter(row => row.request.method === 'agent/sendMessage')).toHaveLength(1);
});

it('Starting activation uses authoritative Admitting owner state', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); s.agents = [{ ...agentFixture(), state: 'admitting', activation_state: 'stopping' }];
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  expect(ui.getByText('Admitting…')).toBeTruthy();
  expect(ui.queryByText('Stopping…')).toBeNull();
  expect((ui.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
  expect((ui.getByRole('textbox', { name: 'Message Agent Worker' }) as HTMLTextAreaElement).disabled).toBe(true);
  expect((ui.getByRole('button', { name: 'Stop' }) as HTMLButtonElement).disabled).toBe(false);
});

it('Job cancellation failure preserves native owner state and translates the diagnostic', async () => {
  const method = 'job/cancel' as const;
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const job: RuntimeClientJob = { job_id: 'job-a', tool_id: 'tool-bash', tool_name: 'bash', state: 'running' }; s.jobs = [job]; server.snapshots.set('A', s);
  server.handlers.set(method, () => { throw new RpcFailure({ code: -32000, message: 'Publication failed', data: { kind: 'job_publication_abandoned', job_id: job.job_id } }); });
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  fireEvent.click(ui.getByRole('button', { name: 'Stop task bash' }));
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Click again to confirm' })); await server.waitFor(method, 1); });
  expect(ui.getByRole('alert').textContent).toContain('Job job-a terminal publication was abandoned');
  expect(ui.queryByText('Waiting for settlement…')).toBeNull();
  expect(ui.queryByText('Request pending…')).toBeNull();
  expect(ui.container.querySelector('[data-job-state]')?.getAttribute('data-job-state')).toBe('running');
  expect(server.requests.filter(row => row.request.method === method)).toHaveLength(1);
  const requests = [...server.requests];
  act(() => localeController.setLocale('zh'));
  expect(ui.getByRole('alert').textContent).toBe(translator('zh')('common:activity.publication-abandoned', { id: job.job_id }));
  expect(ui.container.querySelector('[data-job-state]')?.getAttribute('data-job-state')).toBe('running');
  expect(server.requests).toEqual(requests);
  act(() => localeController.setLocale('en'));
});

it('Job list returns the complete owner metadata through the Web client', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const jobs = Array.from({ length: 32 }, (_, index): RuntimeClientJob => ({ job_id: `exec_00000000-0000-7000-8000-${(66 - index).toString(16).padStart(12, "0")}`, tool_id: 'tool-bash', tool_name: 'bash', state: 'succeeded' }));
  const listing = domainReceipt('jobs',jobs,{total:67,next:(jobs).length});
  server.handlers.set('job/list', () => listing);
  const received = await server.client.request({ method: 'job/list', params: { target: server.client.target('A') } }, 'domain_page');
  expect(received).toEqual(listing);
  expect((received.page.items[0].value as RuntimeClientJob).job_id).toBe('exec_00000000-0000-7000-8000-000000000042');
  expect((received.page.items.at(-1)?.value as RuntimeClientJob).job_id).toBe('exec_00000000-0000-7000-8000-000000000023');
});

it.each([['agent_not_delivered', 'input was not delivered'], ['agent_delivery_unknown', 'do not replay automatically']] as const)('send-message renders %s and preserves the unacknowledged draft', async (kind, expected) => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const s = snapshot(); const agent = { ...agentFixture(), state: 'inactive' as const, current_activation: null }; s.agents = [agent]; server.snapshots.set('A', s);
  server.handlers.set('agent/sendMessage', () => { throw new RpcFailure({ code: -32000, message: 'Delivery failed', data: { kind, agent_id: agent.agent_id } }); });
  const ui = render(<RuntimeFacts snapshot={s} client={server.client} sessionId="A"/>);
  await act(async () => {
    fireEvent.change(ui.getByRole('textbox', { name: 'Message Agent Worker' }), { target: { value: 'Keep this guidance' } });
    fireEvent.click(ui.getByRole('button', { name: /^(Send|Steer)$/ })); await server.waitFor('agent/sendMessage', 1);
  });
  expect(ui.getByRole('alert').textContent).toContain(expected);
  expect((ui.getByRole('textbox', { name: 'Message Agent Worker' }) as HTMLInputElement).value).toBe('Keep this guidance');
  const requests = [...server.requests];
  const input = ui.getByRole('textbox');
  act(() => localeController.setLocale('zh'));
  expect(ui.getByRole('alert').textContent).toBe(translator('zh')(kind === 'agent_not_delivered' ? 'common:activity.not-delivered' : 'common:activity.delivery-unknown', { id: agent.agent_id }));
  expect(ui.getByRole('textbox')).toBe(input);
  expect((input as HTMLInputElement).value).toBe('Keep this guidance');
  expect(ui.container.querySelector('[data-agent-id]')?.getAttribute('data-agent-id')).toBe(agent.agent_id);
  expect(server.requests).toEqual(requests);
  act(() => localeController.setLocale('en'));
  expect(server.requests.filter(row => row.request.method === 'agent/sendMessage')).toHaveLength(1);
});

it('a wait response from an obsolete attachment cannot publish an activation result', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const state = snapshot(); const agent = agentFixture(); state.agents = [agent];
  server.snapshots.set('A', state); server.held.add('agent/wait');
  const ui = render(<RuntimeFacts snapshot={state} client={server.client} sessionId="A"/>);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Wait for activation' })); await server.waitFor('agent/wait', 1); });
  const request = server.requests.find(row => row.request.method === 'agent/wait')!.request;
  await act(async () => { await server.client.release('A'); await server.client.attach('A'); });
  const current = server.client.getSnapshot().views.A;
  await act(async () => { server.socket.success(request, { type: 'agent_wait', agent_id: agent.agent_id, activation_id: 'obsolete-activation', outcome: 'succeeded' }); });
  expect(ui.queryByText(/obsolete-activation/)).toBeNull();
  expect(server.client.getSnapshot().views.A).toBe(current);
  expect(server.requests.filter(row => row.request.method === 'agent/wait')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'agent/interrupt')).toHaveLength(0);
});

it('child transcript cannot publish from a retained cleanup claim and fresh proof starts a fresh read', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  server.held.add('agent/observe'); server.held.add('session/detach');
  const ui = render(<AgentCard agent={agentFixture()} client={server.client} sessionId="A"/>);
  const old = await server.waitFor('agent/observe', 1);
  let release!: Promise<void>;
  await act(async () => { release = server.client.release('A'); void release.catch(() => {}); await server.waitFor('session/detach', 1); });
  const page = (text: string) => ({ entries: [{ cursor: '1', item: { type: 'message' as const, message: { id: text, role: 'assistant' as const, content: [{ type: 'text' as const, text }] } } }] });
  await act(async () => { server.socket.success(old, childConversation(page('obsolete child result'))); });
  expect(ui.queryByText('obsolete child result')).toBeNull(); expect(ui.queryByText('Reading child conversation…')).toBeNull();
  expect(server.claims()).toHaveLength(1);
  await act(async () => { server.reply(await server.waitFor('session/detach', 1)); await release; await server.client.attach('A'); });
  const fresh = await server.waitFor('agent/observe', 2);
  await act(async () => { server.socket.success(fresh, childConversation(page('current child result'))); });
  expect(ui.getByText('current child result')).toBeTruthy(); expect(ui.queryByText('obsolete child result')).toBeNull();
  expect(server.requests.filter(row => row.request.method === 'agent/observe')).toHaveLength(2);
  expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(2);
  expect(server.requests.filter(row => row.request.method === 'session/detach')).toHaveLength(1); expect(server.claims()).toHaveLength(1);
});

it('older child transcript retains its original observation proof through native response', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const originalTarget = server.client.target('A');
  const page = (text: string, cursor: string) => ({ entries: [{ cursor, item: { type: 'message' as const, message: { id: text, role: 'assistant' as const, content: [{ type: 'text' as const, text }] } } }], next_cursor: cursor });
  server.handlers.set('agent/observe', () => childConversation(page('current child page', '10')));
  server.held.add('agent/transcript');
  const ui = render(<AgentCard agent={agentFixture()} client={server.client} sessionId="A"/>);
  await act(async () => { await server.waitFor('agent/observe', 1); });
  expect(ui.getByText('current child page')).toBeTruthy();
  server.held.add('agent/observe'); server.held.add('agent/transcript'); server.held.add('session/detach');
  await act(async () => { await server.waitFor('agent/transcript', 1); });
  const older = await server.waitFor('agent/transcript', 1); let release!: Promise<void>;
  await act(async () => { release = server.client.release('A'); void release.catch(() => {}); await server.waitFor('session/detach', 1); });
  await act(async () => { server.socket.success(older, { type: 'transcript_window', window: { cut: { conversation_id: 'conversation-worker', journal: '10', transcript: '10', mutation_revision: '0' ,ledger_through:'0',surface_revision:'0',transcript_total:'0',pending_through:'0',pending_total:'0'}, page: page('obsolete older page', '1') } }); });
  expect(ui.queryByText('obsolete older page')).toBeNull(); expect(ui.getByText('current child page')).toBeTruthy();
  expect(server.requests.filter(row => row.request.method === 'agent/transcript' && row.request.params.target.attachment_id === originalTarget.attachment_id)).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'session/checkpoint')).toHaveLength(1);
  await act(async () => { server.reply(await server.waitFor('session/detach', 1)); await release; }); expect(server.claims()).toHaveLength(0);
  await act(async () => { await server.client.attach('A'); });
  expect(ui.queryByText('current child page')).toBeNull();
  const fresh = await server.waitFor('agent/observe', 2);
  await act(async () => { server.socket.success(fresh, childConversation(page('fresh child page', '20'))); });
  expect(ui.getByText('fresh child page')).toBeTruthy(); expect(ui.queryByText('current child page')).toBeNull();
  expect(server.requests.filter(row => row.request.method === 'agent/transcript' && row.request.params.target.attachment_id === originalTarget.attachment_id)).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(2);
  expect(server.requests.filter(row => row.request.method === 'session/detach')).toHaveLength(1); expect(server.claims()).toHaveLength(1);
});

it.each(['wait-release', 'interrupt-switch', 'message-release'] as const)('%s cannot publish through a retained cleanup target', async kind => {
  const server = new Server(); servers.push(server); await server.attached('A');
  const agent = agentFixture(), method = kind === 'wait-release' ? 'agent/wait' : kind === 'interrupt-switch' ? 'agent/interrupt' : 'agent/sendMessage';
  server.nodeSnapshots.set('right', { ...snapshot(), conversation_id: 'conversation-right' });
  server.held.add(method); server.held.add('session/detach'); server.held.add('session/switchNode');
  const ui = render(<AgentCard agent={agent} client={server.client} sessionId="A"/>);
  await act(async () => {
    if (kind === 'message-release') { fireEvent.change(ui.getByRole('textbox'), { target: { value: 'Preserve draft' } }); fireEvent.click(ui.getByRole('button', { name: /^(Send|Steer)$/ })); }
    else fireEvent.click(ui.getByRole('button', { name: kind === 'wait-release' ? 'Wait for activation' : 'Stop' }));
    await server.waitFor(method, 1);
  });
  const old = await server.waitFor(method, 1); let cleanup!: Promise<void>;
  await act(async () => { cleanup = kind === 'interrupt-switch' ? server.client.switchNode('A', 'right') : server.client.release('A'); void cleanup.catch(() => {}); await server.waitFor(kind === 'interrupt-switch' ? 'session/switchNode' : 'session/detach', 1); });
  const retained = server.client.getSnapshot().views.A.target; expect(retained).toEqual(server.target('A')); expect(server.claims()).toHaveLength(1);
  await act(async () => { server.socket.success(old, kind === 'message-release'
    ? { type: 'agent_message', agent_id: agent.agent_id, activation_id: agent.activation_id, resumed: false }
    : kind === 'interrupt-switch' ? { type: 'agent_interruption_accepted', agent_id: agent.agent_id, activation_id: 'obsolete' }
    : { type: 'agent_wait', agent_id: agent.agent_id, activation_id: 'obsolete', outcome: 'succeeded' }); });
  expect(ui.queryByText('succeeded')).toBeNull(); expect(ui.queryByRole('alert')).toBeNull();
  // Native accepted input consumes its exact editor revision even though
  // the old observation has lost permission to publish Agent state.
  if (kind === 'message-release') expect((ui.getByRole('textbox') as HTMLTextAreaElement).value).toBe('');
  expect(server.requests.filter(r => r.request.method === 'session/checkpoint')).toHaveLength(1);
  expect(server.requests.filter(r => r.request.method === method)).toHaveLength(1);
  await act(async () => { server.reply(await server.waitFor(kind === 'interrupt-switch' ? 'session/switchNode' : 'session/detach', 1)); await cleanup; });
  expect(server.claims()).toHaveLength(0);
});

it('old child completion cannot clear a fresh proof operation or restore its error', async () => {
  const server = new Server(); servers.push(server); await server.attached('A'); const agent = agentFixture(); server.held.add('agent/wait');
  const ui = render(<AgentCard agent={agent} client={server.client} sessionId="A"/>);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Wait for activation' })); await server.waitFor('agent/wait', 1); });
  const old = await server.waitFor('agent/wait', 1);
  await act(async () => { await server.client.release('A'); await server.client.attach('A'); });
  expect((ui.getByRole('button', { name: 'Wait for activation' }) as HTMLButtonElement).disabled).toBe(false);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Wait for activation' })); await server.waitFor('agent/wait', 2); });
  await act(async () => { server.socket.deliver({ jsonrpc: '2.0', id: old.id, error: { code: -32000, message: 'obsolete error', data: { kind: 'operation_failed' } } }); });
  expect(ui.queryByText('obsolete error')).toBeNull(); expect((ui.getByRole('button', { name: 'Wait for activation' }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => { server.socket.success(await server.waitFor('agent/wait', 2), { type: 'agent_wait', agent_id: agent.agent_id, activation_id: 'fresh', outcome: 'cancelled' }); });
  expect(ui.getByText('cancelled')).toBeTruthy(); expect((ui.getByRole('button', { name: 'Wait for activation' }) as HTMLButtonElement).disabled).toBe(false);
  expect(server.requests.filter(r=>r.request.method==='agent/wait')).toHaveLength(2);expect(server.requests.filter(r=>r.request.method==='session/checkpoint')).toHaveLength(3);
  expect(server.claims()).toHaveLength(1); expect(server.maxClaims).toBe(1);
});

it('current wait settlement remains independent from interrupt completion', async () => {
  const server = new Server(); servers.push(server); await server.attached('A'); const agent = agentFixture();
  const result = { type: 'agent_wait' as const, agent_id: agent.agent_id, activation_id: agent.activation_id, outcome: 'cancelled' as const };
  server.held.add('agent/wait'); server.handlers.set('agent/interrupt', () => ({ type: 'agent_interruption_accepted', agent_id: agent.agent_id, activation_id: agent.activation_id }));
  const ui = render(<AgentCard agent={agent} client={server.client} sessionId="A"/>);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Wait for activation' })); await server.waitFor('agent/wait', 1); });
  expect((ui.getByRole('button', { name: 'Stop' }) as HTMLButtonElement).disabled).toBe(false);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Stop' })); await server.waitFor('agent/interrupt', 1); });
  expect(ui.queryByText('cancelled')).toBeNull(); expect((ui.getByRole('button', { name: 'Wait for activation' }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => { server.socket.success(await server.waitFor('agent/wait', 1), result); await server.waitFor('session/checkpoint', 1); });
  expect(ui.getByText('cancelled')).toBeTruthy(); expect((ui.getByRole('button', { name: 'Wait for activation' }) as HTMLButtonElement).disabled).toBe(false);
  expect(server.requests.filter(r => r.request.method === 'agent/wait')).toHaveLength(1); expect(server.requests.filter(r => r.request.method === 'agent/interrupt')).toHaveLength(1); expect(server.claims()).toHaveLength(1);
});

it('T11/T17 hides cancel the child wait and reopening resumes its locally Applied Cursor', async () => {
  const { checkpoint } = await import('./fixture');
  const server = new Server(); servers.push(server); await server.attached('A');
  const agent = agentFixture(), cursor = {agent_id:agent.agent_id,conversation_id:agent.child_conversation_id,activation_id:agent.activation_id,cursor:'10'};
  server.held.add('agent/observe');
  const card = (visible:boolean) => <AgentCard agent={agent} client={server.client} sessionId="A" visible={visible}/>;
  const ui = render(card(true));
  const first = await server.waitFor('agent/observe',1);
  expect(first.params).toMatchObject({after:null});
  await act(async () => server.socket.success(first,{type:'agent_observe',observation:{type:'checkpoint',agent_id:agent.agent_id,conversation_id:agent.child_conversation_id,activation_id:agent.activation_id,source:'live',cursor,checkpoint:{...checkpoint({...snapshot(),conversation_id:agent.child_conversation_id},'10'),read_cut:{...checkpoint({...snapshot(),conversation_id:agent.child_conversation_id},'10').read_cut,activation_id:agent.activation_id}}}}));
  const oldWait = await server.waitFor('agent/observe',2);
  expect(oldWait.params).toMatchObject({after:cursor});
  await act(async () => { ui.rerender(card(false)); });
  const cancelled = await server.waitFor('agent/observeCancel',1);
  expect(cancelled.params).toEqual({request_id:oldWait.id});
  await act(async () => { ui.rerender(card(true)); });
  expect(server.requests.filter(row=>row.request.method==='agent/observe')).toHaveLength(2);
  await act(async () => server.socket.success(oldWait,{type:'agent_observe',observation:{type:'events',after:cursor,cursor:{...cursor,cursor:'11'},events:[{cursor:'11',event:{type:'message_committed',message:{role:'assistant',id:'obsolete-view-message',content:[{type:'text',text:'Late hidden view content'}]}}}]}}));
  expect(ui.queryByText('Late hidden view content')).toBeNull();
  const resumed = await server.waitFor('agent/observe',3);
  expect(resumed.params).toMatchObject({after:cursor});
  await act(async () => server.socket.success(resumed,{type:'agent_observe',observation:{type:'events',after:cursor,cursor:{...cursor,cursor:'11'},events:[{cursor:'11',event:{type:'projection_transition',events:[{type:'trace_changed'}],metadata:{collections:[],content:[],read_cut:{...checkpoint({...snapshot(),conversation_id:agent.child_conversation_id},'11').read_cut,activation_id:agent.activation_id}}}}]}}));
  const following = await server.waitFor('agent/observe',4);
  expect(following.params).toMatchObject({after:{...cursor,cursor:'11'}});
  expect(server.requests.some(row=>['agent/interrupt','conversation/cancel','job/cancel'].includes(row.request.method))).toBe(false);
});

it('T14/T17 actual Child watch rejects a stale inner revision and resumes only from explicit native repair', async () => {
  const { checkpoint } = await import('./fixture');
  const server = new Server(); servers.push(server); await server.attached('A'); server.held.add('agent/observe');
  const agent = agentFixture(), identity = { agent_id: agent.agent_id, conversation_id: agent.child_conversation_id, activation_id: agent.activation_id };
  const scoped = (cursor: string) => { const value = checkpoint({ ...snapshot(), conversation_id: identity.conversation_id }, cursor); value.read_cut.activation_id = identity.activation_id; return value; };
  const ui = render(<AgentCard agent={agent} client={server.client} sessionId="A"/>);
  const first = await server.waitFor('agent/observe', 1);
  await act(async () => server.socket.success(first, { type: 'agent_observe', observation: { type: 'checkpoint', ...identity, source: 'live', cursor: { ...identity, cursor: '10' }, checkpoint: scoped('10') } }));
  const waiting = await server.waitFor('agent/observe', 2);
  await act(async () => server.socket.success(waiting, { type: 'agent_observe', observation: { type: 'events', after: { ...identity, cursor: '10' }, cursor: { ...identity, cursor: '11' }, events: [{ cursor: '11', event: { type: 'projection_transition', events: [{ type: 'message_committed', message: { role: 'assistant', id: 'refused-child-row', content: [{ type: 'text', text: 'stale inner cut must not render' }] } }], metadata: { read_cut: scoped('10').read_cut, collections: [], content: [] } } }] } }));
  expect(ui.queryByText('stale inner cut must not render')).toBeNull(); expect(ui.getByRole('alert').textContent).toContain('Child event cut evidence mismatch');
  expect(server.requests.filter(row => row.request.method === 'agent/observe')).toHaveLength(2);
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Transcript' })); });
  const repair = await server.waitFor('agent/observe', 3); expect(repair.params).toMatchObject({ after: null });
  await act(async () => server.socket.success(repair, { type: 'agent_observe', observation: { type: 'checkpoint', ...identity, source: 'live', cursor: { ...identity, cursor: '11' }, checkpoint: scoped('11') } }));
  const following = await server.waitFor('agent/observe', 4); expect(following.params).toMatchObject({ after: { ...identity, cursor: '11' } });
  await act(async () => server.socket.success(following, { type: 'agent_observe', observation: { type: 'events', after: { ...identity, cursor: '11' }, cursor: { ...identity, cursor: '12' }, events: [{ cursor: '12', event: { type: 'projection_transition', events: [], metadata: { read_cut: scoped('12').read_cut, collections: [], content: [] } } }] } }));
  expect((await server.waitFor('agent/observe', 5)).params).toMatchObject({ after: { ...identity, cursor: '12' } });
  expect(ui.queryByRole('alert')).toBeNull(); expect(server.requests.some(row => ['agent/interrupt', 'conversation/cancel', 'job/cancel'].includes(row.request.method))).toBe(false);
});

it('a failed child read retains content and draft, exposes explicit recovery, and never replays Agent execution', async () => {
  const { checkpoint } = await import('./fixture');
  const server = new Server(); servers.push(server); await server.attached('A'); server.held.add('agent/observe');
  const agent = agentFixture();
  const receipt = (text: string, cursor: string, live: boolean) => {
    const result = childConversation({ entries: [{ cursor: '1', item: { type: 'message', message: { role: 'assistant', id: 'child-report', content: [{ type: 'text', text }] } } }] }, agent.agent_id, agent.activation_id, agent.child_conversation_id);
    if (result.observation.type !== 'checkpoint') throw Error('Fixture checkpoint');
    const model = cfg3Effective().effective_model!;
    result.observation.checkpoint = checkpoint({ ...result.observation.checkpoint.snapshot, effective_approval_mode: 'full_access', model: { ...model, effective: { ...model.effective!, model: 'child/own-model' } } }, cursor);
    result.observation.checkpoint.read_cut.activation_id = agent.activation_id;
    if (live) { result.observation.source = 'live'; result.observation.cursor = { agent_id: agent.agent_id, conversation_id: agent.child_conversation_id, activation_id: agent.activation_id, cursor }; }
    return result;
  };
  const ui = render(<AgentCard agent={agent} client={server.client} sessionId="A"/>);
  const first = await server.waitFor('agent/observe', 1);
  await act(async () => server.socket.success(first, receipt('Retained child answer', '10', true)));
  const second = await server.waitFor('agent/observe', 2);
  const input = ui.getByRole('textbox', { name: 'Message Agent Worker' }) as HTMLTextAreaElement;
  fireEvent.change(input, { target: { value: 'Retained child draft' } });
  expect(ui.queryByRole('button', { name: 'Model and thinking effort' })).toBeNull();
  expect((ui.getByRole('button', { name: 'Workspace permissions' }) as HTMLButtonElement).disabled).toBe(true);
  const diagnostic = 'child observation: Connection reset by peer (os error 104)';
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: second.id, error: { code: -32000, message: 'Operation rejected', data: { kind: 'observation_unavailable', diagnostic } } }));
  expect(ui.getByText('Retained child answer')).toBeTruthy(); expect(input.value).toBe('Retained child draft');
  const notice = ui.container.querySelector('[data-child-observation-status]')!;
  expect(notice.querySelector('details')?.open).toBe(false); expect(notice.querySelector('pre')?.textContent).toContain(diagnostic);
  expect(ui.container.querySelector('[data-composer-seat]')?.contains(notice)).toBe(true);
  expect((ui.getByRole('button', { name: 'Stop' }) as HTMLButtonElement).disabled).toBe(false);
  expect((ui.getByRole('button', { name: 'Steer' }) as HTMLButtonElement).disabled).toBe(false);
  expect(server.requests.filter(row => row.request.method === 'agent/observe')).toHaveLength(2);
  const retry = ui.getByRole('button', { name: 'Child status unavailable. Click to retry' });
  await act(async () => { fireEvent.click(retry); });
  const repair = await server.waitFor('agent/observe', 3); expect(repair.params).toMatchObject({ after: null });
  const pending = ui.getByRole('button', { name: 'Reconnecting to child…' }) as HTMLButtonElement;
  expect(pending.disabled).toBe(true); fireEvent.click(pending);
  expect(ui.getByText('Retained child answer')).toBeTruthy(); expect(input.value).toBe('Retained child draft');
  await act(async () => server.socket.success(repair, receipt('Recovered canonical child answer', '11', false)));
  expect(ui.getByText('Recovered canonical child answer')).toBeTruthy();
  expect(ui.container.querySelector('[data-child-observation-status]')).toBeNull(); expect(input.value).toBe('Retained child draft');
  expect(server.requests.filter(row => row.request.method === 'agent/observe')).toHaveLength(3);
  expect(server.requests.some(row => ['agent/sendMessage', 'agent/interrupt', 'turn/start'].includes(row.request.method))).toBe(false);
});

it('a running child keeps independent Send and Stop, and double Escape targets its own activation', async () => {
  const server = new Server(); servers.push(server); await server.attached('A'); server.held.add('agent/observe'); server.held.add('agent/interrupt');
  const agent = agentFixture(), ui = render(<AgentCard agent={agent} client={server.client} sessionId="A"/>);
  const input = ui.getByRole('textbox', { name: 'Message Agent Worker' }) as HTMLTextAreaElement;
  expect((ui.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
  expect((ui.getByRole('button', { name: 'Stop' }) as HTMLButtonElement).disabled).toBe(false);
  fireEvent.change(input, { target: { value: 'Keep this child draft' } }); act(() => input.focus());
  expect((ui.getByRole('button', { name: 'Steer' }) as HTMLButtonElement).disabled).toBe(false);
  for (let n = 0; n < 2; n++) { fireEvent.keyDown(input, { key: 'Escape' }); fireEvent.keyUp(input, { key: 'Escape' }); }
  const request = await server.waitFor('agent/interrupt', 1); expect(request.params).toMatchObject({ agent_id: agent.agent_id });
  expect(input.value).toBe('Keep this child draft'); expect(ui.container.querySelector('[data-agent-state="active"]')).toBeTruthy();
  expect(server.requests.filter(row => row.request.method === 'agent/interrupt')).toHaveLength(1);
  expect(server.requests.some(row => row.request.method === 'agent/sendMessage')).toBe(false);
});

import { agentMetrics } from './agent-statistics-fixture';
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { Message } from '../src/app/agent/Message';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { RuntimeFacts } from '../src/app/agent/Activity';
import { agentDuration, SubagentScope, SubagentHeader, SubagentSurface } from '../src/app/agent/Subagents';
import { cellKind } from '../src/app/trajectory/TrajectoryCell';
import { Server, snapshot } from './fixture';
import type { RuntimeClientAgent, MessageBlock } from '../../protocol/app-server/v39';
const servers: Server[] = [];
afterEach(() => { cleanup(); for (const server of servers.splice(0)) server.client.disconnect(); });
const agent: RuntimeClientAgent = { agent_id: 'child', agent: 'Research', parent_agent_id: 'root', child_conversation_id: 'child-conversation', activation_id: 'activation', state: 'inactive', activation_state: 'succeeded', definition_digest: 'd', profile_digest: 'p', started_at: '2026-10-08T00:00:00Z', observation: { revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 1, model_retries: 0, tool_executions: 0 } }, workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' }, resource_state: 'none' } };
const report: MessageBlock = { role: 'user', id: 'return', source: { agent: { agent_id: 'child' } }, content: [{ type: 'text', text: '**Research result**' }] };
it('agent return is a context disclosure, while a human message remains a user bubble', () => {
  const s = snapshot(); s.transcript = { entries: [{ cursor: '1', item: { type: 'message', message: report } }] };
  const ui = render(<AgentTranscript snapshot={s}/>);
  expect(ui.container.querySelector('[data-inbound-source="agent"]')).toBeTruthy();
  expect(ui.container.querySelector('[data-user-message]')).toBeNull();
  expect(ui.queryByLabelText('Message actions')).toBeNull();
  fireEvent.click(ui.getByRole('button', { name: /Message from/ }));
  expect(ui.container.querySelector('strong')?.textContent).toBe('Research result');
  ui.rerender(<Message message={{ ...report, source: 'human' }}/>);
  expect(ui.container.querySelector('[data-inbound-source]')).toBeNull();
});
it('agents move out of the message footer and open through the header without losing parent draft', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  server.handlers.set('agent/statistics', () => ({ type: 'agent_statistics', metrics: agentMetrics }));
  server.snapshots.get('A')!.agents = [agent]; await server.client.refresh('A');
  const ui = render(<SubagentScope client={server.client} sessionId="A"><SubagentHeader title="Parent"/><SubagentSurface><input aria-label="Root draft" defaultValue="keep me"/><RuntimeFacts snapshot={server.snapshots.get('A')!}/></SubagentSurface></SubagentScope>);
  expect(ui.queryByLabelText('Agent Research')).toBeNull();
  fireEvent.click(ui.getByRole('button', { name: 'Subagents' }));
  await act(async () => { fireEvent.click(ui.getByRole('menuitem', { name: /Research/ })); });
  expect(ui.getByLabelText('Agent Research')).toBeTruthy();
  expect(await ui.findByRole('button', { name: /12K tok/ })).toBeTruthy();
  expect(ui.container.querySelector('[data-composer-dock]')).toBeTruthy();
  expect(ui.getByText('10%')).toBeTruthy();
  expect(ui.queryByRole('textbox', { name: 'Root draft' })).toBeNull();
  fireEvent.click(ui.getByRole('button', { name: 'Parent' }));
  expect((ui.getByRole('textbox', { name: 'Root draft' }) as HTMLInputElement).value).toBe('keep me');
});
it('trajectory classifies native agent inbound records as context', () => {
  expect(cellKind({ type: 'RecordRow', record: { kind: 'user', agent_id: 'child' } } as Parameters<typeof cellKind>[0])).toBe('context');
});

it('native active clocks freeze when inactive and exclude idle time', () => {
  const metrics = { ...agentMetrics, duration: { settled_ms: '15000', active: { started_at: '2026-10-08T00:00:00Z', observed_at: '2026-10-08T00:00:02Z', running: false } } };
  expect(agentDuration(metrics, Date.parse('2026-10-08T00:10:00Z'))).toBe(17000);
  metrics.duration.active.running = true;
  expect(agentDuration(metrics, Date.parse('2026-10-08T00:00:05Z'))).toBe(20000);
});

it('large child lists bound background meter requests without blocking the parent', async () => {
  const server = new Server(); servers.push(server); await server.attached('A');
  server.snapshots.get('A')!.agents = Array.from({ length: 80 }, (_, index) => ({ ...agent, agent_id: `child-${index}` }));
  await server.client.refresh('A');
  server.held.add('agent/statistics');
  const { useSubagents } = await import('../src/app/agent/subagent-context');
  function PickLast() { const scope = useSubagents()!; return <button onClick={() => scope.open('child-79')}>Pick last child</button>; }
  const ui = render(<SubagentScope client={server.client} sessionId="A"><input aria-label="Parent input"/><PickLast/></SubagentScope>);
  const reads = () => server.requests.filter(row => row.request.method === 'agent/statistics');
  await waitFor(() => expect(reads()).toHaveLength(2));
  fireEvent.change(ui.getByRole('textbox'), { target: { value: 'Still usable' } });
  expect((ui.getByRole('textbox') as HTMLInputElement).value).toBe('Still usable');
  fireEvent.click(ui.getByText('Pick last child'));
  expect(reads()).toHaveLength(2);
  await act(async () => {
    server.held.delete('agent/statistics');
    for (const row of reads()) server.reply(row.request);
  });
  await waitFor(() => expect(reads()).toHaveLength(80));
  expect((reads()[2].request.params as { agent_id: string }).agent_id).toBe('child-79');
  expect(new Set(reads().map(row => (row.request.params as { agent_id: string }).agent_id)).size).toBe(80);
  await waitFor(() => expect(server.client.agentMeters.getSnapshot().readings.size).toBe(80));
});

it('an obsolete attachment cannot publish late meters or overwrite its successor', async () => {
  const { useSubagents } = await import('../src/app/agent/subagent-context');
  const server = new Server(); servers.push(server); server.snapshots.get('A')!.agents = [agent];
  await server.attached('A'); server.held.add('agent/statistics');
  function Meter() { return <output>{useSubagents()?.metrics.child?.duration.settled_ms ?? 'unknown'}</output>; }
  const ui = render(<SubagentScope client={server.client} sessionId="A"><Meter/></SubagentScope>);
  const old = await server.waitFor('agent/statistics', 1);
  await act(async () => { await server.client.release('A'); await server.client.attach('A'); });
  const current = await server.waitFor('agent/statistics', 2);
  await act(async () => server.socket.success(current, { type: 'agent_statistics', metrics: { ...agentMetrics, duration: { settled_ms: '999', active: null } } }));
  expect(ui.getByText('999')).toBeTruthy();
  await act(async () => server.reply(old));
  expect(ui.getByText('999')).toBeTruthy();
  ui.rerender(<SubagentScope client={server.client} sessionId="A"><Meter/></SubagentScope>);
  expect(server.requests.filter(row => row.request.method === 'agent/statistics')).toHaveLength(2);
});

it('two stalled A reads do not block B; repeated scope switches retain a four-request bound and late A cannot publish', async () => {
  const server = new Server(); servers.push(server); await server.attached('A', 'B');
  server.handlers.set('agent/statistics', () => ({ type: 'agent_statistics', metrics: agentMetrics }));
  for (const id of ['A', 'B']) { server.snapshots.get(id)!.agents = Array.from({ length: 5 }, (_, i) => ({ ...agent, agent_id: `child-${i}` })); await server.client.refresh(id); }
  server.held.add('agent/statistics');
  const { useSubagents } = await import('../src/app/agent/subagent-context');
  function Readings() { return <output>{Object.keys(useSubagents()!.metrics).join(',') || 'unknown'}</output>; }
  const view = (id: string) => <SubagentScope client={server.client} sessionId={id}><Readings/></SubagentScope>;
  const ui = render(view('A')); await server.waitFor('agent/statistics', 2);
  ui.rerender(view('B')); await server.waitFor('agent/statistics', 4);
  const reads = () => server.requests.filter(row => row.request.method === 'agent/statistics');
  expect(reads().map(row => (row.request.params as { target: { session_id: string } }).target.session_id)).toEqual(['A', 'A', 'B', 'B']);
  for (let i = 0; i < 20; i++) { ui.rerender(view('A')); ui.rerender(view('B')); }
  expect(reads()).toHaveLength(4);
  await act(async () => { server.reply(reads()[0].request); await server.waitFor('agent/statistics', 5); });
  expect(ui.getByText('unknown')).toBeTruthy();
  expect((reads()[4].request.params as { target: { session_id: string } }).target.session_id).toBe('B');
  await act(async () => { server.reply(reads()[4].request); });
  await ui.findByText('child-0');
});

it('connection replacement invalidates observations without releasing unacknowledged native work', async () => {
  const server = new Server(); servers.push(server);
  server.snapshots.get('A')!.agents = [agent, { ...agent, agent_id: 'second' }];
  await server.attached('A'); server.held.add('agent/statistics');
  const ui = render(<SubagentScope client={server.client} sessionId="A"><SubagentHeader title="Parent"/></SubagentScope>);
  await server.waitFor('agent/statistics', 2);
  await act(async () => { await server.client.disconnect(); await server.attached('A'); });
  await server.waitFor('agent/statistics', 4);
  await act(async () => { await server.client.disconnect(); await server.attached('A'); });
  expect(server.requests.filter(row => row.request.method === 'agent/statistics')).toHaveLength(4);
  expect(server.client.agentMeters.getSnapshot().blocked).toBe(true);
  ui.unmount();
  render(<SubagentScope client={server.client} sessionId="A"><SubagentHeader title="Parent"/></SubagentScope>);
  expect(server.requests.filter(row => row.request.method === 'agent/statistics')).toHaveLength(4);
});

it('obsolete queued meter reads fail the existing transport admission proof before any RPC is sent', async () => {
  const server = new Server(); servers.push(server);
  for (const id of ['A', 'B']) server.snapshots.get(id)!.agents = [agent, { ...agent, agent_id: 'second' }];
  await server.attached('A', 'B'); server.held.add('session/statistics'); server.held.add('agent/statistics');
  const pending = Array.from({ length: 8 }, () => server.client.request({ method: 'session/statistics', params: { session_id: 'A' } }, 'session_statistics').catch(() => {}));
  const blockers = server.requests.filter(row => row.request.method === 'session/statistics').slice(-8);
  expect(blockers).toHaveLength(8);
  const view = (id: string) => <SubagentScope client={server.client} sessionId={id}><span/></SubagentScope>;
  const ui = render(view('A')); ui.rerender(view('B'));
  expect(server.requests.filter(row => row.request.method === 'agent/statistics')).toHaveLength(0);
  await act(async () => { for (const blocker of blockers) server.reply(blocker.request); await Promise.all(pending); });
  await server.waitFor('agent/statistics', 2);
  expect(server.requests.filter(row => row.request.method === 'agent/statistics').map(row => (row.request.params as { target: { session_id: string } }).target.session_id)).toEqual(['B', 'B']);
});

it('a correlated native failure frees capacity, renders an error without zero usage and is not retried by rerenders', async () => {
  const { RpcFailure } = await import('../src/client/app-server');
  const { useSubagents } = await import('../src/app/agent/subagent-context');
  const server = new Server(); servers.push(server); server.snapshots.get('A')!.agents = [agent]; await server.attached('A');
  server.handlers.set('agent/statistics', () => { throw new RpcFailure({ code: -32000, message: 'durable read unavailable' }); });
  function Reading() { const scope = useSubagents()!; return <output>{scope.metrics.child ? 'metrics' : 'unknown'}:{scope.metricErrors.child}</output>; }
  const view = <SubagentScope client={server.client} sessionId="A"><Reading/></SubagentScope>;
  const ui = render(view); await ui.findByText(/unknown:durable read unavailable/);
  ui.rerender(view);
  await act(async () => { await server.client.request({ method: 'session/statistics', params: { session_id: 'A' } }, 'session_statistics'); });
  expect(server.requests.filter(row => row.request.method === 'agent/statistics')).toHaveLength(1);
  expect(server.client.agentMeters.getSnapshot().blocked).toBe(false);
});

it('Release revokes meter admission and publication before a held detach acknowledges, then Open services fresh demands', async () => {
  const { useSubagents } = await import('../src/app/agent/subagent-context');
  const server = new Server(); servers.push(server);
  server.snapshots.get('A')!.agents = [agent, { ...agent, agent_id: 'second' }, { ...agent, agent_id: 'third' }, { ...agent, agent_id: 'fourth' }];
  await server.attached('A'); server.held.add('agent/statistics'); server.held.add('session/detach');
  function Reading() { return <output>{Object.keys(useSubagents()!.metrics).length}</output>; }
  const ui = render(<SubagentScope client={server.client} sessionId="A"><Reading/></SubagentScope>);
  const reads = () => server.requests.filter(row => row.request.method === 'agent/statistics');
  await server.waitFor('agent/statistics', 2);
  let release!: Promise<void>;
  await act(async () => {
    release = server.client.release('A');
    expect(server.client.getSnapshot().views.A.attachmentIntent).toBe('released');
    expect(server.client.getSnapshot().views.A.attachment).toBe('attached');
    server.reply(reads()[0].request);
  });
  expect(reads()).toHaveLength(2); expect(ui.getByText('0')).toBeTruthy();
  await act(async () => { server.reply(reads()[1].request); });
  expect(reads()).toHaveLength(2); expect(ui.getByText('0')).toBeTruthy();
  await act(async () => { server.reply(await server.waitFor('session/detach', 1)); await release; });
  await act(async () => { server.held.delete('agent/statistics'); await server.client.attach('A'); });
  expect(reads()).toHaveLength(6); expect(ui.getByText('4')).toBeTruthy();
});

it('Release and batched Open cannot admit any replacement scope against T1 before native T2 attach', async () => {
  const { useSubagents } = await import('../src/app/agent/subagent-context');
  const server = new Server(); servers.push(server); server.snapshots.get('A')!.agents = Array.from({ length: 4 }, (_, i) => ({ ...agent, agent_id: `child-${i}` }));
  await server.attached('A'); server.held.add('agent/statistics'); server.held.add('session/detach'); server.held.add('session/attach');
  function Reading() { return <output>{Object.keys(useSubagents()!.metrics).length}</output>; }
  const ui = render(<SubagentScope client={server.client} sessionId="A"><Reading/></SubagentScope>);
  const reads = () => server.requests.filter(row => row.request.method === 'agent/statistics');
  await server.waitFor('agent/statistics', 2);
  const old = server.client.agentMeters.getSnapshot().scope!, oldTarget = server.target('A');
  let release!: Promise<void>, reopen!: Promise<void>;
  await act(async () => { release = server.client.release('A'); reopen = server.client.attach('A'); expect(old.current()).toBe(false); });
  const replacement = server.client.agentMeters.getSnapshot().scope!;
  expect(replacement).not.toBe(old); expect(replacement.current()).toBe(false);
  expect(server.client.getSnapshot().views.A.target).toEqual(oldTarget); expect(() => server.client.target('A')).toThrow('not authoritatively attached'); expect(reads()).toHaveLength(2);
  for (const row of reads()) {
    await act(async () => { server.reply(row.request); });
    expect(reads()).toHaveLength(2); expect(ui.getByText('0')).toBeTruthy();
  }
  await act(async () => { server.reply(await server.waitFor('session/detach', 1)); await release; });
  const attaching = await server.waitFor('session/attach', 2);
  expect(reads()).toHaveLength(2); expect(server.client.getSnapshot().views.A.attachmentObservation).toBeUndefined();
  await act(async () => { server.reply(attaching); await reopen; });
  const newTarget = server.target('A'); expect(newTarget.attachment_id).not.toBe(oldTarget.attachment_id);
  expect(reads()).toHaveLength(4);
  for (let i = 2; i < 6; i++) await act(async () => { server.reply(reads()[i].request); });
  expect(reads()).toHaveLength(6); expect(ui.getByText('4')).toBeTruthy();
  expect(reads().slice(2).every(row => (row.request.params as { target: { attachment_id: string } }).target.attachment_id === newTarget.attachment_id)).toBe(true);
});

it('late T1 results cannot overwrite admitted T2 statistics after Release and reattach', async () => {
  const { useSubagents } = await import('../src/app/agent/subagent-context');
  const server = new Server(); servers.push(server); server.snapshots.get('A')!.agents = [agent, { ...agent, agent_id: 'second' }];
  await server.attached('A'); server.held.add('agent/statistics');
  function Reading() { return <output>{Object.values(useSubagents()!.metrics).map(value => value.duration.settled_ms).join(',') || 'unknown'}</output>; }
  const ui = render(<SubagentScope client={server.client} sessionId="A"><Reading/></SubagentScope>);
  const reads = () => server.requests.filter(row => row.request.method === 'agent/statistics');
  await server.waitFor('agent/statistics', 2);
  await act(async () => { await server.client.release('A'); await server.client.attach('A'); });
  expect(reads()).toHaveLength(4);
  await act(async () => { for (const row of reads().slice(2)) server.socket.success(row.request, { type: 'agent_statistics', metrics: { ...agentMetrics, duration: { settled_ms: '999', active: null } } }); });
  expect(ui.getByText('999,999')).toBeTruthy();
  await act(async () => { for (const row of reads().slice(0, 2)) server.reply(row.request); });
  expect(ui.getByText('999,999')).toBeTruthy(); expect(reads()).toHaveLength(4);
});

it('failed detach retains T1 but Open and refresh cannot mint observation admission; explicit detach and attach recover', async () => {
  const server = new Server(); servers.push(server); server.snapshots.get('A')!.agents = Array.from({ length: 4 }, (_, i) => ({ ...agent, agent_id: `child-${i}` }));
  await server.attached('A'); server.held.add('agent/statistics'); server.held.add('session/detach');
  render(<SubagentScope client={server.client} sessionId="A"><span/></SubagentScope>);
  const reads = () => server.requests.filter(row => row.request.method === 'agent/statistics');
  await server.waitFor('agent/statistics', 2); const target = server.target('A');
  await act(async () => {
    const rejected = expect(server.client.release('A')).rejects.toThrow('detach refused');
    const opening = expect(server.client.attach('A')).rejects.toThrow('Release the retained attachment');
    server.socket.deliver({ jsonrpc: '2.0', id: (await server.waitFor('session/detach', 1)).id, error: { code: -32000, message: 'detach refused' } });
    await rejected; await opening;
    for (const row of reads()) server.reply(row.request);
  });
  expect(server.client.getSnapshot().views.A).toMatchObject({ target, attachment: 'error', attachmentIntent: 'wanted' });
  expect(server.client.getSnapshot().views.A.attachmentObservation).toBeUndefined();
  expect(reads()).toHaveLength(2); expect(server.claims()).toHaveLength(1);
  await act(async () => { server.held.delete('session/detach'); server.held.delete('agent/statistics'); await server.client.release('A'); await server.client.attach('A'); });
  expect(reads()).toHaveLength(6); expect(server.target('A').attachment_id).not.toBe(target.attachment_id);
});

it('Release fences unsent meter requests waiting behind the real eight-slot RPC lane', async () => {
  const server = new Server(); servers.push(server); server.snapshots.get('A')!.agents = [agent, { ...agent, agent_id: 'second' }];
  await server.attached('A'); server.held.add('session/statistics'); server.held.add('session/detach');
  const blockers = Array.from({ length: 8 }, () => server.client.request({ method: 'session/statistics', params: { session_id: 'A' } }, 'session_statistics'));
  const requests = server.requests.filter(row => row.request.method === 'session/statistics').slice(-8);
  render(<SubagentScope client={server.client} sessionId="A"><span/></SubagentScope>);
  let releasing!: Promise<void>;
  await act(async () => { releasing = server.client.release('A'); for (const row of requests) server.reply(row.request); await Promise.all(blockers); });
  expect(server.requests.filter(row => row.request.method === 'agent/statistics')).toHaveLength(0);
  await act(async () => { server.reply(await server.waitFor('session/detach', 1)); await releasing; });
  expect(server.requests.filter(row => row.request.method === 'agent/statistics')).toHaveLength(0);
});

it.each([1, 80])('actual 64-request saturation defers %i stable Agent demands until capacity returns, without render retries', async count => {
  const { useSubagents } = await import('../src/app/agent/subagent-context');
  const server = new Server(); servers.push(server);
  server.snapshots.get('A')!.agents = Array.from({ length: count }, (_, i) => ({ ...agent, agent_id: `child-${i}` }));
  await server.attached('A');
  server.held.add('session/statistics'); server.held.add('agent/statistics');
  const baseline = server.requests.filter(row => row.request.method === 'session/statistics').length;
  const blockers = Array.from({ length: 64 }, () => server.client.request({ method: 'session/statistics', params: { session_id: 'A' } }, 'session_statistics').catch(() => {}));
  const request = vi.spyOn(server.client, 'request');
  const attempts = () => request.mock.calls.filter(([op]) => op.method === 'agent/statistics');
  const reads = () => server.requests.filter(row => row.request.method === 'agent/statistics');
  function Reading() { const scope = useSubagents()!; return <output>{Object.keys(scope.metrics).length}:{Object.keys(scope.metricErrors).length}</output>; }
  const view = () => <SubagentScope client={server.client} sessionId="A"><Reading/></SubagentScope>;
  const ui = render(view());
  await act(async () => {});
  expect(attempts()).toHaveLength(Math.min(count, 2)); expect(reads()).toHaveLength(0);
  expect(ui.getByText('0:0')).toBeTruthy();
  for (let i = 0; i < 10; i++) ui.rerender(view());
  await act(async () => {});
  expect(attempts()).toHaveLength(Math.min(count, 2));
  const first = server.requests.filter(row => row.request.method === 'session/statistics')[baseline];
  await act(async () => { server.reply(first.request); });
  expect(attempts().length).toBeGreaterThan(Math.min(count, 2));
  expect(ui.getByText('0:0')).toBeTruthy();
  await act(async () => {
    server.held.delete('session/statistics');
    for (const row of server.requests.filter(row => row.request.method === 'session/statistics').slice(baseline + 1)) server.reply(row.request);
    await Promise.all(blockers);
  });
  let maximum = 0;
  for (let i = 0; i < count; i++) {
    await server.waitFor('agent/statistics', i + 1);
    maximum = Math.max(maximum, reads().length - i);
    await act(async () => { server.reply(reads()[i].request); });
  }
  expect(reads()).toHaveLength(count); expect(new Set(reads().map(row => (row.request.params as { agent_id: string }).agent_id)).size).toBe(count);
  expect(maximum).toBe(Math.min(count, 2)); expect(ui.getByText(`${count}:0`)).toBeTruthy();
  await act(async () => { server.snapshots.get('A')!.agents = server.snapshots.get('A')!.agents!.map(row => ({ ...row })); await server.client.refresh('A'); });
  for (let i = 0; i < 10; i++) ui.rerender(view());
  expect(reads()).toHaveLength(count);
  request.mockRestore();
});

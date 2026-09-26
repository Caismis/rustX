// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { useSyncExternalStore } from 'react';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { SessionConfiguration } from '../src/app/SessionConfiguration';
import type { Request, RuntimeClientEvent } from '../../protocol/app-server/v25';
import { cfg3Application } from './cfg3-data';
import { Server, snapshot } from './fixture';
afterEach(cleanup);
const servers: Server[] = [];
const server = () => { const value = new Server(); servers.push(value); return value; };
afterEach(() => { for (const s of servers.splice(0)) s.client.disconnect(); });
const reads = (s: Server) => s.requests.filter(item => item.request.method === 'session/configuration');
/** App.tsx shape: the subscribed view flows back in, so a refreshed snapshot identity reaches SessionConfiguration. */
function Harness({ s }: { s: Server }) {
  const state = useSyncExternalStore(s.client.subscribe, s.client.getSnapshot);
  return <SessionConfiguration client={s.client} view={state.views.A}/>;
}
async function settlementReread(event: RuntimeClientEvent) {
  const s = server(); await s.attached('A');
  let settled = false;
  s.handlers.set('session/configuration', () => ({ type: 'session_configuration', application: { ...cfg3Application(), scope: 'A', eligibility: { status: settled ? 'eligible' : 'busy' } } }));
  render(<Harness s={s}/>);
  expect((await screen.findByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText(/Session work must settle/)).toBeTruthy();
  expect(reads(s)).toHaveLength(1);
  expect(s.requests.filter(item => item.request.method === 'session/snapshot')).toHaveLength(0);
  settled = true; s.cursor++; s.snapshots.set('A', snapshot('A'));
  let snapshotRequest: Request | undefined;
  await act(async () => { s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: s.target('A'), cursor: String(s.cursor), event } }); snapshotRequest = await s.waitFor('session/snapshot', 1); });
  expect(snapshotRequest).toMatchObject({ method: 'session/snapshot', params: { target: s.target('A') } });
  await s.waitFor('session/configuration', 2);
  const methods = s.requests.map(item => item.request.method);
  expect(methods.indexOf('session/snapshot')).toBeLessThan(methods.lastIndexOf('session/configuration'));
  await waitFor(() => expect((screen.getByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement).disabled).toBe(false));
  expect(reads(s)).toHaveLength(2);
  expect(s.requests.filter(item => item.request.method === 'session/snapshot')).toHaveLength(1);
  expect(s.requests.filter(item => item.request.method === 'session/adoptConfiguration')).toHaveLength(0);
}

it('C15 background settlement observation drives authoritative configuration reread', () => settlementReread({
  type: 'job_updated',
  job: { job_id: 'execution-1', tool_id: 'bash', tool_name: 'bash', state: 'succeeded' },
}));

it('C15 subagent settlement observation drives authoritative configuration reread', () => settlementReread({
  type: 'agent_updated',
  agent: {
    activation_id: 'subagent-1', agent_id: 'agent-1', parent_agent_id: 'parent-agent', current_activation: null, activation_state: 'succeeded', child_conversation_id: 'conversation-child-1', agent: 'worker',
    definition_digest: 'definition-1', profile_digest: 'profile-1', state: 'inactive',
    observation: { revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 1, model_retries: 0, tool_executions: 0 } },
    started_at: '2026-09-21T00:00:00Z', workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' }, resource_state: 'none' },
  },
}));

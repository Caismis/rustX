// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { useSyncExternalStore } from 'react';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { SessionConfiguration } from '../src/app/SessionConfiguration';
import type { AdoptionEligibility, ConfigurationApplication, RuntimeClientEvent } from '../../protocol/app-server/v35';
import { cfg3Application } from './cfg3-data';
import { Server, snapshot } from './fixture';
import { RpcFailure } from '../src/client/app-server';
afterEach(cleanup);
const servers: Server[] = [];
afterEach(() => { for (const s of servers.splice(0)) s.client.disconnect(); });
const reads = (s: Server) => s.requests.filter(item => item.request.method === 'session/configuration');
const snapshotReads = (s: Server) => s.requests.filter(item => item.request.method === 'session/snapshot');
const adoptions = (s: Server) => s.requests.filter(item => item.request.method === 'session/adoptConfiguration');
const unavailableLine = () => screen.queryByText(/Configuration status unavailable/);
const adopt = () => screen.queryByRole('button', { name: 'Adopt configuration' }) as HTMLButtonElement | null;

/** App.tsx shape: the subscribed view flows back in on every client
 * publication — including every streamed delta. */
function Harness({ s }: { s: Server }) {
  const state = useSyncExternalStore(s.client.subscribe, s.client.getSnapshot);
  return <SessionConfiguration client={s.client} view={state.views.A}/>;
}

/** A Session A whose native application is `application` and whose live
 * runtime published `eligibility` on its Runtime Client snapshot. */
async function session(application: ConfigurationApplication | null, eligibility: AdoptionEligibility['status'] = 'eligible') {
  const s = new Server(); servers.push(s);
  s.snapshots.set('A', { ...snapshot('A'), configuration_adoption_eligibility: { status: eligibility } });
  s.handlers.set('session/configuration', () => ({ type: 'session_configuration', application }));
  await s.attached('A');
  render(<Harness s={s}/>);
  await waitFor(() => expect(reads(s)).toHaveLength(1));
  return s;
}
function emit(s: Server, event: RuntimeClientEvent) {
  s.cursor++;
  s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: s.target('A'), cursor: String(s.cursor), event } });
}
const withCandidate = () => ({ ...cfg3Application(), scope: 'A' });
const settled = () => ({ ...cfg3Application(), scope: 'A', candidate: null, units: { execution_policy: { status: 'applied' as const } } });

it('C18 hundreds of streamed deltas never read Session configuration and never flicker the banner', async () => {
  const s = await session(withCandidate());
  await waitFor(() => expect(adopt()?.disabled).toBe(false));
  // Admission: the runtime publishes Busy once, then streams.
  await act(async () => emit(s, { type: 'configuration_adoption_eligibility_changed', eligibility: { status: 'busy' } }));
  await act(async () => emit(s, { type: 'attempt_started', attempt_id: 'attempt' }));
  await act(async () => emit(s, { type: 'assistant_message_started', attempt_id: 'attempt', message_id: 'message' }));
  const views = new Set<unknown>();
  for (let delta = 0; delta < 300; delta++) {
    await act(async () => emit(s, { type: 'assistant_text_delta', attempt_id: 'attempt', message_id: 'message', block_index: 0, delta: 'x' }));
    views.add(s.client.getSnapshot().views.A.snapshot);
    // Every frame: the same known observation, never "unavailable".
    expect(unavailableLine()).toBeNull();
    expect(adopt()?.disabled).toBe(true);
    expect(screen.getByText(/Session work must settle/)).toBeTruthy();
  }
  // Each delta really published a new Session snapshot identity...
  expect(views.size).toBe(300);
  expect(s.client.getSnapshot().views.A.snapshot?.attempt?.in_flight?.blocks).toEqual([{ type: 'text', block_index: 0, text: 'x'.repeat(300) }]);
  // ...and none of them was a configuration fact.
  expect(reads(s)).toHaveLength(1);
  expect(snapshotReads(s)).toHaveLength(0);
  // Settlement: the runtime publishes Eligible; still no configuration read.
  await act(async () => emit(s, { type: 'attempt_settled', attempt_id: 'attempt', outcome: { type: 'completed', finish_reason: { type: 'stop' } } }));
  expect(adopt()?.disabled).toBe(true);
  await act(async () => emit(s, { type: 'configuration_adoption_eligibility_changed', eligibility: { status: 'eligible' } }));
  await waitFor(() => expect(adopt()?.disabled).toBe(false));
  expect(reads(s)).toHaveLength(1);
  expect(adoptions(s)).toHaveLength(0);
});

it('C18 with nothing to adopt the banner stays absent through a streamed answer', async () => {
  const s = await session(settled());
  await act(async () => {});
  expect(screen.queryByLabelText('Session configuration')).toBeNull();
  await act(async () => emit(s, { type: 'attempt_started', attempt_id: 'attempt' }));
  await act(async () => emit(s, { type: 'assistant_message_started', attempt_id: 'attempt', message_id: 'message' }));
  for (let delta = 0; delta < 200; delta++) {
    await act(async () => emit(s, { type: 'assistant_text_delta', attempt_id: 'attempt', message_id: 'message', block_index: 0, delta: 'y' }));
    expect(screen.queryByLabelText('Session configuration')).toBeNull();
  }
  expect(reads(s)).toHaveLength(1);
});

// The old triggers — Job and Agent settlement — are runtime traffic. They are
// neither a configuration fact nor a browser-derived eligibility: only the
// runtime's own eligibility publication enables adoption.
it.each<[string, RuntimeClientEvent]>([
  ['Job', { type: 'job_updated', job: { job_id: 'execution-1', tool_id: 'bash', tool_name: 'bash', state: 'succeeded' } }],
  ['Agent', { type: 'agent_updated', agent: {
    activation_id: 'subagent-1', agent_id: 'agent-1', parent_agent_id: 'parent-agent', current_activation: null, activation_state: 'succeeded', child_conversation_id: 'conversation-child-1', agent: 'worker',
    definition_digest: 'definition-1', profile_digest: 'profile-1', state: 'inactive',
    observation: { revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 1, model_retries: 0, tool_executions: 0 } },
    started_at: '2026-09-21T00:00:00Z', workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' }, resource_state: 'none' },
  } }],
])('C15 %s settlement neither rereads configuration nor implies eligibility; the runtime\'s Busy -> Eligible publication alone does', async (_, event) => {
  const s = await session(withCandidate(), 'busy');
  await waitFor(() => expect(adopt()?.disabled).toBe(true));
  expect(screen.getByText(/Session work must settle/)).toBeTruthy();
  await act(async () => emit(s, event));
  expect(adopt()?.disabled).toBe(true);
  expect(reads(s)).toHaveLength(1);
  await act(async () => emit(s, { type: 'configuration_adoption_eligibility_changed', eligibility: { status: 'eligible' } }));
  await waitFor(() => expect(adopt()?.disabled).toBe(false));
  expect(reads(s)).toHaveLength(1);
  expect(snapshotReads(s)).toHaveLength(0);
  expect(adoptions(s)).toHaveLength(0);
});

it('C19 a native configuration publication is folded as the observation, never reread', async () => {
  const s = await session(withCandidate());
  await waitFor(() => expect(adopt()).toBeTruthy());
  await act(async () => {
    s.socket.deliver({ jsonrpc: '2.0', method: 'configuration/changed', params: { application: { ...settled(), version: '3' } } });
  });
  await waitFor(() => expect(screen.queryByLabelText('Session configuration')).toBeNull());
  expect(unavailableLine()).toBeNull();
  expect(reads(s)).toHaveLength(1);
});

it('C19 an advisory Eligible never bypasses the native gate: a native Busy refusal is definitive and never replayed', async () => {
  const s = await session(withCandidate());
  s.handlers.set('session/adoptConfiguration', () => {
    throw new RpcFailure({ code: -32000, message: 'Busy', data: { kind: 'configuration_adoption', rejection: { status: 'busy' } } });
  });
  await waitFor(() => expect(adopt()?.disabled).toBe(false));
  await act(async () => adopt()!.click());
  await screen.findByRole('alert');
  // Exactly one submission; its rejection owes one authoritative reread.
  await waitFor(() => expect(reads(s)).toHaveLength(2));
  expect(adoptions(s)).toHaveLength(1);
  expect(screen.getByText(/Prepared configuration is waiting/)).toBeTruthy();
});

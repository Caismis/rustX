import { afterEach, expect, it, vi } from 'vitest';
import type { MethodResult, Request1 } from '../../protocol/app-server/v39';
import { OutcomeUncertain } from '../src/client/app-server';
import { Server, snapshot } from './fixture';
const servers: Server[] = [];
afterEach(() => { for (const s of servers.splice(0)) s.client.disconnect(); vi.useRealTimers(); });
async function connected() { const s = new Server(); servers.push(s); await s.attached('A'); return s; }
type DomainMethod = 'turn/cancel' | 'agent/wait' | 'job/wait' | 'agent/interrupt' | 'job/cancel' | 'agent/sendMessage' | 'context/compact';
function operation(s: Server, method: DomainMethod): Request1 {
  const target = s.client.target('A');
  if (method === 'context/compact') return { method, params: { target, request_id: 'compact-a' } };
  if (method === 'turn/cancel') return { method, params: { target } };
  return method === 'job/wait' || method === 'job/cancel' ? { method, params: { target, job_id: 'job-a' } }
    : method === 'agent/sendMessage' ? { method, params: { target, agent_id: 'agent-a', message: 'input' } }
    : { method, params: { target, agent_id: 'agent-a' } };
}
function result(method: DomainMethod): MethodResult {
  if (method === 'context/compact') return { type: 'context', context: { compaction_count: 1, compaction_in_progress: false } };
  if (method === 'turn/cancel') return { type: 'cancellation_accepted', attempt_id: 'attempt-A' };
  if (method === 'job/wait' || method === 'job/cancel') return { type: 'job', job: { job_id: 'job-a', tool_id: 'bash', tool_name: 'bash', state: 'cancelled' } };
  if (method === 'agent/sendMessage') return { type: 'agent_message', agent_id: 'agent-a', activation_id: 'activation-b', resumed: true };
  return { type: 'agent_wait', agent_id: 'agent-a', activation_id: 'activation-b', outcome: null, agent: {
    agent_id: 'agent-a', parent_agent_id: 'parent', child_conversation_id: 'child', agent: 'Worker',
    activation_id: 'activation-b', current_activation: null, state: 'inactive', activation_state: 'cancelled',
    definition_digest: 'definition', profile_digest: 'profile', started_at: '2026-09-15T00:00:00Z',
    observation: { revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 0, model_retries: 0, tool_executions: 0 } },
    workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' }, resource_state: 'none' },
  } };
}
const methods: DomainMethod[] = ['agent/wait', 'job/wait', 'agent/sendMessage', 'agent/interrupt', 'job/cancel'];
it.each([...methods, 'turn/cancel', 'context/compact'] as const)('%s owns its lifetime without incidental traffic', async method => {
  const s = await connected(); s.held.add(method); vi.useFakeTimers();
  const settled = vi.fn(); const response = result(method);
  const work = s.client.request(operation(s, method), response.type).then(settled);
  void work.catch(() => {}); // Baseline failure remains asserted without an unhandled rejection.
  const request = await s.waitFor(method, 1);
  await vi.advanceTimersByTimeAsync(120_000);
  expect(settled).not.toHaveBeenCalled(); expect(s.client.getSnapshot().connection).toBe('connected');
  await s.client.request({ method: 'session/settings', params: { session_id: 'A' } }, 'settings');
  s.socket.success(request, response); await work;
  s.socket.success(request, response); await Promise.resolve();
  expect(settled).toHaveBeenCalledExactlyOnceWith(response);
});
it.each(['agent/wait', 'job/wait', 'context/compact'] as const)('%s at full capacity stays healthy with notifications and permits controls', async method => {
  const s = await connected(); s.held.add(method); vi.useFakeTimers();
  const response = result(method); const settled = vi.fn();
  const waits = Array.from({ length: 4 }, () => s.client.request(operation(s, method), response.type).then(settled));
  expect(s.requests.filter(row => row.request.method === method)).toHaveLength(4);
  await expect(s.client.request(operation(s, method), response.type)).rejects.toThrow('wait capacity');
  for (let i = 0; i < 4; i++) { await s.update('A', snapshot()); await vi.advanceTimersByTimeAsync(30_001); }
  expect(settled).not.toHaveBeenCalled(); expect(s.client.getSnapshot().connection).toBe('connected');
  for (const control of ['agent/interrupt', 'job/cancel', 'turn/cancel'] as const) {
    s.handlers.set(control, () => result(control));
    await expect(s.client.request(operation(s, control), result(control).type)).resolves.toEqual(result(control));
  }
  await s.client.request({ method: 'session/settings', params: { session_id: 'A' } }, 'settings');
  for (const { request } of s.requests.filter(row => row.request.method === method)) s.socket.success(request, response);
  await Promise.all(waits); expect(settled).toHaveBeenCalledTimes(4);
});
it('full admission and settlement-control lanes preserve inspection capacity', async () => {
  const s = await connected(); vi.useFakeTimers();
  for (const method of ['agent/sendMessage', 'agent/interrupt'] as const) s.held.add(method);
  const work = (['agent/sendMessage', 'agent/sendMessage', 'agent/interrupt', 'agent/interrupt'] as const).map(method => s.client.request(operation(s, method), result(method).type));
  await expect(s.client.request(operation(s, 'agent/sendMessage'), 'agent_message')).rejects.toThrow('admission capacity');
  await expect(s.client.request(operation(s, 'job/cancel'), 'job')).rejects.toThrow('control capacity');
  await vi.advanceTimersByTimeAsync(120_000);
  await s.client.request({ method: 'session/settings', params: { session_id: 'A' } }, 'settings');
  for (const { request } of s.requests.filter(row => s.held.has(row.request.method))) s.socket.success(request, result(request.method as DomainMethod));
  await Promise.all(work);
});
it.each(['close', 'error'] as const)('socket %s settles operations once without replay or stale adoption', async failure => {
  const s = await connected(); for (const method of methods) s.held.add(method);
  const rejected = vi.fn();
  const work = methods.map(method => s.client.request(operation(s, method), result(method).type).catch(error => { rejected(error); return error; }));
  const old = s.socket; const requests = s.requests.filter(row => s.held.has(row.request.method));
  if (failure === 'close') old.close(); else old.onerror?.(new Event('error'));
  const errors = await Promise.all(work);
  expect(errors.slice(0, 2).every(error => !(error instanceof OutcomeUncertain))).toBe(true);
  expect(errors.slice(2).every(error => error instanceof OutcomeUncertain)).toBe(true);
  expect(rejected).toHaveBeenCalledTimes(5);
  await s.connect(); const current = s.client.getSnapshot();
  for (const { request } of requests) old.success(request, result(request.method as DomainMethod));
  expect(s.client.getSnapshot()).toBe(current);
  for (const method of methods) expect(s.requests.filter(row => row.request.method === method)).toHaveLength(1);
  expect(s.client.getSnapshot().uncertain.map(row => row.method)).toEqual(methods.slice(2));
});

it('full wait and RPC lanes preserve controls and retired startup dispatch ownership', async () => {
  const s = await connected(); s.held.add('agent/wait'); s.held.add('session/settings');
  const waits = Array.from({ length: 4 }, () => s.client.request(operation(s, 'agent/wait'), 'agent_wait'));
  const reads = Array.from({ length: 8 }, () => s.client.request({ method: 'session/settings', params: { session_id: 'A' } }, 'settings'));
  let current = true;
  const admission = s.client.send('A', 'retained', [], 'send', undefined, () => current);
  const rejected = expect(admission).rejects.toThrow('before dispatch');
  await expect(s.client.request(operation(s, 'agent/wait'), 'agent_wait')).rejects.toThrow('wait capacity');
  for (const control of ['agent/interrupt', 'job/cancel', 'turn/cancel'] as const) {
    s.handlers.set(control, () => result(control));
    await expect(s.client.request(operation(s, control), result(control).type)).resolves.toEqual(result(control));
  }
  current = false;
  for (const item of s.requests.filter(item => item.request.method === 'session/settings').slice(-8)) s.reply(item.request);
  await Promise.all([...reads, rejected]);
  expect(s.requests.filter(item => item.request.method === 'turn/start')).toEqual([]);
  expect(s.client.getSnapshot().uncertain).toEqual([]);
  expect(s.client.getSnapshot().connection).toBe('connected');
  for (const item of s.requests.filter(item => item.request.method === 'agent/wait')) s.socket.success(item.request, result('agent/wait'));
  await Promise.all(waits);
});

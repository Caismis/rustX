import { afterEach, expect, it, vi } from 'vitest';
import type { Request, ScheduleCommandResult } from '../../protocol/app-server/v72';
import { TaskCreation, taskCreation, type TaskCreationDraft } from '../src/app/scheduler/creation';
import { schedulerModel } from '../src/app/scheduler/model';
import { Server, snapshot } from './fixture';

const servers: Server[] = [];
afterEach(() => { for (const server of servers.splice(0)) server.client.dispose(); vi.restoreAllMocks(); });
const draft = (): TaskCreationDraft => ({ workspaceId: 'workspace-a', title: 'Daily review', text: 'Review repository changes.',
  timing: { type: 'cron', expression: '0 9 * * 1-5', time_zone: 'Asia/Shanghai' },
  model: { model: 'test-model', profile: 'careful', requestParams: { temperature: 0.2 }, maxOutputTokens: 1234 } });
async function fixture() {
  const server = new Server(); servers.push(server); await server.connect();
  const results = new Map<string, ScheduleCommandResult>();
  server.handlers.set('session/create', () => {
    const id = 'created'; server.snapshots.set(id, snapshot(id));
    return { type: 'session_transition', session: { id, active_node: 'node-created', active_conversation_id: 'conversation-created', node_count: 1, created_at: '0', updated_at: '0' } };
  });
  server.handlers.set('schedule/command', request => {
    if (request.method !== 'schedule/command') throw Error('command');
    const id = request.params.command.command_id;
    if (!results.has(id)) results.set(id, { status: 'applied', receipt: { id: { conversation_id: 'conversation-created', ordinal: '17' }, revision: '1',
      spec: { type: 'cron', expression: '0 9 * * 1-5', time_zone: 'Asia/Shanghai' }, state: 'enabled' } });
    return { type: 'schedule_command', result: results.get(id)! };
  });
  server.handlers.set('schedule/catalog', () => ({ type: 'schedule_catalog', page: { entries: [], next: null, unavailable: [] } }));
  const host = { ...server.workspaceHost, listWorkspaces: vi.fn(server.workspaceHost.listWorkspaces),
    resolveWorkspace: vi.fn(async () => ({ cwd: '/workspace/created' })), classifyLocations: vi.fn(server.workspaceHost.classifyLocations) };
  return { server, host, creation: new TaskCreation(server.client, host), results };
}
const requests = (server: Server, method: Request['method']) => server.requests.filter(row => row.request.method === method).map(row => row.request);

it('creates a future Session and exact-node task directly with model settings and fresh Workspace admission', async () => {
  const { server, host, creation } = await fixture();
  const refresh = vi.spyOn(schedulerModel(server.client), 'refresh');
  const result = await creation.submit(draft());
  expect(result).toEqual({ session_id: 'created', node_id: 'node-created', id: { conversation_id: 'conversation-created', ordinal: '17' } });
  expect(creation.getSnapshot()).toMatchObject({ phase: 'created', locked: true, retryable: false, task: result });
  expect(requests(server, 'session/create')[0].params).toEqual({ settings: { cwd: '/workspace/created', model: draft().model } });
  expect(requests(server, 'schedule/command')[0].params).toEqual({ session_id: 'created', node_id: 'node-created', command: {
    command_id: expect.any(String), mutation: { type: 'create', title: draft().title, text: draft().text, timing: draft().timing } } });
  expect(host.listWorkspaces).toHaveBeenCalledTimes(3);
  expect(host.resolveWorkspace).toHaveBeenCalledTimes(3);
  expect(host.classifyLocations).toHaveBeenCalledTimes(3);
  for (const call of host.classifyLocations.mock.calls) expect(call.slice(0, 3)).toEqual([['/workspace/created'], server.client.getSnapshot().endpoint, 'fixture-host']);
  expect(refresh).toHaveBeenCalled();
  const methods = server.requests.map(row => row.request.method);
  for (const method of ['turn/start', 'turn/steer', 'session/attach', 'session/submissionIntent', 'model/select']) expect(methods).not.toContain(method);
  expect(server.claims()).toEqual([]);
  expect(creation.reset()).toBe(true);
});

it('does not duplicate concurrent submits or mutate the admitted draft', async () => {
  const { server, creation } = await fixture(); server.held.add('session/create');
  const input = draft(), pending = creation.submit(input);
  const create = await server.waitFor('session/create', 1);
  input.text = 'Changed while pending'; input.timing = { type: 'after', seconds: 120 };
  expect(await creation.submit(draft())).toBeUndefined(); expect(creation.reset()).toBe(false);
  server.reply(create); await pending;
  expect(requests(server, 'session/create')).toHaveLength(1);
  expect(requests(server, 'schedule/command')[0].params).toMatchObject({ command: { mutation: { text: draft().text, timing: draft().timing } } });
});

it('retries a lost schedule response with the same command and Session after same-authority reconnect', async () => {
  const { server, creation, results } = await fixture(); server.held.add('schedule/command');
  const pending = creation.submit(draft()); const request = await server.waitFor('schedule/command', 1);
  server.commit(request); server.socket.close(); await pending;
  expect(results.size).toBe(1); expect(creation.getSnapshot()).toMatchObject({ phase: 'uncertain', errorKind: 'schedule_uncertain', locked: true, retryable: true });
  expect(creation.reset()).toBe(false); expect(await creation.submit(draft())).toBeUndefined();
  await server.connect(); server.held.delete('schedule/command');
  expect(await creation.retry()).toBeDefined();
  expect(requests(server, 'session/create')).toHaveLength(1);
  expect(requests(server, 'schedule/command').map(row => row.params)).toEqual([request.params, request.params]);
  expect(results.size).toBe(1); expect(creation.getSnapshot().phase).toBe('created');
});

it('never blindly retries a lost Session create, but permits an explicit discard before a new Create gesture', async () => {
  const { server, creation } = await fixture(); server.held.add('session/create');
  const pending = creation.submit(draft()); const request = await server.waitFor('session/create', 1);
  server.commit(request); server.socket.close(); await pending;
  expect(creation.getSnapshot()).toMatchObject({ phase: 'uncertain', errorKind: 'create_uncertain', locked: true, retryable: false });
  await server.connect(); expect(await creation.retry()).toBeUndefined(); expect(await creation.submit(draft())).toBeUndefined();
  expect(requests(server, 'session/create')).toHaveLength(1); expect(requests(server, 'schedule/command')).toHaveLength(0);
  expect(creation.reset()).toBe(true);
  server.held.delete('session/create');
  server.handlers.set('session/create', () => ({ type: 'session_transition', session: {
    id: 'new-gesture', active_node: 'node-new-gesture', active_conversation_id: 'conversation-new-gesture', node_count: 1, created_at: '0', updated_at: '0' } }));
  server.handlers.set('schedule/command', () => ({ type: 'schedule_command', result: { status: 'applied', receipt: {
    id: { conversation_id: 'conversation-new-gesture', ordinal: '1' }, revision: '1', spec: { type: 'at', at_utc: '2099-01-01T00:00:00Z' }, state: 'enabled' } } }));
  expect(await creation.submit(draft())).toMatchObject({ session_id: 'new-gesture', node_id: 'node-new-gesture' });
  expect(requests(server, 'session/create')).toHaveLength(2);
  expect(requests(server, 'schedule/command').map(row => row.params)).toEqual([expect.objectContaining({ session_id: 'new-gesture', node_id: 'node-new-gesture' })]);
});

it.each(['registration', 'cwd', 'host', 'authorization'] as const)('revalidates %s at actual create dispatch', async changed => {
  const { server, host, creation } = await fixture();
  const initial = await host.listWorkspaces(); host.listWorkspaces.mockClear();
  host.listWorkspaces.mockResolvedValueOnce(initial).mockResolvedValue(changed === 'registration' ? { ...initial, workspaces: [] }
    : changed === 'host' ? { ...initial, authorityId: 'replacement-host' } : initial);
  if (changed === 'cwd') host.resolveWorkspace.mockResolvedValueOnce({ cwd: '/workspace/created' }).mockResolvedValue({ cwd: '/workspace/other' });
  if (changed === 'authorization') host.classifyLocations.mockResolvedValueOnce([{ authorized: true }]).mockResolvedValue([{ authorized: false, reason: 'denied' }]);
  expect(await creation.submit(draft())).toBeUndefined();
  expect(creation.getSnapshot()).toMatchObject({ phase: 'failed', locked: false });
  expect(requests(server, 'session/create')).toHaveLength(0); expect(requests(server, 'schedule/command')).toHaveLength(0);
});

it('retains acknowledged Session across Host replacement before schedule dispatch, and reuses it after authorization returns', async () => {
  const { server, host, creation } = await fixture(); const initial = await host.listWorkspaces(); host.listWorkspaces.mockClear();
  host.listWorkspaces.mockResolvedValueOnce(initial).mockResolvedValueOnce(initial).mockResolvedValue({ ...initial, authorityId: 'replacement-host' });
  await creation.submit(draft());
  expect(creation.getSnapshot()).toMatchObject({ phase: 'failed', errorKind: 'authority_changed', locked: true, retryable: true });
  expect(requests(server, 'session/create')).toHaveLength(1); expect(requests(server, 'schedule/command')).toHaveLength(0);
  host.listWorkspaces.mockResolvedValue(initial); expect(await creation.retry()).toBeDefined();
  expect(requests(server, 'session/create')).toHaveLength(1);
});

it('does not retry an acknowledged Session on a replaced native server', async () => {
  const { server, creation } = await fixture(); server.held.add('schedule/command');
  const pending = creation.submit(draft()); await server.waitFor('schedule/command', 1); server.socket.close(); await pending;
  server.authorityId = 'replacement-native-server'; await server.connect();
  expect(await creation.retry()).toBeUndefined();
  expect(creation.getSnapshot()).toMatchObject({ locked: true, retryable: false });
  expect(requests(server, 'session/create')).toHaveLength(1); expect(requests(server, 'schedule/command')).toHaveLength(1);
});

it.each(['workspaceId', 'title', 'text'] as const)('rejects empty %s before native or Host effects', async field => {
  const { server, host, creation } = await fixture();
  await creation.submit({ ...draft(), [field]: ' ' });
  expect(creation.getSnapshot()).toMatchObject({ phase: 'failed', errorKind: 'invalid', locked: false });
  expect(host.listWorkspaces).not.toHaveBeenCalled(); expect(requests(server, 'session/create')).toHaveLength(0);
});

it('rejects invalid timing and disconnected submissions before effects', async () => {
  const { server, host, creation } = await fixture();
  await creation.submit({ ...draft(), timing: { type: 'interval', every_seconds: 0 } });
  expect(creation.getSnapshot().errorKind).toBe('invalid');
  await creation.submit({ ...draft(), timing: { type: 'at', at: '2099-02-30T09:00:00Z' } });
  expect(creation.getSnapshot().errorKind).toBe('invalid');
  server.socket.close(); await creation.submit(draft());
  expect(creation.getSnapshot().errorKind).toBe('disconnected'); expect(host.listWorkspaces).not.toHaveBeenCalled();
});

it('rejects oversized native schedule content before creating its Session', async () => {
  const { server, host, creation } = await fixture();
  await creation.submit({ ...draft(), title: 'a'.repeat(121) });
  await creation.submit({ ...draft(), text: 'a'.repeat(16_385) });
  expect(creation.getSnapshot().errorKind).toBe('invalid');
  expect(host.listWorkspaces).not.toHaveBeenCalled(); expect(requests(server, 'session/create')).toHaveLength(0);
});

it('retains a decoded committed task receipt when transport disappears during acknowledgement publication', async () => {
  const { server, creation } = await fixture();
  creation.subscribe(() => { if (creation.getSnapshot().phase === 'created') server.socket.close(); });
  expect(await creation.submit(draft())).toMatchObject({ session_id: 'created', id: { ordinal: '17' } });
  expect(creation.getSnapshot()).toMatchObject({ phase: 'created', locked: true, retryable: false });
});

it('retains a decoded Session acknowledgement when native transport disappears before scheduling', async () => {
  const { server, creation } = await fixture();
  server.held.add('session/create');
  const pending = creation.submit(draft()), request = await server.waitFor('session/create', 1);
  server.reply(request); server.socket.close(); await pending;
  expect(creation.getSnapshot()).toMatchObject({ phase: 'failed', locked: true, retryable: true });
  expect(requests(server, 'schedule/command')).toHaveLength(0);
  await server.connect(); expect(await creation.retry()).toBeDefined();
  expect(requests(server, 'session/create')).toHaveLength(1);
});

it('keeps creation ownership and immutable submitted inputs across route remount and same-native reconnect', async () => {
  const { server, host } = await fixture(); const owner = taskCreation(server.client, host);
  server.held.add('schedule/command'); const input = draft(), pending = owner.submit(input);
  await server.waitFor('schedule/command', 1); input.text = 'new caller text'; server.socket.close(); await pending;
  expect(taskCreation(server.client, host)).toBe(owner);
  expect(owner.getSnapshot().draft).toEqual(draft());
  await server.connect(); expect(taskCreation(server.client, host)).toBe(owner);
  server.held.delete('schedule/command'); expect(await owner.retry()).toBeDefined();
  expect(requests(server, 'session/create')).toHaveLength(1);
});

it('starts a separate owner on native replacement while keeping the old committed receipt scoped to its authority', async () => {
  const { server, host } = await fixture(); const original = taskCreation(server.client, host);
  const task = await original.submit(draft());
  server.socket.close(); server.authorityId = 'new-native'; await server.connect();
  const replacement = taskCreation(server.client, host);
  expect(replacement).not.toBe(original); expect(replacement.getSnapshot()).toMatchObject({ phase: 'idle' });
  expect(replacement.getSnapshot().task).toBeUndefined();
  expect(original.getSnapshot().task).toEqual(task); expect(original.isCurrentAuthority()).toBe(false);
});

it('allows explicit discard of a definitive schedule rejection before creating a corrected task', async () => {
  const { server, creation } = await fixture();
  server.handlers.set('schedule/command', () => ({ type: 'schedule_command', result: { status: 'invalid', reason: 'At must be future' } }));
  await creation.submit(draft()); expect(creation.getSnapshot()).toMatchObject({ phase: 'failed', locked: true, errorKind: 'schedule_failed' });
  expect(creation.reset()).toBe(true); expect(creation.getSnapshot()).toEqual({ phase: 'idle', locked: false, retryable: false });
  server.handlers.set('schedule/command', () => ({ type: 'schedule_command', result: { status: 'applied', receipt: {
    id: { conversation_id: 'conversation-created', ordinal: '18' }, revision: '1', spec: { type: 'at', at_utc: '2099-01-01T00:00:00Z' }, state: 'enabled' } } }));
  expect(await creation.submit({ ...draft(), timing: { type: 'at', at: '2099-01-01T00:00:00Z' } })).toBeDefined();
  expect(requests(server, 'session/create')).toHaveLength(2);
});

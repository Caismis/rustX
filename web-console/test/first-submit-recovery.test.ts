import { afterEach, expect, it, vi } from 'vitest';
import type { Request } from '../../protocol/app-server/v59';
import { firstSubmissions } from '../src/app/new-conversation/first-submit';
import { firstSubmitPort } from '../src/app/new-conversation/port';
import { Server, snapshot } from './fixture';

const servers: Server[] = [];
afterEach(async () => {
  await Promise.all(servers.splice(0).map(server => server.client.dispose()));
});

async function fixture() {
  const server = new Server(); servers.push(server);
  await server.connect();
  server.held.add('session/subscribe');
  server.held.add('session/settings');
  return server;
}

function expireSubscription(server: Server, request: Request) {
  server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: {
    code: -32000, message: 'Replay expired', data: { kind: 'resync_required' },
  } });
}

it.each([false, true])('notifies attachment exactly once after subscription handoff (recovered: %s), without waiting for Settings', async recover => {
  const server = await fixture(), ready = vi.fn();
  const attaching = server.client.attach('A', undefined, () => true, ready);
  let subscribe = await server.waitFor('session/subscribe', 1);
  if (recover) {
    expireSubscription(server, subscribe);
    subscribe = await server.waitFor('session/subscribe', 2);
  }
  expect(ready).not.toHaveBeenCalled();
  expect(server.client.isAttachmentControlCurrent('A', server.client.getSnapshot().views.A.attachmentObservation)).toBe(false);
  server.reply(subscribe);
  await attaching;
  expect(ready).toHaveBeenCalledExactlyOnceWith(server.client.target('A'));
  expect(server.client.getSnapshot().views.A).toMatchObject({ attachment: 'attached', observationState: 'following' });
  expect(server.client.isAttachmentControlCurrent('A', server.client.getSnapshot().views.A.attachmentObservation)).toBe(true);
  expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'session/checkpoint')).toHaveLength(recover ? 2 : 1);
  const settings = await server.waitFor('session/settings', 1);
  server.reply(settings);
  // Later supplemental reads cannot announce this attachment a second time.
  await Promise.resolve();
  expect(ready).toHaveBeenCalledOnce();
});

it.each(['current', 'navigation', 'disconnect'] as const)('first Send continues only for a current caller after initial subscription recovery (%s)', async caller => {
  const server = await fixture();
  const session = { id: 'created', active_node: 'node-created', active_conversation_id: 'conversation-created', node_count: 1, created_at: '0', updated_at: '0' };
  server.snapshots.set(session.id, snapshot(session.id));
  server.handlers.set('session/create', () => ({ type: 'session_transition', session }));
  const host = { ...server.workspaceHost, resolveWorkspace: async () => ({ cwd: '/workspace/created' }) };
  let navigationCurrent = true;
  const published = vi.fn(), ready = vi.fn();
  const attach = server.client.attach.bind(server.client);
  vi.spyOn(server.client, 'attach').mockImplementation((id, node, current, attached) => attach(id, node, current, target => {
    ready(target); attached?.(target);
  }));
  const port = firstSubmitPort(server.client, host, () => navigationCurrent, () => published);
  const owner = firstSubmissions(server.client);
  const draft = { workspaceId: 'workspace-a', text: 'First request', files: [new File(['attachment'], 'first.txt')] };
  const submitting = owner.submit('new-draft', draft, port);
  const first = await server.waitFor('session/subscribe', 1);
  expireSubscription(server, first);
  const recovered = await server.waitFor('session/subscribe', 2);
  expect(ready).not.toHaveBeenCalled();
  expect(published).not.toHaveBeenCalled();
  expect(server.requests.filter(row => row.request.method === 'session/uploadPrepare' || row.request.method === 'turn/start')).toHaveLength(0);
  if (caller === 'navigation') navigationCurrent = false;
  if (caller === 'disconnect') {
    const socket = server.socket;
    socket.close();
    socket.success(recovered, { type: 'subscribed', after_cursor: '0' });
  } else server.reply(recovered);
  expect(await submitting).toBe(caller === 'current');
  expect(server.requests.filter(row => row.request.method === 'session/create')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'session/subscribe')).toHaveLength(2);
  if (caller === 'current') {
    expect(ready).toHaveBeenCalledExactlyOnceWith(server.client.target(session.id));
    expect(published).toHaveBeenCalledExactlyOnceWith(session.id);
    expect(server.carrierTransfers).toBe(1);
    const starts = server.requests.filter(row => row.request.method === 'turn/start');
    expect(starts).toHaveLength(1);
    expect(starts[0].request.params).toMatchObject({ target: server.client.target(session.id), content: [
      { type: 'upload', session_id: session.id }, { type: 'text', text: draft.text },
    ] });
    expect(owner.session(session.id)?.phase).toBe('admitted');
  } else {
    expect(ready).not.toHaveBeenCalled();
    expect(published).not.toHaveBeenCalled();
    expect(server.carrierTransfers).toBe(0);
    expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(0);
    expect(owner.session(session.id)).toMatchObject({ phase: 'failed', draft });
  }
});

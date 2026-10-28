import { afterEach, expect, it } from 'vitest';
import type { RuntimeClientSnapshot } from '../../protocol/app-server/v62';
import { firstSubmissions } from '../src/app/new-conversation/first-submit';
import { firstSubmitPort } from '../src/app/new-conversation/port';
import { Server, snapshot } from './fixture';

const servers: Server[] = [];
afterEach(async () => {
  await Promise.all(servers.splice(0).map(server => server.client.dispose()));
});

/** Completed native history supplies no recovery input that could arm startup.
 * The next attachment is deliberately cold, while all I/O settles only at ACKs. */
async function coldReconnect() {
  const server = new Server(); servers.push(server);
  const completed: RuntimeClientSnapshot = {
    ...snapshot('A'),
    messages: [
      { id: 'prior-question', role: 'user', source: 'human', content: [{ type: 'text', text: 'Earlier request' }] },
      { id: 'prior-answer', role: 'assistant', content: [{ type: 'text', text: 'Completed answer' }] },
    ],
  };
  server.snapshots.set('A', completed);
  await server.attached('A', 'B');
  const before = server.client.getSnapshot();
  const previous = server.client.target('A');
  const oldSocket = server.socket;
  const oldObservation = before.views.A.attachmentObservation;
  oldSocket.close();
  // Native idle eviction preserves durable history and selected Node identity.
  server.loaded.delete('A'); server.residentConversations.delete('A');
  server.held.add('session/attach');
  const connecting = server.connect();
  const first = await server.waitFor('session/attach', 3);
  // This assertion fails at the dependency cut, rather than waiting for a test
  // watchdog to infer that connect() is parked behind the held MCP attachment.
  expect(server.client.getSnapshot().connection).toBe('connected');
  await connecting;
  const second = await server.waitFor('session/attach', 4);
  expect(first.params).toMatchObject({ session_id: 'A', node_id: 'node-A' });
  expect(second.params).toMatchObject({ session_id: 'B', node_id: 'node-B' });
  const state = server.client.getSnapshot();
  expect(state).toMatchObject({ authorityId: before.authorityId, generation: before.generation + 1 });
  expect(state.authorityRevision).toBe(before.authorityRevision);
  expect(state.views.A).toMatchObject({ attachment: 'attaching', nodeId: 'node-A', snapshot: { messages: completed.messages, inbound: {} } });
  expect(server.loaded.has('A')).toBe(false);
  expect(server.client.isAttachmentControlCurrent('A', oldObservation)).toBe(false);
  expect(() => server.client.target('A')).toThrow('not authoritatively attached');
  // A second Session restores independently while the first remains cold.
  server.reply(second);
  await server.client.waitForAttachment('B', () => true);
  expect(server.client.getSnapshot().views.B.attachment).toBe('attached');
  expect(server.client.getSnapshot().views.A.attachment).toBe('attaching');
  return { server, first, previous, oldSocket, oldObservation };
}

it('same-authority reconnect accepts first Send while a previously completed Session restores a cold MCP runtime', async () => {
  const { server, first, previous, oldSocket, oldObservation } = await coldReconnect();
  const port = firstSubmitPort(server.client, server.workspaceHost, () => true, () => () => {}, true);
  const owner = firstSubmissions(server.client);
  expect(port.current()).toBe(true);
  const submitting = owner.submit('cold-reconnect', { workspaceId: '', text: 'Next request', files: [] }, port, undefined, undefined, 'A');
  const intent = await server.waitFor('session/submissionIntent', 1);
  expect(intent.params).toEqual({ session_id: 'A', node_id: 'node-A' });
  expect(server.loaded.has('A')).toBe(false);
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(0);
  expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(4);
  expect(server.requests.filter(row => row.request.method === 'session/create')).toHaveLength(0);

  server.reply(first);
  await expect(submitting).resolves.toBe(true);
  const target = server.client.target('A');
  expect(target.conversation_id).toBe(previous.conversation_id);
  expect(target.runtime_incarnation).not.toBe(previous.runtime_incarnation);
  expect(target.attachment_id).not.toBe(previous.attachment_id);
  expect(server.coldLoads.get('A')).toBe(2);
  expect(server.client.isAttachmentControlCurrent('A', oldObservation)).toBe(false);
  const starts = server.requests.filter(row => row.request.method === 'turn/start');
  expect(starts).toHaveLength(1);
  expect(starts[0].request.params).toMatchObject({ target, content: [{ type: 'text', text: 'Next request' }] });
  const installed = server.client.getSnapshot().views.A;
  oldSocket.deliver({ jsonrpc: '2.0', method: 'session/closed', params: { target: previous } });
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/closed', params: { target: previous } });
  expect(server.client.getSnapshot().views.A).toBe(installed);
  expect(server.client.getSnapshot().connection).toBe('connected');
  expect(owner.session('A')?.phase).toBe('admitted');
});

it.each(['navigation', 'disconnect', 'replacement'] as const)('a first Send waiting on cold reconnect cannot cross a later %s fence', async fence => {
  const { server, first, previous } = await coldReconnect();
  const socket = server.socket;
  let navigationCurrent = true;
  const port = firstSubmitPort(server.client, server.workspaceHost, () => navigationCurrent, () => () => {}, true);
  const submitting = firstSubmissions(server.client).submit('fenced-reconnect', { workspaceId: '', text: 'Do not replay', files: [] }, port, undefined, undefined, 'A');
  await server.waitFor('session/submissionIntent', 1);
  if (fence === 'navigation') {
    navigationCurrent = false;
    server.reply(first);
  } else {
    if (fence === 'disconnect') socket.close();
    else {
      server.authorityId = 'replacement-native-authority';
      await server.connect();
      expect(server.client.getSnapshot().authorityId).toBe('replacement-native-authority');
      expect(server.client.getSnapshot().views.A).toBeUndefined();
    }
    const beforeLateResponse = server.client.getSnapshot();
    // A late response from the retired carrier cannot establish a new proof.
    socket.success(first, { type: 'attached', target: { ...previous, attachment_id: 'late-old-claim' }, configuration_revision: '0', attachment_kind: 'controller', observation_source: 'live' });
    expect(server.client.getSnapshot()).toBe(beforeLateResponse);
  }
  await expect(submitting).resolves.toBe(false);
  expect(port.current()).toBe(false);
  expect(server.requests.filter(row => row.request.method === 'turn/start')).toHaveLength(0);
  expect(server.requests.filter(row => row.request.method === 'session/submissionIntent')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(4);
});

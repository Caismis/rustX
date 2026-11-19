// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, waitFor } from '@testing-library/react';
import { threadFor } from '../../protocol/app-server/testing.ts';
import { compactReceipt, snapshot } from './fixture';
import { routeFixture } from './shell-navigation-fixture';
import { CommandSession } from '../src/app/commands/native';
import { responseRetryReady } from '../src/app/agent/response-retry';
import { RpcFailure } from '../src/client/errors';
import type { MethodResult, Request, SessionNodeObservation, SessionUserMessageBoundaryObservation, UserInputBlock } from '../../protocol/app-server/v80';
import { Server } from './fixture';

let server: Server;
beforeEach(() => { server = new Server(); localStorage.clear(); });
afterEach(() => { cleanup(); server.client.disconnect(); vi.restoreAllMocks(); });
const methods = () => server.requests.map(item => item.request.method);

const USER_ID = 'user-cut';
const ASSISTANT_ID = 'original-assistant';
const EDITED_TEXT = 'Try the orchard instead';

function userMessage(text = 'Try this') {
  return { id: USER_ID, role: 'user' as const, source: 'human' as const, kind: 'message' as const, timestamp: '2026-09-19T07:00:00Z', content: [{ type: 'text' as const, text }] };
}

function editFixture() {
  const original = snapshot('A');
  const user = userMessage();
  const assistant = { id: ASSISTANT_ID, role: 'assistant' as const, source: 'model' as const, kind: 'message' as const, timestamp: '2026-09-19T08:00:00Z', content: [{ type: 'text' as const, text: 'Original assistant response' }] };
  original.messages = [user, assistant];
  original.transcript.entries = [
    { cursor: '1', item: { type: 'message', message: user } },
    { cursor: '2', completed_response: { closing_locator: { type: 'by_message_id', message_id: ASSISTANT_ID }, deferred_fields: [], retry_locator: { type: 'by_message_id', message_id: USER_ID }, closing_message_id: ASSISTANT_ID, origin: { attempt_id_source: null, conversation_id: 'origin-conversation', closing_message_id: ASSISTANT_ID, attempt_id: 'original-attempt' }, completed_at: '2026-09-19T08:00:00Z', surface_revision: '9007199254740997', retry_message_id: USER_ID }, item: { type: 'message', message: assistant } },
  ];
  const originals = structuredClone(original);
  server.snapshots.set('A', original); server.nodeSnapshots.set('node-A', original);
  const content: UserInputBlock[] = [{ type: 'text', text: 'Try this' }];
  const boundary: SessionUserMessageBoundaryObservation = { surface_revision: '9007199254740997', locator: { type: 'by_message_id', message_id: USER_ID }, block_count: '1', content: [], collections: [], message: user };
  const nodes: SessionNodeObservation[] = [{ id: 'node-A', conversation_id: 'conversation-A', ordinal: '1', origin: { type: 'new' } }];
  server.handlers.set('session/boundaries', () => ({ type: 'boundaries', surface_revision: boundary.surface_revision, boundaries: [boundary] }));
  server.handlers.set('session/messageBoundary', () => ({ type: 'session_message_boundary', boundary: { node_id: 'node-A', conversation_id: 'conversation-A', message_id: USER_ID, surface_revision: boundary.surface_revision, boundary: { type: 'by_message_id', message_id: USER_ID } } }));
  server.handlers.set('session/tree', request => ({ type: 'tree', nodes: request.method === 'session/tree' && request.params.session_id === 'B' ? [{ id: 'node-B', conversation_id: 'conversation-B', ordinal: '1', origin: { type: 'new' } }] : nodes }));
  const transition = (request: Request): MethodResult => {
    if (request.method !== 'session/branch') throw new Error('wrong method');
    if (request.params.surface_revision !== boundary.surface_revision || !(request.params.boundary?.type === 'by_message_id' && request.params.boundary.message_id === USER_ID)) throw new RpcFailure({ code: -32602, message: 'Invalid boundary' });
    nodes.push({ ordinal: String(nodes.length + 1), id: 'branch-A', parent: 'node-A', conversation_id: 'conversation-branch-A', origin: { type: 'fork', source_session: 'A', source_node: 'node-A', source_surface_revision: boundary.surface_revision, side: 'before', source_message: USER_ID } });
    server.nodeSnapshots.set('branch-A', { ...snapshot('A'), conversation_id: 'conversation-branch-A' });
    const session = { id: 'A', active_node: 'branch-A', active_conversation_id: 'conversation-branch-A', node_count: nodes.length, created_at: '0', updated_at: '0' };
    return { type: 'session_transition', session, thread: threadFor(session), editor: server.editor(session, content) };
  };
  server.handlers.set('session/branch', transition);
  server.handlers.set('context/compact', () => compactReceipt({ compaction_in_progress: false, compaction_count: 1 }));
  server.handlers.set('session/messageVersions', () => ({ type: 'session_message_versions', page: { versions: [], next_offset: null } }));
  return { original, originals, content, boundary, nodes };
}

describe('edit user message and send', () => {
  it('drives the native retry transition and hands presentation to the accepted replay', async () => {
    editFixture();
    await server.attached('A', 'B');
    const navigation = routeFixture(server.client);
    const scope = new CommandSession(server.client, 'A', navigation.capture());
    const selection = await scope.userSelection({ type: 'by_message_id', message_id: USER_ID });
    server.held.add('session/branch');
    const work = scope.transition('retry', selection, EDITED_TEXT);
    await server.waitFor('session/branch', 1);
    await act(async () => server.reply(server.requests.find(r => r.request.method === 'session/branch')!.request));
    // Let the transition continue: switchNode + attach + editorRead + inbound/queue.
    await waitFor(() => expect(methods()).toContain('inbound/queue'));
    await act(async () => { await Promise.resolve(); });
    const result = await work;
    expect(result?.replay).toBeTruthy();
    expect(result!.replay!.messageId).toBe('accepted-user');
    const view = server.client.getSnapshot().views['A'];
    expect(view?.target?.conversation_id).toBe('conversation-branch-A');
    expect(sameTargetCheck(view?.target, result!.replay!.target)).toBe(true);
    // Native adoption: the accepted message lands in the branch conversation.
    const adopted = server.nodeSnapshots.get('branch-A')!;
    const editedUser = { ...userMessage(EDITED_TEXT), id: 'accepted-user' };
    adopted.messages = [editedUser];
    adopted.transcript.entries = [{ cursor: '1', item: { type: 'message', message: editedUser } }];
    server.nodeSnapshots.set('branch-A', adopted);
    const retry = { view: view!, replay: result!.replay };
    // responseRetryReady must see the exact accepted id on the exact target.
    const liveView = { ...view!, snapshot: adopted };
    expect(responseRetryReady(retry, liveView)).toBe(true);
  });
});

function sameTargetCheck(a: any, b: any) {
  return a && b && a.session_id === b.session_id && a.conversation_id === b.conversation_id
    && a.runtime_incarnation === b.runtime_incarnation && a.attachment_id === b.attachment_id;
}

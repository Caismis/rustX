import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ConversationLive } from '../src/app/agent/ConversationLive';
import { Server, snapshot, checkpoint } from './fixture';
import type { ContentRef } from '../../protocol/app-server/v61';
let frames: Map<number, FrameRequestCallback>, serial = 0;
beforeEach(() => {
  frames = new Map(); vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++serial, callback); return serial; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
function paint() { act(() => { const pending = [...frames.values()]; frames.clear(); for (const callback of pending) callback(0); }); }
it.each(['user', 'assistant'] as const)('keeps released deferred %s text through a held checkpoint without retaining old read admission', async role => {
  const server = new Server(), text = 'x'.repeat(35000);
  let bytes = text;
  const content = [{ type: 'text' as const, text: '' }];
  const message = role === 'user' ? { role, source: 'human' as const, id: 'deferred-message', content } : { role, id: 'deferred-message', content };
  const value = { ...snapshot(), messages: [message], transcript: { entries: [{ cursor: '1', item: { type: 'message' as const, message } }] } };
  const ref: ContentRef = { conversation_id: value.conversation_id, source: { type: 'canonical_message_block', component: 'payload', locator: { type: 'by_message_id', message_id: message.id }, block_index: 0 }, representation: 'utf8_text', version: 1, content_revision: 1, readable_end: text.length };
  server.snapshots.set('A', value);
  server.handlers.set('session/checkpoint', request => {
    if (request.method !== 'session/checkpoint') throw Error('checkpoint');
    const current = request.params.target.conversation_id === value.conversation_id ? value : snapshot('B');
    return { type: 'checkpoint', checkpoint: { ...checkpoint(current), content: current === value ? [{ path: ['messages', '0', 'content', '0', 'text'], content_ref: ref, deferred: true }] : [] } };
  });
  server.handlers.set('conversation/contentRead', request => {
    if (request.method !== 'conversation/contentRead') throw Error('content');
    const offset = request.params.offset, part = bytes.slice(offset, offset + request.params.max_bytes), end = offset + part.length;
    return { type: 'content_chunk', chunk: { content_ref: request.params.content_ref, offset, end_offset: end, readable_end: bytes.length, data_base64: btoa(part), next_offset: end < bytes.length ? end : null, complete: end === bytes.length } };
  });
  await server.attached('A');
  try {
    const ui = await act(async () => render(<div data-conversation-scroll><ConversationLive client={server.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}}/></div>));
    const displayed = () => ui.container.textContent ?? '';
    const reads = () => server.requests.filter(row => row.request.method === 'conversation/contentRead');
    await waitFor(() => expect(displayed().includes(text)).toBe(true));
    const node = [...ui.container.querySelectorAll('span,p')].find(node => node.textContent === text);
    expect(reads()).toHaveLength(1);
    const before = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
    server.held.add('session/checkpoint');
    await act(async () => server.socket.close());
    expect(displayed().includes(text)).toBe(true); expect(reads()).toHaveLength(1);
    server.loaded.delete('A'); await act(async () => server.connect());
    const held = await server.waitFor('session/checkpoint', before + 1);
    await waitFor(() => expect(server.client.getSnapshot().views.A.snapshot).toBeUndefined());
    expect(displayed().includes(text)).toBe(true); expect(node?.isConnected).toBe(true); expect(reads()).toHaveLength(1);
    expect(server.client.isAttachmentControlCurrent('A', server.client.getSnapshot().views.A.attachmentObservation)).toBe(false);
    const abandoned = server.socket; server.commit(held, abandoned);
    await act(async () => abandoned.close());
    expect(server.client.getSnapshot().views.A.target).toBeUndefined();
    expect(displayed().includes(text)).toBe(true); expect(node?.isConnected).toBe(true); expect(reads()).toHaveLength(1);
    await act(async () => server.connect());
    const retried = await server.waitFor('session/checkpoint', before + 2);
    expect(displayed().includes(text)).toBe(true); expect(node?.isConnected).toBe(true); expect(reads()).toHaveLength(1);
    await act(async () => server.reply(held, abandoned));
    expect(server.client.getSnapshot().views.A.snapshot).toBeUndefined(); expect(reads()).toHaveLength(1);
    server.held.delete('session/checkpoint'); await act(async () => server.reply(retried));
    await waitFor(() => expect(reads()).toHaveLength(2));
    expect(displayed().includes(text)).toBe(true); expect(node?.isConnected).toBe(true);
    const first = reads()[0].request, second = reads()[1].request;
    if (first.method !== 'conversation/contentRead' || second.method !== 'conversation/contentRead') throw Error('read');
    expect(second.params.target.attachment_id).not.toBe(first.params.target.attachment_id);
    server.authorityId = 'replacement-server'; bytes = 'y'.repeat(text.length); server.held.add('conversation/contentRead');
    await act(async () => { server.socket.close(); await server.connect(); await server.client.attach('A'); });
    const fresh = await server.waitFor('conversation/contentRead', 3);
    expect(displayed().includes(text)).toBe(false); expect(displayed().includes(bytes)).toBe(false);
    server.held.delete('conversation/contentRead'); await act(async () => server.reply(fresh));
    await waitFor(() => expect(displayed().includes(bytes)).toBe(true));
    server.nodeSnapshots.set('node-other', snapshot('B'));
    await act(async () => { await server.client.switchNode('A', 'node-other'); await server.client.attach('A', 'node-other'); });
    expect(displayed().includes(text)).toBe(false); expect(displayed().includes(bytes)).toBe(false);
  } finally { server.client.dispose(); }
});
it.each([{ following: false, replaceAuthority: false }, { following: true, replaceAuthority: false }, { following: false, replaceAuthority: true }])('checkpoint-pending reading is scoped to its conversation and authority (following: $following; replacement authority: $replaceAuthority)', async ({ following, replaceAuthority }) => {
  const server = new Server();
  const message = { role: 'user' as const, source: 'human' as const, id: 'large-message', content: [{ type: 'text' as const, text: 'Retained reading text' }] };
  const messages = [message, ...Array.from({ length: 49 }, (_, index) => ({ ...message, id: 'later-' + index }))];
  server.snapshots.set('A', { ...snapshot(), messages, transcript: { entries: messages.map((message, index) => ({ cursor: String(index + 1), item: { type: 'message' as const, message } })) } });
  await server.attached('A');
  try {
    const ui = await act(async () => render(<div data-conversation-scroll data-testid="reading"><ConversationLive client={server.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}}/></div>));
    const port = ui.getByTestId('reading'); Object.defineProperties(port, { scrollHeight: { configurable: true, value: 1000 }, clientHeight: { configurable: true, value: 200 } });
    paint(); port.scrollTop = following ? 800 : 250; fireEvent.scroll(port); fireEvent(port, new Event('scrollend')); paint();
    const surface = ui.container.querySelector('.chat-reading-surface'), body = ui.container.querySelector('[data-chat-anchor-key="message:large-message"]');
    expect(body).not.toBeNull();
    const count = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
    const incarnation = server.client.target('A').runtime_incarnation;
    server.held.add('session/checkpoint');
    await act(async () => server.socket.close());
    // A newly loaded native runtime cannot reuse the old incarnation's seed,
    // while the durable logical Conversation remains exactly the same.
    server.loaded.delete('A');
    if (replaceAuthority) server.authorityId = 'replacement-server';
    await act(async () => server.connect());
    let attaching: Promise<void> | undefined;
    if (replaceAuthority) await act(async () => { attaching = server.client.attach('A'); void attaching.catch(() => {}); });
    let checkpoint = await server.waitFor('session/checkpoint', count + 1);
    await waitFor(() => expect(server.client.getSnapshot().views.A.snapshot).toBeUndefined()); paint();
    const pending = server.client.getSnapshot().views.A;
    expect(pending.target?.conversation_id).toBe('conversation-A');
    expect(pending.target?.runtime_incarnation).not.toBe(incarnation);
    expect(server.client.isAttachmentControlCurrent('A', pending.attachmentObservation)).toBe(false);
    if (replaceAuthority) {
      expect(ui.container.querySelector('.chat-reading-surface')).not.toBe(surface);
      expect(ui.container.querySelector('[data-chat-anchor-key="message:large-message"]')).not.toBe(body);
    } else {
      expect(ui.container.querySelector('.chat-reading-surface')).toBe(surface);
      expect(ui.container.querySelector('[data-chat-anchor-key="message:large-message"]')).toBe(body);
      expect(port.scrollTop).toBe(following ? 800 : 250);
      const abandoned = server.socket, stale = checkpoint; server.commit(stale, abandoned);
      await act(async () => abandoned.close()); paint();
      expect(server.client.getSnapshot().views.A.target).toBeUndefined();
      expect(ui.container.querySelector('.chat-reading-surface')).toBe(surface);
      expect(ui.container.querySelector('[data-chat-anchor-key="message:large-message"]')).toBe(body);
      expect(port.scrollTop).toBe(following ? 800 : 250);
      await act(async () => server.connect());
      checkpoint = await server.waitFor('session/checkpoint', count + 2); paint();
      await act(async () => server.reply(stale, abandoned));
      expect(server.client.getSnapshot().views.A.snapshot).toBeUndefined();
      expect(ui.container.querySelector('.chat-reading-surface')).toBe(surface);
      expect(port.scrollTop).toBe(following ? 800 : 250);
    }
    await act(async () => server.reply(checkpoint));
    if (attaching) await act(async () => attaching);
    await waitFor(() => expect(server.client.isAttachmentControlCurrent('A', server.client.getSnapshot().views.A.attachmentObservation)).toBe(true)); paint();
    if (!replaceAuthority) expect(ui.container.querySelector('.chat-reading-surface')).toBe(surface);
    expect(port.scrollTop).toBe(following || replaceAuthority ? 800 : 250);
  } finally { server.client.dispose(); }
});
it.each([false, true])('retains detached/following intent across admission loss and same-conversation reconnect (following: %s)', async following => {
  const server = new Server(); await server.attached('A');
  try {
    const ui = await act(async () => render(<div data-conversation-scroll data-testid="reading"><ConversationLive client={server.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}}/></div>));
    const port = ui.getByTestId('reading'); Object.defineProperties(port, { scrollHeight: { configurable: true, value: 1000 }, clientHeight: { configurable: true, value: 200 } });
    paint(); port.scrollTop = following ? 800 : 250;
    fireEvent.scroll(port); fireEvent(port, new Event('scrollend')); paint();
    const content = ui.container.querySelector('.chat-reading-surface'); expect(content).not.toBeNull();
    await act(async () => server.socket.close()); paint();
    expect(port.scrollTop).toBe(following ? 800 : 250);
    expect(ui.container.querySelector('.chat-reading-surface')).toBe(content);
    await act(async () => server.connect());
    await waitFor(() => expect(server.client.getSnapshot().views.A.target).toBeDefined()); paint();
    expect(port.scrollTop).toBe(following ? 800 : 250);
    if (!following) {
      server.nodeSnapshots.set('node-other', snapshot('B'));
      await act(async () => server.client.switchNode('A', 'node-other'));
      await act(async () => server.client.attach('A', 'node-other'));
      await waitFor(() => expect(server.client.getSnapshot().views.A.snapshot?.conversation_id).toBe('conversation-B')); paint();
      expect(port.scrollTop).toBe(800);
    }
  } finally { server.client.dispose(); }
});

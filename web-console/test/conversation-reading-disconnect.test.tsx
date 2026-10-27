import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ConversationLive } from '../src/app/agent/ConversationLive';
import { Server, snapshot } from './fixture';
let frames: Map<number, FrameRequestCallback>, serial = 0;
beforeEach(() => {
  frames = new Map(); vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++serial, callback); return serial; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
function paint() { act(() => { const pending = [...frames.values()]; frames.clear(); for (const callback of pending) callback(0); }); }
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
    const checkpoint = await server.waitFor('session/checkpoint', count + 1);
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

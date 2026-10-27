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

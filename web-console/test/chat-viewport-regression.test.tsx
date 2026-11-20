import { act, cleanup, render } from '@testing-library/react';
import { useLayoutEffect } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ChatViewport } from '../src/presentation/layout/ChatViewport';

let frames: Map<number, FrameRequestCallback>;
let nextFrame: number;
let resize: () => void;
beforeEach(() => {
  frames = new Map(); nextFrame = 0;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++nextFrame, callback); return nextFrame; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: () => void) { resize = callback; }
    observe() {} disconnect() {}
  });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
function paint() {
  act(() => { const pending = [...frames.values()]; frames.clear(); for (const callback of pending) callback(0); });
}
function fixture() {
  const metrics: { height: number; port?: HTMLElement } = { height: 1100 };
  function Content({ height }: { height: number }) {
    useLayoutEffect(() => {
      metrics.height = height;
      // Model the browser's native clamp while a committed stream body shrinks.
      if (metrics.port) metrics.port.scrollTop = Math.min(metrics.port.scrollTop, height - 100);
    }, [height]);
    return <div>History</div>;
  }
  const tree = (height: number) => <div data-testid="port" data-conversation-scroll>
    <ChatViewport latestLabel="Latest"><Content height={height}/></ChatViewport>
  </div>;
  const view = render(tree(1100));
  const port = view.getByTestId('port'); metrics.port = port;
  Object.defineProperties(port, { scrollHeight: { get: () => metrics.height }, clientHeight: { get: () => 100 } });
  paint(); expect(port.scrollTop).toBe(1000);
  return { port, view, update: (height: number) => view.rerender(tree(height)) };
}
it('corrects observed layout before paint even after the current animation frame has run', () => {
  const f = fixture();
  f.update(1200);
  act(() => resize());
  // ResizeObserver runs before paint, after this frame's animation callbacks.
  // Scheduling another frame here visibly paints the inherited offset first.
  expect(f.port.scrollTop).toBe(1100);
  expect(frames.size).toBe(0);
});
it('keeps a reader gesture authoritative when resize flushes a queued correction', () => {
  const f = fixture();
  f.update(1200);
  act(() => { f.port.scrollTop = 900; f.port.dispatchEvent(new Event('scroll')); resize(); });
  expect(f.port.scrollTop).toBe(900);
  act(() => f.port.dispatchEvent(new Event('scrollend')));
  expect(f.view.getByRole('button', { name: 'Latest' })).toBeTruthy();
});
it('keeps following when committed DOM shrinks and grows before one layout frame', () => {
  const f = fixture();
  f.update(600); expect(f.port.scrollTop).toBe(500);
  f.update(1200); paint();
  expect(f.port.scrollTop).toBe(1100);
  expect(f.view.queryByRole('button', { name: 'Latest' })).toBeNull();
});
it('adopts reader movement before a React commit even before its scroll event', () => {
  const f = fixture();
  f.port.scrollTop = 900;
  f.update(1200); paint();
  act(() => { f.port.dispatchEvent(new Event('scrollend')); }); paint();
  expect(f.port.scrollTop).toBe(900);
  expect(f.view.getByRole('button', { name: 'Latest' })).toBeTruthy();
});
it('adopts reader movement after a React commit before its queued frame', () => {
  const f = fixture();
  f.update(1200);
  f.port.scrollTop = 950; paint();
  act(() => { f.port.dispatchEvent(new Event('scrollend')); }); paint();
  expect(f.port.scrollTop).toBe(950);
  expect(f.view.getByRole('button', { name: 'Latest' })).toBeTruthy();
});
it('retains detached reading through a temporary DOM clamp', () => {
  const f = fixture();
  act(() => { f.port.scrollTop = 900; f.port.dispatchEvent(new Event('scroll')); f.port.dispatchEvent(new Event('scrollend')); });
  paint();
  f.update(600); f.update(1200); paint();
  act(() => { f.port.dispatchEvent(new Event('scrollend')); }); paint();
  expect(f.port.scrollTop).toBe(900);
  expect(f.view.getByRole('button', { name: 'Latest' })).toBeTruthy();
});
it('restores pending reading after committed content shrinks and grows before sampling settles', () => {
  const f = fixture();
  act(() => { f.port.scrollTop = 900; f.port.dispatchEvent(new Event('scroll')); });
  f.update(600); expect(f.port.scrollTop).toBe(500);
  f.update(1200); paint();
  expect(f.port.scrollTop).toBe(500);
  act(() => f.port.dispatchEvent(new Event('scrollend')));
  expect(frames.size).toBe(1);
  paint();
  expect(f.port.scrollTop).toBe(900);
  expect(f.view.getByRole('button', { name: 'Latest' })).toBeTruthy();
});
it('lets newer reader movement supersede the sampled anchor before its corrective frame', () => {
  const f = fixture();
  act(() => { f.port.scrollTop = 900; f.port.dispatchEvent(new Event('scroll')); });
  f.update(600); f.update(1200); paint();
  act(() => f.port.dispatchEvent(new Event('scrollend')));
  act(() => { f.port.scrollTop = 700; f.port.dispatchEvent(new Event('scroll')); });
  paint();
  expect(f.port.scrollTop).toBe(700);
  act(() => f.port.dispatchEvent(new Event('scrollend')));
  paint();
  expect(f.port.scrollTop).toBe(700);
  expect(f.view.getByRole('button', { name: 'Latest' })).toBeTruthy();
});
it('keeps following inside the tail threshold and protects an anchor whenever the reader leaves it', () => {
  const f = fixture();
  for (const delta of [1, 6, 12]) {
    act(() => {
      f.port.scrollTop -= delta;
      f.port.dispatchEvent(new Event('scroll'));
      f.port.dispatchEvent(new Event('scrollend'));
    });
    expect(f.view.queryByRole('button', { name: 'Latest' })).toBeNull();
  }
  expect(f.port.scrollTop).toBe(981);

  f.update(1200); paint();
  expect(f.port.scrollTop).toBe(1100);
  expect(f.view.queryByRole('button', { name: 'Latest' })).toBeNull();

  act(() => {
    f.port.scrollTop -= 32;
    f.port.dispatchEvent(new Event('scroll'));
    f.port.dispatchEvent(new Event('scrollend'));
  });
  expect(f.view.getByRole('button', { name: 'Latest' })).toBeTruthy();
  const reading = f.port.scrollTop;

  f.update(1300); paint();
  expect(f.port.scrollTop).toBe(reading);

  act(() => f.view.getByRole('button', { name: 'Latest' }).click());
  paint();
  expect(f.port.scrollTop).toBe(1200);
  expect(f.view.queryByRole('button', { name: 'Latest' })).toBeNull();
  f.update(1400); paint();
  expect(f.port.scrollTop).toBe(1300);
});
it('protects a small upward gesture from streaming before the scroll sample settles', () => {
  const f = fixture();
  act(() => { f.port.scrollTop -= 10; f.port.dispatchEvent(new Event('scroll')); });
  f.update(1200); paint();
  expect(f.port.scrollTop).toBe(990);
  act(() => f.port.dispatchEvent(new Event('scrollend')));
  expect(f.view.getByRole('button', { name: 'Latest' })).toBeTruthy();
  f.update(1300); paint();
  expect(f.port.scrollTop).toBe(990);
});
function memoryFixture(memoryKey: string, initialHistoryLoading = false) {
  const view = render(<div data-testid="port" data-conversation-scroll>
    <ChatViewport memoryKey={memoryKey} initialHistoryLoading={initialHistoryLoading} latestLabel="Latest"><div>History</div></ChatViewport>
  </div>);
  const port = view.getByTestId('port');
  Object.defineProperties(port, { scrollHeight: { get: () => 2000 }, clientHeight: { get: () => 100 } });
  paint();
  return { port, view };
}
it('restores a detached reading position for the same memory key', () => {
  const first = memoryFixture('session-a');
  expect(first.port.scrollTop).toBe(1900);
  act(() => { first.port.scrollTop = 900; first.port.dispatchEvent(new Event('scroll')); first.port.dispatchEvent(new Event('scrollend')); });
  paint();
  first.view.unmount();
  const second = memoryFixture('session-a');
  expect(second.port.scrollTop).toBe(900);
  expect(second.view.getByRole('button', { name: 'Latest' })).toBeTruthy();
});
it('keeps following across remounts and never leaks positions between memory keys', () => {
  const first = memoryFixture('session-b');
  act(() => { first.port.scrollTop = 800; first.port.dispatchEvent(new Event('scroll')); first.port.dispatchEvent(new Event('scrollend')); });
  paint();
  first.view.unmount();
  const follower = memoryFixture('session-c');
  expect(follower.port.scrollTop).toBe(1900);
  expect(follower.view.queryByRole('button', { name: 'Latest' })).toBeNull();
  follower.view.unmount();
  const other = memoryFixture('session-d');
  expect(other.port.scrollTop).toBe(1900);
  expect(other.view.queryByRole('button', { name: 'Latest' })).toBeNull();
});
it('parks a pending restore at the tail until the reading window is representable', () => {
  const first = memoryFixture('session-e');
  act(() => { first.port.scrollTop = 900; first.port.dispatchEvent(new Event('scroll')); first.port.dispatchEvent(new Event('scrollend')); });
  paint();
  first.view.unmount();
  const pending = memoryFixture('session-e', true);
  expect(pending.port.scrollTop).toBe(1900);
  pending.view.rerender(<div data-testid="port" data-conversation-scroll>
    <ChatViewport memoryKey="session-e" latestLabel="Latest"><div>History</div></ChatViewport>
  </div>);
  paint();
  expect(pending.port.scrollTop).toBe(900);
  expect(pending.view.getByRole('button', { name: 'Latest' })).toBeTruthy();
});
it('positions the arriving cold transcript synchronously instead of painting the inherited offset', () => {
  const metrics = { height: 200 };
  const tree = (loading: boolean) => <div data-testid="port" data-conversation-scroll>
    <ChatViewport initialHistoryLoading={loading} latestLabel="Latest">
      {loading ? <div>Loading</div> : <div data-chat-anchor-key="message:m1">History</div>}
    </ChatViewport>
  </div>;
  const view = render(tree(true));
  const port = view.getByTestId('port');
  Object.defineProperties(port, { scrollHeight: { get: () => metrics.height }, clientHeight: { get: () => 100 } });
  paint();
  expect(port.scrollTop).toBe(100);
  metrics.height = 9000;
  view.rerender(tree(false));
  // The finished transcript must be positioned before this commit can paint;
  // a queued frame would first show the loading state's inherited offset.
  expect(port.scrollTop).toBe(8900);
  expect(frames.size).toBe(0);
});
it('saves and restores an unsampled upward scroll gesture across remounts', () => {
  const first = memoryFixture('session-unsampled-up');
  expect(first.port.scrollTop).toBe(1900);
  act(() => { first.port.scrollTop = 900; first.port.dispatchEvent(new Event('scroll')); });
  first.view.unmount();
  const second = memoryFixture('session-unsampled-up');
  expect(second.port.scrollTop).toBe(900);
  expect(second.view.getByRole('button', { name: 'Latest' })).toBeTruthy();
});
it('treats an unsampled scroll back inside the tail threshold as following across remounts', () => {
  const first = memoryFixture('session-unsampled-tail');
  act(() => { first.port.scrollTop = 900; first.port.dispatchEvent(new Event('scroll')); first.port.dispatchEvent(new Event('scrollend')); });
  paint();
  act(() => { first.port.scrollTop = 1885; first.port.dispatchEvent(new Event('scroll')); });
  first.view.unmount();
  const second = memoryFixture('session-unsampled-tail');
  expect(second.port.scrollTop).toBe(1900);
  expect(second.view.queryByRole('button', { name: 'Latest' })).toBeNull();
});
it('preserves detached scroll across resetKey and memoryKey switches without unmounting', () => {
  const tree = (resetKey: string, memoryKey: string) => <div data-testid="port" data-conversation-scroll>
    <ChatViewport resetKey={resetKey} memoryKey={memoryKey} latestLabel="Latest"><div>History</div></ChatViewport>
  </div>;
  const view = render(tree('s:c1', 's:c1'));
  const port = view.getByTestId('port');
  Object.defineProperties(port, { scrollHeight: { get: () => 2000 }, clientHeight: { get: () => 100 } });
  paint();
  expect(port.scrollTop).toBe(1900);
  act(() => { port.scrollTop = 900; port.dispatchEvent(new Event('scroll')); port.dispatchEvent(new Event('scrollend')); });
  paint();
  expect(view.getByRole('button', { name: 'Latest' })).toBeTruthy();

  view.rerender(tree('s:c2', 's:c2'));
  expect(port.scrollTop).toBe(1900);
  expect(view.queryByRole('button', { name: 'Latest' })).toBeNull();

  view.rerender(tree('s:c1', 's:c1'));
  expect(port.scrollTop).toBe(900);
  expect(view.getByRole('button', { name: 'Latest' })).toBeTruthy();

  // A same-memoryKey resetKey change still resets to tail following.
  view.rerender(tree('s:c1:replay', 's:c1'));
  expect(port.scrollTop).toBe(1900);
  expect(view.queryByRole('button', { name: 'Latest' })).toBeNull();
});

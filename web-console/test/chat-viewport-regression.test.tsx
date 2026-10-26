import { act, cleanup, render } from '@testing-library/react';
import { useLayoutEffect } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ChatViewport } from '../src/presentation/layout/ChatViewport';

let frames: Map<number, FrameRequestCallback>;
let nextFrame: number;
beforeEach(() => {
  frames = new Map(); nextFrame = 0;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++nextFrame, callback); return nextFrame; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
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

import { cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChatViewport } from '../src/presentation/layout/ChatViewport';
let resize: () => void;
const disconnect = vi.fn();
afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });
it('preserves the stable reading anchor for prepend and growth, follows only at bottom', () => {
  vi.stubGlobal('ResizeObserver', class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect = disconnect; });
  let height = 600;
  const positions: Record<string, number> = { a: 0, b: 200, c: 400 };
  const content = (ids: string[]) => ids.map(id => <div key={id} data-chat-anchor-key={id}>{id}</div>);
  const ui = render(<ChatViewport>{content(['a', 'b', 'c'])}</ChatViewport>);
  const viewport = ui.container.querySelector('.conversation-scroll') as HTMLElement;
  Object.defineProperties(viewport, { scrollHeight: { get: () => height }, clientHeight: { get: () => 200 } });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) {
    const key = this.dataset.chatAnchorKey;
    const top = key ? positions[key] - viewport.scrollTop : 0;
    return { top, bottom: top + 200, left: 0, right: 500, width: 500, height: 200, x: 0, y: top, toJSON() {} };
  });
  resize(); expect(viewport.scrollTop).toBe(400);
  viewport.scrollTop = 210; fireEvent.scroll(viewport);
  // ResizeObserver after reflow keeps b at -10px, independent of total height.
  positions.a += 100; positions.b += 100; positions.c += 100; height += 100;
  resize(); expect(viewport.scrollTop).toBe(310);
  // React's pre-mutation snapshot captures b before adding a preceding row.
  const measure = vi.mocked(HTMLElement.prototype.getBoundingClientRect).getMockImplementation()!;
  let prepended = false;
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) {
    if (!prepended && ui.container.querySelector('[data-chat-anchor-key="older"]')) {
      prepended = true; positions.older = 0; positions.a += 150; positions.b += 150; positions.c += 150; height += 150;
    }
    return measure.call(this);
  });
  ui.rerender(<ChatViewport>{content(['older', 'a', 'b', 'c'])}</ChatViewport>);
  expect(viewport.scrollTop).toBe(460);
  height += 200; resize(); expect(viewport.scrollTop).toBe(460);
  viewport.scrollTop = height - 200; fireEvent.scroll(viewport);
  height += 100; resize(); expect(viewport.scrollTop).toBe(height - 200);
  ui.unmount(); expect(disconnect).toHaveBeenCalledOnce();
});

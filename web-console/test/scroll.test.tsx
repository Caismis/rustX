import { cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChatViewport } from '../src/presentation/layout/ChatViewport';
let resize: () => void;
const disconnect = vi.fn();
afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });
it('preserves the stable reading anchor for prepend and growth, follows only at bottom', () => {
  vi.stubGlobal('ResizeObserver', class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect = disconnect; });
  let frame: FrameRequestCallback | undefined;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frame = callback; return 1; });
  vi.stubGlobal('cancelAnimationFrame', () => { frame = undefined; });
  const flush = () => { const callback = frame; frame = undefined; callback?.(0); };
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
  resize(); flush(); expect(viewport.scrollTop).toBe(400);
  viewport.scrollTop = 210; fireEvent.scroll(viewport);
  // ResizeObserver after reflow keeps b at -10px, independent of total height.
  positions.a += 100; positions.b += 100; positions.c += 100; height += 100;
  resize(); flush(); expect(viewport.scrollTop).toBe(310);
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
  flush(); expect(viewport.scrollTop).toBe(460);
  height += 200; resize(); flush(); expect(viewport.scrollTop).toBe(460);
  viewport.scrollTop = height - 200; fireEvent.scroll(viewport);
  height += 100; resize(); flush(); expect(viewport.scrollTop).toBe(height - 200);
  ui.unmount(); expect(disconnect).toHaveBeenCalledOnce();
});

it('one frame owns 10 observer deliveries and 5 React updates; newer user intent wins', () => {
  let frame: FrameRequestCallback | undefined;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frame = callback; return 1; });
  const cancel = vi.fn(() => { frame = undefined; });
  vi.stubGlobal('cancelAnimationFrame', cancel);
  vi.stubGlobal('ResizeObserver', class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect() {} });
  let height = 600, top = 0, writes = 0;
  const ui = render(<ChatViewport><div data-chat-anchor-key="a">0</div></ChatViewport>);
  const el = ui.container.querySelector('.conversation-scroll') as HTMLElement;
  Object.defineProperties(el, { scrollHeight: { get: () => height }, clientHeight: { get: () => 200 }, scrollTop: { get: () => top, set: value => { top = value; writes++; } } });
  const flush = () => { const cb = frame; frame = undefined; cb?.(0); };
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) {
    const y = this.dataset.chatAnchorKey ? -top : 0;
    return { top: y, bottom: y + 1000, x: 0, y, height: 1000, width: 500, left: 0, right: 500, toJSON() {} };
  });
  for (let n = 0; n < 10; n++) resize();
  for (let n = 0; n < 5; n++) ui.rerender(<ChatViewport><div data-chat-anchor-key="a">{n}</div></ChatViewport>);
  flush(); expect(writes).toBe(1); expect(top).toBe(400);
  height += 100; resize(); top = 100; fireEvent.scroll(el); flush();
  expect(top).toBe(100); expect(writes).toBe(1);
  for (let n = 0; n < 100; n++) { height += 10; ui.rerender(<ChatViewport><div data-chat-anchor-key="a">{n}</div></ChatViewport>); resize(); flush(); }
  expect(writes).toBe(1); expect(top).toBe(100);
  top = height - 200; fireEvent.scroll(el); height += 20; resize(); flush();
  expect(top).toBe(height - 200); expect(writes).toBe(2);
  resize(); flush(); expect(writes).toBe(2);
  resize(); ui.unmount(); expect(cancel).toHaveBeenCalledOnce();
});

it('short-history prepend retains reading ownership through disappearing anchors and shrink', () => {
  let frame: FrameRequestCallback | undefined;
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { frame = cb; return 1; });
  vi.stubGlobal('cancelAnimationFrame', () => { frame = undefined; });
  vi.stubGlobal('ResizeObserver', class { constructor(cb: () => void) { resize = cb; } observe() {} disconnect() {} });
  let height = 100, top = 0;
  const positions: Record<string, number> = { a: 0, b: 100, older: -200 };
  const content = (ids: string[]) => ids.map(id => <div key={id} data-chat-anchor-key={id}>{id}</div>);
  const ui = render(<ChatViewport>{content(['a', 'b'])}</ChatViewport>);
  const el = ui.container.querySelector('.conversation-scroll') as HTMLElement;
  Object.defineProperties(el, { scrollHeight: { get: () => height }, clientHeight: { get: () => 200 }, scrollTop: { get: () => top, set: value => { top = value; } } });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) {
    const offset = ui.container.querySelector('[data-chat-anchor-key="older"]') ? 200 : 0;
    const y = this.dataset.chatAnchorKey ? positions[this.dataset.chatAnchorKey] + offset - top : 0;
    return { top: y, bottom: y + 100, x: 0, y, height: 100, width: 500, left: 0, right: 500, toJSON() {} };
  });
  const flush = () => { const cb = frame; frame = undefined; cb?.(0); };
  flush(); expect(top).toBe(0);
  height = 400; ui.rerender(<ChatViewport>{content(['older', 'a', 'b'])}</ChatViewport>); flush(); expect(top).toBe(200);
  height = 800; resize(); flush(); expect(top).toBe(200); // Prepend transferred ownership even when the old page fitted.
  ui.rerender(<ChatViewport>{content(['older', 'b'])}</ChatViewport>); flush(); expect(top).toBe(200); // Next semantic row remains at +100.
  positions.b += 75; resize(); flush(); expect(top).toBe(275); // Image/Markdown expansion above the survivor.
  height = 350; top = 150; fireEvent.scroll(el); resize(); flush(); expect(top).toBe(150); // Browser clamp is not new follow intent.
  height = 800; resize(); flush(); expect(top).toBe(150);
  ui.rerender(<ChatViewport><div>No anchored rows</div></ChatViewport>); flush(); expect(top).toBe(150); // Bounded absolute fallback, never tail.
});

function coordinatedViewport(historical = false, latestAnchor?: string) {
  let frame: FrameRequestCallback | undefined, height = 1000, top = 0;
  const positions: Record<string, number> = { a: 0, b: 200, 'turn:target': 500 };
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { frame = cb; return 1; });
  vi.stubGlobal('cancelAnimationFrame', () => { frame = undefined; });
  vi.stubGlobal('ResizeObserver', class { constructor(cb: () => void) { resize = cb; } observe() {} disconnect() {} });
  let owner: ChatViewport | null = null;
  const latest = vi.fn(), user = vi.fn(), active=vi.fn();
  const content = (ids: string[]) => ids.map(id => <div key={id} data-chat-anchor-key={id}>{id}</div>);
  const element = (ids: string[], past = historical) => <ChatViewport ref={value => { owner = value; }} historical={past} latestAnchor={latestAnchor} onActiveAnchor={active} latestLabel="Return to latest" onLatest={latest} onUserIntent={user}>{content(ids)}</ChatViewport>;
  const ui = render(element(['a', 'b', 'turn:target']));
  const el = ui.container.querySelector('.conversation-scroll') as HTMLElement;
  Object.defineProperties(el, { scrollHeight: { get: () => height }, clientHeight: { get: () => 200 }, scrollTop: { get: () => top, set: value => { top = value; } } });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) {
    const y = this.dataset.chatAnchorKey ? positions[this.dataset.chatAnchorKey] - top : 0;
    return { top: y, bottom: y + 200, x: 0, y, height: 200, width: 500, left: 0, right: 500, toJSON() {} };
  });
  const flush = () => { const cb = frame; frame = undefined; cb?.(0); };
  flush();
  return { ui, el, owner: () => owner!, latest, user, active, positions, flush, top: () => top,
    scroll(value: number) { top = value; fireEvent.scroll(el); },
    grow(value: number) { height += value; resize(); },
    replace(ids: string[], past = historical) { ui.rerender(element(ids, past)); } };
}

it('ordinary short detached reading exposes Return to latest and subsequently follows streaming', () => {
  const v = coordinatedViewport();
  expect(v.top()).toBe(800); expect(v.ui.queryByRole('button', { name: 'Return to latest' })).toBeNull();
  v.scroll(210); expect(v.ui.getByRole('button', { name: 'Return to latest' })).toBeTruthy();
  v.grow(400); v.flush(); expect(v.top()).toBe(210);
  fireEvent.click(v.ui.getByRole('button', { name: 'Return to latest' })); v.flush();
  expect(v.latest).toHaveBeenCalledOnce(); expect(v.top()).toBe(1200);
  v.grow(300); v.flush(); expect(v.top()).toBe(1500);
  v.scroll(210); v.grow(200); v.flush(); expect(v.top()).toBe(210);
});

it('newer native user scroll retires navigation both before reply and before its layout frame', () => {
  const v = coordinatedViewport();
  const first = v.owner().beginNavigation(); v.scroll(210);
  expect(first.current()).toBe(false); expect(first.commit('turn:target')).toBe(false);
  v.flush(); expect(v.top()).toBe(210);
  const second = v.owner().beginNavigation(); expect(second.commit('turn:target')).toBe(true);
  v.scroll(250); v.flush(); expect(v.top()).toBe(250); expect(second.current()).toBe(false);
});

it('target replacement exits follow, preserves reflow and missing anchors never enter follow', () => {
  const v = coordinatedViewport();
  const ticket = v.owner().beginNavigation();
  v.replace(['turn:target'], true); expect(ticket.commit('turn:target')).toBe(true); v.flush(); expect(v.top()).toBe(500);
  // Content, Tool/image disclosure and column/sidebar/panel reflow all deliver
  // the same observer contract; semantic anchor position, not height, wins.
  for (const reflow of [80, 150, 60, 90]) {
    v.positions['turn:target'] += reflow; v.grow(reflow + 100); v.flush();
    expect(v.top()).toBe(v.positions['turn:target']);
  }
  const missing = v.owner().beginNavigation(); v.replace([], true); missing.commit('turn:gone'); v.flush();
  const retained = v.top(); v.grow(500); v.flush(); expect(v.top()).toBe(retained);
  fireEvent.click(v.ui.getByRole('button', { name: 'Return to latest' }));
  // Latest installation may prepend overlapping content before its frame.
  v.positions.older = 0; v.replace(['older', 'a', 'b', 'turn:target'], false); v.flush();
  const bottom = v.top(); v.grow(100); v.flush(); expect(v.top()).toBe(bottom + 100);
});

it('authority replacement after native installation still retires the scheduled navigation frame', () => {
  const v = coordinatedViewport(); v.scroll(210);
  const ticket = v.owner().beginNavigation(); let authority = true;
  expect(ticket.commit('turn:target', () => authority)).toBe(true);
  authority = false; v.flush(); expect(v.top()).toBe(210);
});

it('follow publishes the live native turn without a locate cursor; historical anchors and Return to latest own active reading',()=>{
 const v=coordinatedViewport(false,'turn:live');
 expect(v.active).toHaveBeenLastCalledWith('turn:live');
 const ticket=v.owner().beginNavigation();v.replace(['turn:target'],true);ticket.commit('turn:target');v.flush();
 expect(v.top()).toBe(500);expect(v.active).toHaveBeenLastCalledWith('turn:target');
 v.grow(300);v.flush();expect(v.top()).toBe(500);
 fireEvent.click(v.ui.getByRole('button',{name:'Return to latest'}));v.replace(['a','b','turn:target'],false);v.flush();
 expect(v.active).toHaveBeenLastCalledWith('turn:live');const bottom=v.top();v.grow(100);v.flush();expect(v.top()).toBe(bottom+100);
});

import { useState } from 'react';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { TurnNavigator } from '../src/app/agent/TurnNavigator';
import { ConversationLive } from '../src/app/agent/ConversationLive';
import { Server, snapshot } from './fixture';
import { turnAnchor } from '../src/client/transcript';
import type { RuntimeClientTranscriptEntry, TurnProcessView } from '../../protocol/app-server/v37';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChatViewport } from '../src/presentation/layout/ChatViewport';
import { railAwareResizeObserver } from './turn-navigator-fixture';
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
  vi.stubGlobal('ResizeObserver', railAwareResizeObserver(callback => { resize = callback; }));
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

function coordinatedViewport(latestTurn?: string) {
  let frame: FrameRequestCallback | undefined, height = 1000, top = 0;
  const positions: Record<string, number> = { a: 0, b: 200, 'turn:target': 500 };
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { frame = cb; return 1; });
  vi.stubGlobal('cancelAnimationFrame', () => { frame = undefined; });
  vi.stubGlobal('ResizeObserver', class { constructor(cb: () => void) { resize = cb; } observe() {} disconnect() {} });
  let owner: ChatViewport | null = null;
  const active=vi.fn();
  const content = (ids: string[]) => ids.map(id => <div key={id} data-chat-anchor-key={id} data-chat-turn-owner={id.startsWith('turn:') ? id : undefined}>{id}</div>);
  const element = (ids: string[]) => <ChatViewport ref={value => { owner = value; }} latestTurn={latestTurn} onActiveTurn={active} latestLabel="Return to latest">{content(ids)}</ChatViewport>;
  const ui = render(element(['a', 'b', 'turn:target']));
  const el = ui.container.querySelector('.conversation-scroll') as HTMLElement;
  Object.defineProperties(el, { scrollHeight: { get: () => height }, clientHeight: { get: () => 200 }, scrollTop: { get: () => top, set: value => { top = value; } } });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) {
    const y = this.dataset.chatAnchorKey ? positions[this.dataset.chatAnchorKey] - top : 0;
    return { top: y, bottom: y + 200, x: 0, y, height: 200, width: 500, left: 0, right: 500, toJSON() {} };
  });
  const flush = () => { const cb = frame; frame = undefined; cb?.(0); };
  flush();
  return { ui, el, owner: () => owner!, active, positions, flush, top: () => top,
    scroll(value: number) { top = value; fireEvent.scroll(el); },
    grow(value: number) { height += value; resize(); },
    replace(ids: string[]) { ui.rerender(element(ids)); } };
}

it('ordinary short detached reading exposes Return to latest and subsequently follows streaming', () => {
  const v = coordinatedViewport();
  expect(v.top()).toBe(800); expect(v.ui.queryByRole('button', { name: 'Return to latest' })).toBeNull();
  v.scroll(210); expect(v.ui.getByRole('button', { name: 'Return to latest' })).toBeTruthy();
  v.grow(400); v.flush(); expect(v.top()).toBe(210);
  fireEvent.click(v.ui.getByRole('button', { name: 'Return to latest' })); v.flush();
  expect(v.top()).toBe(1200);
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
  v.replace(['turn:target']); expect(ticket.commit('turn:target')).toBe(true); v.flush(); expect(v.top()).toBe(500);
  // Content, Tool/image disclosure and column/sidebar/panel reflow all deliver
  // the same observer contract; semantic anchor position, not height, wins.
  for (const reflow of [80, 150, 60, 90]) {
    v.positions['turn:target'] += reflow; v.grow(reflow + 100); v.flush();
    expect(v.top()).toBe(v.positions['turn:target']);
  }
  const missing = v.owner().beginNavigation(); v.replace([]); missing.commit('turn:gone'); v.flush();
  const retained = v.top(); v.grow(500); v.flush(); expect(v.top()).toBe(retained);
  fireEvent.click(v.ui.getByRole('button', { name: 'Return to latest' }));
  // Content may prepend before the frame that returns to the tail.
  v.positions.older = 0; v.replace(['older', 'a', 'b', 'turn:target']); v.flush();
  const bottom = v.top(); v.grow(100); v.flush(); expect(v.top()).toBe(bottom + 100);
});

it('authority replacement after native installation still retires the scheduled navigation frame', () => {
  const v = coordinatedViewport(); v.scroll(210);
  const ticket = v.owner().beginNavigation(); let authority = true;
  expect(ticket.commit('turn:target', () => authority)).toBe(true);
  authority = false; v.flush(); expect(v.top()).toBe(210);
});

it('follow publishes the live native turn without a locate cursor; navigated anchors and Return to latest own active reading',()=>{
 const v=coordinatedViewport('turn:live');
 expect(v.active).toHaveBeenLastCalledWith('turn:live');
 const ticket=v.owner().beginNavigation();v.replace(['turn:target']);ticket.commit('turn:target');v.flush();
 expect(v.top()).toBe(500);expect(v.active).toHaveBeenLastCalledWith('turn:target');
 v.grow(300);v.flush();expect(v.top()).toBe(500);
 fireEvent.click(v.ui.getByRole('button',{name:'Return to latest'}));v.replace(['a','b','turn:target']);v.flush();
 expect(v.active).toHaveBeenLastCalledWith('turn:live');const bottom=v.top();v.grow(100);v.flush();expect(v.top()).toBe(bottom+100);
});

for (const outcome of ['completed', 'timed_out'] as const) it(`native ${outcome} long-turn regions own reading and rail through reflow and the final tail`, async () => {
  const server = new Server();
  await server.attached('A');
  const owner = (attempt: string, cursor: string): TurnProcessView => ({
    conversation_id: 'conversation-A', attempt_id: attempt, control_cursor: cursor,
    outcome, message_count: 2, tool_call_count: 0, final_message_id: `${attempt}-body`,
  });
  const a = owner('A', '1'), b = owner('B', '3');
  const entry = (cursor: string, id: string, turn_process: TurnProcessView): RuntimeClientTranscriptEntry => ({
    cursor, turn_process, item: { type: 'message', message: { role: 'assistant', id, content: [{ type: 'text', text: id }] } },
  });
  const entries = [entry('1', 'A-start', a), entry('2', 'A-body', a), entry('3', 'B-start', b), entry('4', 'B-body', b)];
  server.handlers.set('session/turns', () => ({ type: 'conversation_turns', page: {
    cut: { conversation_id: 'conversation-A', journal: '6', transcript: '4', mutation_revision: '0' },
    offset: 0, total: 2, turns: [a, b].map((turn, index) => ({ id: turn, ordinal: index + 1, cursor: turn.control_cursor, prompt: '', response: '' })),
  } }));
  let frame: FrameRequestCallback | undefined, height = 2000;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frame = callback; return 1; });
  vi.stubGlobal('cancelAnimationFrame', () => { frame = undefined; });
  vi.stubGlobal('ResizeObserver', railAwareResizeObserver(callback => { resize = callback; }));
  const active = vi.fn();
  function Reading() {
    const [current, setCurrent] = useState<string | null>();
    return <ChatViewport onActiveTurn={key => { active(key); setCurrent(key); }}
      overlay={<TurnNavigator client={server.client} sessionId="A" active={current} onNavigate={() => {}}/>}>
      <AgentTranscript snapshot={{ ...snapshot(), transcript: { entries } }}/>
    </ChatViewport>;
  }
  const ui = await act(async () => render(<Reading/>));
  const viewport = ui.container.querySelector<HTMLElement>('.conversation-scroll')!;
  const positions: Record<string, number> = { [turnAnchor(a)]: 0, 'message:A-start': 0, 'message:A-body': 100,
    [turnAnchor(b)]: 1000, 'message:B-start': 1000, 'message:B-body': 1100 };
  Object.defineProperties(viewport, { scrollHeight: { get: () => height }, clientHeight: { get: () => 200 } });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) {
    const key = this.dataset.chatAnchorKey, top = key ? positions[key] - viewport.scrollTop : 0;
    const size = key?.endsWith('-body') ? 900 : 20;
    return { top, bottom: top + size, left: 0, right: 500, width: 500, height: size, x: 0, y: top, toJSON() {} };
  });
  const flush = () => act(() => { const callback = frame; frame = undefined; callback?.(0); });
  const scroll = (top: number) => { viewport.scrollTop = top; fireEvent.scroll(viewport); };
  const expectActive = (turn: TurnProcessView) => {
    expect(active).toHaveBeenLastCalledWith(turnAnchor(turn));
    const marks = [...ui.container.querySelectorAll('[data-turn-id][aria-current="true"]')];
    expect(marks.map(mark => mark.getAttribute('data-turn-id'))).toEqual([turnAnchor(turn).slice(5)]);
  };
  flush(); scroll(400); flush();
  expectActive(a); // A marker ends at 20, A body contains 400, B begins at 1000.
  // Disclosure updates and width/image growth use the same observer and frame.
  if (outcome === 'completed') fireEvent.click(ui.getAllByRole('button', { name: 'Worked' })[0]);
  for (const growth of [80, 150, 60]) {
    positions['message:A-body'] += growth;
    positions[turnAnchor(b)] += growth; positions['message:B-start'] += growth; positions['message:B-body'] += growth;
    height += growth;
    resize(); flush();
    expect(viewport.scrollTop).toBe(positions['message:A-body'] + 300);
    expectActive(a);
  }
  // The reading line is 40px into this 200px viewport; keep the exact 1px boundary.
  scroll(positions[turnAnchor(b)] - 41); expectActive(a);
  scroll(positions[turnAnchor(b)] - 40); expectActive(b);
  scroll(positions['message:B-body'] + 500); flush();
  expectActive(b); // No later anchor; B remains active after its marker is gone.
  ui.unmount(); server.client.disconnect();
});


it('the final native turn remains active beyond its location marker', () => {
  const v = coordinatedViewport();
  v.scroll(750);
  expect(v.active).toHaveBeenLastCalledWith('turn:target');
  v.grow(200); v.flush();
  expect(v.top()).toBe(750);
  expect(v.active).toHaveBeenLastCalledWith('turn:target');
});

async function clippedReading(outcome: 'completed' | 'timed_out', later = true) {
  const server = new Server();
  await server.attached('A');
  const owner = (attempt: string, cursor: string): TurnProcessView => ({
    conversation_id: 'conversation-A', attempt_id: attempt, control_cursor: cursor,
    outcome, message_count: 2, tool_call_count: 1, final_message_id: `${attempt}-body`,
  });
  const a = owner('A', '90'), b = owner('B', '120');
  const row = (cursor: string, id: string, turn_process: TurnProcessView): RuntimeClientTranscriptEntry => ({
    cursor, turn_process, item: { type: 'message', message: { role: 'assistant', id, content: [{ type: 'text', text: id }] } },
  });
  const entries = [row('100', 'A-prefix', a), row('101', 'A-body', a), ...(later ? [row('120', 'B-start', b), row('121', 'B-body', b)] : [])];
  const cut = { conversation_id: 'conversation-A', journal: '130', transcript: '121', mutation_revision: '0' };
  const turns = [a, b].map((turn, index) => ({ id: turn, ordinal: index + 1, cursor: turn.control_cursor, prompt: '', response: '' }));
  server.handlers.set('session/turns', () => ({ type: 'conversation_turns', page: { cut, offset: 0, total: 2, turns } }));
  let frame: FrameRequestCallback | undefined, height = 2200;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frame = callback; return 1; });
  vi.stubGlobal('cancelAnimationFrame', () => { frame = undefined; });
  vi.stubGlobal('ResizeObserver', railAwareResizeObserver(callback => { resize = callback; }));
  const active = vi.fn();
  function Reading() {
    const [current, setCurrent] = useState<string | null>();
    return <ChatViewport onActiveTurn={key => { active(key); setCurrent(key); }}
      overlay={<TurnNavigator client={server.client} sessionId="A" active={current} onNavigate={() => {}}/>}>
      <AgentTranscript snapshot={{ ...snapshot(), transcript: { entries } }}/>
    </ChatViewport>;
  }
  const ui = await act(async () => render(<Reading/>));
  const viewport = ui.container.querySelector<HTMLElement>('.conversation-scroll')!;
  const positions: Record<string, number> = { 'message:A-prefix': 0, 'message:A-body': 100,
    'message:B-start': 1000, 'message:B-body': 1100, [turnAnchor(a)]: 0, [turnAnchor(b)]: 1000 };
  Object.defineProperties(viewport, { scrollHeight: { get: () => height }, clientHeight: { get: () => 200 } });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) {
    const key = this.dataset.chatAnchorKey ?? this.dataset.chatTurnOwner;
    const top = key ? positions[key] - viewport.scrollTop : 0;
    const size = key?.endsWith('-body') ? 900 : 20;
    return { top, bottom: top + size, left: 0, right: 500, width: 500, height: size, x: 0, y: top, toJSON() {} };
  });
  const flush = () => act(() => { const callback = frame; frame = undefined; callback?.(0); });
  const scroll = (top: number) => { viewport.scrollTop = top; fireEvent.scroll(viewport); flush(); };
  const expectActive = (turn: TurnProcessView) => {
    expect(active).toHaveBeenLastCalledWith(turnAnchor(turn));
    expect([...ui.container.querySelectorAll('[data-turn-id][aria-current="true"]')].map(mark => mark.getAttribute('data-turn-id'))).toEqual([turnAnchor(turn).slice(5)]);
  };
  const expectClipped = () => {
    for (const id of ['message:A-prefix', 'message:A-body']) {
      expect(ui.container.querySelector(`[data-chat-anchor-key="${id}"]`)?.getAttribute('data-chat-turn-owner')).toBe(turnAnchor(a));
    }
    expect([...ui.container.querySelectorAll('[data-chat-anchor-key]')].some(row => row.getAttribute('data-chat-anchor-key') === turnAnchor(a))).toBe(false);
  };
  return { server, a, b, ui, viewport, positions, turns, cut, entries, flush, scroll, expectActive, expectClipped,
    grow(amount: number) { positions['message:A-body'] += amount; positions['message:B-start'] += amount; positions['message:B-body'] += amount; positions[turnAnchor(b)] += amount; height += amount; resize(); flush(); } };
}

for (const outcome of ['completed', 'timed_out'] as const) {
  it(`clipped ${outcome} first Attempt owns reading before the exact B boundary through disclosure and reflow`, async () => {
    const v = await clippedReading(outcome);
    try {
      v.flush(); v.scroll(400); v.expectActive(v.a); v.expectClipped();
      if (outcome === 'completed') fireEvent.click(v.ui.getAllByRole('button', { name: 'Worked' })[0]);
      for (const growth of [80, 150]) {
        v.grow(growth); expect(v.viewport.scrollTop).toBe(v.positions['message:A-body'] + 300); v.expectActive(v.a); v.expectClipped();
      }
      // Native ownership switches exactly when B reaches the 40px reading line.
      v.scroll(v.positions['message:B-start'] - 41); v.expectActive(v.a);
      v.scroll(v.positions['message:B-start'] - 40); v.expectActive(v.b);
      v.scroll(v.positions['message:B-body'] + 500); v.expectActive(v.b);
    } finally { v.ui.unmount(); v.server.client.disconnect(); }
  });
  it(`final clipped ${outcome} Attempt owns the whole loaded tail without a fabricated locate anchor`, async () => {
    const v = await clippedReading(outcome, false);
    try {
      v.flush();
      for (const top of [1, 400, 800, 1600]) { v.scroll(top); v.expectActive(v.a); v.expectClipped(); }
      v.grow(100); expect(v.viewport.scrollTop).toBe(1600); v.expectActive(v.a); v.expectClipped();
    } finally { v.ui.unmount(); v.server.client.disconnect(); }
  });
}

it('an older page prepends above the reading position and clipped ownership comes from the merged projection', async () => {
  const v = await clippedReading('completed');
  v.ui.unmount();
  try {
    await act(async () => { v.server.snapshots.set('A', { ...snapshot(), transcript: { entries: v.entries.slice(2), next_cursor: '120' } }); await v.server.client.refresh('A'); });
    await v.server.client.readTurns('A');
    v.server.held.add('session/transcript');
    const ui = await act(async () => render(<ConversationLive client={v.server.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}}/>));
    const viewport = ui.container.querySelector<HTMLElement>('.conversation-scroll')!;
    Object.defineProperties(viewport, { scrollHeight: { value: 2200 }, clientHeight: { value: 200 } });
    // Geometry belongs to the rendered window, just as native ownership does.
    vi.mocked(HTMLElement.prototype.getBoundingClientRect).mockImplementation(function(this: HTMLElement) {
      const key = this.dataset.chatAnchorKey ?? this.dataset.chatTurnOwner;
      const offset = v.server.client.getSnapshot().views.A.history?.page.entries?.[0].cursor === '120' ? 1000 : 0;
      const top = key ? v.positions[key] - offset - viewport.scrollTop : 0;
      return { top, bottom: top + 900, left: 0, right: 500, width: 500, height: 900, x: 0, y: top, toJSON() {} };
    });
    v.flush(); viewport.scrollTop = 400; fireEvent.scroll(viewport); v.flush();
    const current = () => [...ui.container.querySelectorAll('[data-turn-id][aria-current="true"]')].map(mark => mark.getAttribute('data-turn-id'));
    expect(current()).toEqual([turnAnchor(v.b).slice(5)]);
    const older = v.server.client.loadEarlier('A'), read = await v.server.waitFor('session/transcript', 1);
    expect(read.params).toMatchObject({ limit: 64 }); expect(read.params).toHaveProperty('at', { type: 'older', before: '120' });
    await act(async () => {
      v.server.socket.success(read, { type: 'transcript_window', window: { page: { entries: v.entries.slice(0, 2), next_cursor: '100' } } });
      await older;
    });
    v.flush();
    expect(v.server.client.getSnapshot().views.A.history?.page.entries?.map(row => row.cursor)).toEqual(['100', '101', '120', '121']);
    // The prepended page lands above the reader: B stays where it was read.
    expect(viewport.scrollTop).toBe(1400); expect(current()).toEqual([turnAnchor(v.b).slice(5)]);
    viewport.scrollTop = 400; fireEvent.scroll(viewport); v.flush();
    expect(current()).toEqual([turnAnchor(v.a).slice(5)]);
    expect([...ui.container.querySelectorAll('[data-chat-anchor-key]')].some(row => row.getAttribute('data-chat-anchor-key') === turnAnchor(v.a))).toBe(false);
    ui.unmount();
  } finally { v.server.client.disconnect(); }
});

it('unowned gaps retain the reading turn and semantic ownership cannot satisfy an exact locate', () => {
  const v = coordinatedViewport('turn:live');
  v.scroll(750); expect(v.active).toHaveBeenLastCalledWith('turn:target');
  v.positions.unowned = 700; v.replace(['turn:target', 'unowned']); v.flush();
  expect(v.active).toHaveBeenLastCalledWith('turn:target');
  v.replace(['unowned']); v.flush();
  expect(v.active).toHaveBeenLastCalledWith(null);
  const owner = v.owner();
  v.ui.rerender(<ChatViewport onActiveTurn={v.active}><div data-chat-anchor-key="b" data-chat-turn-owner="turn:clipped">clipped</div></ChatViewport>);
  v.flush(); expect(v.active).toHaveBeenLastCalledWith('turn:clipped');
  const top = v.top(), intent = owner.beginNavigation();
  expect(intent.commit('turn:clipped')).toBe(true); v.flush();
  expect(v.top()).toBe(top); // The owner exists, but no exact location is loaded.
});

it('reading line selects the first padded turn and the next visible turn before its top reaches the viewport edge', () => {
  const v = coordinatedViewport();
  v.positions['turn:first'] = 20;
  v.positions['turn:second'] = 520;
  v.replace(['turn:first', 'turn:second']); v.flush();
  v.scroll(0); expect(v.active).toHaveBeenLastCalledWith('turn:first');
  v.scroll(470); expect(v.active).toHaveBeenLastCalledWith('turn:first');
  v.scroll(490); expect(v.active).toHaveBeenLastCalledWith('turn:second');
  v.scroll(470); expect(v.active).toHaveBeenLastCalledWith('turn:first');
});

it('following a settled short final turn selects its native owner even while the preceding turn crosses the reading line', () => {
  const v = coordinatedViewport();
  v.positions['turn:first'] = 20;
  v.positions['turn:second'] = 900;
  v.replace(['turn:first', 'turn:second']); v.flush();
  expect(v.top()).toBe(800);
  expect(v.active).toHaveBeenLastCalledWith('turn:second');
  v.scroll(500); expect(v.active).toHaveBeenLastCalledWith('turn:first');
  fireEvent.click(v.ui.getByRole('button', { name: 'Return to latest' })); v.flush();
  expect(v.active).toHaveBeenLastCalledWith('turn:second');
});

it('reading remains continuous across prompts, nested anchors, padding and hidden turns in both directions', () => {
  const v = coordinatedViewport();
  Object.assign(v.positions, { first: 120, nested: 180, prompt: 400, hidden: 450, second: 620, tail: 700 });
  v.ui.rerender(<ChatViewport onActiveTurn={v.active}>
    <div data-chat-anchor-key="first" data-chat-turn-owner="turn:first">
      <div data-chat-anchor-key="nested">Nested response content</div>
    </div>
    <div data-chat-anchor-key="prompt">Next user prompt</div>
    <div hidden><div data-chat-anchor-key="hidden" data-chat-turn-owner="turn:hidden"/></div>
    <div data-chat-anchor-key="second" data-chat-turn-owner="turn:second"/>
    <div data-chat-anchor-key="tail">Unowned tail</div>
  </ChatViewport>);
  v.flush();
  expect(v.active).toHaveBeenLastCalledWith('turn:second');
  for (const top of [0, 100, 200, 380, 500, 579, 580, 650, 580, 579, 500, 380, 200, 0]) {
    v.scroll(top);
    expect(v.active).toHaveBeenLastCalledWith(top >= 580 ? 'turn:second' : 'turn:first');
  }
});

for (const distance of [0.25, 0.5, 1, 6, 23]) it(`upward reader movement of ${distance}px releases tail follow immediately`, () => {
  const v = coordinatedViewport();
  v.scroll(800 - distance); v.flush();
  expect(v.top()).toBe(800 - distance);
  expect(v.ui.getByRole('button', { name: 'Return to latest' })).toBeTruthy();
  v.grow(40); v.flush();
  expect(v.top()).toBe(800 - distance);
  v.scroll(840); v.flush();
  v.grow(30); v.flush(); expect(v.top()).toBe(870);
});

it('actual reader movement wins a queued layout frame even before scroll event delivery', () => {
  const v = coordinatedViewport();
  v.grow(40);
  v.el.scrollTop = 794; // Native scrolling has happened; its scroll event is still queued.
  v.flush();
  expect(v.top()).toBe(794);
  fireEvent.scroll(v.el); v.flush();
  expect(v.top()).toBe(794);
  v.grow(40); v.flush(); expect(v.top()).toBe(794);
});

it('explicit latest and navigation supersede undelivered earlier scroll samples', () => {
  const v = coordinatedViewport();
  v.el.scrollTop = 700;
  v.owner().returnToBottom(); v.flush(); expect(v.top()).toBe(800);
  v.el.scrollTop = 700;
  const navigation = v.owner().beginNavigation();
  expect(navigation.commit('turn:target')).toBe(true);
  v.flush(); expect(v.top()).toBe(500);
  expect(navigation.current()).toBe(true);
});

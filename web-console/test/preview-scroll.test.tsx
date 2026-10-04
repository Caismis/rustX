import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { PreviewWorkspaceOwner } from '../src/app/preview-workspace';
import { PreviewWorkspace } from '../src/app/components/PreviewWorkspace';
import type { PreviewArtifact } from '../src/app/components/ArtifactPreview';
import { ChatViewport } from '../src/presentation/layout/ChatViewport';
import { RightPanel } from '../src/presentation/right-panel/RightPanel';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { Server } from './fixture';

// Payload and viewer security have separate gated resource tests. Keep the real
// workspace, pane controls, resource leases and both geometry/scroll owners here.
vi.mock('../src/app/components/ArtifactPreview', () => ({ ArtifactPreview: ({ artifact }: { artifact: PreviewArtifact }) => <div data-live-document>{artifact.name}</div> }));

const cleanups: (() => void)[] = [];
afterEach(() => { cleanup(); cleanups.splice(0).forEach(stop => stop()); vi.unstubAllGlobals(); vi.restoreAllMocks(); });
async function fixture(mode: 'following' | 'detached' | 'historical') {
  const server = new Server(); await server.attached('A');
  const authority = new WorkspaceAuthority(server.workspaceHost); await authority.observe();
  const owner = new PreviewWorkspaceOwner(server.client, server.workspaceHost, authority), stop = owner.start();
  owner.selectSession('A');
  cleanups.push(() => { stop(); server.client.disconnect(); });
  const requests = server.requests.length;
  let width = 1000, height = 1400, top = 0, writes = 0, sequence = 0, streamed = 0, chat!: ChatViewport;
  const positions: Record<string, number> = { a: 0, b: 200, 'turn:history': 500, 'turn:stream': 1100 };
  const frames = new Map<number, FrameRequestCallback>();
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++sequence, callback); return sequence; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  const observers: { targets: Set<Element>; deliver: () => void; disconnect: ReturnType<typeof vi.fn> }[] = [];
  vi.stubGlobal('ResizeObserver', class {
    targets = new Set<Element>();
    disconnect = vi.fn(() => this.targets.clear());
    constructor(readonly deliver: () => void) { observers.push(this); }
    observe(target: Element) { this.targets.add(target); }
  });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) {
    const key = this.dataset.chatAnchorKey, y = key ? positions[key] - top : 0;
    const measured = this.hasAttribute('data-preview-workspace') ? width : 500;
    return { top: y, bottom: y + 200, left: 0, right: measured, width: measured, height: 200, x: 0, y, toJSON() {} };
  });
  function Shell() {
    const snapshot = useSyncExternalStore(owner.subscribe, owner.getSnapshot);
    return <>
      <button onClick={() => owner.toggleInspector()}>Inspector</button>
      <button onClick={() => owner.reveal()}>Reopen previews</button>
      <ChatViewport ref={node => { if (node) chat = node; }} historical={mode === 'historical'} latestLabel="Return to latest" latestTurn="turn:latest">
        {Object.keys(positions).map(key => <div key={key} data-chat-anchor-key={key} data-chat-turn-owner={key.startsWith('turn:') ? key : undefined}>{key}{key === 'turn:stream' ? ` streamed chunk ${streamed}` : ''}</div>)}
      </ChatViewport>
      <RightPanel open={snapshot.mode !== 'collapsed'} close={() => owner.collapse()} width={width} canShow={width >= 608} fullscreen={snapshot.workspace?.fullscreen} title="Previews" closeLabel="Collapse previews">
        <PreviewWorkspace owner={owner} snapshot={snapshot} focusRequest={0} returnFocus={() => {}}/>
      </RightPanel>
    </>;
  }
  const ui = render(<Shell/>), viewport = ui.container.querySelector<HTMLElement>('.conversation-scroll')!;
  Object.defineProperties(viewport, {
    scrollHeight: { get: () => height }, clientHeight: { get: () => 200 },
    scrollTop: { get: () => top, set: value => { top = value; writes++; } },
  });
  const flush = () => act(() => { const pending = [...frames.values()]; frames.clear(); pending.forEach(callback => callback(0)); });
  const emit = (predicate: (element: Element) => boolean) => observers.filter(observer => [...observer.targets].some(predicate)).forEach(observer => observer.deliver());
  const resizeChat = () => emit(element => element.classList.contains('conversation-scroll'));
  const scroll = (value: number) => { top = value; fireEvent.scroll(viewport); };
  flush(); expect(top).toBe(1200);
  if (mode === 'detached') { scroll(210); flush(); }
  if (mode === 'historical') {
    act(() => { const navigation = chat.beginNavigation(); expect(navigation.commit('turn:history')).toBe(true); });
    flush(); expect(top).toBe(500);
  }
  return {
    ui, owner, server, requests, frames, observers, flush, scroll, top: () => top, writes: () => writes, floor: () => height - 200,
    open(id: string) { owner.openPreview({ source: { kind: 'artifact', id }, name: `${id}.txt`, image: false }); },
    width(value: number) { width = value; emit(element => element.hasAttribute('data-preview-workspace')); },
    reflow(delta: number) { positions.b += delta; positions['turn:history'] += delta; positions['turn:stream'] += delta; height += delta; resizeChat(); },
    stream(delta: number) { height += delta; streamed++; ui.rerender(<Shell/>); resizeChat(); },
  };
}

it.each(['following', 'detached', 'historical'] as const)('actual preview geometry actions preserve %s Conversation ownership', async mode => {
  const f = await fixture(mode);
  const transition = (action: () => void) => {
    const before = f.top(), writes = f.writes();
    act(action);
    // Preview controls never write transcript scroll position themselves.
    expect(f.top()).toBe(before); expect(f.writes()).toBe(writes);
    act(() => f.reflow(37)); expect(f.top()).toBe(before);
    f.flush(); expect(f.top()).toBe(mode === 'following' ? f.floor() : before + 37);
    const held = f.top(); act(() => f.stream(23)); f.flush();
    expect(f.top()).toBe(mode === 'following' ? f.floor() : held);
    expect(f.ui.queryByRole('button', { name: 'Return to latest' }) !== null).toBe(mode !== 'following');
  };
  transition(() => f.open('A'));
  transition(() => f.open('B'));
  transition(() => fireEvent.click(f.ui.getByRole('button', { name: 'Split preview' })));
  expect(f.owner.getSnapshot().workspace?.panes).toHaveLength(2);
  transition(() => fireEvent.keyDown(f.ui.getByRole('separator', { name: 'Resize preview panes' }), { key: 'ArrowRight' }));
  expect(f.owner.getSnapshot().workspace?.ratio).toBe(.55);
  const divider = f.ui.getByRole('separator', { name: 'Resize preview panes' });
  const capture = vi.fn(), release = vi.fn();
  divider.setPointerCapture = capture; divider.releasePointerCapture = release; divider.hasPointerCapture = () => true;
  vi.stubGlobal('PointerEvent', MouseEvent);
  transition(() => { fireEvent.pointerDown(divider, { button: 0, clientX: 500 }); fireEvent.pointerMove(divider, { clientX: 550 }); fireEvent.pointerUp(divider, { clientX: 550 }); });
  expect(capture).toHaveBeenCalledOnce(); expect(release).toHaveBeenCalledOnce();
  const occurrences = f.owner.getSnapshot().workspace!.tabs.map(tab => tab.id), leases = [...f.owner.getSnapshot().leases.values()];
  transition(() => fireEvent.click(f.ui.getByRole('button', { name: 'Fullscreen preview' })));
  transition(() => fireEvent.click(f.ui.getByRole('button', { name: 'Restore preview' })));
  expect([...f.owner.getSnapshot().leases.values()]).toEqual(leases);
  transition(() => f.width(390)); expect(f.owner.getSnapshot().leases.size).toBe(1);
  transition(() => fireEvent.click(f.ui.getByRole('button', { name: 'Switch preview pane' })));
  transition(() => f.width(1000)); expect(f.owner.getSnapshot().leases.size).toBe(2);
  transition(() => fireEvent.click(f.ui.getByRole('button', { name: 'Inspector' })));
  expect(f.owner.getSnapshot().leases.size).toBe(0);
  transition(() => fireEvent.click(f.ui.getByRole('button', { name: 'Inspector' })));
  transition(() => fireEvent.click(f.ui.getByRole('button', { name: 'Collapse previews' })));
  expect(f.owner.getSnapshot().leases.size).toBe(0);
  transition(() => f.width(390)); transition(() => f.width(1000));
  expect(f.owner.getSnapshot().mode).toBe('collapsed');
  transition(() => fireEvent.click(f.ui.getByRole('button', { name: 'Reopen previews' })));
  expect(f.owner.getSnapshot().workspace!.tabs.map(tab => tab.id)).toEqual(occurrences);
  expect(f.server.requests).toHaveLength(f.requests);
  f.ui.unmount(); expect(f.frames.size).toBe(0);
  expect(f.observers).toHaveLength(2); f.observers.forEach(observer => expect(observer.disconnect).toHaveBeenCalledOnce());
});

it('a user scroll between preview reflow and the owned correction frame wins and remains detached during streaming', async () => {
  const f = await fixture('following');
  act(() => f.open('A'));
  act(() => f.reflow(80));
  const writes = f.writes(); expect(f.frames.size).toBeGreaterThan(0);
  f.scroll(240); f.flush();
  expect(f.top()).toBe(240); expect(f.writes()).toBe(writes);
  act(() => { f.owner.toggleFullscreen(); f.stream(800); }); f.flush();
  expect(f.top()).toBe(240); expect(f.ui.getByRole('button', { name: 'Return to latest' })).toBeTruthy();
  expect(f.server.requests).toHaveLength(f.requests);
});

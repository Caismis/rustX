import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { PreviewWorkspaceOwner, samePreviewSource } from '../src/app/preview-workspace';
import { PreviewWorkspace } from '../src/app/components/PreviewWorkspace';
import { PREVIEW_POLICY } from '../src/client/preview-policy';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import type { PreviewArtifact } from '../src/app/components/ArtifactPreview';
import type { PreviewSource } from '../src/client/session-files';
import { Server, snapshot as nativeSnapshot } from './fixture';

const leases = vi.hoisted(() => ({ created: [] as { id: number; signal: AbortSignal }[], downloads: [] as unknown[] }));
vi.mock('../src/client/session-files', async original => {
  const actual = await original<typeof import('../src/client/session-files')>();
  return { ...actual, FilePreviewCoordinator: class {
    owned: AbortController[] = [];
    acquire(id: number, source: PreviewSource) {
      const abort = new AbortController(); this.owned.push(abort); leases.created.push({ id, signal: abort.signal });
      return { source, signal: abort.signal, dispose: () => abort.abort(), current: () => !abort.signal.aborted };
    }
    dispose() { this.owned.forEach(abort => abort.abort()); }
    async download(...args: unknown[]) { leases.downloads.push(args); }
  } };
});
vi.mock('../src/app/components/ArtifactPreview', () => ({ ArtifactPreview: ({ artifact, onDownload }: { artifact: PreviewArtifact; onDownload: () => void }) => <div data-live-document>{artifact.name}<button data-preview-download onClick={onDownload}>Download {artifact.name}</button></div> }));
let server: Server, owner: PreviewWorkspaceOwner, stop: () => void;
let width = 1000, resize: () => void, disconnect: ReturnType<typeof vi.fn>;
const artifact = (id: string, name = 'same.txt'): PreviewArtifact => ({ source: { kind: 'artifact', id }, name, image: false, mimeType: 'text/plain' });
const file: PreviewSource = { kind: 'session_file', messageId: 'message-a', index: 0, file: { scope: { conversation_id: 'original', device: '1', inode: '2' }, path: 'dir/same.txt', name: 'same.txt', mime_type: 'text/plain' } };
const workspace = () => owner.getSnapshot().workspace!;
const live = () => leases.created.filter(lease => !lease.signal.aborted);
beforeEach(async () => {
  server = new Server(); await server.attached('A', 'B');
  const authority = new WorkspaceAuthority(server.workspaceHost); await authority.observe();
  owner = new PreviewWorkspaceOwner(server.client, server.workspaceHost, authority); stop = owner.start(); owner.selectSession('A');
  leases.created.length = 0; leases.downloads.length = 0; width = 1000; disconnect = vi.fn();
  vi.stubGlobal('ResizeObserver', class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect = disconnect; });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(() => ({ width, height: 500, x: 0, y: 0, top: 0, bottom: 500, left: 0, right: width, toJSON() {} }));
});
afterEach(() => { cleanup(); stop(); server.client.disconnect(); vi.unstubAllGlobals(); });
function Shell({ returnFocus = () => {} }: { returnFocus?: () => void }) {
  const snapshot = useSyncExternalStore(owner.subscribe, owner.getSnapshot);
  return <PreviewWorkspace owner={owner} snapshot={snapshot} focusRequest={0} returnFocus={returnFocus}/>;
}
it('exact closed sources distinguish names, namespaces, declarations, indices and every reference field', () => {
  const sources: PreviewSource[] = [{ kind: 'artifact', id: 'one' }, { kind: 'artifact', id: 'two' }, file,
    { ...file, messageId: 'message-b' }, { ...file, index: 1 }, { ...file, file: { ...file.file, path: 'other/same.txt' } },
    { ...file, file: { ...file.file, scope: { ...file.file.scope, conversation_id: 'other' } } }];
  const ids = sources.map(source => owner.openPreview({ ...artifact(''), source }));
  expect(new Set(ids).size).toBe(7); expect(workspace().tabs).toHaveLength(7);
  for (const source of sources) expect(samePreviewSource(source, structuredClone(source))).toBe(true);
  for (const change of [{ device: '3' }, { inode: '4' }]) expect(samePreviewSource(file, { ...file, file: { ...file.file, scope: { ...file.file.scope, ...change } } })).toBe(false);
  for (const change of [{ name: 'else.txt' }, { mime_type: 'text/markdown' }, { description: '' }]) expect(samePreviewSource(file, { ...file, file: { ...file.file, ...change } })).toBe(false);
});
it('duplicate opens reveal the existing other-pane occurrence; close and reopen allocate a new lifetime', () => {
  const first = owner.openPreview(artifact('first'))!, second = owner.openPreview(artifact('second'))!; owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.split();
  const pane = workspace().activePane; owner.selectTab(first);
  expect(owner.openPreview(artifact('second'))).toBe(second); expect(workspace().activePane).toBe(pane); expect(workspace().tabs).toHaveLength(2);
  const old = owner.getSnapshot().leases.get(second)!; owner.closeTab(second); const reopened = owner.openPreview(artifact('second'))!;
  expect(reopened).toBeGreaterThan(second); expect(old.signal.aborted).toBe(true); expect(workspace().panes).toHaveLength(1);
});
it('eight tabs reject overflow without loss and retain metadata without hidden leases', () => {
  const ids = Array.from({ length: PREVIEW_POLICY.tabsPerSession }, (_, index) => owner.openPreview(artifact(String(index)))!);
  const before = workspace(); expect(owner.openPreview(artifact('overflow'))).toBeUndefined(); expect(owner.getSnapshot().problem).toBe('tabs'); expect(workspace()).toBe(before);
  owner.updateView(ids[0], { bodyScrollTop: 83, wrap: false, pdfPage: 7, workbookOffset: 200, htmlMode: 'source' });
  owner.selectTab(ids[0]); expect(workspace().tabs[0].view).toMatchObject({ bodyScrollTop: 83, wrap: false, pdfPage: 7 }); expect(live()).toHaveLength(1);
  owner.selectTab(ids[1]); expect(live()).toHaveLength(1); owner.selectTab(ids[0]); expect(workspace().tabs[0].view.bodyScrollTop).toBe(83);
});
it('selected close prefers right then left; unselected close preserves selection; final pane has no default document', () => {
  const a = owner.openPreview(artifact('a'))!, b = owner.openPreview(artifact('b'))!, c = owner.openPreview(artifact('c'))!;
  owner.selectTab(b); owner.closeTab(b); expect(workspace().panes[0].selected).toBe(c);
  owner.closeTab(a); expect(workspace().panes[0].selected).toBe(c); owner.closeTab(c);
  expect(owner.getSnapshot().workspace).toBeUndefined(); expect(owner.getSnapshot().mode).toBe('collapsed'); expect(live()).toHaveLength(0);
});
it('split, active-pane open, explicit move and third-pane rejection share one membership contract', () => {
  const a = owner.openPreview(artifact('a'))!, b = owner.openPreview(artifact('b'))!; owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.split(); const second = workspace().activePane;
  const c = owner.openPreview(artifact('c'))!; expect(workspace().tabs.find(tab => tab.id === c)?.pane).toBe(second);
  owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.split(); expect(workspace().panes).toHaveLength(2); expect(live()).toHaveLength(2);
  owner.move(c); expect(workspace().tabs.find(tab => tab.id === c)?.pane).not.toBe(second); expect(workspace().panes.find(pane => pane.id === second)?.selected).toBe(b);
  owner.closeTab(b); expect(workspace().panes).toHaveLength(1); expect(workspace().tabs.map(tab => tab.id)).toEqual([a,c]);
});
it('fullscreen preserves leases; narrow presents active pane and wide restores split; collapse stays closed', () => {
  owner.openPreview(artifact('a')); owner.openPreview(artifact('b')); owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.split(); const before = [...owner.getSnapshot().leases.values()]; const ids = workspace().tabs.map(tab => tab.id);
  owner.toggleFullscreen(publish => { publish(); owner.measure(width, owner.getSnapshot().geometryEpoch); }); owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.toggleFullscreen(publish => { publish(); owner.measure(width, owner.getSnapshot().geometryEpoch); }); owner.measure(1000, owner.getSnapshot().geometryEpoch); expect([...owner.getSnapshot().leases.values()]).toEqual(before);
  owner.measure(390, owner.getSnapshot().geometryEpoch); expect(live()).toHaveLength(1); expect(workspace().panes).toHaveLength(2);
  owner.measure(1000, owner.getSnapshot().geometryEpoch); expect(live()).toHaveLength(2); expect(workspace().tabs.map(tab => tab.id)).toEqual(ids);
  owner.collapse(); owner.measure(390, owner.getSnapshot().geometryEpoch); owner.measure(1200, owner.getSnapshot().geometryEpoch); expect(owner.getSnapshot().mode).toBe('collapsed'); expect(live()).toHaveLength(0);
});
it('Inspector retains logical state and reacquires only selected visible bodies', () => {
  const id = owner.openPreview(artifact('a'))!; owner.updateView(id, { bodyScrollTop: 44 }); const lease = owner.getSnapshot().leases.get(id)!;
  owner.toggleInspector(); expect(lease.signal.aborted).toBe(true); expect(workspace().tabs[0].view.bodyScrollTop).toBe(44); expect(live()).toHaveLength(0);
  owner.toggleInspector(); expect(owner.getSnapshot().mode).toBe('preview'); expect(owner.getSnapshot().leases.get(id)).not.toBe(lease);
});
it('Session A to B retains only metadata and restores exact tab state; scope replacement retires immediately', async () => {
  const a = owner.openPreview(artifact('a'))!; owner.updateView(a, { pdfPage: 7 }); const lease = owner.getSnapshot().leases.get(a)!;
  owner.selectSession('B'); expect(live()).toHaveLength(0); expect(lease.signal.aborted).toBe(true); owner.openPreview(artifact('b'));
  owner.selectSession('A'); expect(workspace().tabs[0].id).toBe(a); expect(workspace().tabs[0].view.pdfPage).toBe(7);
  const active = owner.getSnapshot().leases.get(a)!; await server.client.release('A'); await server.client.attach('A');
  expect(active.signal.aborted).toBe(true); expect(owner.getSnapshot().workspace).toBeUndefined();
});
it('four retained Sessions reject a fifth without evicting unrelated logical state', async () => {
  for (const id of ['C','D','E']) { server.snapshots.set(id, nativeSnapshot(id)); await server.client.attach(id); }
  const ids = new Map<string, number>();
  for (const session of ['A','B','C','D']) { owner.selectSession(session); ids.set(session, owner.openPreview(artifact(session))!); }
  owner.selectSession('E'); owner.openPreview(artifact('E')); expect(owner.getSnapshot().problem).toBe('sessions'); expect(owner.getSnapshot().workspace).toBeUndefined();
  owner.selectSession('A'); expect(workspace().tabs[0].id).toBe(ids.get('A')); owner.closeTab(ids.get('A')!);
  owner.selectSession('E'); expect(owner.openPreview(artifact('E'))).toBeDefined();
});
it('Download does not create, select, reveal or change pane/presentation state', async () => {
  await owner.download(artifact('download')); expect(owner.getSnapshot().workspace).toBeUndefined(); expect(owner.getSnapshot().mode).toBe('collapsed');
  owner.openPreview(artifact('a')); owner.openPreview(artifact('b')); owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.split(); owner.collapse(); const before = owner.getSnapshot();
  await owner.download(artifact('original', '原始.txt')); expect(owner.getSnapshot()).toBe(before); expect(leases.downloads).toHaveLength(2);
});
it('real tabs keyboard path is manual activation; focus returns deterministically on close', () => {
  owner.openPreview(artifact('a','A')); owner.openPreview(artifact('b','B')); const returnFocus = vi.fn(); const ui = render(<Shell returnFocus={returnFocus}/>);
  const a = ui.getByRole('tab', { name: 'A' }), b = ui.getByRole('tab', { name: 'B' }); b.focus(); fireEvent.keyDown(b, { key: 'ArrowLeft' });
  expect(document.activeElement).toBe(a); expect(b.getAttribute('aria-selected')).toBe('true'); fireEvent.click(a); expect(a.getAttribute('aria-selected')).toBe('true');
  fireEvent.click(ui.getByRole('button', { name: 'Close preview A' })); expect(document.activeElement).toBe(b);
  fireEvent.click(ui.getByRole('button', { name: 'Close preview B' })); expect(returnFocus).toHaveBeenCalledOnce(); expect(ui.queryByRole('tab')).toBeNull();
});
it('insufficient measured width explains split, and keyboard divider uses bounded ratio', () => {
  owner.openPreview(artifact('a')); owner.openPreview(artifact('b')); width = 500; const ui = render(<Shell/>);
  const split = ui.getByRole('button', { name: 'Split preview' }); expect(split.getAttribute('aria-disabled')).toBe('true'); expect(split.getAttribute('aria-describedby')).toBe('preview-split-reason');
  fireEvent.click(split); expect(workspace().panes).toHaveLength(1); width = 1000; act(() => resize()); fireEvent.click(split);
  const divider = ui.getByRole('separator', { name: 'Resize preview panes' }); fireEvent.keyDown(divider, { key: 'ArrowRight' }); expect(workspace().ratio).toBe(.55);
  fireEvent.keyDown(divider, { key: 'Home' }); expect(workspace().ratio).toBeCloseTo(300/992); fireEvent.keyDown(divider, { key: 'End' }); expect(workspace().ratio).toBeCloseTo(1-300/992);
  width = 620; act(() => resize());
  for (const key of ['Home', 'End']) {
    fireEvent.keyDown(divider, { key });
    const current = Number(divider.getAttribute('aria-valuenow'));
    expect(current).toBeGreaterThanOrEqual(Number(divider.getAttribute('aria-valuemin')));
    expect(current).toBeLessThanOrEqual(Number(divider.getAttribute('aria-valuemax')));
    expect(Math.min(workspace().ratio, 1 - workspace().ratio) * (width - 8)).toBeGreaterThanOrEqual(300);
  }
});
it('keyboard focus reveals a clipped tab through only its bounded strip without selecting it', () => {
  owner.openPreview(artifact('a','A')); owner.openPreview(artifact('b','B')); const ui = render(<Shell/>);
  const strip = ui.getByRole('tablist'), a = ui.getByRole('tab', { name: 'A' }), b = ui.getByRole('tab', { name: 'B' });
  let viewportWidth = 300, scroll = 0;
  Object.defineProperties(strip, { scrollWidth: { value: 600 }, clientWidth: { value: 300 }, scrollLeft: { get: () => scroll, set: value => { scroll = Math.max(0, Math.min(600 - viewportWidth, value)); } } });
  Object.defineProperty(strip, 'getBoundingClientRect', { value: () => ({ left: 0, right: viewportWidth }) });
  Object.defineProperty(a.parentElement!, 'getBoundingClientRect', { value: () => ({ left: 500 - strip.scrollLeft, right: 600 - strip.scrollLeft }) });
  Object.defineProperty(b.parentElement!, 'getBoundingClientRect', { value: () => ({ left: -strip.scrollLeft, right: 100 - strip.scrollLeft }) });
  b.focus(); fireEvent.keyDown(b, { key: 'ArrowLeft' });
  expect(document.activeElement).toBe(a); expect(strip.scrollLeft).toBe(300); expect(b.getAttribute('aria-selected')).toBe('true');
  fireEvent.keyDown(a, { key: 'ArrowRight' });
  expect(document.activeElement).toBe(b); expect(strip.scrollLeft).toBe(0); expect(b.getAttribute('aria-selected')).toBe('true');
  viewportWidth = 299.5; fireEvent.keyDown(b, { key: 'ArrowLeft' });
  expect(strip.scrollLeft).toBe(300.5); expect(a.parentElement!.getBoundingClientRect().right).toBeLessThanOrEqual(viewportWidth);
});
it('pointer divider captures, coalesces visual frames, commits once and cancellation restores', () => {
  owner.openPreview(artifact('a')); owner.openPreview(artifact('b')); owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.split(); const ui = render(<Shell/>), divider = ui.getByRole('separator');
  let frame: FrameRequestCallback | undefined; const capture = vi.fn(), release = vi.fn();
  divider.setPointerCapture = capture; divider.hasPointerCapture = () => true; divider.releasePointerCapture = release;
  vi.stubGlobal('PointerEvent', MouseEvent); vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { frame = cb; return 1; }); vi.stubGlobal('cancelAnimationFrame', () => { frame = undefined; });
  fireEvent.pointerDown(divider, { button: 0, clientX: 500 }); fireEvent.pointerMove(divider, { clientX: 550 }); fireEvent.pointerMove(divider, { clientX: 600 });
  expect(capture).toHaveBeenCalledOnce(); expect(workspace().ratio).toBe(.5); act(() => frame?.(0)); expect(Number(divider.getAttribute('aria-valuenow'))).toBe(60);
  fireEvent.pointerCancel(divider); expect(workspace().ratio).toBe(.5); expect(divider.getAttribute('aria-valuenow')).toBe('50');
  fireEvent.pointerDown(divider, { button: 0, clientX: 500 }); fireEvent.pointerUp(divider, { clientX: 900 }); expect(workspace().ratio).toBeCloseTo(1-300/992); expect(release).toHaveBeenCalledTimes(2);
});
it('Escape respects composition, repeat and modifiers; fullscreen restores before collapse', () => {
  owner.openPreview(artifact('a')); owner.toggleFullscreen(publish => { publish(); owner.measure(width, owner.getSnapshot().geometryEpoch); }); const returnFocus = vi.fn(), ui = render(<Shell returnFocus={returnFocus}/>); const tab = ui.getByRole('tab');
  fireEvent.keyDown(tab, { key: 'Escape', isComposing: true }); fireEvent.keyDown(tab, { key: 'Escape', repeat: true }); fireEvent.keyDown(tab, { key: 'Escape', ctrlKey: true }); expect(workspace().fullscreen).toBe(true);
  fireEvent.keyDown(tab, { key: 'Escape' }); expect(workspace().fullscreen).toBe(false); expect(owner.getSnapshot().mode).toBe('preview');
  fireEvent.keyDown(tab, { key: 'Escape' }); expect(owner.getSnapshot().mode).toBe('collapsed'); expect(returnFocus).toHaveBeenCalledOnce();
  ui.unmount(); expect(disconnect).toHaveBeenCalledTimes(2);
});

it('divider geometry replacement retires pointer capture and pending paint; keyboard cannot alter an active gesture', () => {
  owner.openPreview(artifact('a')); owner.openPreview(artifact('b')); owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.split(); const ui = render(<Shell/>);
  let frame: FrameRequestCallback | undefined; const capture = vi.fn(), release = vi.fn();
  const divider = ui.getByRole('separator'); divider.setPointerCapture = capture; divider.hasPointerCapture = () => true; divider.releasePointerCapture = release;
  vi.stubGlobal('PointerEvent', MouseEvent); vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { frame = cb; return 1; });
  const cancel = vi.fn(() => { frame = undefined; }); vi.stubGlobal('cancelAnimationFrame', cancel);
  fireEvent.pointerDown(divider, { button: 0, clientX: 500 }); fireEvent.pointerMove(divider, { clientX: 600 });
  fireEvent.keyDown(divider, { key: 'ArrowRight' }); expect(workspace().ratio).toBe(.5);
  width = 800; act(() => resize()); expect(release).toHaveBeenCalledOnce(); expect(cancel).toHaveBeenCalledOnce(); expect(frame).toBeUndefined();
  expect(workspace().ratio).toBe(.5); expect(divider.getAttribute('aria-valuenow')).toBe('50');
  fireEvent.pointerUp(divider, { clientX: 700 }); expect(workspace().ratio).toBe(.5);
  fireEvent.pointerDown(divider, { button: 0, clientX: 400 }); fireEvent.pointerMove(divider, { clientX: 450 }); act(() => frame?.(0));
  width = 390; act(() => resize()); expect(release).toHaveBeenCalledTimes(2); expect(ui.queryByRole('separator')).toBeNull(); expect(workspace().ratio).toBe(.5);
  width = 1000; act(() => resize()); expect(ui.getByRole('separator').getAttribute('aria-valuenow')).toBe('50');
});

it('tab and divider navigation leave IME and modified keys alone, and held Delete closes no extra occurrences', () => {
  owner.openPreview(artifact('a','A')); owner.openPreview(artifact('b','B')); const ui = render(<Shell/>);
  const a = ui.getByRole('tab', { name: 'A' }), b = ui.getByRole('tab', { name: 'B' }); b.focus();
  fireEvent.keyDown(b, { key: 'ArrowLeft', isComposing: true }); fireEvent.keyDown(b, { key: 'ArrowLeft', ctrlKey: true }); expect(document.activeElement).toBe(b);
  fireEvent.keyDown(b, { key: 'Delete', isComposing: true }); fireEvent.keyDown(b, { key: 'Delete', repeat: true }); expect(workspace().tabs).toHaveLength(2);
  fireEvent.click(ui.getByRole('button', { name: 'Split preview' })); const divider = ui.getByRole('separator');
  fireEvent.keyDown(divider, { key: 'ArrowRight', isComposing: true }); fireEvent.keyDown(divider, { key: 'ArrowRight', altKey: true }); expect(workspace().ratio).toBe(.5);
  const moved = ui.getByRole('tab', { name: 'B' }); moved.focus(); fireEvent.keyDown(moved, { key: 'Delete' }); expect(workspace().tabs).toHaveLength(1); expect(document.activeElement).toBe(a);
  fireEvent.keyDown(a, { key: 'Delete', repeat: true }); expect(workspace().tabs).toHaveLength(1);
});

it.each(['Inspector', 'collapse'] as const)('hiding a captured divider via %s retires its frame and capture before hidden input can commit', mode => {
  owner.openPreview(artifact('a')); owner.openPreview(artifact('b')); owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.split(); const ui = render(<Shell/>);
  const divider = ui.getByRole('separator'); let frame: FrameRequestCallback | undefined; const release = vi.fn();
  divider.setPointerCapture = vi.fn(); divider.hasPointerCapture = () => true; divider.releasePointerCapture = release;
  vi.stubGlobal('PointerEvent', MouseEvent); vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { frame = cb; return 1; });
  vi.stubGlobal('cancelAnimationFrame', () => { frame = undefined; });
  fireEvent.pointerDown(divider, { button: 0, clientX: 500 }); fireEvent.pointerMove(divider, { clientX: 600 });
  const oldFrame = frame; act(() => mode === 'Inspector' ? owner.toggleInspector() : owner.collapse());
  expect(release).toHaveBeenCalledOnce(); expect(frame).toBeUndefined(); expect(ui.queryByRole('separator')).toBeNull();
  act(() => oldFrame?.(0)); fireEvent.pointerUp(divider, { clientX: 700 }); expect(workspace().ratio).toBe(.5);
});
it('restoring fullscreen into one visible pane transfers divider focus to the active occurrence', () => {
  owner.openPreview(artifact('a','A')); owner.openPreview(artifact('b','B')); owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.split(); owner.toggleFullscreen(publish => { publish(); owner.measure(width, owner.getSnapshot().geometryEpoch); }); const ui = render(<Shell/>);
  const divider = ui.getByRole('separator'); divider.focus();
  fireEvent.keyDown(divider, { key: 'Escape', keyCode: 229 }); expect(workspace().fullscreen).toBe(true);
  fireEvent.keyDown(divider, { key: 'Escape' }); expect(workspace().fullscreen).toBe(false);
  width = 500; act(() => resize()); expect(ui.queryByRole('separator')).toBeNull(); expect(document.activeElement).toBe(ui.getByRole('tab', { name: 'B' }));
});

it('Download inside an inactive pane leaves pane activation and tab selection unchanged for pointer and keyboard focus', async () => {
  owner.openPreview(artifact('a','A')); owner.openPreview(artifact('b','B')); owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.split(); const ui = render(<Shell/>);
  const before = workspace(), control = ui.getByRole('button', { name: 'Download A' });
  fireEvent.pointerDown(control, { button: 0 }); act(() => control.focus()); fireEvent.click(control);
  expect(workspace()).toBe(before); expect(document.activeElement).toBe(control); expect(leases.downloads).toHaveLength(1);
  fireEvent.pointerDown(control.closest('[data-preview-pane]')!);
  expect(workspace().activePane).not.toBe(before.activePane);
});

it.each(['collapse', 'Inspector'] as const)('%s invalidates hidden wide geometry before reveal can admit the second pane', mode => {
  owner.openPreview(artifact('a')); owner.openPreview(artifact('b'));
  const ui = render(<Shell/>);
  act(() => owner.split()); expect(live()).toHaveLength(2);
  const retained = workspace(), oldDelivery = resize;
  act(() => mode === 'collapse' ? owner.collapse() : owner.toggleInspector());
  expect(live()).toHaveLength(0);
  width = 390; // external layout changes while hidden; no positive hidden measurement
  const createdBeforeReveal = leases.created.length;
  act(() => {
    mode === 'collapse' ? owner.reveal() : owner.toggleInspector();
    expect(live().map(lease => lease.id)).toEqual([retained.panes.find(p => p.id === retained.activePane)!.selected]);
    expect(leases.created.slice(createdBeforeReveal)).toHaveLength(1);
    // Even a queued callback from the old visible presentation cannot authorize it.
    width = 1000; oldDelivery(); width = 390;
    expect(live()).toHaveLength(1);
    expect(leases.created.slice(createdBeforeReveal)).toHaveLength(1);
  });
  act(() => resize()); expect(live()).toHaveLength(1);
  expect(workspace().panes).toEqual(retained.panes); expect(workspace().ratio).toBe(retained.ratio);
  width = 1000; act(() => resize()); expect(live()).toHaveLength(2);
  expect(ui.container.querySelectorAll('[data-live-document]')).toHaveLength(2);
});

it('explicit Inspector Close persists closed intent across refresh, Session round-trip and widening', async () => {
  const id = owner.openPreview(artifact('a'))!; owner.updateView(id, { bodyScrollTop: 83, wrap: false });
  expect(workspace().expanded).toBe(true);
  owner.toggleInspector(); owner.collapse();
  expect(owner.getSnapshot().mode).toBe('collapsed'); expect(workspace().expanded).toBe(false);
  await server.client.refresh('A');
  owner.selectSession('B'); owner.selectSession('A');
  owner.measure(1200, owner.getSnapshot().geometryEpoch);
  expect(owner.getSnapshot().mode).toBe('collapsed'); expect(live()).toHaveLength(0);
  owner.toggleInspector(); owner.toggleInspector(); expect(owner.getSnapshot().mode).toBe('collapsed');
  owner.reveal(); expect(owner.getSnapshot().mode).toBe('preview'); expect(workspace().expanded).toBe(true);
  expect(workspace().tabs[0].view).toEqual({ bodyScrollTop: 83, wrap: false });
  owner.toggleInspector(); owner.toggleInspector(); expect(owner.getSnapshot().mode).toBe('preview');
});

it('Session epochs reject old measurements and fullscreen without a layout witness cannot retain a second lease', () => {
  owner.openPreview(artifact('a')); owner.openPreview(artifact('b'));
  owner.measure(1000, owner.getSnapshot().geometryEpoch); owner.split();
  const oldEpoch = owner.getSnapshot().geometryEpoch;
  owner.selectSession('B'); owner.selectSession('A');
  expect(live()).toHaveLength(1);
  owner.measure(1000, oldEpoch); expect(live()).toHaveLength(1);
  owner.measure(1000, owner.getSnapshot().geometryEpoch); expect(live()).toHaveLength(2);
  owner.toggleFullscreen(publish => publish());
  expect(live()).toHaveLength(1);
  owner.measure(1000, owner.getSnapshot().geometryEpoch); expect(live()).toHaveLength(2);
  owner.measure(0, owner.getSnapshot().geometryEpoch); expect(live()).toHaveLength(1);
});

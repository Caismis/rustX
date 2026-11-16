import { createRef, useLayoutEffect, useRef, useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Workbench, type WorkbenchHandle } from '../src/app/components/Workbench';
import { PreviewWorkspace } from '../src/app/components/PreviewWorkspace';
import { PreviewWorkspaceOwner } from '../src/app/preview-workspace';
import { WorkbenchPreviewRetention } from '../src/app/workbench-previews';
import { PreviewContentCache, PreviewContentOwner } from '../src/client/preview-content';
import { DockController } from '../src/presentation/dockkit/engine/controller';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import type { GeometryDocumentProps } from '../src/app/components/documents/GeometryDocument';
import type { PreviewSource } from '../src/client/session-files';
import { Server } from './fixture';
import { App } from '../src/app/App';

const models = vi.hoisted(() => ({ imports: 0, live: 0, peak: 0, bytes: 4, copies: [] as { name: string; model: object; props: GeometryDocumentProps; disposed: boolean }[], reads: vi.fn() }));
vi.mock('../src/app/components/documents/GeometryDocument', () => ({ preloadGeometry: vi.fn(), GeometryDocument: (props: GeometryDocumentProps) => {
  const model = useRef({ camera: 'kept', selection: 'part', outlines: true });
  useLayoutEffect(() => {
    if (!props.bytes) return;
    const record = { name: props.name, model: model.current, props, disposed: false }; models.copies.push(record);
    models.imports++; models.live++; models.peak = Math.max(models.peak, models.live);
    props.takeSource?.(); props.releaseSource?.();
    const dispose = () => { if (!record.disposed) { record.disposed = true; models.live--; } };
    props.signal.addEventListener('abort', dispose, { once: true });
    return () => { props.signal.removeEventListener('abort', dispose); dispose(); };
  }, [props.bytes, props.copyId, props.signal]);
  return props.bytes ? <canvas data-model={props.name} data-shown={props.visible !== false}/> : null;
} }));
// Range transport is covered by geometry-source tests. Here the complete copy
// exercises the real content owner's transfer/deactivation behavior.
vi.mock('../src/client/geometry-source', () => ({ readGeometrySource: async (read: (options: unknown, signal: AbortSignal) => Promise<{ data: Uint8Array<ArrayBuffer>; version: string }>, signal: AbortSignal) => read({ kind: 'bytes' }, signal) }));
vi.mock('../src/client/session-files', async original => {
  const actual = await original<typeof import('../src/client/session-files')>();
  return { ...actual, FilePreviewCoordinator: class {
    disposed = false;
    owners = new Map<number, PreviewContentOwner>();
    views = new Set<{ dispose(): void }>();
    current = () => !this.disposed;
    sourceCurrent = () => !this.disposed;
    retainedCurrent = () => !this.disposed;
    acquire(id: number, source: PreviewSource) {
      expect(this.views.size).toBeLessThan(2);
      let content = this.owners.get(id);
      if (!content) {
        content = new PreviewContentOwner({ current: this.current, immutable: true, bytes: async () => { models.reads(id); return { data: new Uint8Array(models.bytes), version: 'one' }; },
          page: async () => ({ text: 'plain text', version: 'one', offset: 1, lines: 1, eof: true }), document: async () => { throw Error('unexpected document'); }, probe: async () => 'one' });
        this.owners.set(id, content);
      }
      content.activate();
      const controller = new AbortController(), owner = content;
      const lease = { content, source, signal: controller.signal, current: () => !controller.signal.aborted && !this.disposed, sourceCurrent: this.sourceCurrent,
        dispose: () => { if (controller.signal.aborted) return; controller.abort(); owner.deactivate(); this.views.delete(lease); } };
      this.views.add(lease); return lease;
    }
    forget(id: number) { this.owners.get(id)?.dispose(); this.owners.delete(id); }
    dispose() { this.disposed = true; for (const view of this.views) view.dispose(); for (const owner of this.owners.values()) owner.dispose(); }
  } };
});
let server: Server, authority: WorkspaceAuthority, stops: (() => void)[];
beforeEach(async () => {
  models.imports = models.live = models.peak = 0; models.bytes = 4; models.copies = []; models.reads.mockReset();
  server = new Server(); await server.attached('A', 'B'); authority = new WorkspaceAuthority(server.workspaceHost); await authority.observe(); stops = [];
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({ width: 1000, height: 500, x: 0, y: 0, top: 0, left: 0, bottom: 500, right: 1000, toJSON() {} });
});
afterEach(() => { cleanup(); stops.forEach(stop => stop()); server.client.disconnect(); localStorage.clear(); });
const artifact = (id: string, name = `${id}.stl`) => ({ name, image: false, source: { kind: 'artifact' as const, id } });
function preview() {
  const owner = new PreviewWorkspaceOwner(server.client, server.workspaceHost, authority); stops.push(owner.start()); owner.selectSession('A');
  function Shell() { const snapshot = useSyncExternalStore(owner.subscribe, owner.getSnapshot); return <PreviewWorkspace owner={owner} snapshot={snapshot} focusRequest={0} returnFocus={() => {}}/>; }
  return { owner, ui: render(<Shell/>) };
}
it.each([4, 64 * 1024 * 1024 + 1])('artifact geometry keeps the same canvas, scene and copy across A/B/A (%i source bytes)', async size => {
  models.bytes = size; const { owner, ui } = preview(); let a!: number, b!: number;
  await act(async () => { a = owner.openPreview(artifact('a'))!; });
  await waitFor(() => expect(models.imports).toBe(1));
  const canvas = ui.container.querySelector('[data-model="a.stl"]'), first = models.copies[0], lease = owner.getSnapshot().leases.get(a)!;
  if (size > 64 * 1024 * 1024) expect(first.props.bytes!.byteLength).toBe(0);
  await act(async () => { b = owner.openPreview(artifact('b'))!; });
  await waitFor(() => expect(models.imports).toBe(2));
  expect(canvas?.closest('[role="tabpanel"]')?.hasAttribute('hidden')).toBe(true);
  expect(canvas?.closest('[role="tabpanel"]')?.hasAttribute('inert')).toBe(true);
  expect(ui.getAllByRole('tabpanel')).toHaveLength(1);
  await act(async () => owner.selectTab(a));
  expect(ui.container.querySelector('[data-model="a.stl"]')).toBe(canvas); expect(owner.getSnapshot().leases.get(a)).toBe(lease);
  expect(models.copies[0].model).toBe(first.model); expect(models.imports).toBe(2); expect(models.reads).toHaveBeenCalledTimes(2);
  expect(lease.signal.aborted).toBe(false); expect(owner.getSnapshot().leases.has(b)).toBe(true);
  await act(async () => {
    for (let index = 0; index < 5; index++) { owner.measure(900 + index, owner.getSnapshot().geometryEpoch); owner.updateView(a, { bodyScrollTop: index }); }
    lease.content!.invalidate();
  });
  expect(models.imports).toBe(2); expect(models.reads).toHaveBeenCalledTimes(2);
  expect(ui.container.querySelector('[data-model="a.stl"]')).toBe(canvas);
  await act(async () => owner.collapse()); expect(models.live).toBe(0); expect(lease.signal.aborted).toBe(true);
});
it('artifact admission evicts LRU before replacement, gives both visible split files priority, and releases on scope retirement', async () => {
  const { owner } = preview(); const ids: number[] = [];
  for (const id of ['a', 'b']) { await act(async () => { ids.push(owner.openPreview(artifact(id))!); }); }
  await act(async () => owner.selectTab(ids[0]));
  const b = owner.getSnapshot().leases.get(ids[1])!;
  await act(async () => { ids.push(owner.openPreview(artifact('c'))!); });
  expect(b.signal.aborted).toBe(true); expect([...owner.getSnapshot().leases.keys()].sort()).toEqual([ids[0], ids[2]]);
  await act(async () => owner.split()); expect(models.peak).toBe(2);
  await act(async () => { owner.openPreview(artifact('text', 'plain.txt')); });
  expect(owner.getSnapshot().leases.size).toBe(2); expect(models.peak).toBe(2);
  await act(async () => owner.selectSession('B')); expect(models.live).toBe(0); expect(owner.getSnapshot().leases.size).toBe(0);
});
it('artifact refresh replaces only the selected copy and close/mode changes fully retire geometry', async () => {
  const { owner, ui } = preview(); let a!: number;
  await act(async () => { a = owner.openPreview(artifact('a'))!; });
  const first = models.copies[0], content = owner.getSnapshot().leases.get(a)!.content!;
  await act(async () => content.refresh());
  expect(first.disposed).toBe(true); expect(models.imports).toBe(2); expect(models.live).toBe(1);
  await act(async () => owner.closeTab(a)); expect(models.live).toBe(0);
  await act(async () => { owner.openPreview(artifact('b')); });
  await act(async () => owner.showWorkbench()); expect(models.live).toBe(0); expect(ui.queryAllByRole('tabpanel')).toHaveLength(0);
});

it.each(['Settings', 'Workflows', 'Scheduled Tasks'])('parks the delivered geometry across %s and Home without replacing its canvas or copy', async destination => {
  server.handlers.set('workflow/list', () => ({ type: 'workflow_library', library: { target: { kind: 'user' }, directory: '/workflows', absent_revision: 'missing', diagnostic: null, files: [] } }));
  let owner!: PreviewWorkspaceOwner;
  const start = PreviewWorkspaceOwner.prototype.start;
  vi.spyOn(PreviewWorkspaceOwner.prototype, 'start').mockImplementation(function (this: PreviewWorkspaceOwner) {
    owner = this; return start.call(this);
  });
  localStorage.setItem('rustx-console-view-v3', JSON.stringify({ endpoint: server.client.getSnapshot().endpoint, openViews: ['A', 'B'], route: { kind: 'session', sessionId: 'A' } }));
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost}/>); });
  await waitFor(() => expect(screen.getByLabelText('Session title').textContent).toBe('Session A'));
  await act(async () => { owner.openPreview(artifact('delivered')); });
  await waitFor(() => expect(models.imports).toBe(1));
  const canvas = document.querySelector('[data-model="delivered.stl"]')!, copy = models.copies[0];
  const tab = owner.getSnapshot().workspace!.tabs[0].id, lease = owner.getSnapshot().leases.get(tab)!;
  const navigation = within(screen.getByRole('navigation', { name: 'Primary navigation' }));
  expect(canvas.getAttribute('data-shown')).toBe('true');

  await act(async () => fireEvent.click(navigation.getByRole('button', { name: destination })));
  await waitFor(() => expect(navigation.getByRole('button', { name: destination }).getAttribute('aria-current')).toBe('page'));
  expect(canvas.getAttribute('data-shown')).toBe('false');
  expect(canvas.closest('[hidden]')).not.toBeNull(); expect(canvas.closest('[inert]')).not.toBeNull();
  expect(copy.disposed).toBe(false); expect(lease.signal.aborted).toBe(false);

  await act(async () => fireEvent.click(navigation.getByRole('button', { name: 'Home' })));
  await waitFor(() => expect(canvas.getAttribute('data-shown')).toBe('true'));
  expect(document.querySelector('[data-model="delivered.stl"]')).toBe(canvas);
  expect(owner.getSnapshot().leases.get(tab)).toBe(lease);
  expect(models.copies[0].model).toBe(copy.model); expect(models.imports).toBe(1); expect(models.reads).toHaveBeenCalledTimes(1);
});
it.each([4, 64 * 1024 * 1024 + 1])('Workbench keeps its mounted geometry through file tabs with bounded admission (%i bytes)', async size => {
  models.bytes = size;
  const cache = new PreviewContentCache(), previews = new WorkbenchPreviewRetention(server.client, authority, cache); stops.push(previews.start());
  const reads = vi.fn(), host = { ...server.workspaceHost, workbench: vi.fn(async (_scope, { request }) => {
    if (request.kind === 'resolve') return { path: request.path };
    if (request.kind === 'preview' && request.path.endsWith('.html')) { const data = new TextEncoder().encode('<html><body>retained frame</body></html>'); return { preview: { data, version: 'one', offset: 0, bytes: data.byteLength, eof: true } }; }
    if (request.kind === 'preview' && request.read?.kind === 'text') return { preview: { data: new TextEncoder().encode('plain text'), version: 'one', offset: request.read.offset, lines: 1, bytes: 10, eof: true } };
    if (request.kind === 'preview') { reads(request.path); return { preview: { data: new Uint8Array(size), version: 'one', offset: 0, bytes: size, eof: true } }; }
    return {};
  }) } satisfies typeof server.workspaceHost;
  const ref = createRef<WorkbenchHandle>(), props = { fileRef: ref, host, previews, scope: authority.getCatalog()!, target: { session_id: 'A', active_node: 'node-A' }, visible: true, floatingVisible: false, fullscreen: false, toggleFullscreen() {}, closePanel() {} };
  let dock!: DockController; const open = DockController.prototype.openContent;
  vi.spyOn(DockController.prototype, 'openContent').mockImplementation(function (this: DockController, input) { dock = this; return open.call(this, input); });
  const ui = render(<Workbench {...props}/>);
  await act(async () => { await ref.current!.openFile('a.stl'); });
  await waitFor(() => expect(models.imports).toBe(1));
  const canvas = ui.container.querySelector('[data-model="a.stl"]'), first = models.copies[0];
  await act(async () => { await ref.current!.openFile('b.stl'); });
  await waitFor(() => expect(models.imports).toBe(2));
  await act(async () => { await ref.current!.openFile('a.stl'); });
  expect(ui.container.querySelector('[data-model="a.stl"]')).toBe(canvas); expect(first.disposed).toBe(false); expect(reads).toHaveBeenCalledTimes(2);
  const b = models.copies.find(copy => copy.name === 'b.stl')!;
  await act(async () => { await ref.current!.openFile('c.stl'); });
  await waitFor(() => expect(models.imports).toBe(3)); expect(b.disposed).toBe(true); expect(first.disposed).toBe(false); expect(models.peak).toBe(2);
  const c = Object.values(dock.getSnapshot().state.tabs).find(tab => tab.title === 'c.stl')!;
  act(() => dock.closeTab(c.id)); expect(models.live).toBeLessThanOrEqual(2);
  await act(async () => { await ref.current!.openFile('note.txt'); });
  const plain = ui.container.querySelector('[data-workbench-file="note.txt"]'); expect(plain).not.toBeNull();
  await act(async () => { await ref.current!.openFile('a.stl'); });
  expect(plain!.isConnected).toBe(false);
  ui.rerender(<Workbench {...props} visible={false} floatingVisible/>); expect(models.live).toBe(0);
  ui.rerender(<Workbench {...props}/>); await waitFor(() => expect(models.live).toBe(1));
  await act(async () => { await ref.current!.openFile('review.html'); });
  const frame = await waitFor(() => { const frame = ui.container.querySelector('iframe'); expect(frame).not.toBeNull(); return frame!; });
  frame.dataset.retainedState = 'kept';
  ui.rerender(<Workbench {...props} visible={false} floatingVisible/>);
  expect(ui.container.querySelector('iframe')).toBe(frame); expect(frame.isConnected).toBe(true); expect(models.live).toBe(0);
  ui.rerender(<Workbench {...props}/>);
  expect(ui.container.querySelector('iframe')).toBe(frame); expect(frame.dataset.retainedState).toBe('kept');
  await act(async () => { await ref.current!.openFile('a.stl'); });
  expect(frame.isConnected).toBe(false);
  const a = Object.values(dock.getSnapshot().state.tabs).find(tab => tab.title === 'a.stl')!;
  const floatingCanvas = ui.container.querySelector('[data-model="a.stl"]'), beforeFloat = models.copies.filter(copy => copy.name === 'a.stl').length;
  act(() => dock.floatTab(a.id));
  ui.rerender(<Workbench {...props} visible={false} floatingVisible/>);
  expect(ui.container.querySelector('[data-model="a.stl"]')).toBe(floatingCanvas);
  expect(floatingCanvas!.getAttribute('data-shown')).toBe('true'); expect(models.copies.filter(copy => copy.name === 'a.stl')).toHaveLength(beforeFloat);
  expect(models.live).toBeGreaterThan(0); expect(models.live).toBeLessThanOrEqual(2); expect(models.peak).toBe(2);
  act(() => dock.closeTab(a.id)); expect(models.live).toBe(0);
  ui.rerender(<Workbench {...props}/>);
  await act(async () => { await ref.current!.openFile('a.stl'); });
  await waitFor(() => expect(models.live).toBeGreaterThan(0));
  await act(async () => server.client.release('A')); expect(models.live).toBe(0);
});

it('Workbench defers inactive geometry admission and parks the same Navigator viewport across Home', async () => {
  const previews = new WorkbenchPreviewRetention(server.client, authority, new PreviewContentCache()); stops.push(previews.start());
  const reads = vi.fn(), host = { ...server.workspaceHost, workbench: vi.fn(async (_scope, { request }) => {
    if (request.kind === 'resolve') return { path: request.path };
    if (request.kind === 'preview') { reads(request.path); return { preview: { data: new Uint8Array(4), version: 'one', offset: 0, bytes: 4, eof: true } }; }
    return {};
  }) } satisfies typeof server.workspaceHost;
  const ref = createRef<WorkbenchHandle>(), props = { fileRef: ref, host, previews, scope: authority.getCatalog()!, target: { session_id: 'A', active_node: 'node-A' }, visible: true, floatingVisible: false, fullscreen: false, toggleFullscreen() {}, closePanel() {} };
  const ui = render(<Workbench {...props} active={false}/>);
  await act(async () => { await ref.current!.openFile('parked.stl'); });
  expect(reads).not.toHaveBeenCalled(); expect(models.imports).toBe(0);

  ui.rerender(<Workbench {...props} active/>);
  await waitFor(() => expect(models.imports).toBe(1));
  const canvas = ui.container.querySelector('[data-model="parked.stl"]')!, copy = models.copies[0];
  // Exercise the main file navigator surrounding the geometry body.
  fireEvent.click(ui.getByRole('button', { name: 'Show workspace files' }));
  expect(ui.container.querySelector('[data-files-navigator]')).not.toBeNull();
  expect(canvas.getAttribute('data-shown')).toBe('true');

  ui.rerender(<Workbench {...props} active={false}/>);
  expect(ui.container.querySelector('[data-model="parked.stl"]')).toBe(canvas);
  expect(canvas.getAttribute('data-shown')).toBe('false');
  expect(canvas.closest('[hidden]')).not.toBeNull(); expect(canvas.closest('[inert]')).not.toBeNull();
  expect(copy.disposed).toBe(false); expect(models.live).toBe(1);

  ui.rerender(<Workbench {...props} active/>);
  expect(ui.container.querySelector('[data-model="parked.stl"]')).toBe(canvas);
  expect(canvas.getAttribute('data-shown')).toBe('true');
  expect(ui.getByRole('button', { name: 'Hide workspace files' }).getAttribute('aria-expanded')).toBe('true');
  expect(models.copies[0].model).toBe(copy.model); expect(models.imports).toBe(1); expect(reads).toHaveBeenCalledTimes(1);
});

it.each([{ mimeType: 'image/png', mode: 'bytes' }, { mimeType: 'text/markdown; charset=UTF-8', mode: 'pages' }])('does not park a managed $mimeType artifact with a misleading geometry suffix', async ({ mimeType, mode }) => {
  const { owner } = preview(); let id!: number;
  await act(async () => { id = owner.openPreview({ ...artifact('typed', 'misleading.step'), mimeType })!; });
  const lease = owner.getSnapshot().leases.get(id)!;
  expect(lease.content!.getSnapshot().mode).toBe(mode); expect(models.imports).toBe(0);
  // An obsolete preference cannot override the viewer's supported choices.
  await act(async () => owner.updateView(id, { viewer: 'geometry' }));
  expect(lease.content!.getSnapshot().mode).toBe(mode);
  await act(async () => { owner.openPreview(artifact('real-geometry')); });
  expect(lease.signal.aborted).toBe(true); expect(owner.getSnapshot().leases.size).toBe(1); expect(models.imports).toBe(1);
});

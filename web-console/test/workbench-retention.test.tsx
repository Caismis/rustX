import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { WorkbenchPreviewRetention } from '../src/app/workbench-previews';
import { WorkspaceFile, fileTabView, type WorkbenchCall } from '../src/app/components/WorkbenchFiles';
import { PreviewContentCache } from '../src/client/preview-content';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { Server } from './fixture';
import type { TabId } from '../src/presentation/dockkit/contract/types';

let server: Server | undefined, stop: (() => void) | undefined;
afterEach(() => { cleanup(); stop?.(); server?.client.disconnect(); });
function gate<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
it('Workbench Session return retains content and scroll, reauthorizes before exposure, and drops it on authority retirement', async () => {
  server = new Server(); await server.attached('A', 'B');
  const authority = new WorkspaceAuthority(server.workspaceHost); await authority.observe();
  const retained = new WorkbenchPreviewRetention(server.client, authority, new PreviewContentCache()); stop = retained.start();
  const scope = authority.getCatalog()!, target = { session_id: 'A', active_node: 'node-A' };
  const a = retained.acquire(scope, target), view = fileTabView(retained.cache, a.current); a.views.set('t1' as TabId, view);
  const physical = vi.fn(async () => ({ preview: { data: new TextEncoder().encode('retained text'), version: 'v1', offset: 1, lines: 1, eof: true } }));
  const call: WorkbenchCall = request => request.kind === 'preview' ? physical() : Promise.resolve({});
  const done = gate<void>();
  call.observe = async () => ({ version: 'v1', done: done.promise, close: async () => { done.resolve(); } });
  let ui: ReturnType<typeof render>;
  await act(async () => { ui = render(<WorkspaceFile path="a.txt" call={call} view={view} onOpen={() => {}}/>); });
  expect(ui!.getByText('retained text')).toBeTruthy(); expect(physical).toHaveBeenCalledOnce();
  view.top = 317; view.left = 23;
  ui!.unmount();
  retained.acquire(scope, { session_id: 'B', active_node: 'node-B' });
  const reordered = { ...scope, workspaces: [...scope.workspaces].reverse() };
  expect(retained.acquire(reordered, target)).toBe(a);
  const validate = gate<Awaited<ReturnType<WorkbenchCall>>>();
  const recheck = vi.fn(() => validate.promise);
  const rebind: WorkbenchCall = request => request.kind === 'preview' ? recheck() : Promise.resolve({}); rebind.observe = call.observe;
  await act(async () => { ui = render(<WorkspaceFile path="a.txt" call={rebind} view={view} onOpen={() => {}}/>); });
  expect(ui!.queryByText('retained text')).toBeNull(); expect(recheck).toHaveBeenCalledOnce();
  await act(async () => validate.resolve({ preview: { data: new Uint8Array([114]), version: 'v1', bytes: 1, offset: 0, eof: false } }));
  expect(ui!.getByText('retained text')).toBeTruthy(); expect(physical).toHaveBeenCalledOnce();
  expect(view.top).toBe(317); expect(view.left).toBe(23);
  await act(async () => { await server!.client.release('A'); });
  expect(a.current()).toBe(false); expect(a.views.size).toBe(0);
  expect(view.preview!.owner.getSnapshot().text).toBeUndefined();
  expect(retained.acquire(scope, target)).not.toBe(a);
  await act(async () => { await server!.client.attach('A'); });
  const successor = retained.acquire(scope, target);
  expect(successor).not.toBe(a); expect(successor.current()).toBe(true); expect(successor.views.size).toBe(0);
});
it.each(['cached navigation', 'evicted restoration'] as const)('Workbench %s requests the missing non-EOF pages after authority validation', async action => {
  const view = fileTabView(); view.mode = 'code';
  const calls: { offset: number; result: ReturnType<typeof gate<Awaited<ReturnType<WorkbenchCall>>>> }[] = [];
  const validation = gate<Awaited<ReturnType<WorkbenchCall>>>(), watched: ReturnType<typeof gate<void>>[] = [];
  let observations = 0;
  const call: WorkbenchCall = request => {
    if (request.kind !== 'preview') return Promise.resolve({});
    if (request.read?.kind !== 'text') return validation.promise;
    const result = gate<Awaited<ReturnType<WorkbenchCall>>>(); calls.push({ offset: request.read.offset, result }); return result.promise;
  };
  call.observe = async () => { observations++; const done = gate<void>(); watched.push(done); return { version: 'v1', done: done.promise, close: async () => done.resolve() }; };
  const page = (index: number) => calls[index].result.resolve({ preview: { data: new TextEncoder().encode(`line ${calls[index].offset}\nline ${calls[index].offset + 1}`), bytes: 100, version: 'v1', offset: calls[index].offset, lines: 2, eof: false } });
  let ui!: ReturnType<typeof render>;
  try {
    await act(async () => { ui = render(<WorkspaceFile path="example.ts" call={call} view={view} onOpen={() => {}}/>); });
    await act(async () => page(0));
    if (action === 'evicted restoration') {
      fireEvent.click(ui.getByRole('button', { name: 'Load more' })); await act(async () => page(1));
      expect(view.document?.textLoadedLines).toBe(4);
    }
    ui.unmount(); view.top = 173;
    if (action === 'cached navigation') view.line = 4; else view.preview!.owner.evict();
    await act(async () => { ui = render(<WorkspaceFile path="example.ts" call={call} view={view} onOpen={() => {}}/>); });
    if (action === 'cached navigation') {
      expect(calls.map(call => call.offset)).toEqual([1]); expect(view.line).toBe(4);
      await act(async () => validation.resolve({ preview: { data: new Uint8Array([1]), version: 'v1', bytes: 100, offset: 0, eof: false } }));
      expect(calls.map(call => call.offset)).toEqual([1, 3]); await act(async () => page(1)); expect(view.line).toBeUndefined();
    } else {
      expect(calls.map(call => call.offset)).toEqual([1, 3, 1]); await act(async () => page(2));
      expect(calls.map(call => call.offset)).toEqual([1, 3, 1, 3]); await act(async () => page(3)); expect(view.top).toBe(173);
    }
    expect(view.preview!.owner.getSnapshot().loadedLines).toBe(4); expect(observations).toBe(action === 'cached navigation' ? 1 : 2);
    ui.unmount(); await act(async () => { ui = render(<WorkspaceFile path="example.ts" call={call} view={view} onOpen={() => {}}/>); });
    await act(async () => validation.resolve({ preview: { data: new Uint8Array([1]), version: 'v1', bytes: 100, offset: 0, eof: false } }));
    expect(calls.map(call => call.offset)).toEqual(action === 'cached navigation' ? [1, 3] : [1, 3, 1, 3]);
  } finally { ui?.unmount(); view.preview?.dispose(); watched.forEach(done => done.resolve()); }
});

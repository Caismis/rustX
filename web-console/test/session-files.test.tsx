import { webcrypto } from 'node:crypto';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { FilePreviewCoordinator, SESSION_FILE_MAX_BYTES, samePreviewSource, type PreviewSource } from '../src/client/session-files';
import { ArtifactPreview, PreviewContext } from '../src/app/components/ArtifactPreview';
import { ToolDeliveries } from '../src/app/components/Artifact';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { validateRaster, RASTER_MAX_PIXELS } from '../src/client/raster';
import { WorkspaceHostError, type DeliveryBytes, type DeliveryRead, type ProductHostWorkspaces } from '../src/workspaces/host';
import type { DocumentRequest, DocumentResult } from '../shared/documents';
import type { SessionFileReference, ToolExecutionResult } from '../../protocol/app-server/v55';
import { Server } from './fixture';
import { PreviewWorkspaceOwner } from '../src/app/preview-workspace';
import { PreviewWorkspaceNotice } from '../src/app/components/PreviewWorkspace';
import { useSyncExternalStore } from 'react';
const file: SessionFileReference = { scope: { conversation_id: 'original', device: '1', inode: '2' }, path: 'sub/报告 file.md', name: '报告 file.md', description: 'Explicit report', mime_type: 'text/markdown' };
const source: Extract<PreviewSource,{kind:"session_file"}> = { kind: 'session_file', conversationId: 'conversation-A', locator: {type:'by_message_id' as const,message_id:'canonical-tool'}, index: 0, file };
const otherSource={...source,conversationId:"conversation-B"};
const bytes = '# Report\r\n\r\n**Native** bytes\r\n';
function deferred<T>() { let resolve!: (value: T) => void, reject!: (cause: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
let server: Server;
const owners: FilePreviewCoordinator[] = [];
const create = vi.fn<(blob: Blob) => string>(), revoke = vi.fn();
const viewState = { viewState: {}, onViewStateChange: () => {}, onDownload: vi.fn() };
beforeEach(() => {
  server = new Server(); create.mockReset().mockImplementation(() => `blob:${create.mock.calls.length}`); revoke.mockReset();
  vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: create, revokeObjectURL: revoke })); vi.stubGlobal('crypto', webcrypto);
});
afterEach(() => { cleanup(); owners.splice(0).forEach(owner => owner.dispose()); server.client.disconnect(); vi.unstubAllGlobals(); });
async function fixture(observed = false) {
  await server.attached('A', 'B');
  let hostId = 'host-1';
  const calls: { read: DeliveryRead; signal?: AbortSignal; response: ReturnType<typeof deferred<DeliveryBytes>> }[] = [];
  const watches: { changed: () => void; signal: AbortSignal; close: ReturnType<typeof vi.fn> }[] = [];
  const host: ProductHostWorkspaces = { ...server.workspaceHost,
    listWorkspaces: async () => ({ ...await server.workspaceHost.listWorkspaces(), authorityId: hostId }),
    readDelivery: async (_scope, read, signal) => { const response = deferred<DeliveryBytes>(); calls.push({ read, signal, response }); return response.promise; },
    observeDelivery: observed ? async (_scope, _read, _file, changed, signal) => {
      const ended = deferred<void>(), close = vi.fn(async () => ended.resolve()); watches.push({ changed, signal, close });
      return { version: 'version-1', close, done: ended.promise };
    } : undefined,
  };
  const authority = new WorkspaceAuthority(host); await authority.observe();
  const resources = new FilePreviewCoordinator(server.client, 'A', host, authority); owners.push(resources);
  const lease = resources.acquire(1, source);
  const reply = (at: number, data = btoa(bytes), delivered = file) => {
    const raw = Uint8Array.from(atob(data), c => c.charCodeAt(0)), read = calls[at].read.read;
    calls[at].response.resolve({ file: delivered, data: raw, version: 'version-1', bytes: raw.length, offset: read?.kind === 'text' ? read.offset : 0, eof: true, ...(read?.kind === 'text' ? { lines: bytes.split('\n').length } : {}) });
  };
  return { resources, lease, authority, host, calls, reply, watches, replaceHost: async () => { hostId = 'host-2'; await authority.observe(); } };
}
function WorkspaceBody({ owner }: { owner: PreviewWorkspaceOwner }) {
  const snapshot = useSyncExternalStore(owner.subscribe, owner.getSnapshot), workspace = snapshot.workspace;
  const tab = workspace?.tabs.find(tab => tab.id === workspace.panes.find(pane => pane.id === workspace.activePane)?.selected);
  const lease = tab && snapshot.leases.get(tab.id);
  return tab && lease ? <ArtifactPreview artifact={tab.artifact} resources={lease} viewState={tab.view} onViewStateChange={patch => owner.updateView(tab.id, patch)} onDownload={() => {}}/> : null;
}
function WorkspaceNotice({ owner }: { owner: PreviewWorkspaceOwner }) {
  const snapshot = useSyncExternalStore(owner.subscribe, owner.getSnapshot);
  return <PreviewWorkspaceNotice snapshot={snapshot} dismiss={() => owner.dismissProblem()}/>;
}
it.each(['valid', 'Session', 'attachment', 'Host'] as const)('independent Download failure after the final preview closes reports only its %s scope', async scope => {
  const f = await fixture(), owner = new PreviewWorkspaceOwner(server.client, f.host, f.authority), stop = owner.start(); owner.selectSession('A');
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {}), ui = render(<><WorkspaceBody owner={owner}/><WorkspaceNotice owner={owner}/></>);
  let pending: Promise<void> | undefined;
  try {
    let id!: number; await act(async () => { id = owner.openPreview({ source, name: 'a.txt', image: false })!; }); await act(async () => f.reply(0));
    pending = owner.download({ source, name: file.name, image: false }); await act(async () => owner.closeTab(id));
    if (scope === 'Session') await act(async () => owner.selectSession('B'));
    if (scope === 'attachment') await act(async () => { await server.client.release('A'); await server.client.attach('A'); });
    if (scope === 'Host') await act(async () => f.replaceHost());
    await act(async () => { f.calls[1].response.reject(new Error('physical read failed')); await pending; });
    expect(click).not.toHaveBeenCalled(); expect(create).not.toHaveBeenCalled();
    if (scope === 'valid' || scope === 'Session') expect(ui.getByRole('alert').textContent).toContain('Could not download the authorized original');
    else expect(ui.queryByRole('alert')).toBeNull();
    if (scope === 'Session') await act(async () => owner.selectSession('A'));
    const retry = owner.download({ source, name: file.name, image: false }); expect(f.calls).toHaveLength(3);
    await act(async () => { f.reply(2); await retry; });
    expect(click).toHaveBeenCalledOnce(); expect(revoke).toHaveBeenCalledOnce(); expect(ui.queryByRole('alert')).toBeNull();
  } finally { if (f.calls[1]) f.reply(1); await pending; ui.unmount(); stop(); }
});
it.each(['attachment', 'Host'] as const)('download-only owners recover after %s replacement before the first preview', async replacement => {
  const f = await fixture(), owner = new PreviewWorkspaceOwner(server.client, f.host, f.authority), stop = owner.start(); owner.selectSession('A');
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  const ui = render(<WorkspaceBody owner={owner}/>);
  try {
    const first = owner.download({ source, name: file.name, image: false }); f.reply(0); await first;
    expect(owner.getSnapshot().workspace).toBeUndefined(); expect(click).toHaveBeenCalledOnce();
    await act(async () => { if (replacement === 'attachment') { await server.client.release('A'); await server.client.attach('A'); } else await f.replaceHost(); });
    await act(async () => { owner.openPreview({ source, name: 'recovered.txt', image: false }); });
    expect(f.calls).toHaveLength(2); await act(async () => f.reply(1)); expect(ui.container.querySelector('pre')?.textContent).toBe(bytes);
    const second = owner.download({ source, name: file.name, image: false }); expect(f.calls).toHaveLength(3); f.reply(2); await second;
    expect(click).toHaveBeenCalledTimes(2); expect(owner.getSnapshot().downloadError).toBeUndefined();
  } finally { ui.unmount(); stop(); }
});
it('closing the last preview preserves an independent admitted Download through its physical result', async () => {
  const f = await fixture(), owner = new PreviewWorkspaceOwner(server.client, f.host, f.authority), stop = owner.start(); owner.selectSession('A');
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {}), ui = render(<WorkspaceBody owner={owner}/>);
  try {
    let id!: number; await act(async () => { id = owner.openPreview({ source, name: 'a.txt', image: false })!; });
    await act(async () => f.reply(0));
    const download = owner.download({ source, name: file.name, image: false }); expect(f.calls).toHaveLength(2);
    await act(async () => owner.closeTab(id)); expect(f.calls[1].signal?.aborted).toBe(false);
    f.reply(1); await download; expect(click).toHaveBeenCalledOnce(); expect(revoke).toHaveBeenCalledOnce();
  } finally { ui.unmount(); stop(); }
});
it('retained Sessions share one transient Download and reclaim completed download-only owners', async () => {
  const f = await fixture(), owner = new PreviewWorkspaceOwner(server.client, f.host, f.authority), stop = owner.start(); owner.selectSession('A');
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  let first: Promise<void> | undefined;
  try {
    first = owner.download({ source, name: file.name, image: false }); expect(f.calls).toHaveLength(1);
    owner.selectSession('B'); await owner.download({ source: otherSource, name: file.name, image: false });
    expect(f.calls).toHaveLength(1); expect(owner.getSnapshot().downloadError).toContain('capacity');
    f.reply(0); await first; expect(click).toHaveBeenCalledOnce();
    const next = owner.download({ source: otherSource, name: file.name, image: false }); expect(f.calls).toHaveLength(2); f.reply(1); await next;
    expect(click).toHaveBeenCalledTimes(2); expect(owner.getSnapshot().downloadError).toBeUndefined();
    expect(revoke).toHaveBeenCalledTimes(2); expect(revoke.mock.invocationCallOrder[0]).toBeLessThan(create.mock.invocationCallOrder[1]);
  } finally { if (f.calls[0]) f.reply(0); await first; stop(); }
});
it('observed tab and Session return restores cached pages and scroll only after an authorized version probe', async () => {
  const f = await fixture(true), owner = new PreviewWorkspaceOwner(server.client, f.host, f.authority), stop = owner.start();
  owner.selectSession('A'); const id = owner.openPreview({ source, name: 'cached.txt', image: false })!;
  const ui = render(<WorkspaceBody owner={owner}/>);
  try {
    await act(async () => {}); expect(f.watches).toHaveLength(1); expect(f.calls).toHaveLength(1);
    await act(async () => f.reply(0, btoa('cached result')));
    const body = ui.container.querySelector<HTMLElement>('[data-preview-scroll="body"]')!;
    body.scrollTop = 317; body.scrollLeft = 23; fireEvent.scroll(body);
    await act(async () => owner.selectSession('B')); expect(ui.queryByText('cached result')).toBeNull();
    await act(async () => owner.selectSession('A'));
    expect(f.calls).toHaveLength(2); expect(f.calls[1].read.read).toEqual({ kind: 'bytes', range: { offset: 0, length: 1 } });
    expect(ui.queryByText('cached result')).toBeNull();
    await act(async () => f.calls[1].response.resolve({ file, data: new Uint8Array([1]), version: 'version-1', bytes: 20, offset: 0, eof: false }));
    expect(ui.getByText('cached result')).toBeTruthy(); expect(f.calls).toHaveLength(2); expect(f.watches).toHaveLength(1);
    expect(ui.container.querySelector('[data-preview-scroll="body"]')?.scrollTop).toBe(317);
    expect(ui.container.querySelector('[data-preview-scroll="body"]')?.scrollLeft).toBe(23);
    const retained = owner.getSnapshot().leases.get(id)!.content!;
    await act(async () => owner.closeTab(id)); expect(retained.retainedBytes).toBe(0); expect(f.watches[0].close).toHaveBeenCalledOnce();
    await act(async () => { owner.openPreview({ source, name: 'cached.txt', image: false }); });
    expect(f.watches).toHaveLength(2); expect(f.calls).toHaveLength(3); expect(f.calls[2].read.read).toMatchObject({ kind: 'text', offset: 1 });
    await act(async () => f.reply(2, btoa('new occurrence'))); expect(ui.getByText('new occurrence')).toBeTruthy();
  } finally { ui.unmount(); stop(); }
});
it('file notifications refresh automatically, deduplicate equal versions, and native permission failure removes the protected prefix', async () => {
  const f = await fixture(true), artifact = { source, name: 'observed.txt', image: false };
  const ui = render(<ArtifactPreview artifact={artifact} resources={f.lease} {...viewState}/>);
  await act(async () => {}); await act(async () => f.reply(0, btoa('old value')));
  await act(async () => f.watches[0].changed());
  expect(ui.getByText('old value')).toBeTruthy(); expect(f.calls[1].read.read).toMatchObject({ kind: 'bytes', range: { length: 1 } });
  await act(async () => f.calls[1].response.resolve({ file, data: new Uint8Array([1]), version: 'version-1', bytes: 20, offset: 0, eof: false }));
  expect(f.calls).toHaveLength(2); expect(ui.getByText('old value')).toBeTruthy();
  await act(async () => f.watches[0].changed());
  await act(async () => f.calls[2].response.resolve({ file, data: new Uint8Array([2]), version: 'version-2', bytes: 20, offset: 0, eof: false }));
  expect(f.calls).toHaveLength(4); expect(ui.queryByText('old value')).toBeNull(); expect(f.calls[3].read.read).toMatchObject({ kind: 'text', offset: 1 });
  await act(async () => f.calls[3].response.resolve({ file, data: new TextEncoder().encode('new value'), version: 'version-2', bytes: 9, offset: 1, lines: 1, eof: true }));
  expect(ui.getByText('new value')).toBeTruthy();
  await act(async () => f.watches[0].changed());
  await act(async () => f.calls[4].response.reject(new WorkspaceHostError('denied', 'session_file_read', false, { kind: 'session_file_read', reason: 'unauthorized' })));
  expect(ui.queryByText('new value')).toBeNull(); expect(ui.getByRole('alert').textContent).toContain('authorization was revoked');
  expect(ui.queryByRole('button', { name: 'Retry preview' })).toBeNull(); expect(f.calls).toHaveLength(5); expect(f.lease.content!.retainedBytes).toBe(0);
});
it.each(['clip.zip', 'legacy.doc', 'legacy.xls'])('known unsupported %s acquires no content bytes', async name => {
  const f = await fixture(), ui = render(<ArtifactPreview artifact={{ source, name, image: false }} resources={f.lease} {...viewState}/>);
  expect(f.calls).toHaveLength(0); expect(ui.getByText('This artifact has no supported inline viewer.')).toBeTruthy();
});
it('one authorized original read supplies Markdown; Download invokes its distinct intent', async () => {
  const f = await fixture(), onDownload = vi.fn(), before = server.requests.length;
  const ui = render(<ArtifactPreview artifact={{ source, name: file.name, image: false, mimeType: file.mime_type }} resources={f.lease} {...viewState} onDownload={onDownload}/>);
  expect(f.calls[0].read).toEqual({ target: server.client.target('A'), locator: {type:'by_message_id' as const,message_id:'canonical-tool'}, delivery_index: 0, read: { kind: 'text', offset: 1, limit: 5000 } });
  expect(JSON.stringify(f.calls[0].read)).not.toContain(file.path);
  await act(async () => f.reply(0));
  expect(ui.getByRole('heading', { name: 'Report' })).toBeTruthy(); expect(ui.getByText('Native').tagName).toBe('STRONG');
  fireEvent.click(ui.getByRole('button', { name: 'Download artifact' })); expect(onDownload).toHaveBeenCalledOnce();
  expect(create).not.toHaveBeenCalled(); expect(server.requests).toHaveLength(before);
  ui.unmount(); f.resources.dispose(); expect(revoke).not.toHaveBeenCalled();
});
it('child delivery coordinates carry native ownership and cannot share a root or sibling preview occurrence', async () => {
  const f = await fixture();
  const child = { ...source, conversationId:'conversation-child', owned:()=>true, agentId: 'child-agent' };
  expect(samePreviewSource(source, child)).toBe(false);
  expect(samePreviewSource(child, { ...child, agentId: 'other-child' })).toBe(false);
  const lease = f.resources.acquire(2, child);
  const work = lease.loadPage(1, new AbortController().signal);
  expect(f.calls[0].read).toMatchObject({ agent_id: 'child-agent', locator: {type:'by_message_id' as const,message_id:'canonical-tool'}, delivery_index: 0 });
  f.reply(0); expect(await work).toMatchObject({ text: bytes });
});
it('the workspace clones child coordinates while retaining the live ownership guard through the byte response', async () => {
  const f=await fixture(), workspace=new PreviewWorkspaceOwner(server.client,f.host,f.authority), stop=workspace.start();
  let current=true;
  const owned=()=>current;
  const child={...structuredClone(source),conversationId:'conversation-child',agentId:'child-agent',owned};
  try {
    workspace.selectSession('A');
    const id=workspace.openPreview({source:child,name:file.name,image:false,mimeType:file.mime_type});
    expect(id).toBeDefined();
    const retained=workspace.getSnapshot().workspace!.tabs[0].artifact.source;
    expect(retained.kind).toBe('session_file');
    if(retained.kind!=='session_file')throw Error('Original child source missing');
    expect(retained.owned).toBe(owned);
    expect(retained.locator).not.toBe(child.locator);expect(retained.file).not.toBe(child.file);expect(retained.file.scope).not.toBe(child.file.scope);
    child.file.path='changed input';child.locator={type:'by_ledger_position',position:99};
    expect(retained.file).toEqual(file);expect(retained.locator).toEqual(source.locator);
    const work=workspace.getSnapshot().leases.get(id!)!.load(file.mime_type), rejected=expect(work).rejects.toThrow(/Obsolete/);
    expect(f.calls[0].read).toEqual({target:server.client.target('A'),agent_id:'child-agent',locator:source.locator,delivery_index:0,read:{kind:'bytes'}});
    current=false;f.reply(0);await rejected;
    expect(create).not.toHaveBeenCalled();
  } finally {stop();}
});
it('a child artifact preview uses the native Agent namespace, even if a root artifact has the same id', async () => {
  const f = await fixture();
  server.handlers.set('agent/artifactRead', () => ({type:'artifact_bytes',data:btoa(bytes)}));
  const lease = f.resources.acquire(2, {kind:'artifact',id:'artifact_1',agentId:'child-agent'});
  expect(await lease.loadPage(1, new AbortController().signal)).toMatchObject({text:bytes});
  const request = await server.waitFor('agent/artifactRead',1);
  expect(request.params).toEqual({target:server.client.target('A'),agent_id:'child-agent',artifact_id:'artifact_1'});
  expect(server.requests.filter(row => row.request.method === 'artifact/read')).toHaveLength(0);
});
it.each(['text/plain', 'text/markdown'] as const)('managed %s remains artifact/read with its exact target and original bytes', async () => {
  const f = await fixture(); server.held.add('artifact/read');
  const lease = f.resources.acquire(2, { kind: 'artifact', id: 'immutable-artifact' });
  const work = lease.loadPage(1, new AbortController().signal), request = await server.waitFor('artifact/read', 1);
  expect(request.params).toEqual({ target: server.client.target('A'), artifact_id: 'immutable-artifact' });
  server.socket.success(request, { type: 'artifact_bytes', data: btoa(bytes) });
  expect(await work).toMatchObject({ text: bytes, eof: true }); expect(f.calls).toHaveLength(0);
  f.resources.dispose(); expect(revoke).not.toHaveBeenCalled();
});
it.each([{ image: true, mimeType: undefined }, { image: false, mimeType: 'image/png' }])('extensionless managed image retains typed $image / $mimeType classification and raster admission', async ({ image, mimeType }) => {
  const f = await fixture(), png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII=';
  server.handlers.set('artifact/read', () => ({ type: 'artifact_bytes', data: png }));
  const lease = f.resources.acquire(2, { kind: 'artifact', id: 'artifact_1' });
  const ui = render(<ArtifactPreview artifact={{ source: lease.source, name: 'artifact_1', image, mimeType }} resources={lease} {...viewState}/>);
  await act(async () => {});
  expect(lease.content!.getSnapshot().mode).toBe('bytes'); expect(ui.getByRole('img', { name: 'artifact_1' }).getAttribute('src')).toBe('blob:1');
  expect(f.calls).toHaveLength(0); expect(create).toHaveBeenCalledOnce();
  expect(server.requests.filter(row => row.request.method === 'artifact/read')).toHaveLength(1);
  ui.unmount(); lease.dispose(); expect(revoke).toHaveBeenCalledExactlyOnceWith('blob:1');
});
it('an extensionless typed image cannot allocate a URL for SVG bytes', async () => {
  const f = await fixture(); server.handlers.set('artifact/read', () => ({ type: 'artifact_bytes', data: btoa('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>') }));
  const lease = f.resources.acquire(2, { kind: 'artifact', id: 'artifact_1' });
  const ui = render(<ArtifactPreview artifact={{ source: lease.source, name: 'artifact_1', image: true, mimeType: 'image/svg+xml' }} resources={lease} {...viewState}/>);
  await act(async () => {}); expect(ui.getByRole('alert')).toBeTruthy(); expect(create).not.toHaveBeenCalled(); expect(ui.container.querySelector('img')).toBeNull();
});
it('an extensionless managed Markdown artifact retains its typed renderer', async () => {
  const f = await fixture(); server.handlers.set('artifact/read', () => ({ type: 'artifact_bytes', data: btoa('# Typed Markdown') }));
  const lease = f.resources.acquire(2, { kind: 'artifact', id: 'artifact_1' });
  const ui = render(<ArtifactPreview artifact={{ source: lease.source, name: 'artifact_1', image: false, mimeType: 'text/markdown' }} resources={lease} {...viewState}/>);
  await act(async () => {}); expect(ui.getByRole('heading', { name: 'Typed Markdown' })).toBeTruthy(); expect(lease.content!.getSnapshot().mode).toBe('pages');
});
it.each(['Host', 'attachment', 'authority', 'close', 'abort'] as const)('gated obsolete %s response cannot allocate bytes or a URL', async change => {
  const f = await fixture(), controller = new AbortController();
  const work = f.lease.load(file.mime_type, false, controller.signal), rejected = expect(work).rejects.toThrow(/Obsolete/);
  if (change === 'Host') await f.replaceHost();
  if (change === 'attachment') { await server.client.release('A'); await server.client.attach('A'); }
  if (change === 'authority') server.client.disconnect();
  if (change === 'close') f.resources.dispose();
  if (change === 'abort') controller.abort();
  f.reply(0); await rejected; expect(create).not.toHaveBeenCalled(); f.resources.dispose(); expect(revoke).not.toHaveBeenCalled();
});
it.each(['bytes', 'error'] as const)('close then reopen source gets a new lease; late old %s cannot publish', async terminal => {
  const f = await fixture(), artifact = { source, name: file.name, image: false, mimeType: file.mime_type };
  const ui = render(<ArtifactPreview key="1" artifact={artifact} resources={f.lease} {...viewState}/>);
  f.lease.dispose(); const next = f.resources.acquire(2, source);
  ui.rerender(<ArtifactPreview key="2" artifact={artifact} resources={next} {...viewState}/>);
  expect(f.calls[0].signal?.aborted).toBe(true);
  await act(async () => f.reply(1, btoa('current occurrence')));
  await act(async () => terminal === 'bytes' ? f.reply(0) : f.calls[0].response.reject(new Error('old failure')));
  expect(ui.getByText('current occurrence')).toBeTruthy(); expect(ui.queryByRole('alert')).toBeNull();
  expect(create).not.toHaveBeenCalled(); ui.unmount(); f.resources.dispose(); expect(revoke).not.toHaveBeenCalled();
});
it('two pane URLs plus one transient original Download are bounded and released exactly once', async () => {
  const f = await fixture(), second = f.resources.acquire(2, { ...source, index: 1 });
  expect(() => f.resources.acquire(3, source)).toThrow('capacity');
  const firstRead = f.lease.load(), secondRead = second.load(); f.reply(0); f.reply(1); await Promise.all([firstRead, secondRead]);
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
    expect(this.download).toBe('报告 file.md'); expect(this.href).toBe('blob:3');
  });
  try {
    const download = f.resources.download(source, file.name, file.mime_type);
    await expect(f.resources.download(source, 'second')).rejects.toThrow('capacity');
    f.reply(2); await download; expect(create).toHaveBeenCalledTimes(3); expect(click).toHaveBeenCalledOnce();
    expect(revoke).toHaveBeenCalledExactlyOnceWith('blob:3');
    expect(f.lease.current() && second.current()).toBe(true);
    f.resources.dispose(); expect(revoke.mock.calls.map(call => call[0]).sort()).toEqual(['blob:1', 'blob:2', 'blob:3']);
  } finally { click.mockRestore(); }
});
it('Download waits behind two active reads without increasing native transfer admission', async () => {
  const f = await fixture(), second = f.resources.acquire(2, { ...source, index: 1 });
  const firstRead = f.lease.load(), secondRead = second.load();
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  try {
    const download = f.resources.download(source, file.name);
    expect(f.calls).toHaveLength(2); f.reply(0); await firstRead;
    expect(f.calls).toHaveLength(3); f.reply(1); f.reply(2); await Promise.all([secondRead, download]);
    f.resources.dispose(); expect(revoke).toHaveBeenCalledTimes(3);
  } finally { click.mockRestore(); }
});
it('complete reads admit 600 KiB and reject above 32 MiB before creating a Blob', async () => {
  const f = await fixture(), work = f.lease.load('application/octet-stream', false, undefined, true); f.reply(0, btoa('x'.repeat(600 * 1024)));
  expect((await work).bytes).toHaveLength(600 * 1024); f.lease.release('blob:1');
  const oversized = f.lease.load();
  f.calls[1].response.resolve({ file, data: new Uint8Array(SESSION_FILE_MAX_BYTES + 1), version: 'version-1', bytes: SESSION_FILE_MAX_BYTES + 1, offset: 0, eof: true });
  await expect(oversized).rejects.toThrow('Invalid complete file response'); expect(create).toHaveBeenCalledOnce();
});
it('UTF-8/image decode failures retain bounded original bytes; failed reads allocate no URL and permit explicit retry', async () => {
  const f = await fixture(), invalid = f.lease.loadPage(1, new AbortController().signal), rejected = expect(invalid).rejects.toThrow(); f.reply(0, '/w==');
  await rejected; expect(create).not.toHaveBeenCalled();
  const image = f.lease.load('image/png', true); f.reply(1, 'aGk=');
  expect(await image).toMatchObject({ url: 'blob:1', error: expect.any(String) }); f.lease.release('blob:1');
  const failed = f.lease.load(); f.calls[2].response.reject(new Error('file deleted'));
  await expect(failed).rejects.toThrow('deleted'); expect(create).toHaveBeenCalledTimes(1);
  const retry = f.lease.load(); f.reply(3); await retry; f.resources.dispose(); expect(revoke).toHaveBeenCalledTimes(2);
});
it('Markdown HTML, executable URLs and embedded images remain inert', async () => {
  const f = await fixture(), ui = render(<ArtifactPreview artifact={{ source, name: file.name, image: false, mimeType: file.mime_type }} resources={f.lease} {...viewState}/>);
  await act(async () => f.reply(0, btoa('<script>globalThis.PWNED=1</script>\n\n<img src="/private" onerror="alert(1)">\n\n[x](javascript:alert(1))\n\n![local](file:///etc/passwd) ![remote](https://example.org/track)')));
  expect(ui.container.querySelector('[data-preview-scroll="body"]')!.querySelector('script,img,iframe,object,svg')).toBeNull();
  expect([...ui.container.querySelectorAll('a')].some(a => a.href.startsWith('javascript:'))).toBe(false);
  expect((globalThis as { PWNED?: number }).PWNED).toBeUndefined();
});

it('600 KiB text previews through the page reader without allocating an original Blob', async () => {
  const f = await fixture();
  const ui = render(<ArtifactPreview artifact={{ source, name: 'large.txt', image: false, mimeType: 'text/plain' }} resources={f.lease} {...viewState}/>);
  expect(f.calls[0].read.read).toEqual({ kind: 'text', offset: 1, limit: 5000 });
  await act(async () => f.reply(0, btoa('x'.repeat(600 * 1024))));
  expect(ui.container.querySelector('pre')?.textContent?.length).toBe(600 * 1024);
  expect(create).not.toHaveBeenCalled();
  expect(ui.getByRole('button', { name: 'Download artifact' })).toBeTruthy();
});

it('text pagination appends one document and restarts instead of mixing file versions', async () => {
  const f = await fixture();
  const ui = render(<ArtifactPreview artifact={{ source, name: 'large.txt', image: false, mimeType: 'text/plain' }} resources={f.lease} {...viewState}/>);
  const page = (index: number, text: string, version: string, offset: number, eof: boolean) => f.calls[index].response.resolve({
    file, data: new TextEncoder().encode(text), bytes: 8000000, version, offset, lines: 1, eof,
  });
  await act(async () => page(0, 'first', 'one', 1, false));
  fireEvent.click(ui.getByRole('button', { name: 'Load more' }));
  expect(f.calls[1].read.read).toEqual({ kind: 'text', offset: 2, limit: 5000 });
  await act(async () => page(1, 'second', 'one', 2, false));
  expect(ui.container.querySelector('pre')?.textContent).toBe('first\nsecond');
  fireEvent.click(ui.getByRole('button', { name: 'Load more' }));
  await act(async () => page(2, 'changed tail', 'two', 3, true));
  expect(f.calls[3].read.read).toEqual({ kind: 'text', offset: 1, limit: 5000 });
  await act(async () => page(3, 'replacement', 'two', 1, true));
  expect(ui.container.querySelector('pre')?.textContent).toBe('replacement');
  expect(ui.queryByRole('button', { name: 'Load more' })).toBeNull();
});

it('Office conversion starts on the Host without an original browser read and failures retain Download', async () => {
  const f = await fixture();
  const conversion = vi.fn().mockRejectedValue(new Error('converter_unavailable'));
  f.host.previewDocument = conversion;
  const ui = render(<ArtifactPreview artifact={{ source, name: 'large.docx', image: false }} resources={f.lease} {...viewState}/>);
  await act(async () => {});
  expect(conversion).toHaveBeenCalledOnce();
  expect(conversion.mock.calls[0][1]).not.toHaveProperty('digest');
  expect(f.calls).toHaveLength(0);
  expect(ui.getByRole('alert')).toBeTruthy();
  expect(ui.getByRole('button', { name: 'Download artifact' })).toBeTruthy();
});

it('refresh during the initial read records the newest intent and waits for physical settlement', async () => {
  const f = await fixture();
  const ui = render(<ArtifactPreview artifact={{ source, name: 'large.txt', image: false, mimeType: 'text/plain' }} resources={f.lease} {...viewState}/>);
  const refresh = ui.getByRole('button', { name: 'Refresh' });
  expect(refresh).toHaveProperty('disabled', false);
  fireEvent.click(refresh); fireEvent.click(refresh);
  expect(f.calls).toHaveLength(1);
  expect(f.calls[0].signal?.aborted).toBe(true);
  await act(async () => f.reply(0, btoa('obsolete')));
  expect(ui.queryByText('obsolete')).toBeNull();
  expect(f.calls).toHaveLength(2);
  await act(async () => f.reply(1, btoa('latest')));
  expect(ui.container.querySelector('pre')?.textContent).toBe('latest');
});

it('refresh during load more fences late pages and coalesces replacement behind the real read', async () => {
  const f = await fixture();
  const ui = render(<ArtifactPreview artifact={{ source, name: 'large.txt', image: false, mimeType: 'text/plain' }} resources={f.lease} {...viewState}/>);
  await act(async () => f.calls[0].response.resolve({ file, data: new TextEncoder().encode('first'), bytes: 100, version: 'one', offset: 1, lines: 1, eof: false }));
  fireEvent.click(ui.getByRole('button', { name: 'Load more' }));
  expect(f.calls).toHaveLength(2);
  fireEvent.click(ui.getByRole('button', { name: 'Refresh' }));
  fireEvent.click(ui.getByRole('button', { name: 'Refresh' }));
  await act(async () => {});
  expect(f.calls).toHaveLength(2);
  expect(f.calls[1].signal?.aborted).toBe(true);
  expect(ui.queryByRole('alert')).toBeNull();
  await act(async () => f.calls[1].response.resolve({ file, data: new TextEncoder().encode('late tail'), bytes: 100, version: 'one', offset: 2, lines: 1, eof: true }));
  expect(ui.queryByText(/late tail/)).toBeNull();
  expect(f.calls).toHaveLength(3);
  expect(f.calls[2].read.read).toEqual({ kind: 'text', offset: 1, limit: 5000 });
  await act(async () => f.reply(2, btoa('refreshed')));
  expect(ui.container.querySelector('pre')?.textContent).toBe('refreshed');
});

it('retry after a failed next page retains the loaded prefix and retries that offset', async () => {
  const f = await fixture();
  const ui = render(<ArtifactPreview artifact={{ source, name: 'large.txt', image: false, mimeType: 'text/plain' }} resources={f.lease} {...viewState}/>);
  await act(async () => f.calls[0].response.resolve({ file, data: new TextEncoder().encode('retained'), bytes: 100, version: 'one', offset: 1, lines: 1, eof: false }));
  fireEvent.click(ui.getByRole('button', { name: 'Load more' }));
  await act(async () => f.calls[1].response.reject(new Error('temporary read failure')));
  expect(ui.container.querySelector('pre')?.textContent).toBe('retained');
  fireEvent.click(ui.getByRole('button', { name: 'Retry preview' }));
  expect(f.calls[2].read.read).toEqual({ kind: 'text', offset: 2, limit: 5000 });
});
it.each(['stale_attachment', 'stale_runtime', 'unknown_session', 'unknown_node'])('a typed native %s removes the visible prefix before observation retirement', async kind => {
  const f = await fixture(true), ui = render(<ArtifactPreview artifact={{ source, name: 'large.txt', image: false, mimeType: 'text/plain' }} resources={f.lease} {...viewState}/>);
  await act(async () => {});
  await act(async () => f.calls[0].response.resolve({ file, data: new TextEncoder().encode('protected prefix'), bytes: 100, version: 'version-1', offset: 1, lines: 1, eof: false }));
  expect(ui.container.querySelector('pre')?.textContent).toBe('protected prefix'); fireEvent.click(ui.getByRole('button', { name: 'Load more' }));
  await act(async () => f.calls[1].response.reject(new WorkspaceHostError(kind, kind)));
  expect(ui.getByRole('alert')).toBeTruthy(); expect(ui.container.querySelector('pre')).toBeNull();
  expect(f.lease.content!.retainedBytes).toBe(0); expect(f.watches[0].close).not.toHaveBeenCalled(); expect(f.calls).toHaveLength(2);
});

it('reactivation reloads the saved text prefix before restoring its scroll position', async () => {
  const f = await fixture(), changed = vi.fn();
  const saved = { textLoadedLines: 5001, bodyScrollTop: 1234 };
  const ui = render(<ArtifactPreview artifact={{ source, name: 'large.txt', image: false, mimeType: 'text/plain' }} resources={f.lease}
    viewState={saved} onViewStateChange={changed} onDownload={() => {}}/>);
  const first = Array.from({ length: 5000 }, (_, index) => `line ${index}`).join('\n');
  await act(async () => f.calls[0].response.resolve({ file, data: new TextEncoder().encode(first), version: 'one', bytes: first.length + 5,
    offset: 1, lines: 5000, eof: false }));
  expect(f.calls).toHaveLength(2);
  expect(f.calls[1].read.read).toEqual({ kind: 'text', offset: 5001, limit: 5000 });
  await act(async () => f.calls[1].response.resolve({ file, data: new TextEncoder().encode('tail'), version: 'one', bytes: first.length + 5,
    offset: 5001, lines: 1, eof: true }));
  expect(ui.container.querySelector('pre')?.textContent).toBe(`${first}\ntail`);
  expect(ui.container.querySelector('[data-preview-scroll="body"]')?.scrollTop).toBe(1234);
  expect(changed).toHaveBeenLastCalledWith({ textLoadedLines: 5001 });
});
it('typed committed cards distinguish Preview and Download; prose/JSON/failed facts cannot grant access', () => {
  const openPreview = vi.fn(), download = vi.fn(), intents = { openPreview, download };
  const result: ToolExecutionResult = { status: { type: 'success' }, duration_ms: "0", content: [{ type: 'json', value: { deliveries: [file], path: file.path } }] };
  const ui = render(<PreviewContext.Provider value={intents}><ToolDeliveries conversationId="conversation-A" locator={{type:'by_message_id',message_id:'canonical-tool'}} result={result}/></PreviewContext.Provider>);
  expect(ui.queryAllByRole('button')).toHaveLength(0);
  ui.rerender(<PreviewContext.Provider value={intents}><ToolDeliveries conversationId="conversation-A" locator={{type:'by_message_id',message_id:'canonical-tool'}} result={{ ...result, deliveries: [file] }}/></PreviewContext.Provider>);
  fireEvent.click(ui.getByRole('button', { name: `Preview ${file.name} in sidebar` }));
  expect(openPreview).toHaveBeenCalledWith(expect.objectContaining({ source, name: file.name }));
  fireEvent.click(ui.getByRole('button', { name: `Download ${file.name}` }));
  expect(download).toHaveBeenCalledWith(expect.objectContaining({ source, name: file.name })); expect(openPreview).toHaveBeenCalledOnce();
  ui.rerender(<PreviewContext.Provider value={intents}><ToolDeliveries conversationId="conversation-A" locator={{type:'by_message_id',message_id:'canonical-tool'}} result={{ ...result, deliveries: [file], status: { type: 'failed', error: 'bad declaration' } }}/></PreviewContext.Provider>);
  expect(ui.queryAllByRole('button')).toHaveLength(0);
});
it('typed source equality preserves declarations, delivery indexes, namespaces and managed Artifact distinction', () => {
  expect(samePreviewSource(source, { ...source, file: { ...file } })).toBe(true);
  for (const other of [
    { ...source, locator:{type:'by_message_id' as const,message_id:'another-declaration'} }, { ...source, index: 1 },
    { ...source, file: { ...file, path: 'other/report.md' } },
    { ...source, file: { ...file, scope: { ...file.scope, conversation_id: 'other' } } },
    { kind: 'artifact' as const, id: file.name },
  ]) expect(samePreviewSource(source, other)).toBe(false);
});
it('raster dimensions bound decoded pixels and reject SVG before decoding', () => {
  const png = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6pAAAAABJRU5ErkJggg=='), c => c.charCodeAt(0));
  expect(() => validateRaster(png)).not.toThrow(); new DataView(png.buffer).setUint32(16, 4096); new DataView(png.buffer).setUint32(20, 4096);
  expect(() => validateRaster(png)).toThrow(); expect(RASTER_MAX_PIXELS).toBe(4194304);
  expect(() => validateRaster(new TextEncoder().encode('<svg/>'))).toThrow();
});
it.each(['Host', 'attachment', 'authority', 'close', 'abort'] as const)('gated derived %s response cannot publish to a retired lease', async change => {
  const f = await fixture(), entered = deferred<void>(), held = deferred<DocumentResult>();
  f.host.previewDocument = async () => { entered.resolve(); return held.promise; };
  const controller = new AbortController(), work = f.lease.derive('docx', controller.signal), rejected = expect(work).rejects.toThrow('obsolete');
  await entered.promise;
  if (change === 'Host') await f.replaceHost();
  if (change === 'attachment') { await server.client.release('A'); await server.client.attach('A'); }
  if (change === 'authority') server.client.disconnect();
  if (change === 'close') f.resources.dispose();
  if (change === 'abort') controller.abort();
  held.resolve({ digest: 'a'.repeat(64), file, preview: { kind: 'pdf', data: new TextEncoder().encode('%PDF') } });
  await rejected; expect(create).not.toHaveBeenCalled();
});
it('two visible Office intents serialize; cancel waiting demand immediately, active demand holds until settlement', async () => {
  const f = await fixture(), entered = deferred<void>(), secondEntered = deferred<void>(), held = deferred<DocumentResult>();
  const calls: { request: DocumentRequest; signal?: AbortSignal }[] = [];
  f.host.previewDocument = async (_scope, request, signal) => {
    calls.push({ request, signal });
    if (calls.length === 1) { entered.resolve(); return held.promise; }
    secondEntered.resolve(); return { digest: 'a'.repeat(64), file, preview: { kind: 'pdf', data: new TextEncoder().encode('%PDF') } };
  };
  const first = f.lease.derive('docx', new AbortController().signal), rejected = expect(first).rejects.toThrow('obsolete');
  await entered.promise;
  const waiting = f.resources.acquire(2, source), next = waiting.derive('pptx', new AbortController().signal);
  const waitingRejected = expect(next).rejects.toThrow('Obsolete'); waiting.dispose(); await waitingRejected; expect(calls).toHaveLength(1);
  f.lease.dispose(); expect(calls[0].signal?.aborted).toBe(true);
  const replacement = f.resources.acquire(3, source), final = replacement.derive('docx', new AbortController().signal);
  expect(calls).toHaveLength(1); held.resolve({ digest: 'a'.repeat(64), file, preview: { kind: 'pdf', data: new TextEncoder().encode('%PDF') } });
  await rejected; await secondEntered.promise; await final; expect(calls).toHaveLength(2);
});
it('a new Session coordinator waits for old aborted Office physical settlement', async () => {
  const f = await fixture(), entered = deferred<void>(), held = deferred<DocumentResult>();
  const derive = vi.fn<ProductHostWorkspaces['previewDocument'] & {}>().mockImplementation(async () => {
    entered.resolve(); return held.promise;
  });
  f.host.previewDocument = derive;
  const old = f.lease.derive('docx', new AbortController().signal), oldRejected = expect(old).rejects.toThrow('obsolete'); await entered.promise;
  f.resources.dispose();
  const replacement = new FilePreviewCoordinator(server.client, 'B', f.host, f.authority); owners.push(replacement);
  const lease = replacement.acquire(2, otherSource), next = lease.derive('docx', new AbortController().signal);
  expect(derive).toHaveBeenCalledOnce();
  held.resolve({ digest: 'a'.repeat(64), file, preview: { kind: 'pdf', data: new TextEncoder().encode('%PDF') } }); await oldRejected; await next;
  expect(derive).toHaveBeenCalledTimes(2);
});

it.each(['file', 'document'] as const)('unknown %s settlement fails subsequent admission closed without retry', async kind => {
  const f = await fixture();
  const failure = new WorkspaceHostError('lost settlement', `${kind}_settlement_unknown`);
  if (kind === 'file') {
    const read = vi.fn().mockRejectedValue(failure); f.host.readDelivery = read;
    await expect(f.lease.load()).rejects.toBe(failure);
    await expect(f.lease.load()).rejects.toThrow('File settlement unavailable'); expect(read).toHaveBeenCalledOnce();
  } else {
    const derive = vi.fn().mockRejectedValue(failure); f.host.previewDocument = derive;
    await expect(f.lease.derive('docx', new AbortController().signal)).rejects.toBe(failure);
    await expect(f.lease.derive('docx', new AbortController().signal)).rejects.toThrow('converter_unavailable'); expect(derive).toHaveBeenCalledOnce();
  }
});
it('active derivation reserves a shared read slot so pane and Download transfers cannot race Host reauthorization', async () => {
  const f = await fixture(), entered = deferred<void>(), held = deferred<DocumentResult>();
  f.host.previewDocument = async () => { entered.resolve(); return held.promise; };
  const derive = f.lease.derive('docx', new AbortController().signal); await entered.promise;
  const second = f.resources.acquire(2, source), read = second.load();
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  try {
    const download = f.resources.download(source, file.name);
    expect(f.calls).toHaveLength(1); // derivation reserves the other Host/native read permit
    f.reply(0); await read; expect(f.calls).toHaveLength(2);
    f.reply(1); await download;
    held.resolve({ digest: 'a'.repeat(64), file, preview: { kind: 'pdf', data: new TextEncoder().encode('%PDF') } }); await derive;
    expect(click).toHaveBeenCalledOnce();
  } finally { held.resolve({ digest: 'a'.repeat(64), file, preview: { kind: 'pdf', data: new TextEncoder().encode('%PDF') } }); click.mockRestore(); }
});

it.each([
  ['converter', 'session_file'], ['file', 'session_file'], ['document', 'session_file'],
  ['converter', 'artifact'], ['file', 'artifact'], ['document', 'artifact'],
] as const)('%s uncertainty from %s derivation closes only its physical domains across Sessions and authority replacement', async (kind, sourceKind) => {
  const f = await fixture(), failure = new WorkspaceHostError('unknown physical settlement', `${kind}_settlement_unknown`);
  const derive = vi.fn<NonNullable<ProductHostWorkspaces['previewDocument']>>().mockRejectedValue(failure);
  f.host.previewDocument = derive;
  const signal = new AbortController().signal;
  const artifactSource = { kind: 'artifact' as const, id: 'managed-office' };
  const artifactLease = f.resources.acquire(2, artifactSource);
  const derivingLease = sourceKind === 'artifact' ? artifactLease : f.lease;
  await expect(derivingLease.derive('docx', signal)).rejects.toBe(failure);
  await expect(derivingLease.derive('docx', signal)).rejects.toThrow('converter_unavailable');
  expect(derive).toHaveBeenCalledOnce();
  const next = new FilePreviewCoordinator(server.client, 'B', f.host, f.authority); owners.push(next);
  const lease = next.acquire(2, otherSource);
  await expect(lease.derive('docx', signal)).rejects.toThrow('converter_unavailable');
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  try {
    if (kind === 'converter') {
      const read = f.lease.load(); expect(f.calls).toHaveLength(1); f.reply(0); await read;
      const download = f.resources.download(source, file.name); expect(f.calls).toHaveLength(2); f.reply(1); await download;
      const otherSession = lease.load(); expect(f.calls).toHaveLength(3); f.reply(2); await otherSession;
      expect(click).toHaveBeenCalledOnce(); expect(create).toHaveBeenCalledTimes(3);
      expect(revoke).toHaveBeenCalledExactlyOnceWith('blob:2');
    } else {
      await expect(f.lease.load()).rejects.toThrow('File settlement unavailable');
      await expect(f.resources.download(source, file.name)).rejects.toThrow('File settlement unavailable');
      await expect(lease.load()).rejects.toThrow('File settlement unavailable');
      await expect(next.download(otherSource, file.name)).rejects.toThrow('File settlement unavailable');
      expect(f.calls).toHaveLength(0); expect(create).not.toHaveBeenCalled(); expect(click).not.toHaveBeenCalled();
    }
    expect(derive).toHaveBeenCalledOnce();
    expect(derive.mock.calls[0][1].source).toEqual(sourceKind === 'artifact'
      ? { kind: 'artifact', artifact_id: artifactSource.id }
      : { kind: 'session_file', locator: source.locator, delivery_index: source.index, agent_id:undefined });
    const otherArtifact = next.acquire(3, artifactSource);
    await expect(otherArtifact.derive('docx', signal)).rejects.toThrow('converter_unavailable');
    const privateCalls = f.calls.length, urls = create.mock.calls.length, clicks = click.mock.calls.length;
    server.held.add('artifact/read');
    for (const [index, coordinator, artifact] of [[0, f.resources, artifactLease], [1, next, otherArtifact]] as const) {
      const read = artifact.loadPage(1, new AbortController().signal), request = await server.waitFor('artifact/read', index * 2 + 1);
      expect(request.params).toEqual({ target: server.client.target(index === 0 ? 'A' : 'B'), artifact_id: artifactSource.id });
      server.socket.success(request, { type: 'artifact_bytes', data: btoa(bytes) });
      expect((await read).text).toBe(bytes);
      const download = coordinator.download(artifactSource, 'original.docx');
      const downloadRequest = await server.waitFor('artifact/read', index * 2 + 2);
      expect(downloadRequest.params).toEqual(request.params);
      server.socket.success(downloadRequest, { type: 'artifact_bytes', data: btoa(bytes) });
      await download;
    }
    expect(server.requests.map(item => item.request).filter(request => request.method === 'artifact/read')).toHaveLength(4);
    expect(f.calls).toHaveLength(privateCalls); expect(derive).toHaveBeenCalledOnce();
    expect(click).toHaveBeenCalledTimes(clicks + 2); expect(create).toHaveBeenCalledTimes(urls + 2);
    expect(revoke.mock.calls.map(([url]) => url)).toEqual(kind === 'converter'
      ? ['blob:2', 'blob:4', 'blob:5'] : ['blob:1', 'blob:2']);
    await f.replaceHost();
    await expect(artifactLease.load()).rejects.toThrow('Obsolete');
    await expect(otherArtifact.load()).rejects.toThrow('Obsolete');
    await expect(lease.load()).rejects.toThrow('Obsolete');
    await expect(f.lease.derive('docx', signal)).rejects.toThrow('obsolete');
    const fresh = new FilePreviewCoordinator(server.client, 'B', f.host, f.authority); owners.push(fresh);
    const freshLease = fresh.acquire(3, otherSource), count = f.calls.length, read = freshLease.load();
    expect(f.calls).toHaveLength(count + 1); f.reply(count); await read;
    derive.mockImplementation(async () => ({ digest: 'a'.repeat(64), file, preview: { kind: 'pdf', data: new TextEncoder().encode('%PDF') } }));
    await freshLease.derive('docx', signal);
    const freshArtifact = fresh.acquire(4, artifactSource);
    await freshArtifact.derive('docx', signal); expect(derive).toHaveBeenCalledTimes(3);
    expect(derive.mock.calls[2][1].source).toEqual({ kind: 'artifact', artifact_id: artifactSource.id });
    expect(f.calls).toHaveLength(count + 1);
    expect(create).toHaveBeenCalledTimes(urls + 3);
    expect(server.requests.map(item => item.request).filter(request => request.method === 'artifact/read')).toHaveLength(4);
    owners.forEach(owner => owner.dispose());
    expect(revoke.mock.calls.map(([url]) => url).sort()).toEqual(create.mock.results.map(result => result.value).sort());
  } finally { click.mockRestore(); }
});

it('a failed private raw read leaves the independent two-transfer Artifact owner usable and bounded', async () => {
  const f = await fixture(), failure = new WorkspaceHostError('private read retirement unknown', 'file_settlement_unknown');
  const read = f.lease.load(), rejected = expect(read).rejects.toBe(failure);
  f.calls[0].response.reject(failure); await rejected;
  await expect(f.lease.load()).rejects.toThrow('File settlement unavailable');
  await expect(f.resources.download(source, file.name)).rejects.toThrow('File settlement unavailable');
  const derive = vi.fn<NonNullable<ProductHostWorkspaces['previewDocument']>>(); f.host.previewDocument = derive;
  const other = new FilePreviewCoordinator(server.client, 'B', f.host, f.authority); owners.push(other);
  const otherLease = other.acquire(1, otherSource), signal = new AbortController().signal;
  await expect(otherLease.load()).rejects.toThrow('File settlement unavailable');
  await expect(other.download(otherSource, file.name)).rejects.toThrow('File settlement unavailable');
  await expect(otherLease.derive('docx', signal)).rejects.toThrow('File settlement unavailable');
  f.lease.dispose();
  const artifact = { kind: 'artifact' as const, id: 'managed-office' };
  const first = f.resources.acquire(2, artifact), second = f.resources.acquire(3, artifact);
  await expect(first.derive('docx', signal)).rejects.toThrow('File settlement unavailable');
  server.held.add('artifact/read');
  const firstRead = first.load(), secondRead = second.load();
  await server.waitFor('artifact/read', 2);
  const requests = server.requests.map(item => item.request).filter(request => request.method === 'artifact/read');
  expect(requests).toHaveLength(2);
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  try {
    await expect(f.resources.download(artifact, 'original.docx')).rejects.toThrow('Artifact capacity');
    expect(server.requests.map(item => item.request).filter(request => request.method === 'artifact/read')).toHaveLength(2);
    server.socket.success(requests[0], { type: 'artifact_bytes', data: btoa(bytes) }); await firstRead;
    const download = f.resources.download(artifact, 'original.docx'), request = await server.waitFor('artifact/read', 3);
    expect(request.params).toEqual({ target: server.client.target('A'), artifact_id: artifact.id });
    server.socket.success(request, { type: 'artifact_bytes', data: btoa(bytes) }); await download;
    server.socket.success(requests[1], { type: 'artifact_bytes', data: btoa(bytes) }); await secondRead;
    expect(f.calls).toHaveLength(1); expect(derive).not.toHaveBeenCalled();
    expect(server.requests.map(item => item.request).filter(request => request.method === 'artifact/read')).toHaveLength(3);
    expect(click).toHaveBeenCalledOnce(); expect(create).toHaveBeenCalledTimes(3);
    expect(revoke).toHaveBeenCalledExactlyOnceWith('blob:2');
    f.resources.dispose(); expect(revoke.mock.calls.map(([url]) => url).sort()).toEqual(['blob:1', 'blob:2', 'blob:3']);
  } finally { click.mockRestore(); }
});

it('foreign locator source CID is refused before lookup and a retired Child owner cannot repaint or share a successor preview',async()=>{
 const f=await fixture(),foreign={...source,conversationId:'conversation-B'};
 const foreignLease=f.resources.acquire(7,foreign);await expect(foreignLease.load()).rejects.toThrow('Obsolete');expect(f.calls).toHaveLength(0);foreignLease.dispose();
 let owned=true;
 const child={...source,conversationId:'conversation-child',agentId:'child-agent',owned:()=>owned};
 const work=f.resources.acquire(8,child).load(),rejected=expect(work).rejects.toThrow('Obsolete');expect(f.calls).toHaveLength(1);
 owned=false;f.reply(0);await rejected;expect(create).not.toHaveBeenCalled();
 expect(samePreviewSource(child,{...child,owned:()=>true})).toBe(false);
});

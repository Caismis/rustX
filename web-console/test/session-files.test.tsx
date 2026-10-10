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
import type { SessionFileReference, ToolExecutionResult } from '../../protocol/app-server/v41';
import { Server } from './fixture';
const file: SessionFileReference = { scope: { conversation_id: 'original', device: '1', inode: '2' }, path: 'sub/报告 file.md', name: '报告 file.md', description: 'Explicit report', mime_type: 'text/markdown' };
const source: PreviewSource = { kind: 'session_file', messageId: 'canonical-tool', index: 0, file };
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
async function fixture() {
  await server.attached('A', 'B');
  let hostId = 'host-1';
  const calls: { read: DeliveryRead; signal?: AbortSignal; response: ReturnType<typeof deferred<DeliveryBytes>> }[] = [];
  const host: ProductHostWorkspaces = { ...server.workspaceHost,
    listWorkspaces: async () => ({ ...await server.workspaceHost.listWorkspaces(), authorityId: hostId }),
    readDelivery: async (_scope, read, signal) => { const response = deferred<DeliveryBytes>(); calls.push({ read, signal, response }); return response.promise; },
  };
  const authority = new WorkspaceAuthority(host); await authority.observe();
  const resources = new FilePreviewCoordinator(server.client, 'A', host, authority); owners.push(resources);
  const lease = resources.acquire(1, source);
  const reply = (at: number, data = btoa(bytes), delivered = file) => calls[at].response.resolve({ file: delivered, data });
  return { resources, lease, authority, host, calls, reply, replaceHost: async () => { hostId = 'host-2'; await authority.observe(); } };
}
it('one authorized original read supplies Markdown; Download invokes its distinct intent', async () => {
  const f = await fixture(), onDownload = vi.fn(), before = server.requests.length;
  const ui = render(<ArtifactPreview artifact={{ source, name: file.name, image: false, mimeType: file.mime_type }} resources={f.lease} {...viewState} onDownload={onDownload}/>);
  expect(f.calls[0].read).toEqual({ target: server.client.target('A'), message_id: 'canonical-tool', delivery_index: 0 });
  expect(JSON.stringify(f.calls[0].read)).not.toContain(file.path);
  await act(async () => f.reply(0));
  expect(ui.getByRole('heading', { name: 'Report' })).toBeTruthy(); expect(ui.getByText('Native').tagName).toBe('STRONG');
  fireEvent.click(ui.getByRole('button', { name: 'Download artifact' })); expect(onDownload).toHaveBeenCalledOnce();
  expect(create.mock.calls[0][0].size).toBe(bytes.length); expect(server.requests).toHaveLength(before);
  ui.unmount(); f.resources.dispose(); expect(revoke).toHaveBeenCalledExactlyOnceWith('blob:1');
});
it('child delivery coordinates carry native ownership and cannot share a root or sibling preview occurrence', async () => {
  const f = await fixture();
  const child = { ...source, agentId: 'child-agent' };
  expect(samePreviewSource(source, child)).toBe(false);
  expect(samePreviewSource(child, { ...child, agentId: 'other-child' })).toBe(false);
  const lease = f.resources.acquire(2, child);
  const work = lease.load(file.mime_type);
  expect(f.calls[0].read).toMatchObject({ agent_id: 'child-agent', message_id: 'canonical-tool', delivery_index: 0 });
  f.reply(0); expect(await work).toMatchObject({ text: bytes });
});
it('a child artifact preview uses the native Agent namespace, even if a root artifact has the same id', async () => {
  const f = await fixture();
  server.handlers.set('agent/artifactRead', () => ({type:'artifact_bytes',data:btoa(bytes)}));
  const lease = f.resources.acquire(2, {kind:'artifact',id:'artifact_1',agentId:'child-agent'});
  expect(await lease.load('text/plain')).toMatchObject({text:bytes});
  const request = await server.waitFor('agent/artifactRead',1);
  expect(request.params).toEqual({target:server.client.target('A'),agent_id:'child-agent',artifact_id:'artifact_1'});
  expect(server.requests.filter(row => row.request.method === 'artifact/read')).toHaveLength(0);
});
it.each(['text/plain', 'text/markdown'] as const)('managed %s remains artifact/read with its exact target and original bytes', async mime => {
  const f = await fixture(); server.held.add('artifact/read');
  const lease = f.resources.acquire(2, { kind: 'artifact', id: 'immutable-artifact' });
  const work = lease.load(mime), request = await server.waitFor('artifact/read', 1);
  expect(request.params).toEqual({ target: server.client.target('A'), artifact_id: 'immutable-artifact' });
  server.socket.success(request, { type: 'artifact_bytes', data: btoa(bytes) });
  expect(await work).toEqual({ text: bytes, url: 'blob:1' }); expect(f.calls).toHaveLength(0);
  f.resources.dispose(); expect(revoke).toHaveBeenCalledOnce();
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
  expect(create).toHaveBeenCalledOnce(); ui.unmount(); f.resources.dispose(); expect(revoke).toHaveBeenCalledOnce();
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
it('the Session policy admits 300 KiB and rejects >512 KiB before creating a Blob', async () => {
  const f = await fixture(), work = f.lease.load('text/plain'); f.reply(0, btoa('x'.repeat(300 * 1024)));
  expect((await work).text).toHaveLength(300 * 1024); f.lease.release('blob:1');
  const oversized = f.lease.load(); f.reply(1, 'A'.repeat(Math.ceil((SESSION_FILE_MAX_BYTES + 3) / 3) * 4));
  await expect(oversized).rejects.toThrow('512 KiB'); expect(create).toHaveBeenCalledOnce();
});
it('UTF-8/image decode failures retain bounded original bytes; failed reads allocate no URL and permit explicit retry', async () => {
  const f = await fixture(), invalid = f.lease.load('text/plain'); f.reply(0, '/w==');
  expect(await invalid).toEqual({ url: 'blob:1', error: 'File is not valid UTF-8' }); f.lease.release('blob:1');
  const image = f.lease.load('image/png', true); f.reply(1, 'aGk=');
  expect(await image).toMatchObject({ url: 'blob:2', error: expect.any(String) }); f.lease.release('blob:2');
  const failed = f.lease.load(); f.calls[2].response.reject(new Error('file deleted'));
  await expect(failed).rejects.toThrow('deleted'); expect(create).toHaveBeenCalledTimes(2);
  const retry = f.lease.load(); f.reply(3); await retry; f.resources.dispose(); expect(revoke).toHaveBeenCalledTimes(3);
});
it('Markdown HTML, executable URLs and embedded images remain inert', async () => {
  const f = await fixture(), ui = render(<ArtifactPreview artifact={{ source, name: file.name, image: false, mimeType: file.mime_type }} resources={f.lease} {...viewState}/>);
  await act(async () => f.reply(0, btoa('<script>globalThis.PWNED=1</script>\n\n<img src="/private" onerror="alert(1)">\n\n[x](javascript:alert(1))\n\n![local](file:///etc/passwd) ![remote](https://example.org/track)')));
  expect(ui.container.querySelector('[data-preview-scroll="body"]')!.querySelector('script,img,iframe,object,svg')).toBeNull();
  expect([...ui.container.querySelectorAll('a')].some(a => a.href.startsWith('javascript:'))).toBe(false);
  expect((globalThis as { PWNED?: number }).PWNED).toBeUndefined();
});
it('typed committed cards distinguish Preview and Download; prose/JSON/failed facts cannot grant access', () => {
  const openPreview = vi.fn(), download = vi.fn(), intents = { openPreview, download };
  const result: ToolExecutionResult = { status: { type: 'success' }, duration_ms: 0, content: [{ type: 'json', value: { deliveries: [file], path: file.path } }] };
  const ui = render(<PreviewContext.Provider value={intents}><ToolDeliveries messageId="canonical-tool" result={result}/></PreviewContext.Provider>);
  expect(ui.queryAllByRole('button')).toHaveLength(0);
  ui.rerender(<PreviewContext.Provider value={intents}><ToolDeliveries messageId="canonical-tool" result={{ ...result, deliveries: [file] }}/></PreviewContext.Provider>);
  fireEvent.click(ui.getByRole('button', { name: `Preview ${file.name} in sidebar` }));
  expect(openPreview).toHaveBeenCalledWith(expect.objectContaining({ source, name: file.name }));
  fireEvent.click(ui.getByRole('button', { name: `Download ${file.name}` }));
  expect(download).toHaveBeenCalledWith(expect.objectContaining({ source, name: file.name })); expect(openPreview).toHaveBeenCalledOnce();
  ui.rerender(<PreviewContext.Provider value={intents}><ToolDeliveries messageId="canonical-tool" result={{ ...result, deliveries: [file], status: { type: 'failed', error: 'bad declaration' } }}/></PreviewContext.Provider>);
  expect(ui.queryAllByRole('button')).toHaveLength(0);
});
it('typed source equality preserves declarations, delivery indexes, namespaces and managed Artifact distinction', () => {
  expect(samePreviewSource(source, { ...source, file: { ...file } })).toBe(true);
  for (const other of [
    { ...source, messageId: 'another-declaration' }, { ...source, index: 1 },
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
  const f = await fixture(), entered = deferred<void>(), held = deferred<DocumentResult>(); let request!: DocumentRequest;
  f.host.previewDocument = async (_scope, value) => { request = value; entered.resolve(); return held.promise; };
  const controller = new AbortController(), work = f.lease.derive('xlsx', new Uint8Array([1]), controller.signal), rejected = expect(work).rejects.toThrow('obsolete');
  await entered.promise;
  if (change === 'Host') await f.replaceHost();
  if (change === 'attachment') { await server.client.release('A'); await server.client.attach('A'); }
  if (change === 'authority') server.client.disconnect();
  if (change === 'close') f.resources.dispose();
  if (change === 'abort') controller.abort();
  held.resolve({ digest: request.digest, file, preview: { kind: 'xlsx', sheets: [] } });
  await rejected; expect(create).not.toHaveBeenCalled();
});
it('two visible Office intents serialize; cancel waiting demand immediately, active demand holds until settlement', async () => {
  const f = await fixture(), entered = deferred<void>(), secondEntered = deferred<void>(), held = deferred<DocumentResult>();
  const calls: { request: DocumentRequest; signal?: AbortSignal }[] = [];
  f.host.previewDocument = async (_scope, request, signal) => {
    calls.push({ request, signal });
    if (calls.length === 1) { entered.resolve(); return held.promise; }
    secondEntered.resolve(); return { digest: request.digest, file, preview: { kind: 'xlsx', sheets: [] } };
  };
  const first = f.lease.derive('docx', new Uint8Array([1]), new AbortController().signal), rejected = expect(first).rejects.toThrow('obsolete');
  await entered.promise;
  const waiting = f.resources.acquire(2, source), next = waiting.derive('pptx', new Uint8Array([1]), new AbortController().signal);
  const waitingRejected = expect(next).rejects.toThrow('Obsolete'); waiting.dispose(); await waitingRejected; expect(calls).toHaveLength(1);
  f.lease.dispose(); expect(calls[0].signal?.aborted).toBe(true);
  const replacement = f.resources.acquire(3, source), final = replacement.derive('docx', new Uint8Array([1]), new AbortController().signal);
  expect(calls).toHaveLength(1); held.resolve({ digest: calls[0].request.digest, file, preview: { kind: 'xlsx', sheets: [] } });
  await rejected; await secondEntered.promise; await final; expect(calls).toHaveLength(2);
});
it('a new Session coordinator waits for old aborted Office physical settlement', async () => {
  const f = await fixture(), entered = deferred<void>(), held = deferred<DocumentResult>(); let request!: DocumentRequest;
  const derive = vi.fn<ProductHostWorkspaces['previewDocument'] & {}>().mockImplementation(async (_scope, value) => {
    request = value; entered.resolve(); return held.promise;
  });
  f.host.previewDocument = derive;
  const old = f.lease.derive('docx', new Uint8Array([1]), new AbortController().signal), oldRejected = expect(old).rejects.toThrow('obsolete'); await entered.promise;
  f.resources.dispose();
  const replacement = new FilePreviewCoordinator(server.client, 'B', f.host, f.authority); owners.push(replacement);
  const lease = replacement.acquire(2, source), next = lease.derive('docx', new Uint8Array([1]), new AbortController().signal);
  expect(derive).toHaveBeenCalledOnce();
  held.resolve({ digest: request.digest, file, preview: { kind: 'xlsx', sheets: [] } }); await oldRejected; await next;
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
    await expect(f.lease.derive('docx', new Uint8Array([1]), new AbortController().signal)).rejects.toBe(failure);
    await expect(f.lease.derive('docx', new Uint8Array([1]), new AbortController().signal)).rejects.toThrow('converter_unavailable'); expect(derive).toHaveBeenCalledOnce();
  }
});
it('active derivation reserves a shared read slot so pane and Download transfers cannot race Host reauthorization', async () => {
  const f = await fixture(), entered = deferred<void>(), held = deferred<DocumentResult>(); let request!: DocumentRequest;
  f.host.previewDocument = async (_scope, value) => { request = value; entered.resolve(); return held.promise; };
  const derive = f.lease.derive('docx', new Uint8Array([1]), new AbortController().signal); await entered.promise;
  const second = f.resources.acquire(2, source), read = second.load();
  const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  try {
    const download = f.resources.download(source, file.name);
    expect(f.calls).toHaveLength(1); // derivation reserves the other Host/native read permit
    f.reply(0); await read; expect(f.calls).toHaveLength(2);
    f.reply(1); await download;
    held.resolve({ digest: request.digest, file, preview: { kind: 'xlsx', sheets: [] } }); await derive;
    expect(click).toHaveBeenCalledOnce();
  } finally { held.resolve({ digest: request.digest, file, preview: { kind: 'xlsx', sheets: [] } }); click.mockRestore(); }
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
  await expect(derivingLease.derive('docx', new Uint8Array([1]), signal)).rejects.toBe(failure);
  await expect(derivingLease.derive('docx', new Uint8Array([1]), signal)).rejects.toThrow('converter_unavailable');
  expect(derive).toHaveBeenCalledOnce();
  const next = new FilePreviewCoordinator(server.client, 'B', f.host, f.authority); owners.push(next);
  const lease = next.acquire(2, source);
  await expect(lease.derive('docx', new Uint8Array([1]), signal)).rejects.toThrow('converter_unavailable');
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
      await expect(next.download(source, file.name)).rejects.toThrow('File settlement unavailable');
      expect(f.calls).toHaveLength(0); expect(create).not.toHaveBeenCalled(); expect(click).not.toHaveBeenCalled();
    }
    expect(derive).toHaveBeenCalledOnce();
    expect(derive.mock.calls[0][1].source).toEqual(sourceKind === 'artifact'
      ? { kind: 'artifact', artifact_id: artifactSource.id }
      : { kind: 'session_file', message_id: source.messageId, delivery_index: source.index });
    const otherArtifact = next.acquire(3, artifactSource);
    await expect(otherArtifact.derive('docx', new Uint8Array([1]), signal)).rejects.toThrow('converter_unavailable');
    const privateCalls = f.calls.length, urls = create.mock.calls.length, clicks = click.mock.calls.length;
    server.held.add('artifact/read');
    for (const [index, coordinator, artifact] of [[0, f.resources, artifactLease], [1, next, otherArtifact]] as const) {
      const read = artifact.load('text/plain'), request = await server.waitFor('artifact/read', index * 2 + 1);
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
    expect(click).toHaveBeenCalledTimes(clicks + 2); expect(create).toHaveBeenCalledTimes(urls + 4);
    expect(revoke.mock.calls.map(([url]) => url)).toEqual(kind === 'converter'
      ? ['blob:2', 'blob:5', 'blob:7'] : ['blob:2', 'blob:4']);
    await f.replaceHost();
    await expect(artifactLease.load()).rejects.toThrow('Obsolete');
    await expect(otherArtifact.load()).rejects.toThrow('Obsolete');
    await expect(lease.load()).rejects.toThrow('Obsolete');
    await expect(f.lease.derive('docx', new Uint8Array([1]), signal)).rejects.toThrow('obsolete');
    const fresh = new FilePreviewCoordinator(server.client, 'B', f.host, f.authority); owners.push(fresh);
    const freshLease = fresh.acquire(3, source), count = f.calls.length, read = freshLease.load();
    expect(f.calls).toHaveLength(count + 1); f.reply(count); await read;
    derive.mockImplementation(async (_scope, request) => ({ digest: request.digest, file, preview: { kind: 'xlsx', sheets: [] } }));
    await freshLease.derive('xlsx', new Uint8Array([1]), signal);
    const freshArtifact = fresh.acquire(4, artifactSource);
    await freshArtifact.derive('xlsx', new Uint8Array([1]), signal); expect(derive).toHaveBeenCalledTimes(3);
    expect(derive.mock.calls[2][1].source).toEqual({ kind: 'artifact', artifact_id: artifactSource.id });
    expect(f.calls).toHaveLength(count + 1);
    expect(create).toHaveBeenCalledTimes(urls + 5);
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
  const otherLease = other.acquire(1, source), signal = new AbortController().signal;
  await expect(otherLease.load()).rejects.toThrow('File settlement unavailable');
  await expect(other.download(source, file.name)).rejects.toThrow('File settlement unavailable');
  await expect(otherLease.derive('docx', new Uint8Array([1]), signal)).rejects.toThrow('File settlement unavailable');
  f.lease.dispose();
  const artifact = { kind: 'artifact' as const, id: 'managed-office' };
  const first = f.resources.acquire(2, artifact), second = f.resources.acquire(3, artifact);
  await expect(first.derive('docx', new Uint8Array([1]), signal)).rejects.toThrow('File settlement unavailable');
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

import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { FilePreviewResources, SESSION_FILE_MAX_BYTES, type PreviewSource } from '../src/client/session-files';
import { ArtifactPreview, PreviewContext } from '../src/app/components/ArtifactPreview';
import { ToolDeliveries } from '../src/app/components/Artifact';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { validateRaster, RASTER_MAX_PIXELS } from '../src/client/raster';
import type { DeliveryBytes, DeliveryRead, ProductHostWorkspaces } from '../src/workspaces/host';
import type { SessionFileReference, ToolExecutionResult } from '../../protocol/app-server/v34';
import { Server } from './fixture';
const file: SessionFileReference = { scope: { conversation_id: 'original', device: '1', inode: '2' }, path: 'sub/报告 file.md', name: '报告 file.md', description: 'Explicit report', mime_type: 'text/markdown' };
const source: PreviewSource = { kind: 'session_file', messageId: 'canonical-tool', index: 0, file };
const bytes = '# Report\r\n\r\n**Native** bytes\r\n';
const encoded = (value: string) => btoa(value);
function deferred<T>() { let resolve!: (value: T) => void, reject!: (cause: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
let server: Server;
const create = vi.fn<(blob: Blob) => string>(), revoke = vi.fn();
beforeEach(() => { server = new Server(); create.mockReset().mockImplementation(() => `blob:${create.mock.calls.length}`); revoke.mockReset(); vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: create, revokeObjectURL: revoke })); });
afterEach(() => { cleanup(); server.client.disconnect(); vi.unstubAllGlobals(); });
async function fixture() {
  await server.attached('A', 'B');
  let hostId = 'host-1';
  const calls: { read: DeliveryRead; signal?: AbortSignal; response: ReturnType<typeof deferred<DeliveryBytes>> }[] = [];
  const host: ProductHostWorkspaces = { ...server.workspaceHost,
    listWorkspaces: async () => ({ ...await server.workspaceHost.listWorkspaces(), authorityId: hostId }),
    readDelivery: async (_scope, read, signal) => { const response = deferred<DeliveryBytes>(); calls.push({ read, signal, response }); return response.promise; },
  };
  const authority = new WorkspaceAuthority(host); await authority.observe();
  const resources = new FilePreviewResources(server.client, 'A', host, authority);
  const reply = (at: number, data = encoded(bytes), delivered = file) => calls[at].response.resolve({ file: delivered, data });
  return { resources, authority, calls, reply, replaceHost: async () => { hostId = 'host-2'; await authority.observe(); } };
}
it('one authorized byte read supplies rendered Markdown and original-byte Download; no model request or ArtifactId', async () => {
  const f = await fixture(); const before = server.requests.length;
  const ui = render(<ArtifactPreview artifact={{ source, name: file.name, image: false, mimeType: file.mime_type }} resources={f.resources}/>);
  expect(f.calls[0].read).toEqual({ target: server.client.target('A'), message_id: 'canonical-tool', delivery_index: 0 });
  expect(JSON.stringify(f.calls[0].read)).not.toContain(file.path);
  await act(async () => f.reply(0));
  expect(ui.getByRole('heading', { name: 'Report' })).toBeTruthy();
  expect(ui.getByText('Native').tagName).toBe('STRONG');
  expect(ui.getByRole('link', { name: 'Download artifact' }).getAttribute('download')).toBe(file.name);
  expect(ui.getByRole('link').getAttribute('href')).toBe('blob:1');
  expect(create.mock.calls[0][0].size).toBe(bytes.length);
  expect(server.requests).toHaveLength(before);
  ui.unmount(); f.resources.dispose(); expect(revoke).toHaveBeenCalledExactlyOnceWith('blob:1');
});
it.each(['text/plain', 'text/markdown'] as const)('managed %s uses artifact/read once and retains a download URL', async mime => {
  const f = await fixture(); server.held.add('artifact/read');
  const work = f.resources.load({ kind: 'artifact', id: 'immutable-artifact' }, mime);
  const request = await server.waitFor('artifact/read', 1);
  expect(request.params).toEqual({ target: server.client.target('A'), artifact_id: 'immutable-artifact' });
  server.socket.success(request, { type: 'artifact_bytes', data: encoded(bytes) });
  expect(await work).toEqual({ text: bytes, url: 'blob:1' });
  expect(f.calls).toHaveLength(0); f.resources.dispose(); expect(revoke).toHaveBeenCalledOnce();
});
it.each(['Host', 'attachment', 'authority', 'close', 'abort'] as const)('gated obsolete %s response cannot allocate bytes or a URL', async change => {
  const f = await fixture(); const controller = new AbortController();
  const work = f.resources.load(source, file.mime_type, false, controller.signal);
  const rejected = expect(work).rejects.toThrow(/Obsolete/);
  if (change === 'Host') await f.replaceHost();
  if (change === 'attachment') { await server.client.release('A'); await server.client.attach('A'); }
  if (change === 'authority') server.client.disconnect();
  if (change === 'close') f.resources.dispose();
  if (change === 'abort') controller.abort();
  f.reply(0); await rejected; expect(create).not.toHaveBeenCalled(); f.resources.dispose(); expect(revoke).not.toHaveBeenCalled();
});
it.each(['bytes', 'error'] as const)('selecting B before A completes ignores stale A %s and reclaims B on close', async completion => {
  const f = await fixture(); const a = { source, name: file.name, image: false, mimeType: file.mime_type };
  const bFile = { ...file, name: 'B.txt', path: 'B.txt', mime_type: 'text/plain' };
  const b = { source: { ...source, index: 1, file: bFile } as PreviewSource, name: bFile.name, image: false, mimeType: bFile.mime_type };
  const ui = render(<ArtifactPreview artifact={a} resources={f.resources}/>);
  ui.rerender(<ArtifactPreview artifact={b} resources={f.resources}/>);
  expect(f.calls[0].signal?.aborted).toBe(true);
  await act(async () => f.reply(1, encoded('B current'), bFile));
  await act(async () => completion === 'bytes' ? f.reply(0) : f.calls[0].response.reject(new Error('A stale error')));
  expect(ui.getByText('B current')).toBeTruthy(); expect(ui.queryByRole('alert')).toBeNull();
  expect(create).toHaveBeenCalledOnce(); ui.unmount(); f.resources.dispose(); expect(revoke).toHaveBeenCalledOnce();
});
it('two pending reads and two retained URLs are finite; release permits retry', async () => {
  const f = await fixture(); const a = f.resources.load(source), b = f.resources.load(source);
  await expect(f.resources.load(source)).rejects.toThrow('capacity'); expect(f.calls).toHaveLength(2);
  f.reply(0); f.reply(1); const [one, two] = await Promise.all([a, b]);
  await expect(f.resources.load(source)).rejects.toThrow('capacity');
  f.resources.release(source, one.url); const retry = f.resources.load(source); f.reply(2); await retry;
  f.resources.release(source, two.url); f.resources.dispose(); expect(revoke).toHaveBeenCalledTimes(3);
});
it('the Session policy admits 300 KiB and rejects >512 KiB before creating a Blob', async () => {
  const f = await fixture(); const work = f.resources.load(source, 'text/plain'); f.reply(0, encoded('x'.repeat(300 * 1024)));
  expect((await work).text).toHaveLength(300 * 1024); f.resources.release(source, 'blob:1');
  const oversized = f.resources.load(source); f.reply(1, 'A'.repeat(Math.ceil((SESSION_FILE_MAX_BYTES + 3) / 3) * 4));
  await expect(oversized).rejects.toThrow('512 KiB'); expect(create).toHaveBeenCalledOnce(); f.resources.dispose();
});
it('UTF-8 and image decode failures retain only original-byte Download; read failure has no URL and can retry', async () => {
  const f = await fixture(); const invalid = f.resources.load(source, 'text/plain'); f.reply(0, '/w==');
  expect(await invalid).toEqual({ url: 'blob:1', error: 'File is not valid UTF-8' }); f.resources.release(source, 'blob:1');
  const image = f.resources.load(source, 'image/png', true); f.reply(1, 'aGk=');
  expect(await image).toMatchObject({ url: 'blob:2', error: expect.any(String) }); f.resources.release(source, 'blob:2');
  const failed = f.resources.load(source); f.calls[2].response.reject(new Error('file deleted'));
  await expect(failed).rejects.toThrow('deleted'); expect(create).toHaveBeenCalledTimes(2);
  const retry = f.resources.load(source); f.reply(3); await retry; f.resources.dispose(); expect(revoke).toHaveBeenCalledTimes(3);
});
it('Markdown HTML, executable URLs, local and remote embedded images are inert in the existing renderer', async () => {
  const f = await fixture(); const ui = render(<ArtifactPreview artifact={{ source, name: file.name, image: false, mimeType: file.mime_type }} resources={f.resources}/>);
  await act(async () => f.reply(0, encoded('<script>globalThis.PWNED=1</script>\n\n<img src="/private" onerror="alert(1)">\n\n[x](javascript:alert(1))\n\n![local](file:///etc/passwd) ![remote](https://example.org/track)')));
  expect(ui.container.querySelector('script,img,iframe,object,svg')).toBeNull();
  expect([...ui.container.querySelectorAll('a')].some(a => a.href.startsWith('javascript:'))).toBe(false);
  expect((globalThis as { PWNED?: number }).PWNED).toBeUndefined(); ui.unmount(); f.resources.dispose();
});
it('cards require typed successful facts; prose, basenames and arbitrary JSON remain unprivileged', () => {
  const preview = vi.fn(); const result: ToolExecutionResult = { status: { type: 'success' }, duration_ms: 0, content: [{ type: 'json', value: { deliveries: [file], path: file.path } }] };
  const ui = render(<PreviewContext.Provider value={preview}><ToolDeliveries messageId="canonical-tool" result={result}/></PreviewContext.Provider>);
  expect(ui.queryAllByRole('button')).toHaveLength(0);
  ui.rerender(<PreviewContext.Provider value={preview}><ToolDeliveries messageId="canonical-tool" result={{ ...result, deliveries: [file] }}/></PreviewContext.Provider>);
  fireEvent.click(ui.getByRole('button', { name: `Preview ${file.name}` }));
  expect(preview).toHaveBeenCalledWith(expect.objectContaining({ source, name: file.name }));
  ui.rerender(<PreviewContext.Provider value={preview}><ToolDeliveries messageId="canonical-tool" result={{ ...result, deliveries: [file], status: { type: 'failed', error: 'bad declaration' } }}/></PreviewContext.Provider>);
  expect(ui.queryAllByRole('button')).toHaveLength(0);
});
it('raster dimension inspection bounds decoded pixels and rejects active SVG before decoding', () => {
  const png = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6pAAAAABJRU5ErkJggg=='), c => c.charCodeAt(0));
  expect(() => validateRaster(png)).not.toThrow();
  new DataView(png.buffer).setUint32(16, 4096); new DataView(png.buffer).setUint32(20, 4096);
  expect(4096 ** 2).toBeGreaterThan(RASTER_MAX_PIXELS); expect(() => validateRaster(png)).toThrow('dimensions');
  expect(() => validateRaster(new TextEncoder().encode('<svg width="1" height="1"/>'))).toThrow('Unsupported');
});

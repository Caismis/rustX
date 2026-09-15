import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ArtifactResources, ARTIFACT_MAX_BYTES } from '../src/client/artifacts';
import { Artifact, ArtifactContext } from '../src/app/components/Artifact';
import { InputBar } from '../src/app/components/InputBar';
import type { SessionModelView } from '../../protocol/app-server/v2';
import { Server, snapshot } from './fixture';
let server: Server;
let sequence = 0;
const create = vi.fn(() => `blob:${++sequence}`), revoke = vi.fn();
beforeEach(() => { server = new Server(); vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: create, revokeObjectURL: revoke })); create.mockClear(); revoke.mockClear(); });
afterEach(() => { cleanup(); server.client.disconnect(); vi.unstubAllGlobals(); });
it('bounds reads and rejects stale response without creating URLs', async () => {
  await server.attached('A'); server.held.add('artifact/read');
  const resources = new ArtifactResources(server.client, 'A');
  const a = resources.read('a'); const b = resources.read('b');
  await expect(resources.read('c')).rejects.toThrow('capacity');
  const requests = server.requests.filter(item => item.request.method === 'artifact/read');
  resources.dispose();
  for (const { request } of requests) server.socket.success(request, { type: 'artifact_bytes', data: 'aGk=' });
  await expect(a).rejects.toThrow('Obsolete'); await expect(b).rejects.toThrow('Obsolete');
  expect(create).not.toHaveBeenCalled();
});
it('oversized payload is rejected before decoding and URLs are disposed once', async () => {
  await server.attached('A'); server.held.add('artifact/read');
  const resources = new ArtifactResources(server.client, 'A');
  const huge = resources.read('huge');
  server.socket.success(server.requests.at(-1)!.request, { type: 'artifact_bytes', data: 'A'.repeat(ARTIFACT_MAX_BYTES * 2) });
  await expect(huge).rejects.toThrow('256 KiB');
  const work = resources.read('valid'); server.socket.success(server.requests.at(-1)!.request, { type: 'artifact_bytes', data: 'aGk=' });
  const url = await work; resources.release(url); resources.dispose();
  expect(create).toHaveBeenCalledOnce(); expect(revoke).toHaveBeenCalledExactlyOnceWith(url);
});
it('decode failure does not reload and repeated mount/unmount releases every URL', async () => {
  await server.attached('A'); server.held.add('artifact/read');
  const resources = new ArtifactResources(server.client, 'A');
  for (let i = 0; i < 3; i++) {
    const ui = render(<ArtifactContext.Provider value={resources}><Artifact id="artifact_1" name="image" image /></ArtifactContext.Provider>);
    fireEvent.click(ui.getByRole('button', { name: 'Load attachment' }));
    await act(async () => { server.socket.success(server.requests.at(-1)!.request, { type: 'artifact_bytes', data: 'aGk=' }); });
    fireEvent.error(ui.getByAltText('image'));
    expect(ui.getByRole('button', { name: 'Retry' })).toBeTruthy();
    ui.unmount();
  }
  resources.dispose();
  expect(create).toHaveBeenCalledTimes(3); expect(revoke).toHaveBeenCalledTimes(3);
  expect(server.requests.filter(item => item.request.method === 'artifact/read')).toHaveLength(3);
});
it('mixed draft order and failed admission retain text and attachments, with deterministic URL cleanup', async () => {
  const send = vi.fn(async () => false);
  const ui = render(<InputBar disabled={false} busy={false} active={false} onSend={send} onCancel={() => {}} />);
  fireEvent.change(ui.getByLabelText('Message'), { target: { value: 'keep me' } });
  const image = new File(['png'], 'first.png', { type: 'image/png' });
  const file = new File(['text'], 'second.txt', { type: 'text/plain' });
  fireEvent.change(ui.getByLabelText('Attach files'), { target: { files: [image, file] } });
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'Send' })));
  expect(send).toHaveBeenCalledWith('keep me', false, [image, file]);
  expect((ui.getByLabelText('Message') as HTMLTextAreaElement).value).toBe('keep me');
  expect(ui.getByRole('button', { name: 'Remove second.txt' })).toBeTruthy();
  ui.unmount(); expect(revoke).toHaveBeenCalledTimes(1);
});

const model = (supported: boolean): SessionModelView => {
  const capabilities = { inputModalities: supported ? ['text', 'image', 'file'] as const : ['text'] as const, outputModalities: ['text'] as const, toolCalls: true, reasoning: false };
  const caps = { ...capabilities, inputModalities: [...capabilities.inputModalities], outputModalities: [...capabilities.outputModalities] };
  return { configured: { model: 'fixture/model' }, summary: { mode: 'session' }, effective: { model: 'fixture/model', protocol: 'openai_chat_completions', contextWindow: 128000, modelMaxOutputTokens: 4096, maxOutputTokens: 4096, reasoningEnabled: false, capabilities: caps, declaredCapabilities: caps } };
};
it('effective modality refusal sends neither upload nor turn', async () => {
  await server.attached('A'); server.held.add('session/snapshot');
  const work = server.client.send('A', 'keep text', false, [new File(['x'], 'image.png', { type: 'image/png' })]);
  server.socket.success(server.requests.at(-1)!.request, { type: 'snapshot', cursor: '1', snapshot: { ...snapshot(), model: model(false) } });
  await expect(work).rejects.toThrow('does not support');
  expect(server.requests.some(item => ['artifact/upload', 'turn/start'].includes(item.request.method))).toBe(false);
});
it('supported draft upload retains mixed order and sends one typed content sequence', async () => {
  await server.attached('A'); server.held.add('session/snapshot'); server.held.add('artifact/upload');
  const files = [new File(['x'], 'first.png', { type: 'image/png' }), new File(['y'], 'second.txt', { type: 'text/plain' })];
  for (const file of files) Object.defineProperty(file, 'arrayBuffer', { value: async () => new Uint8Array([1]).buffer });
  const work = server.client.send('A', 'text', false, files);
  server.socket.success(server.requests.at(-1)!.request, { type: 'snapshot', cursor: '1', snapshot: { ...snapshot(), model: model(true) } });
  const first = await server.waitFor('artifact/upload', 1); server.socket.success(first, { type: 'artifact_uploaded', artifact_id: 'artifact_1' });
  const second = await server.waitFor('artifact/upload', 2); server.socket.success(second, { type: 'artifact_uploaded', artifact_id: 'artifact_2' });
  await work;
  const turns = server.requests.filter(item => item.request.method === 'turn/start');
  expect(turns).toHaveLength(1);
  expect(turns[0].request.params).toMatchObject({ content: [{ type: 'text', text: 'text' }, { type: 'image', artifact_id: 'artifact_1' }, { type: 'file', artifact_id: 'artifact_2' }] });
  expect(server.client.getSnapshot().views.A.snapshot?.messages).toEqual([]);
});
it('a missing artifact can be retried explicitly without retaining failed bytes', async () => {
  await server.attached('A'); server.held.add('artifact/read');
  const resources = new ArtifactResources(server.client, 'A');
  const failed = resources.read('missing');
  server.socket.deliver({ jsonrpc: '2.0', id: server.requests.at(-1)!.request.id, error: { code: -32000, message: 'artifact unavailable' } });
  await expect(failed).rejects.toThrow('unavailable');
  const retry = resources.read('missing');
  server.socket.success(server.requests.at(-1)!.request, { type: 'artifact_bytes', data: 'eA==' });
  await retry; resources.dispose(); expect(create).toHaveBeenCalledOnce(); expect(revoke).toHaveBeenCalledOnce();
});
it('URL retention stops at sixteen and releasing one slot permits a new read', async () => {
  await server.attached('A'); server.held.add('artifact/read');
  const resources = new ArtifactResources(server.client, 'A');
  const urls: string[] = [];
  for (let i = 0; i < 16; i++) {
    const work = resources.read(`artifact_${i}`);
    server.socket.success(server.requests.at(-1)!.request, { type: 'artifact_bytes', data: 'eA==' }); urls.push(await work);
  }
  await expect(resources.read('overflow')).rejects.toThrow('capacity');
  resources.release(urls[0]);
  const next = resources.read('new'); server.socket.success(server.requests.at(-1)!.request, { type: 'artifact_bytes', data: 'eA==' }); await next;
  resources.dispose(); expect(revoke).toHaveBeenCalledTimes(17);
});
it('oversized upload drafts are rejected without any model or upload request', async () => {
  await server.attached('A'); const before = server.requests.length;
  const oversized = new File([new Uint8Array(ARTIFACT_MAX_BYTES + 1)], 'huge');
  await expect(server.client.send('A', 'keep text', false, [oversized])).rejects.toThrow('256 KiB');
  expect(server.requests).toHaveLength(before);
});
it('active Attempt preflight uses frozen capabilities even when Session changes', async () => {
  await server.attached('A'); server.held.add('session/snapshot');
  const work = server.client.send('A', 'steer draft', true, [new File(['x'], 'image.png', { type: 'image/png' })]);
  server.socket.success(server.requests.at(-1)!.request, { type: 'snapshot', cursor: '2', snapshot: {
    ...snapshot(), model: model(true), attempt: { attempt_id: 'a', turn: 1, phase: { type: 'running' }, model: { primary: model(false).effective, summary: { mode: 'session' } } },
  } });
  await expect(work).rejects.toThrow('does not support');
  expect(server.requests.some(item => ['artifact/upload', 'turn/steer'].includes(item.request.method))).toBe(false);
});
it('Blob uses safe authoritative MIME while semantic image bytes may omit MIME', async () => {
  await server.attached('A'); server.held.add('artifact/read');
  const resources = new ArtifactResources(server.client, 'A');
  for (const [mime, expected] of [['image/png', 'image/png'], ['text/html', ''], [undefined, '']] as const) {
    const read = resources.read('artifact_1', mime);
    server.socket.success(server.requests.at(-1)!.request, { type: 'artifact_bytes', data: 'aGk=' });
    const url = await read;
    expect((create.mock.calls.at(-1) as unknown as [Blob])[0].type).toBe(expected);
    resources.release(url);
  }
  resources.dispose(); expect(revoke).toHaveBeenCalledTimes(3);
});

import { AttachmentIntake } from '../src/client/uploads';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ArtifactResources, ARTIFACT_MAX_BYTES } from '../src/client/artifacts';
import { Artifact, ArtifactContext } from '../src/app/components/Artifact';
import { AgentComposer } from '../src/app/agent/AgentComposer';
import type { UploadedFile } from '../../protocol/app-server/v44';
import { Server, capabilities, TOKEN } from './fixture';
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
  const ui = render(<AgentComposer uploadPolicy={capabilities.upload_policy} disabled={false} busy={false} active={false} onUpload={async files => [completed(files[0].name, files[0].name === "first.png" ? "one" : "two")]} onSend={send} onCancel={() => {}} />);
  fireEvent.change(ui.getByLabelText('Message'), { target: { value: 'keep me' } });
  const image = new File(['png'], 'first.png', { type: 'image/png' });
  const file = new File(['text'], 'second.txt', { type: 'text/plain' });
  await act(async () => fireEvent.change(ui.getByLabelText('Attach files'), { target: { files: [image, file] } }));
  await act(async () => fireEvent.click(ui.getByRole('button', { name: 'Send' })));
  expect(send).toHaveBeenCalledWith('keep me', [completed('first.png', 'one').receipt, completed('second.txt', 'two').receipt], 'send', expect.any(Function));
  expect((ui.getByLabelText('Message') as HTMLTextAreaElement).value).toBe('keep me');
  expect(ui.getByRole('button', { name: 'Remove second.txt' })).toBeTruthy();
  ui.unmount(); expect(revoke).toHaveBeenCalledTimes(1);
});

const completed = (name: string, token: string): UploadedFile => ({ receipt: { session_id: 'A', batch_id: 'batch', token }, file: { batch_id: 'batch', name }, path: `/workspace/.agents/uploads/A/batch/${name}` });
it('native batch upload preserves order and Send references receipts without a modality preflight', async () => {
  await server.attached('A'); server.held.add('session/uploadPrepare');
  const files = [new File(['x'], 'first.png', { type: 'image/png' }), new File(['y'], 'second.txt')];
  for (const file of files) Object.defineProperty(file, 'arrayBuffer', { value: async () => new Uint8Array([1]).buffer });
  const work = server.client.upload('A', files);
  const upload = await server.waitFor('session/uploadPrepare', 1);
  const uploaded = [completed('first.png', 'one'), completed('second.txt', 'two')];
  server.handlers.set('session/uploadStatus', () => ({ type: 'upload_status', outcome: { state: 'ready', files: uploaded } }));
  server.reply(upload);
  const receipts = (await work).map(item => item.receipt);
  await server.client.send('A', 'text', receipts);
  const turns = server.requests.filter(item => item.request.method === 'turn/start');
  expect(turns).toHaveLength(1);
  expect(turns[0].request.params).toMatchObject({ content: [{ type: 'upload', ...receipts[0] }, { type: 'upload', ...receipts[1] }, { type: 'text', text: 'text' }] });
  expect(server.client.getSnapshot().views.A.snapshot?.messages).toEqual([]);
});
it('lost upload response is uncertain and reconnect does not replay the mutation', async () => {
  await server.attached('A'); server.held.add('session/uploadPrepare');
  const file = new File(['x'], 'file'); Object.defineProperty(file, 'arrayBuffer', { value: async () => new Uint8Array([1]).buffer });
  const operation = 'e'.repeat(32);
  const work = server.client.upload('A', [file], undefined, operation);
  const rejected = expect(work).rejects.toThrow();
  await server.waitFor('session/uploadPrepare', 1);
  server.client.disconnect();
  await rejected;
  expect(server.client.getSnapshot().uncertain).toMatchObject([{ uploadOperationId: operation, sessionId: 'A' }]);
  await server.connect();
  server.handlers.set('session/uploadStatus', () => ({ type: 'upload_status', outcome: { state: 'unresolved' } }));
  await server.client.uploadStatus('A', operation);
  expect(server.client.getSnapshot().uncertain).toHaveLength(1);
  const original = completed('file', 'original');
  server.handlers.set('session/uploadStatus', () => ({ type: 'upload_status', outcome: { state: 'ready', files: [original] } }));
  expect(await server.client.uploadStatus('A', operation)).toEqual({ state: 'ready', files: [original] });
  expect(server.client.getSnapshot().uncertain).toEqual([]);
  expect(server.requests.filter(item => item.request.method === 'session/uploadPrepare')).toHaveLength(1);
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
  const oversized = new File([new Uint8Array(capabilities.upload_policy.max_file_bytes + 1)], 'huge');
  await expect(server.client.upload('A', [oversized])).rejects.toThrow('native upload policy');
  expect(server.requests).toHaveLength(before);
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

it('committed durability uncertainty remains an uncertain draft without replay', async () => {
  await server.attached('A'); server.held.add('session/uploadPrepare');
  const send = vi.fn(async () => true);
  const ui = render(<AgentComposer uploadPolicy={capabilities.upload_policy} disabled={false} busy={false} active={false} onUpload={files => server.client.upload('A', files)} onSend={send} onCancel={() => {}} />);
  const file = new File(['x'], 'file.txt'); Object.defineProperty(file, 'arrayBuffer', { value: async () => new Uint8Array([1]).buffer });
  await act(async () => fireEvent.change(ui.getByLabelText('Attach files'), { target: { files: [file] } }));
  const request = await server.waitFor('session/uploadPrepare', 1);
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'Operation rejected', data: { kind: 'committed_durability_uncertain' } } }));
  expect(ui.getByText(/Upload outcome uncertain/)).toBeTruthy();
  expect((ui.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true);
  expect(send).not.toHaveBeenCalled();
  expect(server.requests.filter(item => item.request.method === 'session/uploadPrepare')).toHaveLength(1);
});


it.each(['picker', 'drop', 'paste'] as const)('retains a second %s selection during an active upload', async source => {
  let finish!: (files: UploadedFile[]) => void;
  const first = new Promise<UploadedFile[]>(resolve => { finish = resolve; });
  const upload = vi.fn().mockReturnValueOnce(first).mockResolvedValueOnce([completed('second.txt', 'two')]);
  const ui = render(<AgentComposer uploadPolicy={capabilities.upload_policy} disabled={false} busy={false} active={false} onUpload={upload} onSend={vi.fn()} onCancel={() => {}} />);
  const a = new File(['a'], 'first.txt'), b = new File(['b'], 'second.txt');
  fireEvent.change(ui.getByLabelText('Attach files'), { target: { files: [a] } });
  if (source === 'picker') fireEvent.change(ui.getByLabelText('Attach files'), { target: { files: [b] } });
  if (source === 'drop') fireEvent.drop(ui.container.firstElementChild!, { dataTransfer: { files: [b] } });
  if (source === 'paste') fireEvent.paste(ui.getByLabelText('Message'), { clipboardData: { files: [b], getData: () => '' } });
  expect(upload).toHaveBeenCalledTimes(2);
  expect(ui.getByRole('button', { name: 'Remove second.txt' })).toBeTruthy();
  await act(async () => finish([completed('first.txt', 'one')]));
  expect(upload).toHaveBeenCalledTimes(2);
  expect(ui.queryByRole('alert')).toBeNull();
});

it('exports bounded original bytes to an occurrence without retaining a second Artifact URL', async () => {
  await server.attached('A'); server.held.add('artifact/read');
  const resources = new ArtifactResources(server.client, 'A');
  const read = resources.readBytes('report', new AbortController().signal);
  server.socket.success(server.requests.at(-1)!.request, { type: 'artifact_bytes', data: btoa('<script>inert</script>') });
  expect(new TextDecoder().decode(await read)).toBe('<script>inert</script>'); expect(create).not.toHaveBeenCalled();
  const obsolete = resources.readBytes('late', new AbortController().signal); resources.dispose();
  server.socket.success(server.requests.at(-1)!.request, { type: 'artifact_bytes', data: 'aGk=' });
  await expect(obsolete).rejects.toThrow('Obsolete');
  expect(revoke).not.toHaveBeenCalled();
});

it('native authority replacement and client disposal retire attachment File owners', async () => {
  await server.attached('A');
  const first = server.client.attachmentIntakes.activate('old', 'old', new AttachmentIntake()); first.add([{ file: new File(['a'], 'a') }], capabilities.upload_policy);
  await server.client.connect('ws://127.0.0.1:8089', TOKEN, 'replace-authority');
  expect(server.client.attachmentIntakes.size).toBe(0);
  server.client.attachmentIntakes.activate('replacement', 'replacement', new AttachmentIntake());
  expect(first.snapshot()).toEqual([]); expect(server.client.attachmentIntakes.size).toBe(1);
  const current = server.client.attachmentIntakes.lookup('replacement')!; current.add([{ file: new File(['b'], 'b') }], capabilities.upload_policy);
  server.client.dispose(); expect(current.snapshot()).toEqual([]); expect(server.client.attachmentIntakes.size).toBe(0);
});

it.each(['absent', 'failed', 'unresolved'] as const)('native %s evidence survives client upload into retained first-submission recovery', async state => {
  const { FirstSubmissions } = await import('../src/app/new-conversation/first-submit');
  await server.attached('A');
  server.handlers.set('session/uploadStatus', () => ({ type: 'upload_status', outcome: { state } }));
  const owner = new FirstSubmissions();
  const session = { id: 'A', node: 'node', conversation: 'conversation' };
  const port = {
    current: () => true, create: async () => session, handoff: () => {}, attach: async () => {},
    upload: async (_session: typeof session, file: File, capture: (receipt: UploadedFile['receipt']) => void, operation?: string) => {
      const [uploaded] = await server.client.upload('A', [file], { current: () => true, acknowledged: files => capture(files[0].receipt) }, operation);
      return uploaded.receipt;
    },
    status: vi.fn(async (_session: typeof session, operation: string) => server.client.uploadStatus('A', operation)),
    send: vi.fn(async () => {}),
  };
  await owner.submit('draft', { workspaceId: 'workspace', text: 'keep', files: [new File(['a'], 'a')] }, port);
  const value = owner.session('A')!;
  expect(value.phase).toBe(state === 'unresolved' ? 'uncertain' : 'failed');
  const recover = vi.fn((retry: boolean) => { void owner.recoverUpload(value, port, retry); });
  const ui = render(<AgentComposer firstSubmission={value} onRetainedRecover={recover} uploadPolicy={capabilities.upload_policy} disabled={false} busy={false} active={false} onUpload={async () => []} onSend={async () => false} onCancel={() => {}}/>);
  expect(ui.queryByRole('button', { name: 'Retry' }) !== null).toBe(state !== 'unresolved');
  expect(ui.queryByRole('button', { name: 'Check status' }) !== null).toBe(state === 'unresolved');
  const count = () => server.requests.filter(item => item.request.method === 'session/uploadPrepare').length;
  if (state === 'unresolved') {
    await owner.recoverUpload(value, port, true); expect(count()).toBe(1);
    await owner.recoverUpload(value, port, false);
    expect(port.status).toHaveBeenCalledExactlyOnceWith(session, value.operations[0]); expect(count()).toBe(1);
  } else {
    const work = owner.recoverUpload(value, port, true);
    await owner.recoverUpload(value, port, true); await work;
    expect(count()).toBe(2); expect(owner.session('A')!.operations[0]).not.toBe(value.operations[0]);
    expect(owner.session('A')!.phase).toBe('failed');
  }
  expect(port.send).not.toHaveBeenCalled();
});

it('explicit prepare refusal is read as no-commit failure and never transmits a carrier', async () => {
  const { UploadFailure } = await import('../src/client/uploads');
  await server.attached('A'); server.held.add('session/uploadPrepare');
  const operation = 'a'.repeat(32);
  const work = server.client.upload('A', [new File(['x'], 'x')], undefined, operation);
  const rejected = expect(work).rejects.toMatchObject(new UploadFailure('failed'));
  const request = await server.waitFor('session/uploadPrepare', 1);
  server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32602, message: 'Invalid params', data: { kind: 'invalid_params' } } });
  await rejected;
  expect(server.requests.filter(item => item.request.method === 'session/uploadStatus').map(item => item.request.params)).toMatchObject([{ operation_id: operation }]);
});
it('retired occurrence abort refuses a queued Artifact read before native dispatch', async () => {
  await server.attached('A'); server.held.add('session/list');
  const occupying = Array.from({ length: 8 }, () => server.client.request({ method: 'session/list', params: { offset: 0, limit: 32 } }, 'sessions'));
  const held = server.requests.filter(item => item.request.method === 'session/list').slice(-8).map(item => item.request);
  expect(held).toHaveLength(8);
  const resources = new ArtifactResources(server.client, 'A'), abort = new AbortController();
  const read = resources.readBytes('hidden-before-dispatch', abort.signal), rejected = expect(read).rejects.toThrow();
  expect(server.requests.filter(item => item.request.method === 'artifact/read')).toHaveLength(0);
  abort.abort(); for (const request of held) server.reply(request);
  await Promise.all(occupying); await rejected;
  expect(server.requests.filter(item => item.request.method === 'artifact/read')).toHaveLength(0);
  expect(create).not.toHaveBeenCalled(); resources.dispose();
});

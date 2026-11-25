import { afterEach, expect, it, vi } from 'vitest';
import { parseGeometryStream } from '../src/app/components/documents/geometry/parse';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';
import type { GeometrySource } from '../shared/geometry-source';
import { FILE_PREVIEW_LIMITS } from '../shared/file-preview';
import { readGeometrySource } from '../src/client/geometry-source';
import { WorkspaceHostError } from '../src/workspaces/host';
class StreamWorker {
  static instances: StreamWorker[] = [];
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null; onmessageerror: (() => void) | null = null;
  messages: Record<string, unknown>[] = []; terminated = 0;
  constructor() { StreamWorker.instances.push(this); }
  postMessage(message: Record<string, unknown>, transfer?: Transferable[]) {
    this.messages.push(message);
    if (transfer) { expect(message.data).toBeInstanceOf(Uint8Array); expect(transfer).toEqual([(message.data as Uint8Array).buffer]); }
  }
  terminate() { this.terminated++; }
}
const flush = async () => { for (let turn = 0; turn < 20; turn++) await Promise.resolve(); };
const model = () => ({ meshes:[{name:'',positions:new Float32Array([0,0,0,1,0,0,0,1,0]),groups:[]}],root:{name:'',meshes:[0],children:[]} });
afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); StreamWorker.instances = []; });
it('transfers a single acknowledged chunk at a time and posts EOF after source validation', async () => {
  vi.stubGlobal('Worker', StreamWorker); let close!: () => void;
  const source: GeometrySource = { consume: async sink => {
    await sink.start({bytes:2,version:'one'}); await sink.chunk(new Uint8Array([1]),0); await sink.chunk(new Uint8Array([2]),1);
    await new Promise<void>(resolve => { close = resolve; }); return {version:'one'};
  } };
  const pending = parseGeometryStream(source,new AbortController().signal); await flush(); const worker = StreamWorker.instances[0];
  expect(worker.messages.map(message=>message.kind)).toEqual(['stream-start','stream-chunk']);
  worker.onmessage!({data:{kind:'stream-ack',offset:0}}); await flush(); expect(worker.messages).toHaveLength(3);
  worker.onmessage!({data:{kind:'stream-ack',offset:1}}); await flush(); expect(worker.messages).toHaveLength(3);
  close(); await flush(); expect(worker.messages.at(-1)?.kind).toBe('stream-end');
  worker.onmessage!({data:{ok:true,model:model()}}); (await pending).dispose(); expect(worker.terminated).toBe(1);
});
it('refuses complete models before EOF and independently rejects invalid output values', async () => {
  vi.stubGlobal('Worker', StreamWorker);
  const source: GeometrySource = {consume: async (_sink, signal) => new Promise((_,reject)=>signal.addEventListener('abort',()=>reject(signal.reason)))};
  const pending = parseGeometryStream(source,new AbortController().signal), rejected = expect(pending).rejects.toMatchObject({code:'invalid'});
  StreamWorker.instances[0].onmessage!({data:{ok:true,model:model()}}); await rejected;
  const completed: GeometrySource = {consume:async()=>({version:'one'})};
  const next = parseGeometryStream(completed,new AbortController().signal); await flush(); const broken=model(); broken.meshes[0].positions[0]=NaN;
  StreamWorker.instances[1].onmessage!({data:{ok:true,model:broken}}); await expect(next).rejects.toMatchObject({code:'invalid'});
});
it('keeps one absolute admission deadline across chunk acknowledgements', async () => {
  vi.stubGlobal('Worker', StreamWorker); vi.useFakeTimers();
  const source: GeometrySource = {consume: async (sink,signal) => {await sink.start({bytes:3,version:'one'}); for(let offset=0;offset<3;offset++)await sink.chunk(new Uint8Array([1]),offset); signal.throwIfAborted();return {version:'one'};}};
  const pending=parseGeometryStream(source,new AbortController().signal), rejected=expect(pending).rejects.toMatchObject({code:'timeout'}); await flush();
  await vi.advanceTimersByTimeAsync(L.timeoutMs-1); StreamWorker.instances[0].onmessage!({data:{kind:'stream-ack',offset:0}});await flush();
  await vi.advanceTimersByTimeAsync(1);await rejected;expect(StreamWorker.instances[0].terminated).toBe(1);
});
it('copies only a bounded subview rather than transferring its retained owner buffer', async () => {
  vi.stubGlobal('Worker', StreamWorker);
  const owner = new Uint8Array(8*1024*1024), page = owner.subarray(3,7); page.set([1,2,3,4]);
  const source: GeometrySource = {consume:async sink=>{await sink.start({bytes:4,version:'one'});await sink.chunk(page,0);return {version:'one'};}};
  const pending = parseGeometryStream(source,new AbortController().signal);await flush();const worker=StreamWorker.instances[0], sent=worker.messages[1].data as Uint8Array;
  expect(sent).toEqual(new Uint8Array([1,2,3,4]));expect(sent.buffer.byteLength).toBe(4);expect(sent.buffer).not.toBe(owner.buffer);expect(owner.byteLength).toBe(8*1024*1024);
  worker.onmessage!({data:{kind:'stream-ack',offset:0}});await flush();worker.onmessage!({data:{ok:true,model:model()}});(await pending).dispose();
});
it('immediately retires a failed EOF send and aborts its source lifetime', async () => {
  class EofFailure extends StreamWorker { override postMessage(message: Record<string,unknown>, transfer?:Transferable[]) { if(message.kind==='stream-end')throw new Error('port closed');super.postMessage(message,transfer); } }
  vi.stubGlobal('Worker',EofFailure);let sourceSignal!:AbortSignal;
  const source:GeometrySource={consume:async(sink,signal)=>{sourceSignal=signal;await sink.start({bytes:0,version:'one'});return {version:'one'};}};
  await expect(parseGeometryStream(source,new AbortController().signal)).rejects.toMatchObject({code:'worker'});
  expect(sourceSignal.aborted).toBe(true);expect(StreamWorker.instances[0].terminated).toBe(1);
});
it('preserves an immediate source setup failure and retires the Worker', async () => {
  vi.stubGlobal('Worker',StreamWorker);let sourceSignal!:AbortSignal;
  const failure = new Error('source setup failed');
  const source:GeometrySource={consume:(_sink,signal)=>{sourceSignal=signal;throw failure;}};
  await expect(parseGeometryStream(source,new AbortController().signal)).rejects.toBe(failure);
  expect(sourceSignal.aborted).toBe(true);expect(StreamWorker.instances[0].terminated).toBe(1);
});

it('preserves the size limit across real source admission without reading a page', async () => {
  vi.stubGlobal('Worker', StreamWorker);
  const read = vi.fn(), close = vi.fn(async () => {});
  const source: GeometrySource = { consume: (sink, signal) => readGeometrySource(
    async () => ({ bytes: FILE_PREVIEW_LIMITS.geometryBytes + 1, version: 'one', read, close }), signal, sink) };
  await expect(parseGeometryStream(source, new AbortController().signal)).rejects.toMatchObject({ kind: 'too_large' });
  expect(read).not.toHaveBeenCalled();
  expect(close).toHaveBeenCalledOnce();
  expect(StreamWorker.instances[0].terminated).toBe(1);
});

it.each(['capacity', 'source_changed', 'file_settlement_unknown'])('preserves %s from the real streaming source and releases admission', async kind => {
  vi.stubGlobal('Worker', StreamWorker);
  const failure = new WorkspaceHostError(kind, kind);
  const source: GeometrySource = { consume: (sink, signal) => readGeometrySource(async () => ({
    bytes: 1, version: 'one',
    read: async () => { throw kind === 'file_settlement_unknown' ? new Error('read failed') : failure; },
    close: async () => { if (kind === 'file_settlement_unknown') throw failure; },
  }), signal, sink) };
  for (let index = 0; index <= L.owners; index++) {
    await expect(parseGeometryStream(source, new AbortController().signal)).rejects.toBe(failure);
  }
  expect(StreamWorker.instances).toHaveLength(L.owners + 1);
  expect(StreamWorker.instances.every(worker => worker.terminated === 1 && !worker.messages.some(message => message.kind === 'stream-end'))).toBe(true);
});

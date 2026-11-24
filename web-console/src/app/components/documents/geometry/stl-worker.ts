import { importStl } from './stl';
import { StlStreamDecoder } from './stl-stream';
import { GeometryError, requireGeometry as check } from './limits';
import type { GeometryModel } from './model';
import { serveGeometry } from './worker';
let completed: GeometryModel | undefined;
serveGeometry((bytes, format, stage) => {
  if (format !== 'stl') throw new GeometryError('unsupported');
  stage('importing');
  if (completed) { const model = completed; completed = undefined; return model; }
  return importStl(bytes);
});
const complete = globalThis.onmessage!.bind(globalThis as unknown as Window);
let decoder: StlStreamDecoder | undefined, deadline = 0, started = false, ended = false;
globalThis.onmessage = event => {
  const message = event.data;
  if (!message?.kind?.startsWith('stream-')) { complete(event); return; }
  try {
    if (message.kind === 'stream-start') {
      check(!started && Number.isFinite(message.deadline) && typeof message.version === 'string' && message.version.length > 0); started = true; deadline = message.deadline;
      decoder = new StlStreamDecoder(message.bytes);
      globalThis.postMessage({ kind: 'stage', stage: 'importing' });
    }
    check(started && !ended && performance.timeOrigin + performance.now() < deadline);
    if (message.kind === 'stream-chunk') {
      check(message.data instanceof Uint8Array && message.data.buffer instanceof ArrayBuffer);
      decoder!.chunk(message.data, message.offset);
      globalThis.postMessage({ kind: 'stream-ack', offset: message.offset });
    } else if (message.kind === 'stream-end') {
      completed = decoder!.finish(); const bytes = decoder!.previewSource ?? new Uint8Array(); decoder = undefined; ended = true;
      complete({ data: { bytes, format: 'stl', deadline, preparePreview: true } } as MessageEvent);
    } else check(message.kind === 'stream-start');
  } catch (error) {
    ended = true; decoder = undefined;
    globalThis.postMessage({ ok: false, code: error instanceof GeometryError ? error.code : 'invalid' });
  }
};

import { GeometryError } from './limits';
import { modelTransfers, type GeometryModel } from './model';
import type { GeometryFormat } from './format';
import { optimizeModel } from './optimize';

export function serveGeometry(importer: (bytes: Uint8Array<ArrayBuffer>, format: Exclude<GeometryFormat, 'glb'>) => GeometryModel | Promise<GeometryModel>) {
  globalThis.onmessage = (event: MessageEvent<{ bytes: Uint8Array<ArrayBuffer>; format: Exclude<GeometryFormat, 'glb'> }>) => {
    void (async () => {
      const model = await optimizeModel(await importer(event.data.bytes, event.data.format));
      globalThis.postMessage({ ok: true, model }, { transfer: modelTransfers(model) });
    })().catch(error => globalThis.postMessage({ ok: false, code: error instanceof GeometryError ? error.code : 'invalid' }));
  };
}

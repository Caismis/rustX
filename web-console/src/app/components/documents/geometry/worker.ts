import { GeometryError } from './limits';
import { modelTransfers, type GeometryModel } from './model';
import type { GeometryFormat } from './format';
import { optimizeModel } from './optimize';

export function serveGeometry(importer: (bytes: Uint8Array<ArrayBuffer>, format: Exclude<GeometryFormat, 'glb'>) => GeometryModel | Promise<GeometryModel>) {
  globalThis.onmessage = (event: MessageEvent<{ bytes: Uint8Array<ArrayBuffer>; format: Exclude<GeometryFormat, 'glb'>; deadline: number; profile?: boolean }>) => {
    void (async () => {
      const start = performance.now(), imported = await importer(event.data.bytes, event.data.format), importedAt = performance.now();
      // Cooperative 200 ms preparation budget. Start no optional work within
      // the final second of the original transfer/receiver deadline. Individual
      // JS allocations cannot be hard-preempted, so the reserve remains separate.
      const budget = Math.min(performance.now() + 200, event.data.deadline - performance.timeOrigin - 1000);
      const model = optimizeModel(imported, budget);
      const profile = event.data.profile ? { importMs: importedAt - start, prepareMs: performance.now() - importedAt,
        cad: performance.getEntriesByType('measure').filter(entry => entry.name.startsWith('geometry:cad-')).map(entry => ({ name: entry.name, ms: entry.duration })) } : undefined;
      globalThis.postMessage({ ok: true, model, profile }, { transfer: modelTransfers(model) });
    })().catch(error => globalThis.postMessage({ ok: false, code: error instanceof GeometryError ? error.code : 'invalid' }));
  };
}

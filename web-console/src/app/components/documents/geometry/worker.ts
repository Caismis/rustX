import { GeometryError } from './limits';
import { modelTransfers, type GeometryModel } from './model';
import type { GeometryFormat } from './format';
import { optimizeModel } from './optimize';
import { geometryStageObserver, type GeometryImportStage } from './stages';

export function serveGeometry(importer: (bytes: Uint8Array<ArrayBuffer>, format: Exclude<GeometryFormat, 'glb'>, stage: (stage: GeometryImportStage) => void, module?: WebAssembly.Module) => GeometryModel | Promise<GeometryModel>) {
  globalThis.onmessage = (event: MessageEvent<{ bytes: Uint8Array<ArrayBuffer>; format: Exclude<GeometryFormat, 'glb'>; deadline: number; profile?: boolean; module?: WebAssembly.Module }>) => {
    void (async () => {
      const start = performance.now(), observations: { stage: GeometryImportStage; ms: number }[] = [];
      const stage = geometryStageObserver(event.data.format, value => {
        observations.push({ stage: value, ms: performance.now() - start });
        globalThis.postMessage({ kind: 'stage', stage: value });
      });
      const imported = await importer(event.data.bytes, event.data.format, stage, event.data.module), importedAt = performance.now();
      stage('preparing');
      // Cooperative 200 ms preparation budget. Start no optional work within
      // the final second of the original transfer/receiver deadline. Individual
      // JS allocations cannot be hard-preempted, so the reserve remains separate.
      const budget = Math.min(performance.now() + 200, event.data.deadline - performance.timeOrigin - 1000);
      const model = optimizeModel(imported, budget);
      const profile = event.data.profile ? { importMs: importedAt - start, prepareMs: performance.now() - importedAt,
        stages: observations,
        cad: performance.getEntriesByType('measure').filter(entry => entry.name.startsWith('geometry:cad-')).map(entry => ({ name: entry.name, ms: entry.duration })) } : undefined;
      globalThis.postMessage({ ok: true, model, profile }, { transfer: modelTransfers(model) });
    })().catch(error => globalThis.postMessage({ ok: false, code: error instanceof GeometryError ? error.code : 'invalid' }));
  };
}

import { GeometryError } from './limits';
import { modelTransfers, type GeometryModel } from './model';
import type { GeometryFormat } from './format';
import { optimizeModel } from './optimize';
import { geometryStageObserver, type GeometryImportStage } from './stages';
import { previewJobs, preparePreviews } from './preview-data';
import { GEOMETRY_LIMITS as L } from './limits';

export function serveGeometry(importer: (bytes: Uint8Array<ArrayBuffer>, format: Exclude<GeometryFormat, 'glb'>, stage: (stage: GeometryImportStage) => void, module?: WebAssembly.Module, partial?: (model: GeometryModel, meshIndex: number) => void) => GeometryModel | Promise<GeometryModel>) {
  let beginPreview: (() => void) | undefined;
  globalThis.onmessage = (event: MessageEvent<{ kind?: 'preview-start'; bytes: Uint8Array<ArrayBuffer>; format: Exclude<GeometryFormat, 'glb'>; deadline: number; profile?: boolean; preparePreview?: boolean; progressive?: boolean; module?: WebAssembly.Module }>) => {
    if (event.data.kind === 'preview-start') { beginPreview?.(); beginPreview = undefined; return; }
    const { format, deadline, profile: profiling, preparePreview, module } = event.data;
    let bytes: Uint8Array<ArrayBuffer> | undefined = event.data.bytes; event.data.bytes = new Uint8Array();
    void (async () => {
      const start = performance.now(), observations: { stage: GeometryImportStage; ms: number }[] = [];
      const stage = geometryStageObserver(format, value => {
        observations.push({ stage: value, ms: performance.now() - start });
        globalThis.postMessage({ kind: 'stage', stage: value });
      });
      let sequence = 0;
      const partial = event.data.progressive ? (model: GeometryModel, meshIndex: number) => {
        if (performance.timeOrigin + performance.now() >= deadline) return;
        globalThis.postMessage({ kind: 'partial', sequence: ++sequence, meshIndex, model }, { transfer: modelTransfers(model) });
      } : undefined;
      let model = await importer(bytes!, format, stage, module, partial);
      const importedAt = performance.now();
      stage('preparing');
      // Cooperative 200 ms preparation budget. Start no optional work within
      // the final second of the original transfer/receiver deadline. Individual
      // JS allocations cannot be hard-preempted, so the reserve remains separate.
      const budget = Math.min(performance.now() + 200, deadline - performance.timeOrigin - 1000);
      model = optimizeModel(model, budget);
      const jobs = preparePreview ? previewJobs(model, format, bytes!) : []; bytes = undefined;
      const profile = profiling ? { importMs: importedAt - start, prepareMs: performance.now() - importedAt,
        stages: observations,
        cad: performance.getEntriesByType('measure').filter(entry => entry.name.startsWith('geometry:cad-')).map(entry => ({ name: entry.name, ms: entry.duration })) } : undefined;
      const startPreview = jobs.length ? new Promise<void>(resolve => { beginPreview = resolve; }) : Promise.resolve();
      globalThis.postMessage({ ok: true, model, profile, pendingPreviews: jobs.map(job => job.mesh) }, { transfer: modelTransfers(model) });
      if (jobs.length) {
        await startPreview;
        const previewStart = performance.now();
        try {
          const previews = await preparePreviews(jobs, Math.min(previewStart + L.previewTimeoutMs, deadline - performance.timeOrigin - 1000));
          globalThis.postMessage({ kind: 'preview', previews, ms: performance.now() - previewStart }, { transfer: previews.flatMap(value => [value.data.positions.buffer, value.data.normals.buffer, value.data.sourceVertices.buffer, value.data.normalVertices.buffer]) });
        } catch { globalThis.postMessage({ kind: 'preview', previews: [] }); }
      }
    })().catch(error => globalThis.postMessage({ ok: false, code: error instanceof GeometryError ? error.code : 'invalid' }));
  };
}

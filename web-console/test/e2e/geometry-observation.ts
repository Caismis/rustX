import type { Page } from '@playwright/test';

export async function instrument(page: Page) {
  await page.addInitScript(() => {
    const counts = { workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 };
    const drawing = { calls: 0, modelCalls: 0, scheduled: false, frameTriangles: 0, lastModelTriangles: 0 };
    const imports: { vertices: number; triangles: number }[][] = [];
    const importTimings: { elapsedMs: number; profile: unknown }[] = [];
    const previews: {elapsedMs:number;meshes:{triangles:number;error:number;bytes:number}[]}[]=[];
    const modelFrames:{ms:number;triangles:number;width:number;height:number}[]=[];
    for (const method of ['drawArrays', 'drawElements'] as const) {
      const original = WebGL2RenderingContext.prototype[method];
      Object.defineProperty(WebGL2RenderingContext.prototype, method, { value: function (this: WebGL2RenderingContext, ...args: unknown[]) {
        drawing.calls++; const result = Reflect.apply(original, this, args);
        if (args[0] === this.TRIANGLES) {
          drawing.modelCalls++;
          drawing.frameTriangles += Number(args[method === 'drawArrays' ? 2 : 1]) / 3;
          if (!drawing.scheduled) { drawing.scheduled = true; queueMicrotask(() => { drawing.scheduled = false; drawing.lastModelTriangles = drawing.frameTriangles; drawing.frameTriangles = 0; if(modelFrames.length<1024)modelFrames.push({ms:performance.now(),triangles:drawing.lastModelTriangles,width:this.canvas.width,height:this.canvas.height}); document.dispatchEvent(new CustomEvent('geometry-test-model-submitted', { detail: this.canvas })); }); }
        }
        return result;
      } });
    }
    const owned = { buffers: new Map<unknown, unknown>(), textures: new Map<unknown, unknown>() };
    const NativeWorker = window.Worker;
    window.Worker = class extends NativeWorker {
      private live = true;
      constructor(url: string | URL, options?: WorkerOptions) { const start = performance.now(); super(url, options); counts.workers++;
        this.addEventListener('message', event => { if (event.data?.ok && Array.isArray(event.data.model?.meshes)) {
          imports.push(event.data.model.meshes.map((mesh: any) => ({ vertices: mesh.positions.length / 3, triangles: (mesh.indices?.length ?? mesh.positions.length / 3) / 3 })));
          importTimings.push({ elapsedMs: performance.now() - start, profile: event.data.profile });
        } });
        this.addEventListener('message',event=>{if(event.data?.kind==='preview'&&Array.isArray(event.data.previews))previews.push({elapsedMs:performance.now()-start,meshes:event.data.previews.map((value:any)=>({triangles:value.data.positions.length/9,error:value.data.error,bytes:value.data.positions.byteLength+value.data.normals.byteLength+value.data.sourceVertices.byteLength+value.data.normalVertices.byteLength}))});});
      }
      postMessage(message: any, options?: Transferable[] | StructuredSerializeOptions) {
        const profiled = message?.bytes ? { ...message, profile: true } : message;
        if (Array.isArray(options)) super.postMessage(profiled, options); else super.postMessage(profiled, options);
      }
      terminate() { if (this.live) { this.live = false; counts.workers--; } super.terminate(); }
    };
    const getContext = HTMLCanvasElement.prototype.getContext;
    Object.defineProperty(HTMLCanvasElement.prototype, 'getContext', { value: function (this: HTMLCanvasElement, ...args: unknown[]) {
      const value = Reflect.apply(getContext, this, args);
      if (value && args[0] === 'webgl2' && !this.dataset.countedGeometryContext) { this.dataset.countedGeometryContext = 'true'; counts.contexts++; this.addEventListener('webglcontextlost', () => {
        counts.contexts--;
        // Context loss releases Three.js's internal fallback textures as well
        // as objects already explicitly deleted during renderer disposal.
        for (const key of ['buffers', 'textures'] as const) for (const [item, context] of owned[key]) if (context === value) { owned[key].delete(item); counts[key]--; }
      }, { once: true }); }
      return value;
    } });
    for (const [create, remove, key] of [['createBuffer', 'deleteBuffer', 'buffers'], ['createTexture', 'deleteTexture', 'textures']] as const) {
      const live = owned[key], prototype = WebGL2RenderingContext.prototype;
      const originalCreate = prototype[create], originalRemove = prototype[remove];
      Object.defineProperty(prototype, create, { value: function (...args: unknown[]) { const value = Reflect.apply(originalCreate, this, args); if (value) { live.set(value, this); counts[key]++; } return value; } });
      Object.defineProperty(prototype, remove, { value: function (...args: unknown[]) { if (live.delete(args[0])) counts[key]--; return Reflect.apply(originalRemove, this, args); } });
    }
    const createUrl = URL.createObjectURL, revokeUrl = URL.revokeObjectURL, urls = new Set<string>();
    URL.createObjectURL = value => { const url = createUrl(value); urls.add(url); counts.urls++; return url; };
    URL.revokeObjectURL = url => { if (urls.delete(url)) counts.urls--; revokeUrl(url); };
    Object.assign(window, { geometryResources: counts, geometryDrawCalls: drawing, geometryImports: imports, geometryImportTimings: importTimings, geometryPreviewTimings:previews, geometryModelFrames:modelFrames });
  });
}
export const resources = (page: Page) => page.evaluate(() => ({ ...(window as any).geometryResources }));

/** One capture per page; first-frame proxy and trusted input diagnostics. */
export async function observeGeometryLatency(page: Page, startSelector = 'button[aria-label^="Preview "]') {
  await page.evaluate(startSelector => {
    const state = { start: 0, first: 0, firstGpuReady: 0, coarseFirst:0, coarsePending:0, coarseSamples:[] as number[], pending: 0, cancelled: 0, samples: [] as number[], gpuSamples: [] as number[], stages: [] as { dispatch: number; submission: number; gpu: number; renderOpportunity: number }[] };
    const waiting: { start: number; received: number }[] = [];
    const coarseWaiting:number[]=[];let coarseScheduled=false;
    const coarseObserver=new MutationObserver(records=>{
      const svg=document.querySelector<SVGSVGElement>('svg[data-geometry-rough]');
      if(!svg?.querySelector('path')?.getAttribute('d')||!state.start)return;
      if(!state.coarseFirst&&!coarseScheduled){coarseScheduled=true;requestAnimationFrame(()=>requestAnimationFrame(()=>{if(svg.isConnected)state.coarseFirst=performance.now()-state.start;else coarseScheduled=false;}));}
      if(records.some(record=>record.target===svg&&record.attributeName==='data-geometry-rough-render')&&coarseWaiting.length){const inputs=coarseWaiting.splice(0);requestAnimationFrame(()=>requestAnimationFrame(()=>{if(svg.isConnected)for(const start of inputs)state.coarseSamples.push(performance.now()-start);else state.cancelled+=inputs.length;state.coarsePending-=inputs.length;}));}
    });
    coarseObserver.observe(document,{childList:true,subtree:true,attributes:true,attributeFilter:['data-geometry-rough-render']});
    let firstSubmitted = false;
    // This test-only event runs at the microtask checkpoint AFTER the actual
    // model-render callback, including all parts and axes. A busy renderer's
    // skipped rAF cannot count as the response to a newer camera event.
    document.addEventListener('geometry-test-model-submitted', event => {
      const canvas = (event as CustomEvent).detail;
      if (!(canvas instanceof HTMLCanvasElement) || canvas !== document.querySelector('[data-geometry-format] canvas')) return;
      const submitted = performance.now(), inputs = waiting.splice(0), first = !firstSubmitted && state.start > 0;
      if (!first && !inputs.length) return;
      if (first) firstSubmitted = true;
      const gl = canvas.getContext('webgl2')!;
      const fence = gl.fenceSync(gl.SYNC_GPU_COMMANDS_COMPLETE, 0)!; gl.flush();
      // A few non-clamped event-loop turns avoid adding an entire Windows timer
      // quantum to a fast GPU. Bound active probing, then yield to the timer.
      const channel = new MessageChannel(); let probes = 0;
      channel.port1.onmessage = () => settled();
      const settled = () => {
        const status = gl.clientWaitSync(fence, 0, 0);
        if (status === gl.TIMEOUT_EXPIRED) { if (probes++ < 8) channel.port2.postMessage(null); else setTimeout(settled, 1); return; }
        if (status === gl.WAIT_FAILED) {
          channel.port1.close(); channel.port2.close(); gl.deleteSync(fence);
          if (!canvas.isConnected || gl.isContextLost()) { state.cancelled += inputs.length; state.pending -= inputs.length; return; }
          throw new Error('Geometry measurement fence failed');
        }
        channel.port1.close(); channel.port2.close();
        const completed = performance.now();
        if (first) state.firstGpuReady = completed - state.start;
        for (const input of inputs) state.gpuSamples.push(completed - input.start);
        gl.deleteSync(fence);
        requestAnimationFrame(() => {
          const frameReady = performance.now();
          if (first) state.first = frameReady - state.start;
          for (const input of inputs) {
            state.samples.push(frameReady - input.start);
            state.stages.push({ dispatch: input.received - input.start, submission: submitted - input.received, gpu: completed - submitted, renderOpportunity: frameReady - completed });
          }
          state.pending -= inputs.length;
        });
      };
      settled();
    });
    document.addEventListener('pointerdown', event => {
      if (event.target instanceof Element && event.target.closest(startSelector)) state.start = event.timeStamp;
    }, { capture: true });
    document.addEventListener('pointermove', event => {
      if(event.isTrusted&&event.buttons&&event.target instanceof SVGElement&&event.target.closest('svg[data-geometry-rough]')){state.coarsePending++;coarseWaiting.push(event.timeStamp);return;}
      if (!event.isTrusted || !event.buttons || !(event.target instanceof HTMLCanvasElement)) return;
      state.pending++;
      waiting.push({ start: event.timeStamp, received: performance.now() });
    }, { capture: true });
    Object.assign(window, { geometryLatency: state });
  }, startSelector);
}

import wasmUrl from 'occt-import-js/dist/occt-import-js.wasm?url';
import { GEOMETRY_LIMITS } from './limits';

let compiled: Promise<WebAssembly.Module> | undefined;
/** One immutable code module per page. Never retain a source, heap or instance. */
export function cadCode(): Promise<WebAssembly.Module> {
  if (!compiled) {
    const start = performance.now(), controller = new AbortController(), timer = setTimeout(() => controller.abort(), GEOMETRY_LIMITS.timeoutMs);
    let pending: Promise<WebAssembly.Module>;
    try { pending = WebAssembly.compileStreaming(fetch(wasmUrl, { credentials: 'same-origin', signal: controller.signal })).then(module => {
      const name = 'geometry:cad-code-compile'; performance.clearMeasures(name); performance.measure(name, { start }); return module;
    }).finally(() => clearTimeout(timer)); }
    catch (error) { clearTimeout(timer); throw error; }
    compiled = pending;
    void pending.catch(() => { if (compiled === pending) compiled = undefined; });
  }
  return compiled;
}

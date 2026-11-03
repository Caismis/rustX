import type { Page } from '@playwright/test';

export async function instrument(page: Page) {
  await page.addInitScript(() => {
    const counts = { workers: 0, contexts: 0, buffers: 0, textures: 0, urls: 0 };
    const drawing = { calls: 0 };
    for (const method of ['drawArrays', 'drawElements'] as const) {
      const original = WebGL2RenderingContext.prototype[method];
      Object.defineProperty(WebGL2RenderingContext.prototype, method, { value: function (this: WebGL2RenderingContext, ...args: unknown[]) { drawing.calls++; return Reflect.apply(original, this, args); } });
    }
    const owned = { buffers: new Map<unknown, unknown>(), textures: new Map<unknown, unknown>() };
    const NativeWorker = window.Worker;
    window.Worker = class extends NativeWorker {
      private live = true;
      constructor(url: string | URL, options?: WorkerOptions) { super(url, options); counts.workers++; }
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
    Object.assign(window, { geometryResources: counts, geometryDrawCalls: drawing });
  });
}
export const resources = (page: Page) => page.evaluate(() => ({ ...(window as any).geometryResources }));

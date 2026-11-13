import { describe, expect, it } from 'vitest';
import { isCadRuntimeRequest, isCadWasmRequest, isGeometryRuntimeRequest } from './e2e/geometry-requests';

const url = (path: string) => new URL(path, 'http://127.0.0.1:5174');

describe('geometry request boundaries', () => {
  it.each([
    '/node_modules/occt-import-js/dist/occt-import-js.wasm',
    '/node_modules/.pnpm/occt-import-js@0.0.23/node_modules/occt-import-js/dist/occt-import-js.wasm?t=123',
    '/assets/occt-import-js-BhHfLpto.wasm',
  ])('counts and holds actual CAD binary %s', path => {
    expect(isCadWasmRequest(url(path))).toBe(true);
    expect(isCadRuntimeRequest(url(path))).toBe(true);
    expect(isGeometryRuntimeRequest(url(path))).toBe(true);
  });

  it.each([
    '/node_modules/occt-import-js/dist/occt-import-js.wasm?import&url',
    '/node_modules/occt-import-js/dist/occt-import-js.wasm?url&import&t=123',
    '/node_modules/occt-import-js/dist/occt-import-js.wasm?import',
    '/node_modules/occt-import-js/dist/occt-import-js.wasm?raw',
    '/src/app/components/documents/geometry/cad-worker.ts?worker',
    '/src/app/components/documents/geometry/cad-worker.ts?url',
  ])('allows URL metadata without counting runtime bytes/code %s', path => {
    expect(isCadWasmRequest(url(path))).toBe(false);
    expect(isCadRuntimeRequest(url(path))).toBe(false);
    expect(isGeometryRuntimeRequest(url(path))).toBe(false);
  });

  it.each([
    '/src/app/components/documents/geometry/cad-worker.ts?worker_file&type=module',
    '/node_modules/.vite/deps/occt-import-js.js?v=123',
    '/node_modules/occt-import-js/dist/occt-import-js.js',
    '/assets/cad-worker-CZCC4I3J.js',
  ])('detects executable CAD code %s', path => {
    expect(isCadWasmRequest(url(path))).toBe(false);
    expect(isCadRuntimeRequest(url(path))).toBe(true);
    expect(isGeometryRuntimeRequest(url(path))).toBe(true);
  });

  it.each([
    '/src/app/components/documents/geometry/GeometryPreview.tsx',
    '/src/app/components/documents/geometry/GeometryPreview.module.css',
    '/src/app/components/documents/geometry/stl-worker.ts?worker_file&type=module',
    '/assets/GeometryPreview-DoBxmhXN.js',
    '/assets/GeometryPreview-BwY0O7ks.css',
    '/assets/stl-worker-Bvr69GRF.js',
  ])('keeps non-CAD geometry behind the initial lazy boundary %s', path => {
    expect(isCadRuntimeRequest(url(path))).toBe(false);
    expect(isGeometryRuntimeRequest(url(path))).toBe(true);
  });
});

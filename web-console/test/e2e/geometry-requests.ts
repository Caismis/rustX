// Vite serves ?url/?raw imports (and ?worker constructors) as JavaScript
// metadata, not the referenced bytes/code. In dev cad-code.ts imports the
// WASM URL eagerly; only its later fetch reads the binary. Production emits
// the same binary as assets/occt-import-js-<hash>.wasm.
const metadata = (url: URL) => ['url', 'raw', 'worker'].some(key => url.searchParams.has(key));

export function isCadWasmRequest(url: URL): boolean {
  return /\/occt-import-js(?:-[\w-]+)?\.wasm$/.test(url.pathname)
    && !metadata(url) && !url.searchParams.has('import');
}

export function isCadRuntimeRequest(url: URL): boolean {
  if (metadata(url)) return false;
  return isCadWasmRequest(url)
    || /\/(?:cad-worker(?:-[\w-]+)?\.(?:ts|js)|occt-import-js(?:-[\w-]+)?\.js)$/.test(url.pathname);
}

export function isGeometryRuntimeRequest(url: URL): boolean {
  if (metadata(url)) return false;
  return isCadRuntimeRequest(url)
    || /\/(?:GeometryPreview(?:-[\w-]+)?\.(?:tsx|js|css)|GeometryPreview\.module\.css|stl-worker(?:-[\w-]+)?\.(?:ts|js))$/.test(url.pathname);
}

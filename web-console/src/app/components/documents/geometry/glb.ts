import { imageSize } from 'image-size';
import { validateRaster } from '../../../../client/raster';
import { Box3, LoadingManager, type Object3D, type Texture } from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';
import { disposeScene, validateScene } from './scene-resources';

function object(value: unknown): Record<string, unknown> { check(value && typeof value === 'object' && !Array.isArray(value)); return value as Record<string, unknown>; }
function list(value: unknown, limit: number): Record<string, unknown>[] {
  if (value === undefined) return [];
  check(Array.isArray(value)); check(value.length <= limit, 'limit'); return value.map(object);
}
function integer(value: unknown, max: number): number { check(typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 && value <= max); return value; }
function dataUri(uri: string): Uint8Array<ArrayBuffer> {
  const match = /^data:(?:application\/(?:octet-stream|gltf-buffer)|image\/(?:png|jpeg));base64,([A-Za-z0-9+/]*={0,2})$/.exec(uri);
  check(match, 'external'); check(match[1].length <= L.sourceBytes * 4 / 3 + 4, 'limit');
  const decoded = atob(match[1]); return Uint8Array.from(decoded, char => char.charCodeAt(0));
}

/** No loader receives model bytes until declared expansion and every URI are admitted. */
export function admitGlb(bytes: Uint8Array<ArrayBuffer>): Set<string> {
  check(bytes.length <= L.sourceBytes, 'limit'); check(bytes.length >= 20);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  check(view.getUint32(0, true) === 0x46546c67 && view.getUint32(4, true) === 2 && view.getUint32(8, true) === bytes.length);
  let json: Record<string, unknown> | undefined;
  let bin: Uint8Array<ArrayBuffer> = new Uint8Array(0);
  for (let at = 12; at < bytes.length;) {
    check(at + 8 <= bytes.length); const length = view.getUint32(at, true), kind = view.getUint32(at + 4, true);
    check(length % 4 === 0 && at + 8 + length <= bytes.length);
    if (at === 12) { check(kind === 0x4e4f534a && length <= L.jsonBytes, 'limit'); json = object(JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(at + 8, at + 8 + length)))); }
    else { check(kind === 0x004e4942 && bin.length === 0); bin = bytes.subarray(at + 8, at + 8 + length); }
    at += 8 + length;
  }
  check(json); check(object(json.asset).version === '2.0', 'unsupported');
  const supportedExtensions = ['KHR_materials_unlit', 'KHR_texture_transform', 'KHR_materials_ior', 'KHR_materials_emissive_strength'];
  const uris = new Set<string>(), queue = [{ value: json as unknown, depth: 0 }]; let items = 0;
  while (queue.length) {
    const { value, depth } = queue.pop()!; check(++items <= 100_000 && depth <= L.depth + 8, 'limit');
    if (!value || typeof value !== 'object') { if (typeof value === 'number') check(Number.isFinite(value)); continue; }
    for (const [key, child] of Object.entries(value)) {
      if (key === 'uri') { check(typeof child === 'string', 'external'); dataUri(child); uris.add(child); }
      if (key === 'extensions') for (const extension of Object.keys(object(child))) check(supportedExtensions.includes(extension), 'unsupported');
      queue.push({ value: child, depth: depth + 1 });
    }
  }
  check(!json.extensionsRequired || Array.isArray(json.extensionsRequired));
  for (const extension of (json.extensionsRequired ?? []) as unknown[]) check(supportedExtensions.includes(String(extension)), 'unsupported');
  check(!json.skins && !json.animations, 'unsupported');
  const buffers = list(json.buffers, 1); check(buffers.length === 1);
  const buffer = buffers[0].uri ? dataUri(String(buffers[0].uri)) : bin;
  const bufferLength = integer(buffers[0].byteLength, L.sourceBytes); check(bufferLength <= buffer.length);
  const views = list(json.bufferViews, 4096);
  const ranges = views.map(item => {
    check(item.buffer === 0); const offset = integer(item.byteOffset ?? 0, bufferLength), length = integer(item.byteLength, bufferLength);
    check(offset + length <= bufferLength);
    if (item.byteStride !== undefined) check(integer(item.byteStride, 252) >= 4 && Number(item.byteStride) % 4 === 0);
    return { offset, length };
  });
  let decoded = 0;
  const accessors = list(json.accessors, 4096);
  for (const accessor of accessors) {
    check(!accessor.sparse, 'unsupported');
    const count = integer(accessor.count, L.triangles * 3), components = ({ SCALAR: 1, VEC2: 2, VEC3: 3, VEC4: 4, MAT2: 4, MAT3: 9, MAT4: 16 } as Record<string, number>)[String(accessor.type)];
    const width = ({ 5120: 1, 5121: 1, 5122: 2, 5123: 2, 5125: 4, 5126: 4 } as Record<string, number>)[String(accessor.componentType)];
    check(components && width); decoded += count * components * Math.max(4, width); check(decoded <= L.decodedBytes, 'limit');
    const index = integer(accessor.bufferView, views.length - 1), offset = integer(accessor.byteOffset ?? 0, ranges[index].length);
    const stride = Number(views[index].byteStride ?? components * width); check(stride >= components * width);
    check(offset + (count ? (count - 1) * stride + components * width : 0) <= ranges[index].length);
  }
  const meshes = list(json.meshes, L.parts), meshCosts: { triangles: number; draws: number }[] = []; let triangles = 0, vertices = 0, draws = 0;
  for (const mesh of meshes) {
    const cost = { triangles: 0, draws: 0 }; meshCosts.push(cost);
    check(!mesh.weights, 'unsupported');
    for (const primitive of list(mesh.primitives, L.drawCalls)) {
      check(++draws <= L.drawCalls, 'limit'); check((primitive.mode ?? 4) === 4 && !primitive.targets, 'unsupported');
      const attributes = object(primitive.attributes), position = accessors[integer(attributes.POSITION, accessors.length - 1)];
      check(position.type === 'VEC3' && position.componentType === 5126); vertices += Number(position.count);
      const elements = primitive.indices === undefined ? position.count : accessors[integer(primitive.indices, accessors.length - 1)].count;
      if (primitive.indices !== undefined) {
        const index = accessors[Number(primitive.indices)]; check(index.type === 'SCALAR' && [5121, 5123, 5125].includes(Number(index.componentType)));
      }
      check(Number(elements) % 3 === 0); triangles += Number(elements) / 3;
      cost.triangles += Number(elements) / 3; cost.draws++;
    }
  }
  check(draws > 0 && vertices > 0, 'empty'); check(vertices <= L.vertices && triangles <= L.triangles, 'limit');
  const nodes = list(json.nodes, L.nodes), pending: { index: number; depth: number }[] = [];
  let instanceTriangles = 0, instanceDraws = 0;
  for (const node of nodes) if (node.mesh !== undefined) {
    const cost = meshCosts[integer(node.mesh, meshes.length - 1)]; instanceTriangles += cost.triangles; instanceDraws += cost.draws;
    check(instanceTriangles <= L.triangles && instanceDraws <= L.drawCalls, 'limit');
  }
  const parents = new Set<number>();
  for (const node of nodes) if (node.children !== undefined) {
    check(Array.isArray(node.children));
    for (const value of node.children) { const child = integer(value, nodes.length - 1); check(!parents.has(child)); parents.add(child); }
  }
  let roots = 0;
  for (const scene of list(json.scenes, 8)) {
    check(Array.isArray(scene.nodes)); roots += scene.nodes.length; check(roots <= L.nodes, 'limit');
    for (const value of scene.nodes) integer(value, nodes.length - 1);
  }
  // Check every node, including nodes in unselected scenes, before loader recursion.
  function visit(index: number, depth: number, path: Set<number>) {
    check(depth <= L.depth, 'limit'); check(!path.has(index));
    path.add(index); const node = nodes[index]; check(node);
    for (const key of ['matrix', 'translation', 'rotation', 'scale']) if (node[key] !== undefined) {
      const values = node[key]; check(Array.isArray(values) && values.length === (key === 'matrix' ? 16 : key === 'rotation' ? 4 : 3));
      check(values.every(value => typeof value === 'number' && Number.isFinite(value) && Math.abs(value) <= L.coordinate));
    }
    if (node.mesh !== undefined) integer(node.mesh, meshes.length - 1);
    if (node.children !== undefined) { check(Array.isArray(node.children) && node.children.length <= L.nodes); for (const child of node.children) visit(integer(child, nodes.length - 1), depth + 1, path); }
    path.delete(index);
  }
  for (let index = 0; index < nodes.length; index++) pending.push({ index, depth: 0 });
  for (const { index, depth } of pending) visit(index, depth, new Set());
  list(json.materials, L.materials); list(json.textures, L.textures);
  let pixels = 0;
  for (const image of list(json.images, L.textures)) {
    let content: Uint8Array<ArrayBuffer>;
    if (image.uri) content = dataUri(String(image.uri));
    else { check(image.mimeType === 'image/png' || image.mimeType === 'image/jpeg', 'unsupported'); const range = ranges[integer(image.bufferView, views.length - 1)]; content = buffer.subarray(range.offset, range.offset + range.length); }
    validateRaster(content);
    const size = imageSize(content); check((size.type === 'png' || size.type === 'jpg') && size.width > 0 && size.height > 0, 'unsupported');
    pixels += size.width * size.height; check(size.width <= L.textureDimension && size.height <= L.textureDimension && pixels <= L.texturePixels, 'limit');
  }
  return uris;
}

export async function importGlb(bytes: Uint8Array<ArrayBuffer>, signal: AbortSignal) {
  signal.throwIfAborted();
  check(glbImports < L.owners, 'capacity');
  const uris = admitGlb(bytes), manager = new LoadingManager();
  glbImports++;
  const pending = new Set<Texture>(), decodes: Promise<unknown>[] = [], imageUrls = new Set<string>(); let scenes: Object3D[] = [], retired = false;
  const releaseUrl = (url: string) => { if (imageUrls.delete(url)) URL.revokeObjectURL(url); };
  manager.setURLModifier(url => {
    const supplied = uris.has(url), generated = !supplied && url.startsWith(`blob:${globalThis.location.origin}/`);
    check(supplied || generated, 'external');
    if (retired) { if (generated) URL.revokeObjectURL(url); throw new DOMException('Geometry preview closed', 'AbortError'); }
    if (generated) imageUrls.add(url);
    return url;
  });
  // The pinned loader revokes generated URLs on success only. LoadingManager
  // reports errors before item completion, so failures have an explicit owner.
  manager.onError = releaseUrl;
  manager.onProgress = url => { imageUrls.delete(url); };
  const dispose = () => {
    retired = true; signal.removeEventListener('abort', dispose);
    disposeScene(scenes, pending); scenes = []; pending.clear();
  };
  const loader = new GLTFLoader(manager).register(parser => ({
    name: 'rustx_texture_ownership',
    beforeRoot: () => {
      signal.throwIfAborted();
      // Admit all declared texture work before constructing any scene. The
      // physical import slot survives cancellation until every decode settles.
      for (let index = 0; index < (parser.json.textures?.length ?? 0); index++) decodes.push(parser.getDependency('texture', index));
      return Promise.all(decodes).then(() => undefined);
    },
    loadTexture: index => parser.loadTexture(index).then(texture => {
      check(texture);
      // Decoding is browser-owned and may complete after parse rejection or
      // cancellation. Own its result even when no scene can be returned.
      if (retired) disposeScene([], [texture]); else pending.add(texture);
      return texture;
    }),
  }));
  signal.addEventListener('abort', dispose, { once: true });
  try {
    const loaded = await loader.parseAsync(bytes.slice().buffer, ''); scenes = loaded.scenes;
    signal.throwIfAborted(); check(!retired); validateScene(loaded.scenes);
    return { scene: loaded.scene, scenes: loaded.scenes, bounds: new Box3().setFromObject(loaded.scene), dispose };
  } catch (error) { dispose(); throw error; }
  finally { void Promise.allSettled(decodes).then(() => { for (const url of imageUrls) releaseUrl(url); glbImports--; }); }
}
let glbImports = 0;

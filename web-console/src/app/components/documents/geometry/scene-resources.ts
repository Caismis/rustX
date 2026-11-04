import { Box3, BufferAttribute, BufferGeometry, Color, Group, Material, Mesh, MeshStandardMaterial, Sphere, Texture, Vector3, type Object3D } from 'three';
import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';
import { validateModelStructure, type GeometryModel, type GeometryNode, type GeometryPreviewData } from './model';

export interface GeometryPreviewState { pending: boolean; geometry?: BufferGeometry; error: number }
const previews = new WeakMap<BufferGeometry, GeometryPreviewState>();
export const geometryPreviewState = (geometry: BufferGeometry) => previews.get(geometry);
export function takeGeometryPreview(geometry: BufferGeometry) {
  const state = previews.get(geometry);
  if (!state?.geometry) return;
  const result = { geometry: state.geometry, error: state.error }; state.geometry = undefined; return result;
}

function materials(root: Object3D[]): Set<Material> {
  const values = new Set<Material>();
  for (const scene of root) scene.traverse(object => { if (object instanceof Mesh) for (const item of Array.isArray(object.material) ? object.material : [object.material]) values.add(item); });
  return values;
}
function textures(values: Set<Material>): Set<Texture> {
  const result = new Set<Texture>();
  for (const material of values) for (const value of Object.values(material)) if (value instanceof Texture) result.add(value);
  return result;
}
export function disposeScene(roots: Object3D[], additionalTextures: Iterable<Texture> = []): void {
  const geometry = new Set<BufferGeometry>(), mats = materials(roots), images = new Set<unknown>();
  for (const texture of new Set([...textures(mats), ...additionalTextures])) {
    texture.dispose();
    const image = texture.source.data;
    if (!images.has(image) && image && typeof image === 'object' && 'close' in image && typeof image.close === 'function') image.close();
    images.add(image);
  }
  for (const material of mats) material.dispose();
  for (const root of roots) { root.traverse(object => { if (object instanceof Mesh) geometry.add(object.geometry); }); root.removeFromParent(); }
  for (const item of geometry) { previews.get(item)?.geometry?.dispose(); previews.delete(item); item.dispose(); }
}
function validateBounds(bounds: Box3): Box3 {
  check(!bounds.isEmpty(), 'empty');
  check([...bounds.min.toArray(), ...bounds.max.toArray()].every(value => Number.isFinite(value) && Math.abs(value) <= L.coordinate));
  check(Math.max(bounds.max.x - bounds.min.x, bounds.max.y - bounds.min.y, bounds.max.z - bounds.min.z) > 1e-12, 'empty');
  return bounds;
}
function positionBounds(position: BufferAttribute): Box3 {
  const values = position.array;
  let minX = Infinity, minY = Infinity, minZ = Infinity, maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  for (let index = 0; index < values.length; index += 3) {
    const x = values[index], y = values[index + 1], z = values[index + 2];
    check(Number.isFinite(x) && Number.isFinite(y) && Number.isFinite(z)
      && Math.abs(x) <= L.coordinate && Math.abs(y) <= L.coordinate && Math.abs(z) <= L.coordinate);
    minX = Math.min(minX, x); minY = Math.min(minY, y); minZ = Math.min(minZ, z);
    maxX = Math.max(maxX, x); maxY = Math.max(maxY, y); maxZ = Math.max(maxZ, z);
  }
  return new Box3(new Vector3(minX, minY, minZ), new Vector3(maxX, maxY, maxZ));
}
export function validateScene(roots: Object3D[], selected?: Object3D): Box3 {
  return admitScene(roots, selected);
}
/** Generated geometries include every transferred stream, including unreferenced
 * meshes. Scan them once; the graph walk separately charges occurrences. */
function admitScene(roots: Object3D[], selected?: Object3D, generated?: BufferGeometry[]): Box3 {
  const unique = new Set<BufferGeometry>(), mats = materials(roots);
  let vertices = 0, triangles = 0, bytes = 0, nodes = 0, parts = 0, draws = 0, pixels = 0;
  check(mats.size <= L.materials, 'limit');
  const admitGeometry = (geometry: BufferGeometry) => {
      const position = geometry.getAttribute('position'); check(position && position.itemSize === 3);
      if (unique.has(geometry)) return; unique.add(geometry); vertices += position.count;
      check(vertices <= L.vertices, 'limit');
      if (geometry.index) {
        const attribute = geometry.index;
        if (attribute instanceof BufferAttribute && !attribute.normalized && (attribute.array instanceof Uint32Array || attribute.array instanceof Uint16Array || attribute.array instanceof Uint8Array)) {
          for (let index = 0; index < attribute.array.length; index++) check(attribute.array[index] < position.count);
        } else for (let index = 0; index < attribute.count; index++) { const value = attribute.getX(index); check(Number.isSafeInteger(value) && value >= 0 && value < position.count); }
      }
      const bounds = position instanceof BufferAttribute && position.array instanceof Float32Array && !position.normalized ? positionBounds(position) : undefined;
      for (const [name, attribute] of Object.entries(geometry.attributes)) {
        check(attribute.count === position.count && attribute.itemSize >= 1 && attribute.itemSize <= 4);
        bytes += attribute.count * attribute.itemSize * 4;
        check(bytes <= L.decodedBytes, 'limit');
        if (attribute === position && bounds) continue;
        if (attribute instanceof BufferAttribute && attribute.array instanceof Float32Array && !attribute.normalized) {
          const values = attribute.array, minimum = generated && name === 'color' ? 0 : -L.coordinate, maximum = generated && name === 'color' ? 1 : L.coordinate;
          for (let index = 0; index < values.length; index++) check(Number.isFinite(values[index]) && values[index] >= minimum && values[index] <= maximum);
          continue;
        }
        for (let index = 0; index < attribute.count; index++) for (let component = 0; component < attribute.itemSize; component++) {
          const value = attribute.getComponent(index, component); check(Number.isFinite(value) && Math.abs(value) <= L.coordinate);
        }
      }
      // File-declared accessor bounds are not an authority for framing or size.
      if (bounds) {
        geometry.boundingBox = bounds;
        // The position-derived AABB diagonal conservatively encloses every
        // vertex; culling and picking never trust file-declared accessor bounds.
        geometry.boundingSphere = new Sphere(bounds.getCenter(new Vector3()), bounds.getSize(new Vector3()).length() / 2);
      } else { geometry.computeBoundingBox(); geometry.computeBoundingSphere(); }
      const sphere = geometry.boundingSphere;
      check(sphere && sphere.center.toArray().every(value => Number.isFinite(value) && Math.abs(value) <= L.coordinate)
        && Number.isFinite(sphere.radius) && sphere.radius >= 0);
      bytes += geometry.index?.array.byteLength ?? 0;
  };
  if (generated) for (const geometry of generated) admitGeometry(geometry);
  for (const root of roots) {
    root.updateMatrixWorld(true);
    root.traverse(object => {
      check(++nodes <= L.nodes && object.matrixWorld.elements.every(value => Number.isFinite(value) && Math.abs(value) <= L.coordinate), 'limit');
      if (!(object instanceof Mesh)) return;
      check(++parts <= L.parts, 'limit');
      const geometry: BufferGeometry = object.geometry, position = geometry.getAttribute('position'); check(position && position.itemSize === 3);
      const count = geometry.index?.count ?? position.count; check(count > 0 && count % 3 === 0, 'empty');
      triangles += count / 3; draws += Math.max(1, geometry.groups.length);
      check(triangles <= L.triangles && draws <= L.drawCalls, 'limit');
      admitGeometry(geometry);
    });
  }
  check(parts > 0, 'empty');
  check(vertices <= L.vertices && triangles <= L.triangles && bytes <= L.decodedBytes && draws <= L.drawCalls, 'limit');
  const images = textures(mats); check(images.size <= L.textures, 'limit');
  for (const texture of images) {
    const image = texture.source.data; check(image && typeof image === 'object' && 'width' in image && 'height' in image && typeof image.width === 'number' && typeof image.height === 'number' && Number.isSafeInteger(image.width) && Number.isSafeInteger(image.height));
    pixels += image.width * image.height;
    check(image.width <= L.textureDimension && image.height <= L.textureDimension && pixels <= L.texturePixels, 'limit');
  }
  const bounds = new Box3(); let selectedBounds: Box3 | undefined;
  for (const root of roots) {
    const box = new Box3().setFromObject(root); bounds.union(box);
    if (root === selected) selectedBounds = box;
  }
  validateBounds(bounds);
  if (selected === undefined) return bounds;
  check(selectedBounds); return validateBounds(selectedBounds);
}

/** Geometry stays in source coordinates; only camera framing changes. */
export function modelScene(model: GeometryModel) {
  validateModelStructure(model);
  const root = new Group(), resources = new Group();
  const palette = new Map<string, MeshStandardMaterial>();
  const material = (color: Color, vertexColors = false) => {
    const key = `${color.toArray().join(',')}:${vertexColors}`;
    let value = palette.get(key);
    if (!value) { check(palette.size < L.materials, 'limit'); value = new MeshStandardMaterial({ color, vertexColors, metalness: 0.05, roughness: 0.65 }); palette.set(key, value); }
    return value;
  };
  try {
    const meshes = model.meshes.map(item => {
      const geometry = new BufferGeometry();
      geometry.setAttribute('position', new BufferAttribute(item.positions, 3));
      if (item.indices) geometry.setIndex(new BufferAttribute(item.indices, 1));
      if (item.normals) geometry.setAttribute('normal', new BufferAttribute(item.normals, 3)); else geometry.computeVertexNormals();
      if (item.colors) geometry.setAttribute('color', new BufferAttribute(item.colors, 3));
      const base = new Color(0.56, 0.65, 0.78); if (item.color) base.setRGB(...item.color);
      const mats = [material(base, !!item.colors)];
      const mesh = new Mesh(geometry, mats); mesh.name = item.name; resources.add(mesh);
      let end = 0;
      for (const group of [...item.groups].sort((a, b) => a.start - b.start)) {
        check(group.start >= end);
        if (end < group.start) geometry.addGroup(end, group.start - end, 0);
        const color = material(new Color().setRGB(...group.color));
        let index = mats.indexOf(color); if (index < 0) { index = mats.length; mats.push(color); }
        geometry.addGroup(group.start, group.count, index); end = group.start + group.count;
      }
      const count = item.indices?.length ?? item.positions.length / 3;
      if (end < count) geometry.addGroup(end, count - end, 0);
      return mesh;
    });
    function node(item: GeometryNode, parent: Group) {
      const group = new Group(); group.name = item.name; parent.add(group);
      for (const index of item.meshes) group.add(meshes[index].clone());
      for (const child of item.children) node(child, group);
    }
    node(model.root, root);
    const bounds = admitScene([root], undefined, meshes.map(mesh => mesh.geometry));
    let previewBytes = 0, previewTriangles = 0;
    return { scene: root, bounds,
      markPreviews(indices: number[]) {
        check(indices.length <= L.parts && new Set(indices).size === indices.length);
        for (const index of indices) {
          check(Number.isInteger(index) && index >= 0 && index < meshes.length);
          check(!model.meshes[index].colors && model.meshes[index].groups.length === 0);
          previews.set(meshes[index].geometry, { pending: true, error: 0 });
        }
      },
      setPreview(index: number, data: GeometryPreviewData) {
        check(Number.isInteger(index) && index >= 0 && index < meshes.length);
        const original = meshes[index].geometry, state = previews.get(original), source = model.meshes[index];
        check(state?.pending && !state.geometry && !source.colors && source.groups.length === 0);
        check(data && data.positions instanceof Float32Array && data.normals instanceof Float32Array && data.sourceVertices instanceof Uint32Array && data.normalVertices instanceof Uint32Array
          && data.positions.length > 0 && data.positions.length % 9 === 0 && data.normals.length === data.positions.length && data.sourceVertices.length === data.positions.length / 3 && data.normalVertices.length === data.sourceVertices.length);
        const sourceBox = original.boundingBox!, size = sourceBox.getSize(new Vector3()), scale = Math.max(size.x, size.y, size.z);
        check(Number.isFinite(data.error) && data.error >= 0 && data.error <= scale * L.previewError);
        previewBytes += data.positions.byteLength + data.normals.byteLength + data.sourceVertices.byteLength + data.normalVertices.byteLength; previewTriangles += data.positions.length / 9;
        check(previewBytes <= L.previewBytes && previewTriangles <= L.previewTriangles, 'limit');
        const input = new Uint32Array(source.positions.buffer, source.positions.byteOffset, source.positions.length), output = new Uint32Array(data.positions.buffer, data.positions.byteOffset, data.positions.length), sourceNormals = original.getAttribute('normal').array as Float32Array;
        const normalInput = new Uint32Array(sourceNormals.buffer, sourceNormals.byteOffset,sourceNormals.length), normalOutput = new Uint32Array(data.normals.buffer,data.normals.byteOffset,data.normals.length);
        for (const stream of [data.positions, data.normals, data.sourceVertices, data.normalVertices]) check(stream.byteOffset === 0 && stream.byteLength === stream.buffer.byteLength);
        for (let vertex = 0; vertex < data.sourceVertices.length; vertex++) {
          const sourceVertex = data.sourceVertices[vertex], normalVertex = data.normalVertices[vertex]; check(sourceVertex < source.positions.length / 3 && normalVertex < sourceNormals.length / 3);
          for (let component = 0; component < 3; component++) {
            check(output[vertex * 3 + component] === input[sourceVertex * 3 + component]);
            check(normalOutput[vertex*3+component] === normalInput[normalVertex*3+component]);
          }
        }
        const box = positionBounds(new BufferAttribute(data.positions, 3)), error = data.error;
        for (const axis of ['x', 'y', 'z'] as const) if (Math.abs(box.min[axis] - sourceBox.min[axis]) > error || Math.abs(box.max[axis] - sourceBox.max[axis]) > error) return;
        const geometry = new BufferGeometry().setAttribute('position', new BufferAttribute(data.positions, 3)).setAttribute('normal', new BufferAttribute(data.normals, 3));
        geometry.addGroup(0, data.sourceVertices.length, 0); geometry.boundingBox = sourceBox.clone(); geometry.boundingSphere = original.boundingSphere!.clone();
        state.geometry = geometry; state.error = error;
      },
      finishPreviews() { for (const mesh of meshes) { const state = previews.get(mesh.geometry); if (state) state.pending = false; } },
    };
  } catch (error) { disposeScene([resources, root]); throw error; }
}

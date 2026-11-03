import { Box3, BufferAttribute, BufferGeometry, Color, Group, Material, Mesh, MeshStandardMaterial, Texture, type Object3D } from 'three';
import { GEOMETRY_LIMITS as L, requireGeometry as check } from './limits';
import { validateModel, type GeometryModel, type GeometryNode } from './model';

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
  for (const item of geometry) item.dispose();
}
export function validateScene(roots: Object3D[]): Box3 {
  const unique = new Set<BufferGeometry>(), mats = materials(roots);
  let vertices = 0, triangles = 0, bytes = 0, nodes = 0, parts = 0, draws = 0, pixels = 0;
  check(mats.size <= L.materials, 'limit');
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
      if (unique.has(geometry)) return; unique.add(geometry); vertices += position.count;
      check(vertices <= L.vertices, 'limit');
      if (geometry.index) for (let index = 0; index < geometry.index.count; index++) { const value = geometry.index.getX(index); check(Number.isSafeInteger(value) && value >= 0 && value < position.count); }
      for (const attribute of Object.values(geometry.attributes)) {
        check(attribute.count === position.count && attribute.itemSize >= 1 && attribute.itemSize <= 4);
        bytes += attribute.count * attribute.itemSize * 4;
        check(bytes <= L.decodedBytes, 'limit');
        for (let index = 0; index < attribute.count; index++) for (let component = 0; component < attribute.itemSize; component++) {
          const value = attribute.getComponent(index, component); check(Number.isFinite(value) && Math.abs(value) <= L.coordinate);
        }
      }
      // File-declared accessor bounds are not an authority for framing or size.
      geometry.computeBoundingBox();
      geometry.computeBoundingSphere();
      const sphere = geometry.boundingSphere;
      check(sphere && sphere.center.toArray().every(value => Number.isFinite(value) && Math.abs(value) <= L.coordinate)
        && Number.isFinite(sphere.radius) && sphere.radius >= 0);
      bytes += geometry.index?.array.byteLength ?? 0;
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
  const bounds = new Box3(); for (const root of roots) bounds.union(new Box3().setFromObject(root));
  check(!bounds.isEmpty(), 'empty');
  check([...bounds.min.toArray(), ...bounds.max.toArray()].every(value => Number.isFinite(value) && Math.abs(value) <= L.coordinate));
  check(Math.max(bounds.max.x - bounds.min.x, bounds.max.y - bounds.min.y, bounds.max.z - bounds.min.z) > 1e-12, 'empty');
  return bounds;
}

/** Geometry stays in source coordinates; only camera framing changes. */
export function modelScene(model: GeometryModel): { scene: Group; bounds: Box3 } {
  validateModel(model);
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
    node(model.root, root); return { scene: root, bounds: validateScene([root]) };
  } catch (error) { disposeScene([resources, root]); throw error; }
}

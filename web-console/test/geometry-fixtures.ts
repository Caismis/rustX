import { BoxGeometry, BufferAttribute, BufferGeometry, PlaneGeometry } from 'three';

/** Original analytical fixtures, generated in the test runner, never in product code. */
function binaryStl(geometry: BufferGeometry): Uint8Array<ArrayBuffer> {
  const positions = geometry.getAttribute('position'), normals = geometry.getAttribute('normal');
  const bytes = new Uint8Array(84 + positions.count / 3 * 50), view = new DataView(bytes.buffer);
  view.setUint32(80, positions.count / 3, true);
  for (let vertex = 0, at = 84; vertex < positions.count; vertex += 3, at += 50) {
    for (let component = 0; component < 3; component++) view.setFloat32(at + component * 4, normals?.getComponent(vertex, component) ?? 0, true);
    for (let corner = 0; corner < 3; corner++) for (let component = 0; component < 3; component++) view.setFloat32(at + 12 + (corner * 3 + component) * 4, positions.getComponent(vertex + corner, component), true);
  }
  geometry.dispose(); return bytes;
}
export function denseBoxStl() {
  const box = new BoxGeometry(1, 1, 1, 166, 166, 166), triangles = box.toNonIndexed(); box.dispose();
  return binaryStl(triangles);
}
export function planeAndTinySolidStl() {
  const plane = new PlaneGeometry(1, 1, 100, 100), flat = plane.toNonIndexed(); plane.dispose();
  const box = new BoxGeometry(1e-6, 1e-6, 1e-6), solid = box.toNonIndexed(); box.dispose(); solid.translate(0, 0, 2);
  const first = flat.getAttribute('position').array, second = solid.getAttribute('position').array, positions = new Float32Array(first.length + second.length);
  positions.set(first); positions.set(second, first.length); flat.dispose(); solid.dispose();
  return binaryStl(new BufferGeometry().setAttribute('position', new BufferAttribute(positions, 3)));
}

// @vitest-environment node
import { expect, it } from 'vitest';
import { Matrix4, Mesh, PerspectiveCamera, PlaneGeometry, Scene, Triangle, Vector2, Vector3 } from 'three';
import { createDisplayQuality } from '../src/app/components/documents/geometry/display-quality';
import { previewJobs, preparePreviews } from '../src/app/components/documents/geometry/preview-data';
import { disposeScene, modelScene } from '../src/app/components/documents/geometry/scene-resources';
import type { GeometryModel } from '../src/app/components/documents/geometry/model';

it.each([false, true])('keeps the admitted original when actual panned perspective surface displacement exceeds one CSS pixel (shear: %s)', async sheared => {
  const source = new PlaneGeometry(2,2,151,151), positions = source.getAttribute('position');
  for (let index = 0; index < positions.count; index++) { const x = positions.getX(index), y = positions.getY(index); positions.setZ(index, 0.004 * (1-x*x) * (1-y*y)); }
  // Keep the plane's authored flat normals while varying its geometry.
  const model: GeometryModel = { meshes: [{ name: 'shallow surface', positions: Float32Array.from(positions.array), normals: Float32Array.from(source.getAttribute('normal').array), indices: Uint32Array.from(source.index!.array), groups: [] }], root: { name: '', meshes: [0], children: [] } };
  const previews = await preparePreviews(previewJobs(model, 'step', new Uint8Array()), Infinity);
  expect(previews).toHaveLength(1); const preview = previews[0].data;
  expect(preview.positions.length / 9).toBe(610);
  const prepared = modelScene(model), mesh = prepared.scene.children[0].children[0] as Mesh, original = mesh.geometry, scene = new Scene(); scene.add(prepared.scene);
  if (sheared) { mesh.matrixAutoUpdate = false; mesh.matrix.copy(new Matrix4().set(1,0,4,0, 0,1,0,0, 0,0,1,0, 0,0,0,1)); scene.updateMatrixWorld(true); }
  prepared.markPreviews([0]); prepared.setPreview(0, preview); prepared.finishPreviews();
  const quality = createDisplayQuality(prepared.scene), width = 4096, height = 480;
  const camera = new PerspectiveCamera(45,width/height,0.001,100); camera.position.set(-3,0,0.9); camera.lookAt(-3,0,0); camera.updateMatrixWorld(true);
  const witness = new Vector3(-0.5231788158416748, 0.01986755058169365, 0.002903989050537348), nearest = new Vector3(), candidate = new Vector3(), a = new Vector3(), b = new Vector3(), c = new Vector3(), triangle = new Triangle(); let distance = Infinity;
  expect(Array.from({ length: positions.count }, (_, index) => witness.distanceToSquared(new Vector3().fromBufferAttribute(positions, index))).some(value => value === 0)).toBe(true);
  witness.applyMatrix4(mesh.matrixWorld);
  for (let at = 0; at < preview.positions.length; at += 9) {
    a.fromArray(preview.positions,at).applyMatrix4(mesh.matrixWorld); b.fromArray(preview.positions,at+3).applyMatrix4(mesh.matrixWorld); c.fromArray(preview.positions,at+6).applyMatrix4(mesh.matrixWorld); triangle.set(a,b,c).closestPointToPoint(witness,candidate);
    const next = witness.distanceToSquared(candidate); if (next < distance) { distance = next; nearest.copy(candidate); }
  }
  const project = (point: Vector3) => { const ndc = point.clone().project(camera); return new Vector2(ndc.x*width/2,ndc.y*height/2); };
  expect(project(witness).distanceTo(project(nearest))).toBeGreaterThan(sheared ? 1 : 1.4);
  const ndc = witness.clone().project(camera); expect(Math.abs(ndc.x)).toBeLessThan(1); expect(Math.abs(ndc.y)).toBeLessThan(1); expect(Math.abs(ndc.z)).toBeLessThan(1);
  try {
    quality.draw(scene,true,camera,width,height,()=>expect(mesh.geometry === original).toBe(true)); expect(mesh.geometry).toBe(original);
    camera.position.set(0,0,3); camera.lookAt(0,0,0);
    quality.draw(scene,true,camera,width,height,()=>expect(mesh.geometry === original).toBe(false));
    // Selection must use this pose before the renderer updates camera matrices.
    camera.position.set(-3,0,0.9); camera.lookAt(-3,0,0);
    quality.draw(scene,true,camera,width,height,()=>expect(mesh.geometry === original).toBe(true));
    camera.position.set(0,0,0.001); camera.lookAt(0,0,-1);
    quality.draw(scene,true,camera,width,height,()=>expect(mesh.geometry === original).toBe(true));
    camera.position.set(0,0,3); camera.lookAt(0,0,0); camera.projectionMatrix.elements[4] = 0.5;
    quality.draw(scene,true,camera,width,height,()=>expect(mesh.geometry === original).toBe(true));
    expect(mesh.geometry).toBe(original);
  }
  finally { quality.dispose(); disposeScene([prepared.scene]); source.dispose(); }
});

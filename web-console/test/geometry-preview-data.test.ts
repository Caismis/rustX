// @vitest-environment node
import { expect, it } from 'vitest';
import { BoxGeometry, PlaneGeometry, SphereGeometry, BufferAttribute, type BufferGeometry } from 'three';
import { importStl } from '../src/app/components/documents/geometry/stl';
import { exactPreview } from '../src/app/components/documents/geometry/exact-preview';
import { modelScene, disposeScene, geometryPreviewState } from '../src/app/components/documents/geometry/scene-resources';
import { previewJobs, preparePreviews } from '../src/app/components/documents/geometry/preview-data';
import type { GeometryModel } from '../src/app/components/documents/geometry/model';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';

it('prepares a complete closed surface instead of thinning independent triangles', async () => {
  const indexed = new BoxGeometry(1,1,1,100,100,100), geometry = indexed.toNonIndexed(); indexed.dispose();
  const positions = geometry.getAttribute('position').array as Float32Array, normals = geometry.getAttribute('normal').array as Float32Array;
  const model: GeometryModel = { meshes:[{name:'box',positions,normals,groups:[]}],root:{name:'',meshes:[0],children:[]} };
  const jobs = previewJobs(model,'stl',new Uint8Array()), previews = await preparePreviews(jobs,performance.now()+L.previewTimeoutMs);
  expect(previews).toHaveLength(1); const preview = previews[0].data, vertices = preview.positions;
  expect(preview.error).toBeLessThanOrEqual(L.previewError); expect(vertices.length/9).toBeLessThan(positions.length/9);
  const points = new Map<string,number>(), edges = new Map<string,number>(); let volume = 0;
  const id = (at:number) => { const key=`${vertices[at]},${vertices[at+1]},${vertices[at+2]}`; if(!points.has(key)) points.set(key,points.size); return points.get(key)!; };
  for(let at=0;at<vertices.length;at+=9) {
    const ids=[id(at),id(at+3),id(at+6)]; for(let side=0;side<3;side++){const a=ids[side],b=ids[(side+1)%3],key=a<b?`${a},${b}`:`${b},${a}`;edges.set(key,(edges.get(key)??0)+1);}
    const p=vertices; volume+=(p[at]*(p[at+4]*p[at+8]-p[at+5]*p[at+7])+p[at+1]*(p[at+5]*p[at+6]-p[at+3]*p[at+8])+p[at+2]*(p[at+3]*p[at+7]-p[at+4]*p[at+6]))/6;
  }
  expect([...edges.values()].every(value=>value===2)).toBe(true); expect(volume).toBeCloseTo(1,4);
  for(let vertex=0;vertex<preview.sourceVertices.length;vertex++) for(let component=0;component<3;component++) expect(vertices[vertex*3+component]).toBe(positions[preview.sourceVertices[vertex]*3+component]);
  geometry.dispose();
});
it('keeps colored/material-seamed geometry out of position-only simplification', () => {
  const positions=new Float32Array(90_000),mesh={name:'',positions,normals:positions,colors:positions,groups:[]};
  expect(previewJobs({meshes:[mesh],root:{name:'',meshes:[0],children:[]}},'stl',new Uint8Array())).toEqual([]);
});

function largeRaw(kind: string) {
  const indexed = kind === 'curved' ? new SphereGeometry(1, 512, 350) : new PlaneGeometry(1, 1, 420, 420);
  if (kind === 'hole') {
    const original = indexed.index!.array, hole = (210 * 420 + 210) * 6, indices = new Uint32Array(original.length - 6);
    indices.set(original.subarray(0, hole)); indices.set(original.subarray(hole + 6), hole); indexed.setIndex(new BufferAttribute(indices, 1));
  }
  const geometry: BufferGeometry = indexed.toNonIndexed(); indexed.dispose();
  const positions = geometry.getAttribute('position'), normals = geometry.getAttribute('normal'), triangles = positions.count / 3;
  const raw = new Uint8Array(84 + triangles * 50), view = new DataView(raw.buffer); view.setUint32(80, triangles, true);
  for (let triangle = 0; triangle < triangles; triangle++) {
    const at = 84 + triangle * 50;
    for (let c = 0; c < 3; c++) view.setFloat32(at + c * 4, kind === 'normal seams' && c === 0 && triangle % 2 ? -0 : normals.getComponent(triangle * 3, c), true);
    for (let corner = 0; corner < 3; corner++) for (let c = 0; c < 3; c++) view.setFloat32(at + 12 + (corner * 3 + c) * 4, positions.getComponent(triangle * 3 + corner, c), true);
  }
  geometry.dispose(); return raw;
}
it.each(['curved', 'hole', 'normal seams'])('retains the standard large raw STL path for %s that exact runs cannot certify', async kind => {
  const raw = largeRaw(kind), model = importStl(raw), jobs = previewJobs(model, 'stl', raw);
  expect(jobs).toHaveLength(1); expect(jobs[0].raw).toBe(raw); expect(exactPreview(raw, Infinity, L.previewTriangles)).toBeUndefined();
  const previews = await preparePreviews(jobs, Infinity); expect(previews).toHaveLength(1);
  expect(previews[0].data.positions.length).toBeLessThan(model.meshes[0].positions.length);
  // The receiver independently checks budgets, source/normal BIT provenance,
  // absolute error and bounds, exactly as it does for an actual Worker result.
  const scene = modelScene(model);
  try {
    scene.markPreviews([0]); scene.setPreview(0, previews[0].data);
    const states: unknown[] = [];
    scene.scene.traverse(item => { if ('geometry' in item) states.push(geometryPreviewState((item as import('three').Mesh).geometry)?.geometry); });
    expect(states.some(Boolean)).toBe(true);
  } finally { disposeScene([scene.scene]); }
});

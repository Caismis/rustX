// @vitest-environment node
import { expect, it } from 'vitest';
import { BoxGeometry } from 'three';
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

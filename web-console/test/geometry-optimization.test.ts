// @vitest-environment node
import { expect, it } from 'vitest';
import { optimizeModel } from '../src/app/components/documents/geometry/optimize';
import { validateModel, type GeometryModel } from '../src/app/components/documents/geometry/model';
import { modelScene, disposeScene } from '../src/app/components/documents/geometry/scene-resources';

function grid(side = 180, hole = false, colored = false): GeometryModel {
  const points: number[] = [], colors: number[] = [];
  for (let y = 0; y < side; y++) for (let x = 0; x < side; x++) {
    if (hole && x >= side / 2 - 1 && x < side / 2 + 1 && y >= side / 2 - 1 && y < side / 2 + 1) continue;
    points.push(x,y,0, x+1,y,0, x,y+1,0, x+1,y,0, x+1,y+1,0, x,y+1,0);
    if (colored) for (let vertex = 0; vertex < 6; vertex++) colors.push(...(x < side / 2 ? [1,0,0] : [0,0,1]));
  }
  const positions = new Float32Array(points), normals = new Float32Array(points.length);
  for (let at = 2; at < normals.length; at += 3) normals[at] = 1;
  const model = { meshes: [{ name: 'Grid', positions, normals, colors: colored ? new Float32Array(colors) : undefined, groups: [] }], root: { name: 'Assembly', meshes: [0], children: [] } };
  validateModel(model); return model;
}
function area(model: GeometryModel) {
  const mesh = model.meshes[0], p = mesh.positions, indices = mesh.indices!; let total = 0;
  for (let at = 0; at < indices.length; at += 3) {
    const a = indices[at]*3, b = indices[at+1]*3, c = indices[at+2]*3;
    total += Math.abs((p[b]-p[a])*(p[c+1]-p[a+1])-(p[b+1]-p[a+1])*(p[c]-p[a]))/2;
  }
  return total;
}
it('reduces a dense coplanar surface while retaining its boundary, area, hierarchy and original arrays', async () => {
  const source = grid(), before = source.meshes[0].positions.slice(), result = await optimizeModel(source);
  expect(result.meshes[0].indices!.length/3).toBeLessThan(2500);
  expect(area(result)).toBe(180*180); expect(result.root).toBe(source.root);
  expect(source.meshes[0].positions).toEqual(before); expect(source.meshes[0].indices).toBeUndefined();
  const admitted = modelScene(result);
  try { expect(admitted.bounds.min.toArray()).toEqual([0,0,0]); expect(admitted.bounds.max.toArray()).toEqual([180,180,0]); }
  finally { disposeScene([admitted.scene]); }
});
it('retains a small hole and hard color seams instead of closing or blending them', async () => {
  const result = await optimizeModel(grid(180, true, true));
  expect(area(result)).toBe(180*180-4);
  const {positions:p,indices,colors} = result.meshes[0];
  for (let at = 0; at < indices!.length; at += 3) {
    const values = [indices![at], indices![at+1], indices![at+2]];
    expect(new Set(values.map(index => colors![index*3])).size).toBe(1);
    for (const index of values) expect(p[index*3] <= 90 || colors![index*3] === 0).toBe(true);
  }
});
it('keeps small meshes and material-range meshes untouched', async () => {
  const small = grid(4); expect(await optimizeModel(small)).toBe(small);
  const colored = grid(); colored.meshes[0].groups.push({start:0,count:3,color:[1,0,0]});
  expect(await optimizeModel(colored)).toBe(colored);
});

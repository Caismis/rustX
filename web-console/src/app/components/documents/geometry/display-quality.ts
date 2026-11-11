import { Box3, BufferGeometry, LineSegments, Matrix4, Mesh, ShaderMaterial, type Camera, type Object3D, type Scene, type Raycaster } from 'three';
import { takeGeometryPreview } from './scene-resources';

/** Upper bound on affine displacement amplification, including shear. Scaling
 * before A^T A avoids overflow/underflow; its absolute row sums bound lambdaMax. */
function displacementScale(matrix: Matrix4) {
  const m = matrix.elements, maximum = Math.max(Math.abs(m[0]),Math.abs(m[1]),Math.abs(m[2]),Math.abs(m[4]),Math.abs(m[5]),Math.abs(m[6]),Math.abs(m[8]),Math.abs(m[9]),Math.abs(m[10]));
  if (maximum === 0) return 0;
  const a=m[0]/maximum,b=m[1]/maximum,c=m[2]/maximum,d=m[4]/maximum,e=m[5]/maximum,f=m[6]/maximum,g=m[8]/maximum,h=m[9]/maximum,i=m[10]/maximum;
  const xy=Math.abs(a*d+b*e+c*f),xz=Math.abs(a*g+b*h+c*i),yz=Math.abs(d*g+e*h+f*i);
  return maximum / Math.abs(m[15]) * Math.sqrt(Math.max(a*a+b*b+c*c+xy+xz,d*d+e*e+f*f+xy+yz,g*g+h*h+i*i+xz+yz)) * (1+32*Number.EPSILON);
}

/** Motion may use a prepared complete surface within one CSS pixel of error.
 * Pending preparation and settled draws retain the admitted original geometry. */
export function createDisplayQuality(root: Object3D) {
  const meshes: Mesh[] = [], lods = new Map<BufferGeometry, { geometry: BufferGeometry; error: number }>();
  root.traverse(item => { if (item instanceof Mesh) meshes.push(item); });
  const originals = meshes.map(mesh => mesh.geometry), viewFromLocal = new Matrix4(), bounds = new Box3();
  function representation(mesh: Mesh, camera: Camera, width: number, height: number) {
    const original = mesh.geometry, incoming = takeGeometryPreview(original);
    if (incoming) lods.set(original, incoming);
    const lod = lods.get(original);
    if (!lod || !original.boundingBox || !Number.isFinite(lod.error) || lod.error < 0 || !Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) return original;
    const p = camera.projectionMatrix.elements;
    if (!p.every(Number.isFinite) || p[1] !== 0 || p[4] !== 0 || p[3] !== 0 || p[7] !== 0) return original;
    const perspective = p[15] === 0 && p[11] < 0 && p[12] === 0 && p[13] === 0;
    const orthographic = p[11] === 0 && p[15] !== 0 && p[8] === 0 && p[9] === 0;
    if (!perspective && !orthographic) return original;
    const divisor = perspective ? -p[11] : p[15], focal = Math.max(width*Math.abs(p[0]/divisor),height*Math.abs(p[5]/divisor))/2;
    if (!Number.isFinite(focal) || focal <= 0) return original;
    viewFromLocal.multiplyMatrices(camera.matrixWorldInverse, mesh.matrixWorld);
    const m=viewFromLocal.elements;
    if (!m.every(Number.isFinite) || m[3] !== 0 || m[7] !== 0 || m[11] !== 0 || m[15] === 0) return original;
    const scale = displacementScale(viewFromLocal); let error = lod.error * scale;
    if (!Number.isFinite(error) || (error === 0 && lod.error !== 0 && scale !== 0)) return original;
    if (error === 0) return lod.geometry;
    const local=original.boundingBox, magnitude=Math.max(Math.abs(local.min.x),Math.abs(local.min.y),Math.abs(local.min.z),Math.abs(local.max.x),Math.abs(local.max.y),Math.abs(local.max.z));
    // Include transform roundoff when large source coordinates and camera
    // translations cancel; both source and approximated points may round.
    const roundoff=64*Number.EPSILON*(3*scale*magnitude+Math.max(Math.abs(m[12]),Math.abs(m[13]),Math.abs(m[14]))/Math.abs(m[15]));
    error += 2*roundoff;
    if (!Number.isFinite(error)) return original;
    if (!perspective) return error*focal*(1+64*Number.EPSILON) <= 1 ? lod.geometry : original;
    bounds.copy(local).applyMatrix4(viewFromLocal).expandByScalar(roundoff);
    const depth=-bounds.max.z, radial=Math.hypot(Math.max(Math.abs(bounds.min.x),Math.abs(bounds.max.x)),Math.max(Math.abs(bounds.min.y),Math.abs(bounds.max.y)));
    if (!Number.isFinite(depth) || !Number.isFinite(radial) || !Number.isFinite(error) || depth <= error) return original;
    // For camera-space p and displacement d:
    // delta(p.xy/p.z) = (d.xy - p.xy*d.z/p.z) / (p.z+d.z).
    // Bound its operator norm and denominator over the complete source box.
    const relative=error/(depth-error), pixels=relative*focal;
    if (relative === 0 || pixels === 0) return original;
    const projected=pixels*Math.hypot(1,radial/depth)*(1+64*Number.EPSILON);
    return Number.isFinite(projected) && projected <= 1 ? lod.geometry : original;
  }
  const withRepresentations = <T>(work: () => T, camera: Camera, width: number, height: number) => {
    camera.updateWorldMatrix(true, false);
    try { meshes.forEach(mesh => { mesh.geometry = representation(mesh, camera, width, height); }); return work(); }
    finally { meshes.forEach((mesh, index) => { mesh.geometry = originals[index]; }); }
  };
  return {
    draw(scene: Scene, moving: boolean, camera: Camera, width: number, height: number, submit: () => void) {
      if (!moving) { submit(); return; }
      const lines: LineSegments[] = [];
      try {
        withRepresentations(() => {
          scene.traverse(item => { if (item instanceof LineSegments && item.material instanceof ShaderMaterial && item.visible) { lines.push(item); item.visible = false; } });
          submit();
        }, camera, width, height);
      } finally { lines.forEach(line => { line.visible = true; }); }
    },
    // A display approximation cannot prove foreground occlusion. Geometry refs
    // are already restored after each submission; pick those admitted originals.
    pick(raycaster: Raycaster, candidates: Mesh[]) { return raycaster.intersectObjects(candidates, false)[0]; },
    refresh() { for (const original of originals) { const incoming = takeGeometryPreview(original); if (incoming) lods.set(original, incoming); } },
    dispose() { for (const value of lods.values()) value.geometry.dispose(); lods.clear(); },
  };
}

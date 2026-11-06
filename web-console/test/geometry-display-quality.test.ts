import { expect, it, vi } from 'vitest';
import { BoxGeometry, PlaneGeometry, Raycaster, Vector3, Group, LineSegments, BufferGeometry, Mesh, MeshStandardMaterial, ShaderMaterial, Scene, OrthographicCamera, PerspectiveCamera } from 'three';
import { createDisplayQuality } from '../src/app/components/documents/geometry/display-quality';
import { modelScene, disposeScene } from '../src/app/components/documents/geometry/scene-resources';

const camera = () => { const value = new PerspectiveCamera(45, 1, .001, 100); value.position.z = 3; value.lookAt(0,0,0); value.updateMatrixWorld(true); return value; };
function generated(side = 100) {
  const indexed = new BoxGeometry(1,1,1,side,side,side), geometry = indexed.toNonIndexed(); indexed.dispose();
  const positions = geometry.getAttribute('position').array as Float32Array, normals = geometry.getAttribute('normal').array as Float32Array; geometry.dispose();
  const prepared = modelScene({ meshes: [{name:'Body',positions,normals,groups:[]}], root:{name:'',meshes:[0],children:[]} });
  const mesh = prepared.scene.children[0].children[0] as Mesh; return { prepared, mesh, positions };
}
it('keeps all geometry, authored material and UV during motion, and restores optional outlines', () => {
  const scene = new Scene(), root = new Group(), geometry = new BoxGeometry(), material = new MeshStandardMaterial(); scene.add(root);
  const mesh = new Mesh(geometry, material), line = new LineSegments(new BufferGeometry(), new ShaderMaterial()); root.add(mesh, line);
  const quality = createDisplayQuality(root);
  try { quality.draw(scene, true, camera(), 480, () => { expect(mesh.geometry).toBe(geometry); expect(mesh.material).toBe(material); expect(line.visible).toBe(false); expect(mesh.geometry.getAttribute('uv')).toBe(geometry.getAttribute('uv')); }); expect(line.visible).toBe(true); }
  finally { quality.dispose(); disposeScene([scene]); line.geometry.dispose(); line.material.dispose(); }
});
it('keeps the intact original while surface preparation is pending or times out, including failed draws', () => {
  const { prepared, mesh } = generated(), original = mesh.geometry, material = mesh.material, scene = new Scene(); scene.add(prepared.scene); prepared.markPreviews([0]);
  const quality = createDisplayQuality(prepared.scene), line = new LineSegments(new BufferGeometry(), new ShaderMaterial()); scene.add(line);
  const positions = original.getAttribute('position'), normals = original.getAttribute('normal'), before = positions.array.slice();
  const disposed = vi.spyOn(original, 'dispose');
  try {
    expect(() => quality.draw(scene, true, camera(), 480, () => {
      expect(mesh.geometry).toBe(original); expect(mesh.geometry.getAttribute('position')).toBe(positions);
      expect(mesh.geometry.getAttribute('normal')).toBe(normals); expect(mesh.material).toBe(material); expect(line.visible).toBe(false); throw new Error('submission failed');
    })).toThrow('submission failed');
    expect(mesh.geometry).toBe(original); expect(mesh.material).toBe(material); expect(line.visible).toBe(true);
    prepared.finishPreviews(); quality.refresh();
    quality.draw(scene, true, camera(), 480, () => expect(mesh.geometry).toBe(original));
    quality.draw(scene, false, camera(), 480, () => expect(mesh.geometry).toBe(original));
    expect(positions.array).toEqual(before); expect(disposed).not.toHaveBeenCalled();
  } finally { quality.dispose(); disposeScene([prepared.scene]); line.geometry.dispose(); line.material.dispose(); }
});
it.each(['perspective', 'orthographic'] as const)('uses a complete surface only within the %s pixel error budget and releases it once', projection => {
  const { prepared, mesh, positions } = generated(10), original = mesh.geometry, scene = new Scene(); scene.add(prepared.scene);
  const cube = new BoxGeometry().toNonIndexed(), p = cube.getAttribute('position').array as Float32Array, n = cube.getAttribute('normal').array as Float32Array;
  const ids = new Map<string, number>(); for (let i=0;i<positions.length;i+=3) ids.set(`${positions[i]},${positions[i+1]},${positions[i+2]}`, i/3);
  const sourceVertices = Uint32Array.from({length:p.length/3}, (_,i) => ids.get(`${p[i*3]},${p[i*3+1]},${p[i*3+2]}`)!);
  const sourceNormals=original.getAttribute('normal').array, normalVertices=new Uint32Array(sourceVertices.length);
  for(let vertex=0;vertex<normalVertices.length;vertex++) for(let source=0;source<sourceNormals.length;source+=3) if(sourceNormals[source]===n[vertex*3] && sourceNormals[source+1]===n[vertex*3+1] && sourceNormals[source+2]===n[vertex*3+2]) {normalVertices[vertex]=source/3;break;}
  prepared.markPreviews([0]); prepared.setPreview(0, { positions:p, normals:n, sourceVertices, normalVertices,error:.002 }); prepared.finishPreviews();
  const quality = createDisplayQuality(prepared.scene); let lod: BufferGeometry | undefined;
  const view = projection === 'perspective' ? camera() : new OrthographicCamera(-1,1,1,-1,.001,100);
  if (view instanceof OrthographicCamera) { view.position.z = 3; view.updateMatrixWorld(true); }
  const material = mesh.material;
  try {
    quality.draw(scene, true, view, 480, () => { lod=mesh.geometry; expect(lod).not.toBe(original); expect(lod.getAttribute('position').count/3).toBe(12); expect(mesh.material).toBe(material); });
    expect(mesh.geometry).toBe(original);
    if (view instanceof PerspectiveCamera) view.position.z = 1.2;
    else view.zoom = 10;
    view.updateProjectionMatrix(); view.updateMatrixWorld(true);
    quality.draw(scene, true, view, 480, () => expect(mesh.geometry).toBe(original));
    if (view instanceof PerspectiveCamera) view.position.z = 3;
    else view.zoom = 1;
    view.updateProjectionMatrix(); view.updateMatrixWorld(true);
    expect(() => quality.draw(scene, true, view, 480, () => { expect(mesh.geometry).toBe(lod); throw new Error('submission failed'); })).toThrow('submission failed');
    expect(mesh.geometry).toBe(original);
    quality.draw(scene, false, view, 480, () => expect(mesh.geometry).toBe(original));
    const disposed = vi.spyOn(lod!, 'dispose'); quality.dispose(); expect(disposed).toHaveBeenCalledOnce();
  } finally { quality.dispose(); disposeScene([prepared.scene]); cube.dispose(); }
});
it('keeps generated motion colors, normals and original PBR references',()=>{
  const {prepared,mesh}=generated(),scene=new Scene();scene.add(prepared.scene);prepared.markPreviews([0]);
  const original=mesh.material,quality=createDisplayQuality(prepared.scene);
  try{quality.draw(scene,true,camera(),480,()=>{expect(mesh.material).toBe(original);expect(mesh.geometry.getAttribute('normal')).toBeDefined();});expect(mesh.material).toBe(original);}
  finally{quality.dispose();disposeScene([prepared.scene]);}
});
it.each([false, true])('picks the opaque dense foreground while surface preparation is pending (moving=%s)', moving => {
  const plane = new PlaneGeometry(1,1,200,200).toNonIndexed(), scene = new Scene();
  const positions = plane.getAttribute('position').array as Float32Array, normals = plane.getAttribute('normal').array as Float32Array;
  const prepared = modelScene({ meshes:[{name:'foreground',positions,normals,groups:[]}], root:{name:'',meshes:[0],children:[]} });
  const front = prepared.scene.children[0].children[0] as Mesh, original = front.geometry;
  const back = new Mesh(new PlaneGeometry(2,2), new MeshStandardMaterial()); back.position.z = -.1;
  scene.add(prepared.scene, back); scene.updateMatrixWorld(true); prepared.markPreviews([0]);
  const quality = createDisplayQuality(scene), raycaster = new Raycaster();
  try {
    quality.draw(scene, moving, camera(), 480, () => {});
    for (let triangle=0;triangle<50;triangle++) {
      const at=triangle*9, x=(positions[at]+positions[at+3]+positions[at+6])/3,y=(positions[at+1]+positions[at+4]+positions[at+7])/3;
      raycaster.set(new Vector3(x,y,1),new Vector3(0,0,-1));
      expect(quality.pick(raycaster,[front,back])?.object).toBe(front);
    }
    expect(front.geometry).toBe(original);
  } finally { quality.dispose(); disposeScene([scene]); plane.dispose(); }
});

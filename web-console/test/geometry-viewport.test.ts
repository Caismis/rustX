import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Box3, BoxGeometry, Group, LineSegments, Mesh, MeshBasicMaterial, MeshStandardMaterial, OrthographicCamera, ShaderMaterial, Vector2, Vector3, type BufferGeometry, type PerspectiveCamera } from 'three';
import { mountGeometry, prepareGeometryGraphics } from '../src/app/components/documents/geometry/viewport';
import { outlineData } from '../src/app/components/documents/geometry/outline-data';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';
import { validateScene, modelScene, disposeScene } from '../src/app/components/documents/geometry/scene-resources';
import { en } from '../src/locale/dictionaries/geometry';
import { parseGeometry } from '../src/app/components/documents/geometry/parse';
import { cameraFromPose, progressivePresentation } from '../src/app/components/documents/geometry/progressive';
const runtime = vi.hoisted(() => ({ renderers: [] as any[], controls: [] as any[] }));
vi.mock('three', async original => {
  const actual = await original<typeof import('three')>();
  return { ...actual, WebGLRenderer: class {
    domElement = document.createElement('canvas'); size = new actual.Vector2();
    gl = { TIMEOUT_EXPIRED: 1, WAIT_FAILED: 2, ALREADY_SIGNALED: 3, SYNC_GPU_COMMANDS_COMPLETE: 4,
      clientWaitSync: vi.fn(() => 3), fenceSync: vi.fn(() => ({})), deleteSync: vi.fn(), flush: vi.fn() };
    getContext = () => this.gl;
    setDrawingBufferSize = vi.fn((width: number, height: number) => this.size.set(width, height));
    getSize = (target: Vector2) => target.copy(this.size);
    setViewport = vi.fn();compile=vi.fn(); render = vi.fn(); clearDepth = vi.fn(); dispose = vi.fn(); forceContextLoss = vi.fn();
    constructor() { runtime.renderers.push(this); }
  } };
});
vi.mock('three/addons/controls/OrbitControls.js', async () => {
  const { EventDispatcher, Vector3 } = await import('three');
  return { OrbitControls: class extends EventDispatcher<any> {
    mouseButtons = {}; touches = {};
    target = new Vector3(); dispose = vi.fn(); listenToKeyEvents = vi.fn();
    constructor(readonly object: PerspectiveCamera | OrthographicCamera) { super(); runtime.controls.push(this); }
    update() { this.object.lookAt(this.target); this.object.updateMatrixWorld(true); this.dispatchEvent({ type: 'change' }); }
  } };
});
let resize: () => void, disconnect: ReturnType<typeof vi.fn>, frames: Map<number, FrameRequestCallback>, microtasks:(()=>void)[],queries: { listener?: () => void; removeEventListener: ReturnType<typeof vi.fn> }[];
const mounted: ReturnType<typeof mountGeometry>[] = [];
beforeEach(() => {
  vi.useFakeTimers({ now: 0, toFake: ['setTimeout', 'clearTimeout', 'performance'] });
  runtime.renderers.length = 0; runtime.controls.length = 0; frames = new Map();microtasks=[]; queries = []; disconnect = vi.fn();
  vi.stubGlobal('queueMicrotask',(callback:()=>void)=>{microtasks.push(callback);});
  vi.stubGlobal('devicePixelRatio', 1);
  vi.stubGlobal('ResizeObserver', class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect = disconnect; });
  let frame = 0;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++frame, callback); return frame; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  vi.stubGlobal('matchMedia', () => { const query = { listener: undefined as (() => void) | undefined, addEventListener: (_type: string, listener: () => void) => { query.listener = listener; }, removeEventListener: vi.fn(() => { query.listener = undefined; }) }; queries.push(query); return query; });
});
afterEach(() => { mounted.splice(0).forEach(view => view.dispose()); vi.useRealTimers(); vi.unstubAllGlobals(); });
function mount(object = new Group(), saved?: Parameters<typeof mountGeometry>[3], options?: Parameters<typeof mountGeometry>[7]) {
  if (!object.children.length) object.add(new Mesh(new BoxGeometry(), new MeshBasicMaterial()));
  const element = document.createElement('div'); Object.defineProperties(element, { clientWidth: { value: 640, configurable: true }, clientHeight: { value: 480, configurable: true } });
  const changed = vi.fn(), lost = vi.fn(), view = mountGeometry(element, object, new Box3().setFromObject(object), saved, en, changed, lost, options); mounted.push(view);
  return { view, changed, lost, element, renderer: runtime.renderers.at(-1), control: () => runtime.controls.at(-1) };
}
function draw() { while(microtasks.length)microtasks.shift()!();const pending = [...frames.values()]; frames.clear(); pending.forEach(callback => callback(0)); }
function move(control: any, x: number) { control.object.position.x = x; control.update(); }
function tier(level: 0 | 1 = 0, x=0) {
  const value=modelScene({meshes:[0,2].map(offset=>({name:`Surface ${offset}`,positions:new Float32Array([x+offset,0,0,x+offset+1,0,0,x+offset,1,0]),groups:[]})),root:{name:'Assembly',meshes:[],children:[0,1].map(index=>({name:`Part ${index}`,meshes:[index],children:[]}))}});
  return {level,scene:value.scene,bounds:value.bounds,dispose:vi.fn(()=>disposeScene([value.scene]))};
}
it('mounts a hidden body without a frame, buffer allocation, timers or a replacement saved camera', () => {
  const camera = { projection: 'orthographic' as const, verticalSpan: 2, position: [3, 2, 4] as [number, number, number], target: [0, 0, 0] as [number, number, number], up: [0, 1, 0] as [number, number, number] };
  const f = mount(undefined, { copyId: 'copy', camera }, { generated: true, visible: false });
  resize(); draw(); vi.advanceTimersByTime(1000); draw();
  expect(f.renderer.render).not.toHaveBeenCalled(); expect(f.renderer.setDrawingBufferSize).not.toHaveBeenCalled();
  expect(f.control().enabled).toBe(false); expect(f.changed).not.toHaveBeenCalled(); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0);
  Object.defineProperty(f.element, 'clientWidth', { value: 800 }); vi.stubGlobal('devicePixelRatio', 1.5);
  f.view.setVisible(true); draw();
  expect(f.control().enabled).toBe(true); expect(f.renderer.setDrawingBufferSize).toHaveBeenCalledExactlyOnceWith(800, 480, .5);
  expect(f.changed).toHaveBeenCalledOnce(); expect(f.changed.mock.lastCall![0].camera).toEqual(camera);
  vi.advanceTimersByTime(L.refineDelayMs); draw(); expect(f.renderer.setDrawingBufferSize).toHaveBeenLastCalledWith(800, 480, 1.5);
});
it('pauses pending GPU fence polling and refinement and resumes the retained renderer and pose', () => {
  const ready = vi.fn(), f = mount(undefined, undefined, { generated: true, onFirstGpuReady: ready });
  draw(); f.renderer.gl.clientWaitSync.mockReturnValue(f.renderer.gl.TIMEOUT_EXPIRED); draw();
  const callbacks = [...frames.values()], calls = f.renderer.gl.clientWaitSync.mock.calls.length;
  f.view.setVisible(false); callbacks.forEach(callback => callback(0)); resize();
  vi.advanceTimersByTime(1000); draw();
  expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0); expect(ready).not.toHaveBeenCalled();
  expect(f.renderer.gl.clientWaitSync).toHaveBeenCalledTimes(calls); expect(f.renderer.render).toHaveBeenCalledOnce();
  expect(f.renderer.dispose).not.toHaveBeenCalled(); expect(f.renderer.gl.deleteSync).not.toHaveBeenCalled();
  const camera = f.control().object.position.clone(); f.renderer.gl.clientWaitSync.mockReturnValue(f.renderer.gl.ALREADY_SIGNALED);
  f.view.setVisible(true); draw();
  expect(runtime.renderers).toHaveLength(1); expect(f.control().object.position.equals(camera)).toBe(true); expect(ready).toHaveBeenCalledOnce();
  expect(f.renderer.render).toHaveBeenCalledTimes(2);
});
it('starts no hidden outlines and fences an outline request waiting for imported surface preparation', async () => {
  const worker = vi.fn(); vi.stubGlobal('Worker', class { constructor() { worker(); } });
  const f = mount(undefined, undefined, { generated: true, visible: false, pendingPreview: true });
  expect(await f.view.setOutlines(true)).toBe('off'); expect(worker).not.toHaveBeenCalled();
  f.view.setVisible(true); const request = f.view.setOutlines(true); f.view.setVisible(false); f.view.refreshPreview();
  expect(await request).toBe('off'); expect(worker).not.toHaveBeenCalled(); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0);
  outlineWorker(); f.view.setVisible(true); expect(await f.view.setOutlines(true)).toBe('on');
});
it('lets an in-flight outline finish while hidden without scheduling or submitting a frame', async () => {
  let finish: (() => void) | undefined;
  vi.stubGlobal('Worker', class {
    onmessage: any; onerror: any; onmessageerror: any; terminate() {}
    postMessage({ inputs }: { inputs: Parameters<typeof outlineData>[0][] }) {
      finish = () => this.onmessage?.({ data: { ok: true, data: inputs.map(value => outlineData(value, L.outlineSegments, performance.now() + 2000)) } });
    }
  });
  const f = mount(undefined, undefined, { generated: true }), request = f.view.setOutlines(true);
  await vi.dynamicImportSettled(); expect(finish).toBeDefined(); f.view.setVisible(false); finish!();
  expect(await request).toBe('on'); draw();
  expect(f.renderer.render).not.toHaveBeenCalled(); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0);
  f.view.setVisible(true); draw(); expect(f.renderer.render).toHaveBeenCalledOnce();
});
it('keeps hidden progressive tiers unpainted and hands final content to a hidden body with one renderer', async () => {
  const element = document.createElement('div'); Object.defineProperties(element, { clientWidth: { value: 640, configurable: true }, clientHeight: { value: 480, configurable: true } });
  const graphics = prepareGeometryGraphics(true, vi.fn(), { visible: false }), displayed = vi.fn(), show = vi.fn(), a = tier();
  const owner = progressivePresentation({ container: element, graphics, visible: false, current: () => true, copyId: 'copy', labels: () => en, mounted: show, displayed, failed: vi.fn() });
  try {
    owner.add(a); draw(); expect(show).not.toHaveBeenCalled(); expect(frames.size).toBe(0); expect(graphics.renderer.compile).not.toHaveBeenCalled();
    await owner.complete(); owner.dispose(); expect(a.dispose).toHaveBeenCalledOnce();
    const object = new Group(); object.add(new Mesh(new BoxGeometry(), new MeshBasicMaterial()));
    const full = mountGeometry(element, object, new Box3().setFromObject(object), undefined, en, vi.fn(), vi.fn(), { generated: true, graphics, visible: false }); mounted.push(full);
    draw(); expect(graphics.renderer.render).not.toHaveBeenCalled(); expect(graphics.renderer.setDrawingBufferSize).not.toHaveBeenCalled(); expect(displayed).not.toHaveBeenCalled();
    full.setVisible(true); draw(); expect(runtime.renderers).toHaveLength(1); expect(graphics.renderer.render).toHaveBeenCalledOnce();
  } finally { owner.dispose(); graphics.dispose(); }
});
it('settles a hidden progressive gesture immediately and retains its last visible framing for handoff', async () => {
  const element = document.createElement('div'); Object.defineProperties(element, { clientWidth: { value: 640, configurable: true }, clientHeight: { value: 480, configurable: true } });
  const graphics = prepareGeometryGraphics(true, vi.fn()), a = tier(); let preview: ReturnType<typeof mountGeometry> | undefined;
  const owner = progressivePresentation({ container: element, graphics, current: () => true, copyId: 'copy', labels: () => en, mounted: view => { preview = view; }, displayed: vi.fn(), failed: vi.fn() });
  try {
    owner.add(a); draw(); draw(); preview!.setProjection('perspective'); const control = runtime.controls.at(-1);
    control.dispatchEvent({ type: 'start' }); move(control, control.object.position.x + 1);
    const expected = control.object.position.clone(); const pending = owner.complete();
    Object.defineProperties(element, { clientWidth: { value: 0 }, clientHeight: { value: 0 } }); owner.setVisible(false);
    const seed = await pending; expect(preview).toBeUndefined(); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0);
    const camera = cameraFromPose(seed!.pose!, a.bounds, seed!.viewportSize, seed!.projection);
    expect(new Vector3().fromArray(camera.position).distanceTo(expected)).toBeLessThan(1e-12);
    expect(graphics.renderer.dispose).not.toHaveBeenCalled();
    const final = tier();
    const full = mountGeometry(element, final.scene, final.bounds, { copyId: 'copy', camera }, en, vi.fn(), vi.fn(), { generated: true, graphics, visible: false }); mounted.push(full);
    owner.dispose(); expect(a.dispose).toHaveBeenCalledOnce(); draw();
    expect(frames.size).toBe(0); expect(runtime.renderers).toHaveLength(1);
    Object.defineProperties(element, { clientWidth: { value: 640 }, clientHeight: { value: 480 } });
    full.setVisible(true); expect(runtime.controls.at(-1).object.position.distanceTo(expected)).toBeLessThan(1e-12);
    full.dispose(); final.dispose();
  } finally { owner.dispose(); graphics.dispose(); }
});
it('replaces whole-model tiers, waits for the active gesture, and upgrades with one context', async () => {
  const element=document.createElement('div');Object.defineProperties(element,{clientWidth:{value:640},clientHeight:{value:480}});
  const graphics=prepareGeometryGraphics(true,vi.fn()),displayed=vi.fn(),failed=vi.fn(),a=tier(),b=tier(1,2);
  let preview: ReturnType<typeof mountGeometry> | undefined;
  const owner=progressivePresentation({container:element,graphics,current:()=>true,copyId:'copy',labels:()=>en,mounted:view=>{preview=view;},displayed,failed});
  try {
    owner.add(a);draw();draw();draw();expect(displayed).toHaveBeenCalledOnce();
    const renderer=runtime.renderers[0];expect(renderer.domElement.dataset.geometryCompleteness).toBe('preview');expect(renderer.domElement.dataset.geometryLod).toBe('coarse');expect(runtime.controls.at(-1).object).toBeInstanceOf(OrthographicCamera);
    preview!.setProjection('perspective');
    const graph=a.scene.parent;
    renderer.compile.mockImplementation(() => {
      expect(graph?.children).toEqual([b.scene]); expect(a.dispose).not.toHaveBeenCalled();
      const names:string[]=[];graph!.traverse(part=>{if(part instanceof Mesh)names.push(part.name);});expect(names).toEqual(['Surface 0','Surface 2']);
    });
    owner.add(b);draw();draw();renderer.compile.mockReset();expect(a.scene.parent).toBeNull();expect(graph?.children).toEqual([b.scene]);expect(a.dispose).toHaveBeenCalledOnce();expect(renderer.domElement.dataset.geometryLod).toBe('medium');
    const control=runtime.controls.at(-1);control.dispatchEvent({type:'start'});move(control,control.object.position.x+1);
    const expectedPosition=control.object.position.clone().add(a.bounds.getCenter(new Vector3())).sub(b.bounds.getCenter(new Vector3()));
    const expectedTarget=control.target.clone().add(a.bounds.getCenter(new Vector3())).sub(b.bounds.getCenter(new Vector3()));
    let completed=false;const pending=owner.complete().then(value=>{completed=true;return value;});await Promise.resolve();expect(completed).toBe(false);
    control.dispatchEvent({type:'end'});await vi.advanceTimersByTimeAsync(119);expect(completed).toBe(false);
    await vi.advanceTimersByTimeAsync(1);const seed=await pending;expect(seed?.pose?.direction.every(Number.isFinite)).toBe(true);
    const restored=cameraFromPose(seed!.pose!,b.bounds,element,seed!.projection);
    expect(new Vector3().fromArray(restored.position).distanceTo(expectedPosition)).toBeLessThan(1e-12);expect(new Vector3().fromArray(restored.target).distanceTo(expectedTarget)).toBeLessThan(1e-12);
    expect(renderer.dispose).not.toHaveBeenCalled();expect(runtime.renderers).toHaveLength(1);
    const object=new Group();object.add(new Mesh(new BoxGeometry(),new MeshBasicMaterial()));
    const full=mountGeometry(element,object,new Box3().setFromObject(object),undefined,en,vi.fn(),vi.fn(),{generated:true,graphics});mounted.push(full);
    owner.dispose();expect(a.dispose).toHaveBeenCalledOnce();expect(b.dispose).toHaveBeenCalledOnce();expect(renderer.domElement.dataset.geometryCompleteness).toBe('complete');
    expect(runtime.renderers).toHaveLength(1);full.dispose();expect(renderer.dispose).toHaveBeenCalledOnce();expect(failed).not.toHaveBeenCalled();
  } finally {owner.dispose();graphics.dispose();}
});
it('disposes superseded unpainted tiers and mounts only the newest whole-model graph', () => {
  const graphics=prepareGeometryGraphics(true,vi.fn()),coarse=tier(),medium=tier(1),mounted=vi.fn();
  const owner=progressivePresentation({container:document.createElement('div'),graphics,current:()=>true,copyId:'copy',labels:()=>en,mounted,displayed:vi.fn(),failed:vi.fn()});
  try {
    owner.add(coarse);owner.add(medium);expect(coarse.dispose).toHaveBeenCalledOnce();expect(mounted).not.toHaveBeenCalled();
    draw();draw();expect(runtime.controls).toHaveLength(1);expect(runtime.renderers[0].domElement.dataset.geometryLod).toBe('medium');
    expect(medium.scene.parent?.children).toEqual([medium.scene]);expect(medium.dispose).not.toHaveBeenCalled();
  } finally {owner.dispose();graphics.dispose();}
  expect(medium.dispose).toHaveBeenCalledOnce();
});
it('uses only latest-tier bounds for reset and skips pending refinement when final is ready', async () => {
  const element=document.createElement('div');Object.defineProperties(element,{clientWidth:{value:640},clientHeight:{value:480}});
  const graphics=prepareGeometryGraphics(true,vi.fn()),coarse=tier(0,1000),medium=tier(1,-1000);let view:ReturnType<typeof mountGeometry>|undefined;
  const owner=progressivePresentation({container:element,graphics,current:()=>true,copyId:'copy',labels:()=>en,mounted:value=>{view=value;},displayed:vi.fn(),failed:vi.fn()});
  try {
    owner.add(coarse);draw();draw();owner.add(medium);draw();draw();view!.reset();
    expect(runtime.controls.at(-1).target.x).toBe(-2000);
    const seed=await owner.complete();expect(seed).toBeUndefined();
    const late=tier(1);owner.add(late);expect(late.dispose).toHaveBeenCalledOnce();
  } finally {owner.dispose();graphics.dispose();}
});
it('disposes an unpainted newer tier immediately when a displayed tier hands off to final', async () => {
  const graphics=prepareGeometryGraphics(true,vi.fn()),coarse=tier(),medium=tier(1),show=vi.fn();
  const owner=progressivePresentation({container:document.createElement('div'),graphics,current:()=>true,copyId:'copy',labels:()=>en,mounted:show,displayed:vi.fn(),failed:vi.fn()});
  try {
    owner.add(coarse);draw();draw();const compilations=runtime.renderers[0].compile.mock.calls.length;
    owner.add(medium);await owner.complete();draw();expect(medium.dispose).toHaveBeenCalledOnce();expect(runtime.renderers[0].compile).toHaveBeenCalledTimes(compilations);
    expect(show.mock.lastCall).toEqual([undefined]);
  } finally {owner.dispose();graphics.dispose();}
});
it('drops an unpainted LOD when the final result is already available, without another view', async () => {
  const graphics=prepareGeometryGraphics(true,vi.fn()),show=vi.fn(),a=tier();
  const owner=progressivePresentation({container:document.createElement('div'),graphics,current:()=>true,copyId:'copy',labels:()=>en,mounted:show,displayed:vi.fn(),failed:vi.fn()});
  try{owner.add(a);await owner.complete();draw();expect(show).toHaveBeenCalledExactlyOnceWith(undefined);expect(runtime.controls).toHaveLength(0);owner.dispose();expect(a.dispose).toHaveBeenCalledOnce();}
  finally{owner.dispose();graphics.dispose();}
});
it('settles a progressive handoff on retirement and fences captured late rendering callbacks', async () => {
  const graphics=prepareGeometryGraphics(true,vi.fn()),show=vi.fn(),a=tier();
  const owner=progressivePresentation({container:document.createElement('div'),graphics,current:()=>true,copyId:'copy',labels:()=>en,mounted:vi.fn(),displayed:show,failed:vi.fn()});
  owner.add(a);draw();draw();draw();runtime.controls.at(-1).dispatchEvent({type:'start'});
  const late=[...frames.values()],pending=owner.complete();owner.dispose();expect(await pending).toBeUndefined();late.forEach(callback=>callback(0));
  expect(show).toHaveBeenCalledOnce();expect(a.dispose).toHaveBeenCalledOnce();expect(runtime.renderers[0].dispose).toHaveBeenCalledOnce();
  const next=prepareGeometryGraphics(true,vi.fn());next.dispose();graphics.dispose();
});
it('keeps a provisional graph out of picking and outline preparation', async () => {
  const f=mount(undefined,undefined,{generated:true,provisional:true});f.changed.mockClear();
  f.view.select(f.view.parts.find(part=>part.kind==='mesh')!.id);expect(f.changed).not.toHaveBeenCalled();expect(await f.view.setOutlines(true)).toBe('off');
  expect(f.renderer.domElement.dataset.geometryCompleteness).toBe('preview');
  draw();expect(f.renderer.domElement.dataset.geometryQuality).toBe('preview');
});
it('hands progressive content to the complete view through one renderer/context and retires old callbacks', () => {
  const oldReady=vi.fn(),nextReady=vi.fn(),f=mount(undefined,undefined,{generated:true,onFirstGpuReady:oldReady}); draw();
  const late=[...frames.values()],graphics=f.view.handoff();
  expect(graphics).toBeDefined();expect(f.renderer.dispose).not.toHaveBeenCalled();expect(f.renderer.forceContextLoss).not.toHaveBeenCalled();
  const object=new Group();object.add(new Mesh(new BoxGeometry(),new MeshBasicMaterial()));
  const next=mountGeometry(f.element,object,new Box3().setFromObject(object),undefined,en,vi.fn(),vi.fn(),{generated:true,graphics,onFirstGpuReady:nextReady});mounted.push(next);
  expect(runtime.renderers).toHaveLength(1);late.forEach(callback=>callback(0));expect(oldReady).not.toHaveBeenCalled();expect(nextReady).not.toHaveBeenCalled();
  draw();draw();expect(nextReady).toHaveBeenCalledOnce();f.view.dispose();expect(f.renderer.dispose).not.toHaveBeenCalled();
  next.dispose();expect(f.renderer.dispose).toHaveBeenCalledOnce();expect(f.renderer.forceContextLoss).toHaveBeenCalledOnce();
});
it('serializes two panes manual outlines behind pending import Workers and preserves Off through settlement', async () => {
  const workers: any[] = []; let live = 0, peak = 0;
  vi.stubGlobal('Worker', class {
    onmessage: any; onerror: any; onmessageerror: any; ended = false;
    constructor(_url: unknown, readonly options: WorkerOptions) { workers.push(this); peak = Math.max(peak, ++live); }
    postMessage(input: any) {
      if (input.inputs) void Promise.resolve().then(() => this.onmessage?.({data:{ok:true,data:input.inputs.map((value:Parameters<typeof outlineData>[0])=>outlineData(value,L.outlineSegments,performance.now()+2000))}}));
    }
    terminate() { if (!this.ended) { this.ended = true; live--; } }
  });
  const owners: Awaited<ReturnType<typeof parseGeometry>>[] = [], views: ReturnType<typeof mountGeometry>[] = [], changes: ReturnType<typeof vi.fn>[] = [];
  try {
    for (let i=0;i<2;i++) {
      const pending = parseGeometry(new Uint8Array([1]),'stl',new AbortController().signal);
      workers.at(-1).onmessage({data:{ok:true,model:{meshes:[{name:'',positions:new Float32Array([0,0,0,1,0,0,0,1,0]),normals:new Float32Array([0,0,1,0,0,1,0,0,1]),groups:[]}],root:{name:'',meshes:[0],children:[]}},pendingPreviews:[0]}});
      const owner = await pending; owners.push(owner);
      const element=document.createElement('div');Object.defineProperties(element,{clientWidth:{value:640},clientHeight:{value:480}});
      const changed=vi.fn();changes.push(changed);views.push(mountGeometry(element,owner.scene,owner.bounds,undefined,en,changed,vi.fn(),{generated:true,pendingPreview:true}));
    }
    const requests=views.map(view=>view.setOutlines(true));
    await vi.dynamicImportSettled(); expect(live).toBe(2); expect(peak).toBe(2);
    const off=views[0].setOutlines(false); expect(await off).toBe('off');
    for (const worker of workers.slice(0,2)) worker.onmessage?.({data:{kind:'preview',previews:[]}});
    await Promise.all(owners.map(owner=>owner.ready)); views.forEach(view=>view.refreshPreview());
    expect(await Promise.all(requests)).toEqual(['off','on']); expect(peak).toBe(2); expect(live).toBe(0);
    expect(changes[0].mock.lastCall![0].outlines).toBe(false); expect(views[0].inspect().triangles).toBe(1);
  } finally { views.forEach(view=>view.dispose());owners.forEach(owner=>owner.dispose()); }
});
it('owns bounded early shader preparation without submitting a model or leaking context admission', () => {
  const lost = vi.fn(), first = prepareGeometryGraphics(true, lost), second = prepareGeometryGraphics(false, lost);
  try {
    expect(() => prepareGeometryGraphics(true, lost)).toThrow('capacity');
    const warm = runtime.renderers[0].compile.mock.calls[0][0] as Group, mesh = warm.children.find(item => item instanceof Mesh) as Mesh;
    expect(mesh.geometry.getAttribute('position').count).toBe(3); expect(first.renderer.render).not.toHaveBeenCalled();
    expect(second.renderer.compile).not.toHaveBeenCalled();
    const geometry = vi.spyOn(mesh.geometry, 'dispose'), material = vi.spyOn(mesh.material as MeshStandardMaterial, 'dispose');
    first.renderer.domElement.dispatchEvent(new Event('webglcontextlost', { cancelable: true }));
    first.dispose(); expect(lost).toHaveBeenCalledOnce(); expect(geometry).toHaveBeenCalledOnce(); expect(material).toHaveBeenCalledOnce();
    expect(first.renderer.dispose).toHaveBeenCalledOnce();
    const next = prepareGeometryGraphics(false, lost); next.dispose();
  } finally { first.dispose(); second.dispose(); }
});
it('coalesces poses while the GPU owns a frame and releases its fence on retirement', () => {
  const f = mount(); draw(); expect(f.renderer.render).toHaveBeenCalledTimes(1);
  f.renderer.gl.clientWaitSync.mockReturnValue(f.renderer.gl.TIMEOUT_EXPIRED);
  move(f.control(), 2); draw(); move(f.control(), 3); draw();
  expect(f.renderer.render).toHaveBeenCalledTimes(1); expect(frames.size).toBe(1);
  f.renderer.gl.clientWaitSync.mockReturnValue(f.renderer.gl.ALREADY_SIGNALED); draw();
  expect(f.renderer.render).toHaveBeenCalledTimes(2); expect(frames.size).toBe(0);
  f.view.dispose(); expect(f.renderer.gl.deleteSync).toHaveBeenCalledTimes(2);
});
it('starts background work only after the first GPU fence settles, without another submitted frame', () => {
  const ready=vi.fn(),f=mount(undefined,undefined,{generated:true,onFirstGpuReady:ready}); draw();
  expect(ready).not.toHaveBeenCalled(); f.renderer.gl.clientWaitSync.mockReturnValue(f.renderer.gl.TIMEOUT_EXPIRED); draw(); expect(ready).not.toHaveBeenCalled();
  f.renderer.gl.clientWaitSync.mockReturnValue(f.renderer.gl.ALREADY_SIGNALED); draw(); expect(ready).toHaveBeenCalledOnce(); expect(f.renderer.render).toHaveBeenCalledTimes(1);
  f.view.dispose(); expect(frames.size).toBe(0);
});
it('keeps the admitted surface intact before simplification is ready, including motion, and stops drawing after settling', () => {
  const indexed = new BoxGeometry(1, 1, 1, 100, 100, 100), geometry = indexed.toNonIndexed(); indexed.dispose();
  const prepared = modelScene({ meshes:[{name:'',positions:geometry.getAttribute('position').array as Float32Array,normals:geometry.getAttribute('normal').array as Float32Array,groups:[]}],root:{name:'',meshes:[0],children:[]} }); geometry.dispose(); prepared.markPreviews([0]);
  const object = prepared.scene, mesh = object.children[0].children[0] as Mesh, original = mesh.geometry;
  const f = mount(object), before = f.view.inspect(), samples: { triangles: number; material: unknown }[] = [];
  f.renderer.render.mockImplementation((scene: Group & { overrideMaterial?: unknown }) => {
    if (scene.children.includes(object.parent!)) samples.push({ triangles: (mesh.geometry.index?.count ?? mesh.geometry.getAttribute('position').count) / 3, material: mesh.material });
  });
  draw(); expect(samples[0].triangles).toBe(120_000); expect(samples[0].material).toBe(mesh.material);
  expect(mesh.geometry).toBe(original); expect(f.view.inspect()).toEqual(before);
  vi.advanceTimersByTime(150); draw(); expect(samples.at(-1)!.triangles).toBe(120_000); expect(samples.at(-1)!.material).toBe(mesh.material);
  f.control().dispatchEvent({ type: 'start' }); move(f.control(), 4); draw(); expect(samples.at(-1)!.triangles).toBe(120_000);
  vi.advanceTimersByTime(300); draw(); expect(samples.at(-1)!.triangles).toBe(120_000);
  f.control().dispatchEvent({ type: 'end' }); vi.advanceTimersByTime(150); draw(); expect(samples.at(-1)!.triangles).toBe(120_000);
  const draws = samples.length; vi.advanceTimersByTime(1000); draw(); expect(samples).toHaveLength(draws);
  expect(mesh.geometry).toBe(original); expect(f.view.inspect()).toEqual(before);
});
it('schedules no refinement for a gesture that leaves the complete pose unchanged', () => {
  const f = mount(); draw(); vi.advanceTimersByTime(L.refineDelayMs); draw(); const calls = f.renderer.render.mock.calls.length;
  f.control().dispatchEvent({ type: 'start' }); f.control().dispatchEvent({ type: 'end' }); vi.advanceTimersByTime(1000); draw();
  expect(f.renderer.render).toHaveBeenCalledTimes(calls); expect(vi.getTimerCount()).toBe(0); expect(frames.size).toBe(0);
});
it.each(['creation', 'wait'])('retires graphics and reports a GPU-fence %s failure', failure => {
  const f = mount();
  if (failure === 'creation') f.renderer.gl.fenceSync.mockReturnValue(null);
  else { draw(); f.renderer.gl.clientWaitSync.mockReturnValue(f.renderer.gl.WAIT_FAILED); move(f.control(), 2); }
  draw(); expect(f.lost).toHaveBeenCalledOnce(); expect(f.renderer.dispose).toHaveBeenCalledOnce(); expect(frames.size).toBe(0);
});
it('publishes moving cameras at most every 100 ms and flushes the latest immutable pose on end and disposal', () => {
  const f = mount(); const initial = f.changed.mock.calls[0][0]; f.changed.mockClear();
  for (let i = 1; i <= 9; i++) { move(f.control(), i); vi.advanceTimersByTime(10); }
  expect(f.changed).not.toHaveBeenCalled(); vi.advanceTimersByTime(10);
  expect(f.changed).toHaveBeenCalledOnce(); expect(f.changed.mock.lastCall![0].camera.position[0]).toBe(9);
  move(f.control(), 11); f.control().dispatchEvent({ type: 'end' });
  expect(f.changed).toHaveBeenCalledTimes(2); expect(f.changed.mock.lastCall![0].camera.position[0]).toBe(11);
  move(f.control(), 12); f.view.dispose();
  expect(f.changed).toHaveBeenCalledTimes(3); expect(f.changed.mock.lastCall![0].camera.position[0]).toBe(12); expect(initial.camera.position[0]).not.toBe(12);
  vi.advanceTimersByTime(1000); expect(f.changed).toHaveBeenCalledTimes(3); expect(frames.size).toBe(0);
});
it('keeps pointer-gesture metadata out of workspace/React updates and flushes its exact final pose', () => {
  const f = mount(); f.changed.mockClear(); f.control().dispatchEvent({ type: 'start' });
  for (let i = 0; i < 20; i++) { move(f.control(), i + 1); vi.advanceTimersByTime(100); }
  expect(f.changed).not.toHaveBeenCalled();
  f.control().dispatchEvent({ type: 'end' });
  expect(f.changed).toHaveBeenCalledOnce(); expect(f.changed.mock.lastCall![0].camera.position[0]).toBe(20);
  f.control().dispatchEvent({ type: 'start' }); move(f.control(), 21); f.view.dispose();
  expect(f.changed.mock.lastCall![0].camera.position[0]).toBe(21); expect(vi.getTimerCount()).toBe(0);
});
it('restores hidden parts in one publication and publishes selection and visibility immediately', () => {
  const object = new Group(); for (let i = 0; i < 20; i++) object.add(new Mesh(new BoxGeometry(), new MeshBasicMaterial()));
  const f = mount(object, { copyId: 'copy', hidden: Array.from({ length: 20 }, (_, i) => `0.${i}`), selected: '0.1' });
  expect(f.changed).toHaveBeenCalledOnce(); expect(f.changed.mock.lastCall![0].hidden).toHaveLength(20);
  f.view.select('0.2'); expect(f.changed.mock.lastCall![0].selected).toBe('0.2');
  f.view.visibility('0.2', true); expect(f.changed.mock.lastCall![0].hidden).not.toContain('0.2');
  draw(); f.changed.mockClear();
  f.view.select('0.2'); f.view.visibility('0.2', true);
  expect(f.changed).not.toHaveBeenCalled(); expect(frames.size).toBe(0);
});
it('settles graphics exactly once when a final-pose subscriber retires the view synchronously', () => {
  const f = mount(); move(f.control(), 12);
  f.changed.mockImplementationOnce(() => f.view.dispose());
  f.view.dispose();
  expect(f.renderer.dispose).toHaveBeenCalledOnce(); expect(f.renderer.forceContextLoss).toHaveBeenCalledOnce();
  expect(f.control().dispose).toHaveBeenCalledOnce(); expect(vi.getTimerCount()).toBe(0); expect(frames.size).toBe(0);
});
it('prunes hidden subtrees before raycasting and selects the nearest visible mesh', () => {
  const object = new Group(), hidden = new Group(); hidden.name = 'Hidden assembly';
  const heavy = new Mesh(new BoxGeometry(3, 3, 3, 80, 80, 80), new MeshBasicMaterial()); hidden.add(heavy); object.add(hidden);
  const far = new Mesh(new BoxGeometry(), new MeshBasicMaterial()), near = far.clone(); far.position.z = -2; near.position.z = 2; object.add(far, near);
  const f = mount(object); f.view.fit('front'); f.view.visibility('0.0', false);
  const hiddenRaycast = vi.spyOn(heavy, 'raycast'), nearRaycast = vi.spyOn(near, 'raycast');
  const canvas = f.renderer.domElement as HTMLCanvasElement;
  vi.spyOn(canvas, 'getBoundingClientRect').mockReturnValue({ left: 0, top: 0, width: 640, height: 480 } as DOMRect);
  canvas.dispatchEvent(new MouseEvent('pointerdown', { clientX: 320, clientY: 240 })); canvas.dispatchEvent(new MouseEvent('pointerup', { clientX: 320, clientY: 240 }));
  expect(hiddenRaycast).not.toHaveBeenCalled(); expect(nearRaycast).toHaveBeenCalledOnce(); expect(f.changed.mock.lastCall![0].selected).toBe('0.2');
  f.view.select('0.0'); expect(f.changed.mock.lastCall![0].selected).toBe('0.0');
});
it('coalesces size, DPR and quality into drawing-buffer updates and releases all idle scheduling and listeners', () => {
  const f = mount(); expect(f.renderer.setDrawingBufferSize).not.toHaveBeenCalled();
  resize(); resize(); draw(); expect(f.renderer.setDrawingBufferSize).toHaveBeenCalledExactlyOnceWith(640, 480, .5);
  vi.advanceTimersByTime(L.refineDelayMs); draw(); expect(f.renderer.setDrawingBufferSize).toHaveBeenLastCalledWith(640, 480, 1);
  resize(); resize(); expect(f.renderer.setDrawingBufferSize).toHaveBeenCalledTimes(2);
  vi.stubGlobal('devicePixelRatio', 1.5); queries[0].listener!(); resize();
  draw(); vi.advanceTimersByTime(L.refineDelayMs); draw();
  expect(f.renderer.setDrawingBufferSize).toHaveBeenCalledTimes(4); expect(f.renderer.setDrawingBufferSize).toHaveBeenLastCalledWith(640, 480, 1.5);
  expect(f.renderer.domElement.style.width).toBe('640px'); expect(queries[0].removeEventListener).toHaveBeenCalledOnce();
  Object.defineProperty(f.element, 'clientWidth', { value: 800 }); resize(); resize();
  expect(f.renderer.setDrawingBufferSize).toHaveBeenCalledTimes(4); draw(); expect(f.renderer.setDrawingBufferSize).toHaveBeenCalledTimes(5);
  draw(); vi.advanceTimersByTime(L.refineDelayMs); draw();
  const draws = f.renderer.render.mock.calls.length; vi.advanceTimersByTime(1000); expect(frames.size).toBe(0); expect(f.renderer.render).toHaveBeenCalledTimes(draws);
  f.view.dispose(); expect(disconnect).toHaveBeenCalledOnce(); expect(queries.at(-1)!.removeEventListener).toHaveBeenCalledOnce(); expect(f.control().dispose).toHaveBeenCalledOnce(); expect(f.renderer.dispose).toHaveBeenCalledOnce();
});
it('recreates orbit controls for top and bottom and keeps final-flush listeners on the replacement', () => {
  const f = mount(), first = f.control(); f.view.fit('top'); expect(first.dispose).toHaveBeenCalledOnce();
  const top = f.control(); expect(top.object.up.toArray()).toEqual([0, 0, -1]); f.view.fit('bottom'); expect(top.dispose).toHaveBeenCalledOnce();
  expect(f.control().object.up.toArray()).toEqual([0, 0, 1]); move(f.control(), 27); f.control().dispatchEvent({ type: 'end' }); expect(f.changed.mock.lastCall![0].camera.position[0]).toBe(27);
});
it('focuses transformed visible selection while preserving direction, and resets orientation without showing hidden parts', () => {
  const object = new Group(), a = new Mesh(new BoxGeometry(), new MeshBasicMaterial()), b = a.clone();
  a.position.set(100, 200, 300); b.position.set(120, 200, 300); object.add(a, b);
  const f = mount(object); f.view.fit('front');
  f.view.select('0.1'); f.view.focus();
  expect(f.control().target.toArray()).toEqual([10, 0, 0]);
  const pose = f.changed.mock.lastCall![0].camera;
  expect(pose.position[0]).toBe(10); expect(pose.position[1]).toBe(0); expect(pose.position[2]).toBeGreaterThan(0);
  f.view.visibility('0.1', false); f.changed.mockClear(); f.view.focus(); expect(f.changed).not.toHaveBeenCalled();
  f.view.reset(); expect(f.control().target.toArray()).toEqual([0, 0, 0]);
  expect(f.changed.mock.lastCall![0].hidden).toEqual(['0.1']);
  const reset = f.changed.mock.lastCall![0].camera.position; expect(reset[0]).toBeCloseTo(reset[1]); expect(reset[1]).toBeCloseTo(reset[2]);
});
it('batches isolation and restoration into one publication/draw and ignores repeated operations', () => {
  const f = mount(); draw(); f.view.select('0.0'); draw(); f.changed.mockClear();
  f.view.isolate(); expect(f.changed).toHaveBeenCalledOnce(); expect(frames.size).toBe(1);
  f.view.isolate(); expect(f.changed).toHaveBeenCalledOnce(); draw();
  f.view.restore(); expect(f.changed).toHaveBeenCalledTimes(2); draw();
  f.view.restore(); f.view.showAll(); expect(f.changed).toHaveBeenCalledTimes(2); expect(frames.size).toBe(0);
});
it('focuses a small occurrence and restores its close camera without a whole-model distance floor', () => {
  const object = new Group(), a = new Mesh(new BoxGeometry(), new MeshBasicMaterial()), tiny = a.clone();
  a.position.x = 1000; tiny.scale.setScalar(.000001); object.add(a, tiny);
  const f = mount(object); f.view.select('0.1'); f.view.focus();
  const state = f.changed.mock.lastCall![0], distance = f.control().object.position.distanceTo(f.control().target);
  expect(distance).toBeLessThan(.00001); expect(f.control().minDistance).toBeLessThan(.000001);
  f.view.dispose();
  const next = mount(object, { copyId: 'copy', ...state });
  expect(next.control().object.position.toArray()).toEqual(state.camera.position); expect(next.control().target.toArray()).toEqual(state.camera.target);
});
function generatedFixture() {
  const object = new Group(), material = new MeshStandardMaterial();
  const sources = [10, 10.5].map(x => {
    const geometry = new BoxGeometry().translate(x, 0, 0); geometry.clearGroups(); geometry.addGroup(0, geometry.index!.count, 0);
    const source = new Mesh(geometry, [material]); object.add(source); return source;
  });
  return { object, sources };
}
function outlineWorker() {
  vi.stubGlobal('Worker', class {
    onmessage: any; onerror: any; onmessageerror: any; terminate() {}
    postMessage({ inputs }: { inputs: Parameters<typeof outlineData>[0][] }) { void Promise.resolve().then(() => this.onmessage?.({ data: { ok: true, data: inputs.map(value => outlineData(value, L.outlineSegments, performance.now() + 2000)) } })); }
  });
}
it('owns generated outline buffers beside centered sources and preserves picking, focus, inspection and visibility', async () => {
  outlineWorker();
  const { object, sources } = generatedFixture(), f = mount(object, { copyId: 'copy', hidden: ['0.1'] }, { generated: true }), inspection = f.view.inspect();
  expect(await f.view.setOutlines(true)).toBe('on'); draw();
  const scene = f.renderer.render.mock.calls[0][0] as Group, lines: LineSegments<BufferGeometry, ShaderMaterial>[] = [];
  scene.traverse(item => { if (item instanceof LineSegments && item.material instanceof ShaderMaterial) lines.push(item); });
  expect(lines).toHaveLength(1); const line = lines[0], visibility = () => line.material.uniforms.occurrenceVisible.value.slice(0, 2); scene.updateMatrixWorld(true);
  expect(line.matrixWorld.equals(sources[0].matrixWorld)).toBe(true); expect(object.children).toEqual(sources); sources.forEach(source => expect(source.children).toHaveLength(0));
  expect(visibility()).toEqual(new Float32Array([1, 0])); expect(f.view.inspect()).toEqual(inspection);
  const sourceRaycast = vi.spyOn(sources[0], 'raycast'); f.view.fit('front');
  const canvas = f.renderer.domElement as HTMLCanvasElement;
  vi.spyOn(canvas, 'getBoundingClientRect').mockReturnValue({ left: 0, top: 0, width: 640, height: 480 } as DOMRect);
  canvas.dispatchEvent(new MouseEvent('pointerdown', { clientX: 320, clientY: 240 })); canvas.dispatchEvent(new MouseEvent('pointerup', { clientX: 320, clientY: 240 }));
  expect(sourceRaycast).toHaveBeenCalledOnce(); expect(f.changed.mock.lastCall![0].selected).toBe('0.0');
  f.view.visibility('0.1', true); f.view.select('0.1'); f.view.focus();
  expect(f.control().target.toArray()).toEqual([.25, 0, 0]);
  f.view.isolate(); expect(visibility()).toEqual(new Float32Array([0, 1]));
  f.view.restore(); expect(visibility()).toEqual(new Float32Array([1, 1]));
  const geometryDispose = vi.spyOn(line.geometry, 'dispose'), materialDispose = vi.spyOn(line.material, 'dispose'); f.view.dispose(); f.view.dispose();
  expect(geometryDispose).toHaveBeenCalledOnce(); expect(materialDispose).toHaveBeenCalledOnce(); sources.forEach(source => expect(source.layers.mask).toBe(1));
});
it('keeps authored outlines attached to the original occurrences with their local normal-matrix shader', async () => {
  outlineWorker();
  const { object, sources } = generatedFixture(), f = mount(object, undefined, { generated: false }); expect(await f.view.setOutlines(true)).toBe('on'); draw();
  for (const source of sources) {
    expect(source.children).toHaveLength(1); const line = source.children[0] as LineSegments<BufferGeometry, ShaderMaterial>;
    expect(line.material.vertexShader).not.toContain('occurrenceVisible'); expect(source.layers.mask).toBe(1);
  }
});
it('switches projection with one renderer and preserves target-plane scale, target, up and orientation', () => {
  const f = mount(); f.view.setProjection('perspective'); f.view.fit('top'); draw();
  const perspective = f.control().object as PerspectiveCamera, target = f.control().target.clone(), up = perspective.up.clone(), position = perspective.position.clone(), rotation = perspective.quaternion.clone();
  const span = 2 * perspective.position.distanceTo(target) * Math.tan(perspective.fov * Math.PI / 360) / perspective.zoom;
  const old = f.control(); f.changed.mockClear(); f.view.setProjection('orthographic');
  expect(old.dispose).toHaveBeenCalledOnce(); expect(f.control().object).toBeInstanceOf(OrthographicCamera); expect(runtime.renderers).toHaveLength(1);
  const ortho = f.control().object as OrthographicCamera; expect((ortho.top - ortho.bottom) / ortho.zoom).toBeCloseTo(span, 12);
  expect(f.control().target.equals(target)).toBe(true); expect(ortho.up.equals(up)).toBe(true); expect(ortho.position.equals(position)).toBe(true); expect(ortho.quaternion.angleTo(rotation)).toBeLessThan(1e-7);
  expect(f.changed).toHaveBeenCalledOnce(); expect(f.changed.mock.lastCall![0].camera).toMatchObject({ projection: 'orthographic', verticalSpan: span });
  f.view.setProjection('orthographic'); expect(f.changed).toHaveBeenCalledOnce();
  f.view.setProjection('perspective'); expect(f.view.projection).toBe('perspective'); expect(f.control().object.position.distanceTo(position)).toBeLessThan(1e-12);
  expect(f.control().object.up.equals(up)).toBe(true); expect(f.control().target.equals(target)).toBe(true); expect(f.changed.mock.lastCall![0].camera).not.toHaveProperty('verticalSpan');
});
it('keeps orthographic effective height on resize and resets/focuses without changing projection', () => {
  const f = mount(); f.view.setProjection('orthographic'); const camera = f.control().object as OrthographicCamera;
  camera.zoom = 2; f.control().update(); const span = (camera.top - camera.bottom) / camera.zoom;
  Object.defineProperty(f.element, 'clientWidth', { value: 320 }); resize();
  expect((camera.top - camera.bottom) / camera.zoom).toBe(span); expect((camera.right - camera.left) / (camera.top - camera.bottom)).toBeCloseTo(320 / 480);
  f.view.select('0.0'); f.view.focus(); expect(f.view.projection).toBe('orthographic'); expect(Number.isFinite(f.control().minZoom) && f.control().minZoom > 0).toBe(true); expect(Number.isFinite(f.control().maxZoom)).toBe(true);
  f.view.fit('front'); f.view.visibility('0.0', false); f.view.reset(); expect(f.view.projection).toBe('orthographic');
  expect(f.changed.mock.lastCall![0].hidden).toEqual(['0.0']); const position = camera.position; expect(position.x).toBeCloseTo(position.y); expect(position.y).toBeCloseTo(position.z);
});
it('restores effective orthographic zoom and flushes a final zoom-only pose through retirement', () => {
  const f = mount(); f.view.setProjection('orthographic'); const camera = f.control().object as OrthographicCamera, target = f.control().target.clone();
  camera.zoom = 3; camera.updateProjectionMatrix(); const retired = vi.fn(); f.changed.mockClear(); f.view.flush(retired);
  expect(retired).toHaveBeenCalledOnce(); expect(f.changed).not.toHaveBeenCalled(); const state = retired.mock.lastCall![0]; f.view.dispose();
  const restored = mount(undefined, { copyId: 'copy', ...state }); expect(restored.view.projection).toBe('orthographic');
  const next = restored.control().object as OrthographicCamera; expect((next.top - next.bottom) / next.zoom).toBeCloseTo(state.camera.verticalSpan, 12); expect(restored.control().target.equals(target)).toBe(true);
  restored.view.setProjection('perspective'); expect(restored.control().object.position.distanceTo(restored.control().target)).toBeCloseTo(state.camera.verticalSpan / (2 * Math.tan(Math.PI / 8)), 12);
});
it.each([NaN, Infinity, -1, 0, 1e20, '12', true, false])('rejects the entire saved orthographic pose with invalid span %s', verticalSpan => {
  const f = mount(undefined, { copyId: 'copy', camera: { projection: 'orthographic', verticalSpan: verticalSpan as number, position: [9, 8, 7], target: [1, 2, 3], up: [0, 0, 1] } });
  expect(f.view.projection).toBe('orthographic'); expect(f.control().target.toArray()).toEqual([0, 0, 0]); expect(f.control().object.up.toArray()).toEqual([0, 1, 0]);
});
it('uses orthographic rays for picking and admits a restored distance above the old default controls maximum', () => {
  const object = new Group(), far = new Mesh(new BoxGeometry(), new MeshBasicMaterial()), near = far.clone(); far.position.z = -2; near.position.z = 2; object.add(far, near);
  const f = mount(object, { copyId: 'copy', camera: { projection: 'orthographic', verticalSpan: 8, position: [0, 0, 500], target: [0, 0, 0], up: [0, 1, 0] } }); draw();
  expect(f.view.projection).toBe('orthographic'); expect(f.control().object.position.z).toBe(500); expect(f.control().maxDistance).toBeGreaterThanOrEqual(500);
  expect(f.control().object.far).toBeGreaterThan(500 + new Vector3().setFromMatrixPosition(near.matrixWorld).length());
  const canvas = f.renderer.domElement as HTMLCanvasElement; vi.spyOn(canvas, 'getBoundingClientRect').mockReturnValue({ left: 0, top: 0, width: 640, height: 480 } as DOMRect);
  canvas.dispatchEvent(new MouseEvent('pointerdown', { clientX: 320, clientY: 240 })); canvas.dispatchEvent(new MouseEvent('pointerup', { clientX: 320, clientY: 240 }));
  expect(f.changed.mock.lastCall![0].selected).toBe('0.1');
});
it.each(['projection', 'fit', 'reset'] as const)('publishes one metadata update for an owner-driven %s after idle', action => {
  const f = mount(); f.view.fit('back'); draw(); vi.advanceTimersByTime(150); f.changed.mockClear();
  if (action === 'projection') f.view.setProjection('perspective');
  else if (action === 'fit') f.view.fit('front');
  else f.view.reset();
  expect(f.changed).toHaveBeenCalledOnce(); expect(frames.size).toBe(1);
  vi.advanceTimersByTime(1000); expect(f.changed).toHaveBeenCalledOnce(); draw(); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0);
});
it('restores its own orthographic fit for a large admitted model beyond the source-coordinate camera cap', () => {
  const object = new Group(); object.add(new Mesh(new BoxGeometry(1e12, 1e12, 1e12), new MeshBasicMaterial()));
  expect(() => validateScene([object])).not.toThrow();
  const f = mount(object); f.view.setProjection('orthographic'); const saved = { copyId: 'copy', ...f.changed.mock.lastCall![0] };
  expect(saved.camera.position.some((value: number) => Math.abs(value) > L.coordinate)).toBe(true); f.view.dispose();
  const next = mount(object, saved); expect(next.view.projection).toBe('orthographic');
  expect(next.changed.mock.lastCall![0].camera.verticalSpan).toBe(saved.camera.verticalSpan);
  next.control().object.position.toArray().forEach((value: number, index: number) => expect(value).toBeCloseTo(saved.camera.position[index], -2));
});

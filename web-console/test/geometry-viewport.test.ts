import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Box3, BoxGeometry, Group, Mesh, MeshBasicMaterial, Vector2, type PerspectiveCamera } from 'three';
import { mountGeometry } from '../src/app/components/documents/geometry/viewport';
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
    setViewport = vi.fn(); render = vi.fn(); clearDepth = vi.fn(); dispose = vi.fn(); forceContextLoss = vi.fn();
    constructor() { runtime.renderers.push(this); }
  } };
});
vi.mock('three/addons/controls/OrbitControls.js', async () => {
  const { EventDispatcher, Vector3 } = await import('three');
  return { OrbitControls: class extends EventDispatcher<any> {
    target = new Vector3(); dispose = vi.fn(); listenToKeyEvents = vi.fn();
    constructor(readonly object: PerspectiveCamera) { super(); runtime.controls.push(this); }
    update() { this.object.lookAt(this.target); this.object.updateMatrixWorld(true); this.dispatchEvent({ type: 'change' }); }
  } };
});
let resize: () => void, disconnect: ReturnType<typeof vi.fn>, frames: Map<number, FrameRequestCallback>, queries: { listener?: () => void; removeEventListener: ReturnType<typeof vi.fn> }[];
const mounted: ReturnType<typeof mountGeometry>[] = [];
beforeEach(() => {
  vi.useFakeTimers({ now: 0, toFake: ['setTimeout', 'clearTimeout', 'performance'] });
  runtime.renderers.length = 0; runtime.controls.length = 0; frames = new Map(); queries = []; disconnect = vi.fn();
  vi.stubGlobal('devicePixelRatio', 1);
  vi.stubGlobal('ResizeObserver', class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect = disconnect; });
  let frame = 0;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++frame, callback); return frame; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  vi.stubGlobal('matchMedia', () => { const query = { listener: undefined as (() => void) | undefined, addEventListener: (_type: string, listener: () => void) => { query.listener = listener; }, removeEventListener: vi.fn(() => { query.listener = undefined; }) }; queries.push(query); return query; });
});
afterEach(() => { mounted.splice(0).forEach(view => view.dispose()); vi.useRealTimers(); vi.unstubAllGlobals(); });
function mount(object = new Group(), saved?: Parameters<typeof mountGeometry>[3]) {
  if (!object.children.length) object.add(new Mesh(new BoxGeometry(), new MeshBasicMaterial()));
  const element = document.createElement('div'); Object.defineProperties(element, { clientWidth: { value: 640, configurable: true }, clientHeight: { value: 480, configurable: true } });
  const changed = vi.fn(), lost = vi.fn(), view = mountGeometry(element, object, new Box3().setFromObject(object), saved, { canvas: 'Model', axes: 'Axes' }, changed, lost); mounted.push(view);
  return { view, changed, lost, element, renderer: runtime.renderers.at(-1), control: () => runtime.controls.at(-1) };
}
function draw() { const pending = [...frames.values()]; frames.clear(); pending.forEach(callback => callback(0)); }
function move(control: any, x: number) { control.object.position.x = x; control.update(); }
it('coalesces poses while the GPU owns a frame and releases its fence on retirement', () => {
  const f = mount(); draw(); expect(f.renderer.render).toHaveBeenCalledTimes(2);
  f.renderer.gl.clientWaitSync.mockReturnValue(f.renderer.gl.TIMEOUT_EXPIRED);
  move(f.control(), 2); draw(); move(f.control(), 3); draw();
  expect(f.renderer.render).toHaveBeenCalledTimes(2); expect(frames.size).toBe(1);
  f.renderer.gl.clientWaitSync.mockReturnValue(f.renderer.gl.ALREADY_SIGNALED); draw();
  expect(f.renderer.render).toHaveBeenCalledTimes(4); expect(frames.size).toBe(0);
  f.view.dispose(); expect(f.renderer.gl.deleteSync).toHaveBeenCalledTimes(2);
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
it('writes the drawing buffer once per changed size or DPR and releases all idle scheduling and listeners', () => {
  const f = mount(); expect(f.renderer.setDrawingBufferSize).toHaveBeenCalledExactlyOnceWith(640, 480, 1);
  resize(); resize(); expect(f.renderer.setDrawingBufferSize).toHaveBeenCalledTimes(1);
  vi.stubGlobal('devicePixelRatio', 1.5); queries[0].listener!(); resize();
  expect(f.renderer.setDrawingBufferSize).toHaveBeenCalledTimes(2); expect(f.renderer.setDrawingBufferSize).toHaveBeenLastCalledWith(640, 480, 1.5);
  expect(f.renderer.domElement.style.width).toBe('640px'); expect(queries[0].removeEventListener).toHaveBeenCalledOnce();
  Object.defineProperty(f.element, 'clientWidth', { value: 800 }); resize(); resize(); expect(f.renderer.setDrawingBufferSize).toHaveBeenCalledTimes(3);
  draw(); const draws = f.renderer.render.mock.calls.length; vi.advanceTimersByTime(1000); expect(frames.size).toBe(0); expect(f.renderer.render).toHaveBeenCalledTimes(draws);
  f.view.dispose(); expect(disconnect).toHaveBeenCalledOnce(); expect(queries.at(-1)!.removeEventListener).toHaveBeenCalledOnce(); expect(f.control().dispose).toHaveBeenCalledOnce(); expect(f.renderer.dispose).toHaveBeenCalledOnce();
});
it('recreates orbit controls for top and bottom and keeps final-flush listeners on the replacement', () => {
  const f = mount(), first = f.control(); f.view.fit('top'); expect(first.dispose).toHaveBeenCalledOnce();
  const top = f.control(); expect(top.object.up.toArray()).toEqual([0, 0, -1]); f.view.fit('bottom'); expect(top.dispose).toHaveBeenCalledOnce();
  expect(f.control().object.up.toArray()).toEqual([0, 0, 1]); move(f.control(), 27); f.control().dispatchEvent({ type: 'end' }); expect(f.changed.mock.lastCall![0].camera.position[0]).toBe(27);
});

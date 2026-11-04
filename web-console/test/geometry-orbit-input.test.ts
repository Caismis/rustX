import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Box3, BoxGeometry, Group, Mesh, MeshBasicMaterial, Vector2, Vector3 } from 'three';
import { mountGeometry } from '../src/app/components/documents/geometry/viewport';
import { en } from '../src/locale/dictionaries/geometry';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';

// Keep the installed OrbitControls implementation, including its absolute EPS.
// Only the GPU boundary is replaced; inputs go through real DOM listeners.
const runtime = vi.hoisted(() => ({ renderers: [] as any[] }));
vi.mock('three', async original => {
  const actual = await original<typeof import('three')>();
  return { ...actual, WebGLRenderer: class {
    domElement = document.createElement('canvas'); size = new actual.Vector2();
    gl = { TIMEOUT_EXPIRED: 1, WAIT_FAILED: 2, ALREADY_SIGNALED: 3, SYNC_GPU_COMMANDS_COMPLETE: 4,
      clientWaitSync: () => 3, fenceSync: () => ({}), deleteSync() {}, flush() {} };
    getContext = () => this.gl;
    setDrawingBufferSize = (width: number, height: number) => this.size.set(width, height);
    getSize = (target: Vector2) => target.copy(this.size);
    setViewport() {} render = vi.fn(); clearDepth() {} dispose = vi.fn(); forceContextLoss() {}
    constructor() {
      Object.defineProperties(this.domElement, { clientWidth: { value: 640 }, clientHeight: { value: 480 } });
      this.domElement.setPointerCapture = vi.fn(); this.domElement.releasePointerCapture = vi.fn();
      runtime.renderers.push(this);
    }
  } };
});
let frames: Map<number, FrameRequestCallback>;
const mounted: ReturnType<typeof mountGeometry>[] = [];
beforeEach(() => {
  vi.useFakeTimers({ now: 0, toFake: ['setTimeout', 'clearTimeout', 'performance'] });
  runtime.renderers.length = 0; frames = new Map(); let serial = 0;
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++serial, callback); return serial; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  vi.stubGlobal('matchMedia', () => ({ addEventListener() {}, removeEventListener() {} }));
});
afterEach(() => { mounted.splice(0).forEach(view => view.dispose()); document.body.replaceChildren(); vi.useRealTimers(); vi.unstubAllGlobals(); });
function draw() { const pending = [...frames.values()]; frames.clear(); pending.forEach(callback => callback(0)); }
function mount() {
  const object = new Group(), normal = new Mesh(new BoxGeometry(), new MeshBasicMaterial()), tiny = normal.clone();
  normal.position.x = 1000; tiny.scale.setScalar(1e-6); object.add(normal, tiny);
  const element = document.createElement('div'); document.body.append(element);
  Object.defineProperties(element, { clientWidth: { value: 640 }, clientHeight: { value: 480 } });
  const changed = vi.fn(), view = mountGeometry(element, object, new Box3().setFromObject(object), undefined, en, changed, vi.fn()); mounted.push(view);
  view.select('0.1'); view.fit('top'); view.focus(); // Replaces OrbitControls through its public lifecycle.
  draw(); changed.mockClear();
  const renderer = runtime.renderers.at(-1), canvas = renderer.domElement as HTMLCanvasElement;
  return { view, changed, renderer, canvas };
}
function pointer(target: EventTarget, type: string, x: number) {
  const event = new MouseEvent(type, { button: 2, buttons: type === 'pointerup' ? 0 : 2, clientX: x, clientY: 200, bubbles: true });
  Object.defineProperties(event, { pointerId: { value: 1 }, pointerType: { value: 'mouse' } }); target.dispatchEvent(event);
}
it('renders a 120px sub-EPS pan during the gesture and persists its final pose without idle polling', async () => {
  const f = mount(); const before = f.renderer.render.mock.calls.at(-2)![1].position.clone();
  pointer(f.canvas, 'pointerdown', 300); pointer(document, 'pointermove', 420);
  await Promise.resolve(); expect(frames.size).toBe(1); expect(f.changed).not.toHaveBeenCalled();
  draw(); expect(f.renderer.render.mock.calls.at(-2)![1].position.distanceTo(before)).toBeGreaterThan(1e-7);
  pointer(document, 'pointerup', 420);
  expect(f.changed).toHaveBeenCalledOnce(); const pose = f.changed.mock.lastCall![0].camera;
  expect(Math.abs(pose.position[0] - before.x)).toBeGreaterThan(1e-7); expect(Math.abs(pose.position[0] - before.x)).toBeLessThan(1e-5);
  vi.advanceTimersByTime(L.refineDelayMs); draw();
  const draws = f.renderer.render.mock.calls.length; vi.advanceTimersByTime(1000); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0); expect(f.renderer.render).toHaveBeenCalledTimes(draws);
});
it('retires an input pose before its queued observation through the supplied retirement callback only', async () => {
  const f = mount(), retired = vi.fn();
  f.canvas.dispatchEvent(new KeyboardEvent('keydown', { code: 'ArrowRight', bubbles: true, cancelable: true }));
  f.view.flush(retired); f.view.dispose();
  expect(retired).toHaveBeenCalledOnce(); expect(f.changed).not.toHaveBeenCalled();
  await Promise.resolve(); vi.advanceTimersByTime(1000);
  expect(retired).toHaveBeenCalledOnce(); expect(f.changed).not.toHaveBeenCalled(); expect(frames.size).toBe(0);
});
it('captures tiny keyboard pans and wheel zooms, and fences queued observations on disposal', async () => {
  const f = mount();
  f.canvas.dispatchEvent(new KeyboardEvent('keydown', { code: 'ArrowRight', bubbles: true, cancelable: true }));
  await Promise.resolve(); expect(frames.size).toBe(1); vi.advanceTimersByTime(100);
  expect(f.changed).toHaveBeenCalledOnce(); const before = f.changed.mock.lastCall![0].camera;
  f.canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: -100, clientX: 300, clientY: 200, bubbles: true, cancelable: true }));
  expect(f.changed).toHaveBeenCalledTimes(2); expect(f.changed.mock.lastCall![0].camera.position).not.toEqual(before.position);
  f.canvas.dispatchEvent(new KeyboardEvent('keydown', { code: 'ArrowRight', bubbles: true, cancelable: true }));
  f.view.dispose(); const publications = f.changed.mock.calls.length;
  await Promise.resolve(); vi.advanceTimersByTime(1000);
  expect(f.changed).toHaveBeenCalledTimes(publications); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0); expect(f.renderer.dispose).toHaveBeenCalledOnce();
});
it('persists real orthographic wheel zoom with an unchanged position and no idle work', () => {
  const f = mount(); f.view.setProjection('orthographic'); draw(); f.changed.mockClear();
  const camera = f.renderer.render.mock.calls.at(-2)![1], position = camera.position.clone(), before = (camera.top - camera.bottom) / camera.zoom;
  f.canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: -100, clientX: 300, clientY: 200, bubbles: true, cancelable: true }));
  expect(f.changed).toHaveBeenCalledOnce(); expect(camera.position.distanceTo(position)).toBeLessThan(1e-12);
  expect(f.changed.mock.lastCall![0].camera.projection).toBe('orthographic'); expect(f.changed.mock.lastCall![0].camera.verticalSpan).toBeLessThan(before);
  draw(); vi.advanceTimersByTime(L.refineDelayMs); draw();
  const draws = f.renderer.render.mock.calls.length; vi.advanceTimersByTime(1000); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0); expect(f.renderer.render).toHaveBeenCalledTimes(draws);
  f.view.setProjection('perspective'); expect(runtime.renderers).toHaveLength(1); expect(Number.isFinite(f.changed.mock.lastCall![0].camera.position[0])).toBe(true);
});

it.each(['front', 'top'] as const)('preserves an off-origin tiny focused part through a %s projection roundtrip', viewAngle => {
  const object = new Group(), material = new MeshBasicMaterial();
  for (const offset of [-1000, 1000, 500]) {
    const part = new Mesh(new BoxGeometry(), material); part.position[viewAngle === 'top' ? 'z' : 'y'] = offset;
    if (offset === 500) part.scale.setScalar(1e-6);
    object.add(part);
  }
  const element = document.createElement('div'); document.body.append(element);
  Object.defineProperties(element, { clientWidth: { value: 640 }, clientHeight: { value: 480 } });
  const changed = vi.fn(), view = mountGeometry(element, object, new Box3().setFromObject(object), undefined, en, changed, vi.fn()); mounted.push(view);
  view.fit(viewAngle); view.select('0.2'); view.focus(); draw();
  const before = changed.mock.lastCall![0].camera;
  const distance = new Vector3().fromArray(before.position).distanceTo(new Vector3().fromArray(before.target));
  expect(distance).toBeGreaterThan(2e-6); expect(distance).toBeLessThan(3e-6);
  changed.mockClear();
  for (const projection of ['orthographic', 'perspective'] as const) {
    view.setProjection(projection); draw();
    const after = changed.mock.lastCall![0].camera;
    expect(after.projection).toBe(projection); expect(after.target).toEqual(before.target); expect(after.up).toEqual(before.up);
    expect(new Vector3().fromArray(after.position).distanceTo(new Vector3().fromArray(before.position))).toBeLessThan(distance * 1e-6);
    expect(new Vector3().fromArray(after.position).distanceTo(new Vector3().fromArray(after.target)) / distance).toBeCloseTo(1, 6);
    if (projection === 'orthographic') expect(after.verticalSpan / (2 * distance * Math.tan(Math.PI / 8))).toBeCloseTo(1, 6);
  }
  expect(changed).toHaveBeenCalledTimes(2); expect(runtime.renderers).toHaveLength(1);
  vi.advanceTimersByTime(L.refineDelayMs); draw();
  const renderer = runtime.renderers[0], draws = renderer.render.mock.calls.length;
  vi.advanceTimersByTime(1000); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0); expect(renderer.render).toHaveBeenCalledTimes(draws);
});

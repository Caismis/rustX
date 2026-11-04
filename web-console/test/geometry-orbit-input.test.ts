import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Box3, BoxGeometry, Group, Mesh, MeshBasicMaterial, Vector2 } from 'three';
import { mountGeometry } from '../src/app/components/documents/geometry/viewport';
import { en } from '../src/locale/dictionaries/geometry';

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
  vi.advanceTimersByTime(1000); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0);
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

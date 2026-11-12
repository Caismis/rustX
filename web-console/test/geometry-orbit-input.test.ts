import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { act, cleanup, render } from '@testing-library/react';
import { createElement } from 'react';
import { Box3, BoxGeometry, Frustum, Group, Matrix4, Mesh, MeshBasicMaterial, OrthographicCamera, PerspectiveCamera, Vector2, Vector3 } from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { mountGeometry, prepareGeometryGraphics } from '../src/app/components/documents/geometry/viewport';
import { progressivePresentation } from '../src/app/components/documents/geometry/progressive';
import { en } from '../src/locale/dictionaries/geometry';
import { localeController } from '../src/locale/controller';
import type { PreviewViewState } from '../src/presentation/right-panel/preview-view-state';
import GeometryPreview from '../src/app/components/documents/geometry/GeometryPreview';
import type { GeometryLod } from '../src/app/components/documents/geometry/parse';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';

// Keep the installed OrbitControls implementation, including its absolute EPS.
// Only the GPU boundary is replaced; inputs go through real DOM listeners.
const imports = vi.hoisted(() => ({ cad: [] as { resolve: (value: unknown) => void; lod: (value: GeometryLod) => void }[] }));
vi.mock('../src/app/components/documents/geometry/parse', () => ({ parseGeometry: (_bytes: unknown, _format: unknown, _signal: unknown, _stage: unknown, _take: unknown, lod: (value: GeometryLod) => void) => new Promise(resolve => { imports.cad.push({ resolve, lod }); }) }));
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
    setViewport() {} compile(){} render = vi.fn(); clearDepth() {} dispose = vi.fn(); forceContextLoss() {}
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
  imports.cad = []; runtime.renderers.length = 0; frames = new Map(); let serial = 0;
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++serial, callback); return serial; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  vi.stubGlobal('matchMedia', () => ({ addEventListener() {}, removeEventListener() {} }));
});
afterEach(() => { cleanup(); localeController.setLocale('en'); mounted.splice(0).forEach(view => view.dispose()); document.body.replaceChildren(); vi.useRealTimers(); vi.unstubAllGlobals(); });
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
function pointer(target: EventTarget, type: string, x: number, button = 2) {
  const event = new MouseEvent(type, { button, buttons: type === 'pointerup' ? 0 : button === 0 ? 1 : 2, clientX: x, clientY: 200, bubbles: true });
  Object.defineProperties(event, { pointerId: { value: 1 }, pointerType: { value: 'mouse' } }); target.dispatchEvent(event);
}
it('renders a 120px sub-EPS pan during the gesture and persists its final pose without idle polling', async () => {
  const f = mount(); const before = f.renderer.render.mock.calls.at(-1)![1].position.clone();
  const draws = f.renderer.render.mock.calls.length;
  pointer(f.canvas, 'pointerdown', 300); pointer(document, 'pointermove', 420);
  await Promise.resolve(); await Promise.resolve(); expect(frames.size).toBe(0); expect(f.renderer.render).toHaveBeenCalledTimes(draws + 1); expect(f.changed).not.toHaveBeenCalled();
  draw(); expect(f.renderer.render.mock.calls.at(-1)![1].position.distanceTo(before)).toBeGreaterThan(1e-7);
  pointer(document, 'pointerup', 420);
  expect(f.changed).toHaveBeenCalledOnce(); const pose = f.changed.mock.lastCall![0].camera;
  expect(Math.abs(pose.position[0] - before.x)).toBeGreaterThan(1e-7); expect(Math.abs(pose.position[0] - before.x)).toBeLessThan(1e-5);
  vi.advanceTimersByTime(L.refineDelayMs); draw();
  const settledDraws = f.renderer.render.mock.calls.length; vi.advanceTimersByTime(1000); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0); expect(f.renderer.render).toHaveBeenCalledTimes(settledDraws);
});

it.each(['orthographic', 'perspective'] as const)('uses left-drag Pan and Orbit through real %s controls after axis/projection rebuild and pause', async projection => {
  const updates = vi.spyOn(OrbitControls.prototype, 'update'), disposals = vi.spyOn(OrbitControls.prototype, 'dispose');
  try {
    const f = mount(); f.view.reset(); f.view.setNavigationMode('pan');
    const switched = disposals.mock.calls.length;
    f.view.setNavigationMode('orbit'); f.view.setNavigationMode('pan');
    expect(disposals).toHaveBeenCalledTimes(switched);
    f.view.fit('front'); f.view.fit('top'); f.view.setProjection(projection); draw();
    const control = updates.mock.contexts.at(-1) as OrbitControls, camera = control.object as OrthographicCamera | PerspectiveCamera;
    f.view.setVisible(false); f.view.setVisible(true); draw();
    const before = { position: camera.position.clone(), target: control.target.clone(), direction: camera.position.clone().sub(control.target), zoom: camera.zoom };
    pointer(f.canvas, 'pointerdown', 300, 0); pointer(document, 'pointermove', 420, 0); pointer(document, 'pointerup', 420, 0);
    await Promise.resolve();
    expect(camera.position.distanceTo(before.position)).toBeGreaterThan(.01);
    expect(control.target.distanceTo(before.target)).toBeGreaterThan(.01);
    expect(camera.position.clone().sub(before.position).distanceTo(control.target.clone().sub(before.target))).toBeLessThan(1e-9);
    expect(camera.position.clone().sub(control.target).distanceTo(before.direction)).toBeLessThan(1e-9); expect(camera.zoom).toBe(before.zoom);
    expect(f.changed.mock.lastCall![0].navigationMode).toBe('pan');
    f.view.setNavigationMode('orbit'); const target = control.target.clone(), direction = camera.position.clone().sub(target);
    pointer(f.canvas, 'pointerdown', 300, 0); pointer(document, 'pointermove', 420, 0); pointer(document, 'pointerup', 420, 0);
    expect(control.target.distanceTo(target)).toBeLessThan(1e-9);
    expect(camera.position.clone().sub(target).distanceTo(direction)).toBeGreaterThan(.01);
    expect(f.changed.mock.lastCall![0].navigationMode).toBeUndefined(); expect(runtime.renderers).toHaveLength(1);
  } finally { updates.mockRestore(); disposals.mockRestore(); }
});

it.each(['en', 'zh'] as const)('keeps selected Pan through native coarse-to-full handoff without importing or allocating graphics again (%s)', async locale => {
  localeController.setLocale(locale);
  const updates = vi.spyOn(OrbitControls.prototype, 'update');
  try {
    const changed = vi.fn(), ui = render(createElement(GeometryPreview, { bytes: new Uint8Array([1]), name: 'assembly.step', copyId: 'copy', signal: new AbortController().signal, current: () => true, viewState: {}, onViewStateChange: changed, onGeometryRetire: vi.fn() }));
    const container = ui.container.querySelector('section > div')!;
    Object.defineProperties(container, { clientWidth: { value: 640 }, clientHeight: { value: 480 } });
    const makeScene = () => { const scene = new Group(); scene.add(new Mesh(new BoxGeometry(), new MeshBasicMaterial())); return { scene, bounds: new Box3().setFromObject(scene), dispose: vi.fn() }; };
    act(() => { imports.cad[0].lod({ level: 0, ...makeScene() }); draw(); draw(); });
    const pan = ui.getByRole('button', { name: locale === 'en' ? 'Pan' : '平移' }) as HTMLButtonElement;
    expect(pan.getAttribute('aria-pressed')).toBe('false'); expect(pan.disabled).toBe(false); expect(ui.queryByRole('button', { name: locale === 'en' ? 'Orbit' : '旋转' })).toBeNull();
    act(() => { pan.click(); }); expect(pan.getAttribute('aria-pressed')).toBe('true');
    act(() => { pan.click(); }); expect(pan.getAttribute('aria-pressed')).toBe('false');
    act(() => { pan.click(); }); expect(pan.getAttribute('aria-pressed')).toBe('true');
    await act(async () => { imports.cad[0].resolve({ ...makeScene(), prepareStart: 0, ready: Promise.resolve() }); });
    expect(pan.getAttribute('aria-pressed')).toBe('true'); expect(changed.mock.lastCall![0].geometry.navigationMode).toBe('pan');
    const canvas = container.querySelector('canvas')!, control = updates.mock.contexts.at(-1) as OrbitControls, target = control.target.clone();
    act(() => { pointer(canvas, 'pointerdown', 300, 0); pointer(document, 'pointermove', 420, 0); pointer(document, 'pointerup', 420, 0); });
    expect(control.target.distanceTo(target)).toBeGreaterThan(.01); expect(imports.cad).toHaveLength(1); expect(runtime.renderers).toHaveLength(1);
  } finally { updates.mockRestore(); }
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
  const f = mount(); f.view.setProjection('perspective'); draw(); f.changed.mockClear();
  const draws = f.renderer.render.mock.calls.length;
  f.canvas.dispatchEvent(new KeyboardEvent('keydown', { code: 'ArrowRight', bubbles: true, cancelable: true }));
  await Promise.resolve(); await Promise.resolve(); expect(frames.size).toBe(0); expect(f.renderer.render).toHaveBeenCalledTimes(draws + 1); vi.advanceTimersByTime(100);
  expect(f.changed).toHaveBeenCalledOnce(); const before = f.changed.mock.lastCall![0].camera;
  f.canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: -100, clientX: 300, clientY: 200, bubbles: true, cancelable: true }));
  expect(f.changed).toHaveBeenCalledTimes(2); expect(f.changed.mock.lastCall![0].camera.position).not.toEqual(before.position);
  f.canvas.dispatchEvent(new KeyboardEvent('keydown', { code: 'ArrowRight', bubbles: true, cancelable: true }));
  f.view.dispose(); const publications = f.changed.mock.calls.length;
  await Promise.resolve(); vi.advanceTimersByTime(1000);
  expect(f.changed).toHaveBeenCalledTimes(publications); expect(frames.size).toBe(0); expect(vi.getTimerCount()).toBe(0); expect(f.renderer.dispose).toHaveBeenCalledOnce();
});
it('persists real orthographic wheel zoom with an unchanged position and no idle work', () => {
  const f = mount(); expect(f.view.projection).toBe('orthographic'); draw(); f.changed.mockClear();
  const camera = f.renderer.render.mock.calls.at(-1)![1], position = camera.position.clone(), before = (camera.top - camera.bottom) / camera.zoom;
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
  view.setProjection('perspective'); view.fit(viewAngle); view.select('0.2'); view.focus(); draw();
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

it.each(['perspective', 'orthographic'] as const)('replaces a whole-model tier without interrupting a %s gesture and fits only the live model', projection => {
  const element = document.createElement('div'); document.body.append(element);
  Object.defineProperties(element, { clientWidth: { value: 640 }, clientHeight: { value: 480 } });
  const graphics = prepareGeometryGraphics(true, vi.fn()), failed = vi.fn();
  let view: ReturnType<typeof mountGeometry> | undefined;
  const owner = progressivePresentation({ container: element, graphics, current: () => true, copyId: 'copy', labels: () => en,
    mounted: value => { view = value; }, displayed: vi.fn(), failed });
  const tier = (level: 0 | 1, offsets: number[]) => {
    const scene = new Group(), geometry = new BoxGeometry(), material = new MeshBasicMaterial();
    for (const offset of offsets) { const mesh = new Mesh(geometry, material); mesh.position.setScalar(offset); scene.add(mesh); }
    return { level, scene, bounds: new Box3().setFromObject(scene), dispose: vi.fn(() => { scene.removeFromParent(); geometry.dispose(); material.dispose(); }) };
  };
  const first = tier(0, [10,11]), distant = tier(1, [10,-1000]), disposeControls = vi.spyOn(OrbitControls.prototype, 'dispose'), updateControls = vi.spyOn(OrbitControls.prototype, 'update');
  try {
    owner.add(first); draw(); draw();
    expect(view!.projection).toBe('orthographic'); expect(runtime.renderers[0].render.mock.calls.at(-1)![1]).toBeInstanceOf(OrthographicCamera);
    view!.setProjection(projection); draw();
    const renderer = runtime.renderers[0], canvas = renderer.domElement as HTMLCanvasElement;
    canvas.focus(); expect(document.activeElement).toBe(canvas);
    canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: -1000, bubbles: true, cancelable: true })); draw();
    pointer(canvas, 'pointerdown', 300); pointer(document, 'pointermove', 320);
    const camera = renderer.render.mock.calls.at(-1)![1], control = updateControls.mock.contexts.at(-1) as OrbitControls, graph = first.scene.parent!, before = {
      position: camera.position.clone(), rotation: camera.quaternion.clone(), up: camera.up.clone(), zoom: camera.zoom, near: camera.near,
      span: camera instanceof OrthographicCamera ? camera.top - camera.bottom : undefined,
      origin: graph.parent!.position.clone(), target: control.target.clone(), minDistance: control.minDistance, maxDistance: control.maxDistance,
    };
    disposeControls.mockClear(); updateControls.mockClear();
    owner.add(distant); draw(); draw();
    expect(document.activeElement).toBe(canvas);
    expect(camera.position.toArray()).toEqual(before.position.toArray()); expect(camera.quaternion.equals(before.rotation)).toBe(true);
    expect(camera.up.equals(before.up)).toBe(true); expect(camera.zoom).toBe(before.zoom); expect(camera.near).toBe(before.near);
    if (camera instanceof OrthographicCamera) expect(camera.top - camera.bottom).toBe(before.span);
    expect(control.target.equals(before.target)).toBe(true); expect(control.minDistance).toBe(before.minDistance); expect(control.maxDistance).toBeGreaterThan(before.maxDistance);
    expect(updateControls).not.toHaveBeenCalled();
    expect(graph.parent!.position.equals(before.origin)).toBe(true); expect(graph.children).toEqual([distant.scene]); expect(first.dispose).toHaveBeenCalledOnce(); expect(disposeControls).not.toHaveBeenCalled();
    expect(camera.far).toBeGreaterThan(camera.position.distanceTo(distant.bounds.getCenter(new Vector3())) + Math.sqrt(3));
    pointer(document, 'pointermove', 340); pointer(document, 'pointerup', 340);
    expect(camera.position.distanceTo(before.position)).toBeGreaterThan(0);
    // A growth update must not clamp a close camera or the orthographic zoom
    // when the next real input calls OrbitControls.update().
    if (camera instanceof OrthographicCamera) expect(camera.zoom).toBe(before.zoom);
    else expect(camera.position.distanceTo(before.position)).toBeLessThan(1);
    for (const action of [() => view!.fit('front'), () => view!.reset()]) {
      action(); draw(); camera.updateMatrixWorld(true);
      const frustum = new Frustum().setFromProjectionMatrix(new Matrix4().multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse));
      for (const admitted of [distant]) {
        const box = new Box3().setFromObject(admitted.scene);
        for (const x of [box.min.x, box.max.x]) for (const y of [box.min.y, box.max.y]) for (const z of [box.min.z, box.max.z]) expect(frustum.containsPoint(new Vector3(x, y, z))).toBe(true);
      }
    }
    expect(runtime.renderers).toHaveLength(1); expect(failed).not.toHaveBeenCalled();
  } finally { owner.dispose(); graphics.dispose(); disposeControls.mockRestore(); updateControls.mockRestore(); }
});

it.each(['canvas', 'external'] as const)('preserves %s focus through graphics handoff and keeps adopted keyboard navigation active', async focus => {
  const element = document.createElement('div'), external = document.createElement('button'); document.body.append(element, external);
  Object.defineProperties(element, { clientWidth: { value: 640 }, clientHeight: { value: 480 } });
  const object = new Group(); object.add(new Mesh(new BoxGeometry(), new MeshBasicMaterial()));
  const old = mountGeometry(element, object, new Box3().setFromObject(object), undefined, en, vi.fn(), vi.fn(), { generated: true, provisional: true }); mounted.push(old); draw();
  const renderer = runtime.renderers[0], canvas = renderer.domElement as HTMLCanvasElement, focused = focus === 'canvas' ? canvas : external;
  focused.focus(); expect(document.activeElement).toBe(focused);
  const graphics = old.handoff(); expect(canvas.parentElement).toBe(element); expect(document.activeElement).toBe(focused);
  const append = vi.spyOn(element, 'append'), changed = vi.fn();
  const next = mountGeometry(element, object, new Box3().setFromObject(object), undefined, en, changed, vi.fn(), { generated: true, graphics }); mounted.push(next); draw();
  expect(append).not.toHaveBeenCalledWith(canvas); append.mockRestore(); expect(document.activeElement).toBe(focused); expect(runtime.renderers).toHaveLength(1);
  changed.mockClear();
  focused.dispatchEvent(new KeyboardEvent('keydown', { code: 'ArrowRight', bubbles: true, cancelable: true }));
  await Promise.resolve(); vi.advanceTimersByTime(100);
  if (focus === 'canvas') expect(changed).toHaveBeenCalledOnce();
  else expect(changed).not.toHaveBeenCalled();
  next.dispose(); expect(canvas.isConnected).toBe(false); expect(renderer.dispose).toHaveBeenCalledOnce();
});

it('removes the retained canvas when graphics are disposed before adoption', () => {
  const f = mount(), graphics = f.view.handoff();
  expect(f.canvas.isConnected).toBe(true); graphics!.dispose();
  expect(f.canvas.isConnected).toBe(false); expect(f.renderer.dispose).toHaveBeenCalledOnce();
});

it('starts a fresh view with a real orthographic camera and restores a saved perspective camera', () => {
  const fresh = mount();
  expect(fresh.view.projection).toBe('orthographic');
  expect(fresh.renderer.render.mock.calls.at(-1)![1]).toBeInstanceOf(OrthographicCamera);
  const saved: NonNullable<NonNullable<Parameters<typeof mountGeometry>[3]>['camera']> = { projection: 'perspective', position: [9, 8, 7], target: [1, 2, 3], up: [0, 0, 1] };
  const object = new Group(); object.add(new Mesh(new BoxGeometry(10, 10, 10), new MeshBasicMaterial()));
  const element = document.createElement('div'); document.body.append(element);
  Object.defineProperties(element, { clientWidth: { value: 640 }, clientHeight: { value: 480 } });
  const changed = vi.fn(), view = mountGeometry(element, object, new Box3().setFromObject(object), { copyId: 'copy', camera: saved }, en, changed, vi.fn()); mounted.push(view); draw();
  const camera = runtime.renderers.at(-1).render.mock.calls.at(-1)![1];
  expect(camera).toBeInstanceOf(PerspectiveCamera); expect(view.projection).toBe('perspective');
  const restored = changed.mock.lastCall![0].camera; expect(restored.target).toEqual(saved.target); expect(restored.up).toEqual(saved.up);
  saved.position.forEach((value, index) => expect(restored.position[index]).toBeCloseTo(value, 12));
  const before = camera.position.clone(), canvas = runtime.renderers.at(-1).domElement;
  canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: -100, clientX: 300, clientY: 200, bubbles: true, cancelable: true }));
  expect(camera.position.distanceTo(before)).toBeGreaterThan(0); expect(changed.mock.lastCall![0].camera.projection).toBe('perspective');
});

it('changes a perspective view to orthographic right and left through one axis action at a time', () => {
  const f = mount(); f.view.setProjection('perspective'); f.view.fit('front'); draw();
  expect(f.view.projection).toBe('perspective');
  const axis = f.canvas.parentElement!.querySelector<SVGGElement>('[data-axis="X"]')!;
  for (const sign of [1, -1]) {
    f.changed.mockClear(); axis.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(f.changed).toHaveBeenCalledOnce(); const state = f.changed.mock.lastCall![0], camera = state.camera;
    expect(camera.projection).toBe('orthographic'); expect(state.selected).toBe('0.1'); expect(state.hidden).toEqual([]);
    expect((camera.position[0] - camera.target[0]) * sign).toBeGreaterThan(0);
    expect(camera.position[1]).toBeCloseTo(camera.target[1], 8); expect(camera.position[2]).toBeCloseTo(camera.target[2], 8);
    draw(); expect(f.renderer.render.mock.calls.at(-1)![1]).toBeInstanceOf(OrthographicCamera); expect(runtime.renderers).toHaveLength(1);
    vi.advanceTimersByTime(1000); draw(); expect(f.changed).toHaveBeenCalledOnce();
  }
});

it.each(['axis', 'aligned-axis', 'canvas', 'external', 'moved-focus'] as const)('retains %s ownership across native coarse-to-full handoff', async focus => {
  const external = document.createElement('button'); document.body.append(external);
  const ui = render(createElement(GeometryPreview, { bytes: new Uint8Array([1]), name: 'assembly.step', copyId: 'copy', signal: new AbortController().signal, current: () => true, viewState: {}, onViewStateChange: vi.fn(), onGeometryRetire: vi.fn(), releaseSource: () => { if (focus === 'moved-focus') external.focus(); } }));
  const container = ui.container.querySelector('section > div')!;
  Object.defineProperties(container, { clientWidth: { value: 640 }, clientHeight: { value: 480 } });
  const makeScene = () => { const scene = new Group(); scene.add(new Mesh(new BoxGeometry(), new MeshBasicMaterial())); return { scene, bounds: new Box3().setFromObject(scene), dispose: vi.fn() }; };
  act(() => { imports.cad[0].lod({ level: 0, ...makeScene() }); draw(); draw(); });
  const canvas = container.querySelector('canvas')!, axis = container.querySelector<SVGGElement>('[data-axis="X"]')!;
  const wantsAxis = focus === 'axis' || focus === 'aligned-axis' || focus === 'moved-focus';
  if (wantsAxis) axis.focus(); else (focus === 'canvas' ? canvas : external).focus();
  if (focus === 'aligned-axis') act(() => { axis.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); });
  expect(document.activeElement).toBe(wantsAxis ? axis : focus === 'canvas' ? canvas : external);
  await act(async () => { imports.cad[0].resolve({ ...makeScene(), prepareStart: 0, ready: Promise.resolve() }); await vi.advanceTimersByTimeAsync(L.refineDelayMs); });
  const nextAxis = container.querySelector<SVGGElement>('[data-axis="X"]')!;
  expect(nextAxis).not.toBe(axis); expect(container.querySelector('canvas')).toBe(canvas); expect(runtime.renderers).toHaveLength(1);
  if (focus === 'axis' || focus === 'aligned-axis') {
    expect(document.activeElement).toBe(nextAxis);
    const nextView = focus === 'aligned-axis' ? 'left' : 'right'; expect(nextAxis.dataset.geometryAxis).toBe(nextView);
    // No intervening frame: adopted labels and Enter behavior already read the final camera.
    act(() => { nextAxis.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); draw(); });
    const camera = runtime.renderers[0].render.mock.calls.at(-1)![1]; expect(camera).toBeInstanceOf(OrthographicCamera);
    expect(camera.position.x * (nextView === 'right' ? 1 : -1)).toBeGreaterThan(0);
  } else expect(document.activeElement).toBe(focus === 'canvas' ? canvas : external);
});


it('restores same-copy Pan after remount, retains it while hidden, and defaults a new copy to Orbit', async () => {
  const updates = vi.spyOn(OrbitControls.prototype, 'update');
  let geometry: PreviewViewState['geometry'] = { copyId: 'copy', navigationMode: 'pan', outlines: false };
  const bytes = new Uint8Array([1]), signal = new AbortController().signal;
  const props = { bytes, name: 'assembly.step', signal, current: () => true, onViewStateChange: (patch: Partial<PreviewViewState>) => { geometry = patch.geometry; }, onGeometryRetire: vi.fn() };
  const makeScene = () => { const scene = new Group(); scene.add(new Mesh(new BoxGeometry(), new MeshBasicMaterial())); return { scene, bounds: new Box3().setFromObject(scene), dispose: vi.fn(), prepareStart: 0, ready: Promise.resolve() }; };
  try {
    for (const [index, copyId] of ['copy', 'copy', 'fresh'].entries()) {
      const saved = { geometry }, ui = render(createElement(GeometryPreview, { ...props, copyId, viewState: saved }));
      const container = ui.container.querySelector('section > div')!;
      Object.defineProperties(container, { clientWidth: { value: 640 }, clientHeight: { value: 480 } });
      await act(async () => { imports.cad[index].resolve(makeScene()); draw(); });
      expect(ui.getByRole('button', { name: 'Pan' }).getAttribute('aria-pressed')).toBe(String(copyId === 'copy'));
      const control = updates.mock.contexts.at(-1) as OrbitControls, target = control.target.clone(), camera = control.object.position.clone();
      const canvas = container.querySelector('canvas')!;
      act(() => { pointer(canvas, 'pointerdown', 300, 0); pointer(document, 'pointermove', 420, 0); pointer(document, 'pointerup', 420, 0); });
      expect(control.object.position.distanceTo(camera)).toBeGreaterThan(.01);
      if (copyId === 'copy') {
        expect(control.target.distanceTo(target)).toBeGreaterThan(.01); expect(geometry?.navigationMode).toBe('pan');
        ui.rerender(createElement(GeometryPreview, { ...props, copyId, viewState: { geometry }, visible: false }));
        ui.rerender(createElement(GeometryPreview, { ...props, copyId, viewState: { geometry }, visible: true }));
        expect(ui.getByRole('button', { name: 'Pan' }).getAttribute('aria-pressed')).toBe('true');
        expect(container.querySelector('canvas')).toBe(canvas); expect(imports.cad).toHaveLength(index + 1); expect(runtime.renderers).toHaveLength(index + 1);
      } else { expect(control.target.distanceTo(target)).toBeLessThan(1e-9); expect(geometry?.navigationMode).toBeUndefined(); }
      ui.unmount();
    }
  } finally { updates.mockRestore(); }
});

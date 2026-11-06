import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Box3, BoxGeometry, Group, Mesh, MeshStandardMaterial, type Vector2 } from 'three';
import GeometryPreview from '../src/app/components/documents/geometry/GeometryPreview';
import { outlineData } from '../src/app/components/documents/geometry/outline-data';
import { GEOMETRY_LIMITS as L } from '../src/app/components/documents/geometry/limits';
import { localeController } from '../src/locale/controller';

const runtime = vi.hoisted(() => ({ imports: [] as ((value: unknown) => void)[], workers: [] as { finish: (status?: 'on' | 'failed' | 'limit') => void; terminate: ReturnType<typeof vi.fn> }[] }));
vi.mock('../src/app/components/documents/geometry/parse', () => ({ parseGeometry: () => new Promise(resolve => runtime.imports.push(resolve)) }));
vi.mock('three', async original => {
  const actual = await original<typeof import('three')>();
  return { ...actual, WebGLRenderer: class {
    domElement = document.createElement('canvas'); size = new actual.Vector2();
    gl = { TIMEOUT_EXPIRED: 1, WAIT_FAILED: 2, ALREADY_SIGNALED: 3, SYNC_GPU_COMMANDS_COMPLETE: 4,
      clientWaitSync: () => 3, fenceSync: () => ({}), deleteSync() {}, flush() {} };
    getContext = () => this.gl;
    setDrawingBufferSize = (width: number, height: number) => this.size.set(width, height);
    getSize = (target: Vector2) => target.copy(this.size);
    setViewport() {} compile() {} render() {} clearDepth() {} dispose() {} forceContextLoss() {}
  } };
});
vi.mock('three/addons/controls/OrbitControls.js', async () => {
  const { EventDispatcher, Vector3 } = await import('three');
  return { OrbitControls: class extends EventDispatcher<any> {
    mouseButtons = {}; touches = {}; target = new Vector3();
    constructor(readonly object: import('three').Camera) { super(); }
    dispose() {} listenToKeyEvents() {}
    update() { this.object.lookAt(this.target); this.object.updateMatrixWorld(true); this.dispatchEvent({ type: 'change' }); }
  } };
});
beforeEach(() => {
  localeController.setLocale('en'); runtime.imports.length = 0; runtime.workers.length = 0;
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  vi.stubGlobal('requestAnimationFrame', () => 1); vi.stubGlobal('cancelAnimationFrame', () => {});
  vi.stubGlobal('matchMedia', () => ({ addEventListener() {}, removeEventListener() {} }));
  vi.stubGlobal('Worker', class {
    onmessage: ((event: unknown) => void) | null = null; onerror = null; onmessageerror = null; terminate = vi.fn();
    postMessage({ inputs }: { inputs: Parameters<typeof outlineData>[0][] }) {
      runtime.workers.push({ terminate: this.terminate, finish: (status = 'on') => this.onmessage?.({ data: status === 'on'
        ? { ok: true, data: inputs.map(input => outlineData(input, L.outlineSegments, performance.now() + 2000)) }
        : { ok: false, code: status === 'limit' ? 'limit' : 'worker' } }) });
    }
  });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
function source() {
  const scene = new Group(), geometry = new BoxGeometry(); scene.add(new Mesh(geometry, new MeshStandardMaterial()));
  let settle!: () => void;
  const ready = new Promise<void>(resolve => { settle = resolve; });
  return { settle, value: { scene, bounds: new Box3().setFromObject(scene), prepareStart: performance.now(), ready, pendingPreview: true, startPreview() {}, dispose() { geometry.dispose(); } } };
}
function preview() {
  const props = { bytes: new Uint8Array([1]), name: 'cube.stl', copyId: 'copy', signal: new AbortController().signal, current: () => true, viewState: {}, onViewStateChange: vi.fn(), onGeometryRetire: vi.fn() };
  const ui = render(<GeometryPreview {...props}/>);
  return { ui, props, button: () => ui.getByRole('button', { name: 'Model outlines' }), show: (visible: boolean) => ui.rerender(<GeometryPreview {...props} visible={visible}/>) };
}
async function imported(value: ReturnType<typeof source>, index = 0) { await act(async () => runtime.imports[index](value.value)); }
async function settle(value: ReturnType<typeof source>) { await act(async () => { value.settle(); await vi.dynamicImportSettled(); }); }
async function finish(index = 0, status: 'on' | 'failed' | 'limit' = 'on') { await act(async () => runtime.workers[index].finish(status)); }

it('resumes default outlines when surface preparation settles while hidden', async () => {
  const f = preview(), data = source(); await imported(data);
  expect(f.button().getAttribute('aria-busy')).toBe('true');
  f.show(false); await settle(data);
  expect(runtime.workers).toHaveLength(0);
  f.show(true); await act(async () => vi.dynamicImportSettled());
  expect(runtime.workers).toHaveLength(1);
  await finish(); expect(f.button().getAttribute('aria-pressed')).toBe('true');
  f.show(false); f.show(true); expect(runtime.workers).toHaveLength(1);
});

it('retains one pending request when shown before surface preparation settles', async () => {
  const f = preview(), data = source(); await imported(data);
  f.show(false); f.show(true); await settle(data);
  expect(runtime.workers).toHaveLength(1); await finish();
  expect(f.button().getAttribute('aria-pressed')).toBe('true');
});

it('keeps an explicit Off after a hidden suspended request settles', async () => {
  const f = preview(), data = source(); await imported(data);
  f.show(false); await settle(data); f.show(true);
  fireEvent.click(f.button()); await act(async () => vi.dynamicImportSettled());
  expect(f.button().getAttribute('aria-pressed')).toBe('false');
  expect(runtime.workers).toHaveLength(0);
  f.show(false); f.show(true); await act(async () => vi.dynamicImportSettled());
  expect(runtime.workers).toHaveLength(0);
});

it('lets a hidden Off supersede the request still waiting for surface preparation', async () => {
  const f = preview(), data = source(); await imported(data); f.show(false);
  fireEvent.click(f.button()); await settle(data); f.show(true);
  await act(async () => vi.dynamicImportSettled());
  expect(runtime.workers).toHaveLength(0); expect(f.button().getAttribute('aria-busy')).toBe('false');
  expect(f.button().getAttribute('aria-pressed')).toBe('false');
});

it('keeps a running outline worker through hiding and resumes without restarting it', async () => {
  const f = preview(), data = source(); await imported(data); await settle(data);
  expect(runtime.workers).toHaveLength(1); f.show(false); f.show(true);
  await act(async () => vi.dynamicImportSettled()); expect(runtime.workers).toHaveLength(1);
  f.show(false); await finish(); f.show(true);
  expect(runtime.workers).toHaveLength(1); expect(f.button().getAttribute('aria-pressed')).toBe('true');
});

it('fences stale preparation and requests after a replacement copy mounts', async () => {
  const f = preview(), old = source(); await imported(old); f.show(false);
  const props = { ...f.props, bytes: new Uint8Array([2]), copyId: 'new' };
  f.ui.rerender(<GeometryPreview {...props} visible={false}/>);
  const next = source(); await imported(next, 1); await settle(old); await settle(next);
  expect(runtime.workers).toHaveLength(0);
  f.ui.rerender(<GeometryPreview {...props}/>); await act(async () => vi.dynamicImportSettled());
  expect(runtime.workers).toHaveLength(1); await finish();
  expect(f.button().getAttribute('aria-pressed')).toBe('true');
});

it('ignores a retired worker completion while the replacement is waiting for preparation', async () => {
  const f = preview(), old = source(); await imported(old); await settle(old);
  expect(runtime.workers).toHaveLength(1);
  const props = { ...f.props, bytes: new Uint8Array([2]), copyId: 'new' };
  f.ui.rerender(<GeometryPreview {...props}/>);
  const next = source(); await imported(next, 1);
  expect(runtime.workers[0].terminate).toHaveBeenCalledOnce();
  await finish(); expect(f.button().getAttribute('aria-busy')).toBe('true');
  f.ui.rerender(<GeometryPreview {...props} visible={false}/>); await settle(next);
  f.ui.rerender(<GeometryPreview {...props}/>); await act(async () => vi.dynamicImportSettled());
  expect(runtime.workers).toHaveLength(2); await finish(1);
  expect(f.button().getAttribute('aria-pressed')).toBe('true');
});

it.each(['failed', 'limit'] as const)('does not retry an actual outline %s across hiding', async status => {
  const f = preview(), data = source(); await imported(data); await settle(data);
  expect(runtime.workers).toHaveLength(1); f.show(false); await finish(0, status);
  f.show(true); await act(async () => vi.dynamicImportSettled());
  expect(runtime.workers).toHaveLength(1); expect(f.button().getAttribute('aria-busy')).toBe('false');
  expect(f.ui.getByRole('status').textContent).toBeTruthy();
});

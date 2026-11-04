import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { BoxGeometry, Group, Mesh, MeshStandardMaterial } from 'three';
import { Profiler } from 'react';
import GeometryPreview from '../src/app/components/documents/geometry/GeometryPreview';
import { disposeScene, validateScene } from '../src/app/components/documents/geometry/scene-resources';
import { localeController } from '../src/locale/controller';
import type { GeometryImportStage } from '../src/app/components/documents/geometry/stages';
import { GeometryError } from '../src/app/components/documents/geometry/limits';
const graphics = vi.hoisted(() => vi.fn(() => ({ dispose: vi.fn() })));
const imports = vi.hoisted(() => ({ pending: [] as { resolve: (value: unknown) => void; reject: (reason: unknown) => void }[], cad: [] as { stage: (stage: GeometryImportStage) => void; resolve: (value: unknown) => void }[], mounts: [] as { fit: ReturnType<typeof vi.fn>; setOutlines: ReturnType<typeof vi.fn>; setProjection: ReturnType<typeof vi.fn>; dispose: ReturnType<typeof vi.fn>; publish: (patch: unknown) => void; pending?: unknown; saved?: unknown; flush: ReturnType<typeof vi.fn> }[] }));
vi.mock('../src/app/components/documents/geometry/glb', () => ({ importGlb: () => new Promise((resolve, reject) => { imports.pending.push({ resolve, reject }); }) }));
vi.mock('../src/app/components/documents/geometry/parse', () => ({ parseGeometry: (_bytes: unknown, _format: unknown, _signal: unknown, stage: (stage: GeometryImportStage) => void) => new Promise(resolve => { imports.cad.push({ stage, resolve }); }) }));
vi.mock('../src/app/components/documents/geometry/viewport', () => ({ prepareGeometryGraphics:graphics,mountGeometry: (_element: unknown, _object: unknown, _bounds: unknown, saved: unknown, _labels: unknown, publish: (patch: unknown) => void) => {
  const control = { fit: vi.fn(), reset: vi.fn(), focus: vi.fn(), isolate: vi.fn(), restore: vi.fn(), showAll: vi.fn(), refreshPreview: vi.fn(), projection: 'perspective', setProjection: vi.fn(), setOutlines: vi.fn(async () => 'on'), relabel: vi.fn(), inspect: () => ({ name: 'Body', triangles: 12, visibleTriangles: 12, min: [-5, -5, -5], max: [5, 5, 5], dimensions: [10, 10, 10] }), dispose: vi.fn(), publish, saved, pending: undefined as unknown, flush: vi.fn((save = publish) => { if (control.pending) { const patch = control.pending; control.pending = undefined; save(patch); } }), select: vi.fn(), visibility: vi.fn(), parts: [{ id: '0', name: 'Body', depth: 0, kind: 'mesh' }] }; imports.mounts.push(control); return control;
} }));
afterEach(() => { cleanup(); imports.pending = []; imports.cad = []; imports.mounts = []; });
function graph() {
  const scene = new Group(), geometry = new BoxGeometry(10, 10, 10); scene.add(new Mesh(geometry, new MeshStandardMaterial()));
  const disposed = vi.spyOn(geometry, 'dispose'); return { value: { scene, scenes: [scene], bounds: validateScene([scene]), dispose: () => disposeScene([scene]) }, disposed };
}
it('reports early graphics failure and disposes an import that completes after retirement', async () => {
  graphics.mockImplementationOnce(() => { throw new GeometryError('webgl'); });
  const source = graph(), ui = render(<GeometryPreview bytes={new Uint8Array([1])} name="cube.glb" copyId="copy" signal={new AbortController().signal} current={() => true} viewState={{}} onViewStateChange={vi.fn()} onGeometryRetire={vi.fn()}/>);
  await act(async () => {}); expect(ui.getByRole('alert')).toBeDefined(); expect(imports.mounts).toHaveLength(0);
  await act(async () => imports.pending[0].resolve(source.value)); expect(source.disposed).toHaveBeenCalledOnce();
});
it.each([undefined, true, false])('starts outlines by default and respects the saved preference: %s', async preference => {
  const source = graph();
  const ui = render(<GeometryPreview bytes={new Uint8Array([1])} name="cube.glb" copyId="copy" signal={new AbortController().signal} current={() => true} viewState={{ geometry: { copyId: 'copy', outlines: preference } }} onViewStateChange={vi.fn()} onGeometryRetire={vi.fn()}/>);
  await act(async () => imports.pending[0].resolve(source.value));
  expect(imports.mounts[0].setOutlines).toHaveBeenCalledTimes(preference === false ? 0 : 1);
  expect(ui.getByRole('button', { name: 'Model outlines' }).getAttribute('aria-pressed')).toBe(String(preference !== false));
});
it('copy retirement fences an import before React unmount and disposes its late graph', async () => {
  let owned = true; const changed = vi.fn(), source = graph();
  render(<GeometryPreview bytes={new Uint8Array([1])} name="cube.glb" copyId="first" signal={new AbortController().signal} current={() => owned} viewState={{}} onViewStateChange={changed} onGeometryRetire={vi.fn()}/>);
  owned = false; await act(async () => imports.pending[0].resolve(source.value));
  expect(imports.mounts).toHaveLength(0); expect(changed).not.toHaveBeenCalled(); expect(source.disposed).toHaveBeenCalledOnce();
});
it.each(['success', 'failure'])('late %s from a superseded copy cannot clear the replacement controls', async outcome => {
  const signal = new AbortController().signal, changed = vi.fn(), props = { name: 'cube.glb', signal, current: () => true, viewState: {}, onViewStateChange: changed, onGeometryRetire: vi.fn() };
  const ui = render(<GeometryPreview {...props} bytes={new Uint8Array([1])} copyId="first"/>);
  ui.rerender(<GeometryPreview {...props} bytes={new Uint8Array([2])} copyId="second"/>);
  const current = graph(); await act(async () => imports.pending[1].resolve(current.value));
  const late = graph(); await act(async () => { if (outcome === 'success') imports.pending[0].resolve(late.value); else imports.pending[0].reject(new Error('late')); });
  fireEvent.click(ui.getByRole('button', { name: 'Fit view' })); expect(imports.mounts[0].fit).toHaveBeenCalledOnce();
  act(() => imports.mounts[0].publish({ selected: '0', hidden: [] })); expect(changed).toHaveBeenLastCalledWith({ geometry: { copyId: 'second', selected: '0', hidden: [] } });
  expect(ui.queryByRole('alert')).toBeNull(); if (outcome === 'success') expect(late.disposed).toHaveBeenCalledOnce();
  ui.unmount(); expect(current.disposed).toHaveBeenCalledOnce(); expect(imports.mounts[0].dispose).toHaveBeenCalledOnce();
});
it('camera-only publications do not render the React parts tree', async () => {
  const rendered = vi.fn(), changed = vi.fn(), source = graph();
  const ui = render(<Profiler id="geometry" onRender={rendered}><GeometryPreview bytes={new Uint8Array([1])} name="cube.glb" copyId="first" signal={new AbortController().signal} current={() => true} viewState={{}} onViewStateChange={changed} onGeometryRetire={vi.fn()}/></Profiler>);
  await act(async () => imports.pending[0].resolve(source.value)); rendered.mockClear();
  for (let i = 0; i < 20; i++) act(() => imports.mounts[0].publish({ camera: { projection: 'perspective', position: [i, 2, 3], target: [0, 0, 0], up: [0, 1, 0] }, hidden: [] }));
  expect(changed).toHaveBeenCalledTimes(20); expect(rendered).not.toHaveBeenCalled();
  act(() => imports.mounts[0].publish({ selected: '0', hidden: [] })); expect(rendered).toHaveBeenCalledOnce(); ui.unmount();
});
it.each(['hide', 'unmount'] as const)('flushes final inert metadata on same-copy %s and restores it on remount', async retirement => {
  const signal = new AbortController(), changed = vi.fn(), source = graph(); let geometry: any;
  const props = { bytes: new Uint8Array([1]), name: 'cube.glb', copyId: 'copy', signal: signal.signal, current: () => !signal.signal.aborted, viewState: {}, onViewStateChange: changed, onGeometryRetire: (value: unknown) => { geometry = value; } };
  const ui = render(<GeometryPreview {...props}/>); await act(async () => imports.pending[0].resolve(source.value));
  const camera = { projection: 'orthographic', verticalSpan: 12, position: [9, 8, 7], target: [0, 0, 0], up: [0, 1, 0] }; imports.mounts[0].pending = { camera, selected: '0', hidden: [] };
  if (retirement === 'hide') act(() => signal.abort());
  ui.unmount(); expect(geometry).toEqual({ copyId: 'copy', camera, selected: '0', hidden: [] }); expect(changed).not.toHaveBeenCalled();
  const next = render(<GeometryPreview {...props} signal={new AbortController().signal} current={() => true} viewState={{ geometry }}/>);
  await act(async () => imports.pending[1].resolve(graph().value)); expect(imports.mounts[1].saved).toEqual(geometry); next.unmount();
});
it('shows projection choices in the existing View menu and keeps orthographic camera-only poses out of React', async () => {
  localeController.setLocale('en'); const rendered = vi.fn(), source = graph();
  const ui = render(<Profiler id="geometry" onRender={rendered}><GeometryPreview bytes={new Uint8Array([1])} name="cube.glb" copyId="copy" signal={new AbortController().signal} current={() => true} viewState={{}} onViewStateChange={vi.fn()} onGeometryRetire={vi.fn()}/></Profiler>);
  await act(async () => imports.pending[0].resolve(source.value));
  fireEvent.click(ui.getByRole('button', { name: 'View angle' })); expect(ui.getByRole('menuitem', { name: 'Perspective' }).getAttribute('aria-current')).toBe('true');
  fireEvent.click(ui.getByRole('menuitem', { name: 'Orthographic' })); expect(imports.mounts[0].setProjection).toHaveBeenCalledExactlyOnceWith('orthographic');
  act(() => imports.mounts[0].publish({ camera: { projection: 'orthographic', verticalSpan: 12, position: [9, 8, 7], target: [0, 0, 0], up: [0, 1, 0] }, hidden: [] }));
  fireEvent.click(ui.getByRole('button', { name: 'View angle' })); expect(ui.getByRole('menuitem', { name: 'Orthographic' }).getAttribute('aria-current')).toBe('true'); fireEvent.keyDown(ui.getByRole('menu'), { key: 'Escape' });
  rendered.mockClear();
  for (let i = 0; i < 20; i++) act(() => imports.mounts[0].publish({ camera: { projection: 'orthographic', verticalSpan: 12 / (i + 1), position: [9 + i, 8, 7], target: [0, 0, 0], up: [0, 1, 0] }, hidden: [] }));
  expect(rendered).not.toHaveBeenCalled();
});
it('ignores projection metadata retained for a different content copy', async () => {
  const ui = render(<GeometryPreview bytes={new Uint8Array([1])} name="cube.glb" copyId="new" signal={new AbortController().signal} current={() => true} viewState={{ geometry: { copyId: 'old', camera: { projection: 'orthographic', verticalSpan: 12, position: [9, 8, 7], target: [0, 0, 0], up: [0, 1, 0] } } }} onViewStateChange={vi.fn()} onGeometryRetire={vi.fn()}/>);
  await act(async () => imports.pending[0].resolve(graph().value)); expect(imports.mounts[0].saved).toBeUndefined(); ui.unmount();
});
it.each(['en', 'zh'] as const)('shows observed localized import stages and removes loading only after display preparation (%s)', async locale => {
  localeController.setLocale(locale);
  const ui = render(<GeometryPreview bytes={new Uint8Array([1])} name="cube.step" copyId="copy" signal={new AbortController().signal} current={() => true} viewState={{}} onViewStateChange={vi.fn()} onGeometryRetire={vi.fn()}/>);
  const labels = locale === 'en' ? ['Loading model…', 'Initializing CAD runtime…', 'Importing model…', 'Preparing display…'] : ['正在加载模型…', '正在初始化 CAD 运行环境…', '正在导入模型…', '正在准备显示…'];
  expect(ui.getByRole('status').textContent).toBe(labels[0]); expect(imports.mounts).toHaveLength(0);
  for (const [index, stage] of (['initializing', 'importing', 'preparing'] as const).entries()) {
    act(() => imports.cad[0].stage(stage)); expect(ui.getByRole('status').textContent).toBe(labels[index + 1]); expect(imports.mounts).toHaveLength(0);
  }
  await act(async () => imports.cad[0].resolve({ ...graph().value, prepareStart: performance.now(), ready: Promise.resolve() }));
  expect(imports.mounts).toHaveLength(1); expect(ui.queryByRole('status')).toBeNull();
  expect(performance.getEntriesByName('geometry:display-prepare:step')).toHaveLength(1);
});
it('fences late stage observations from superseded or cancelled imports', () => {
  const signal = new AbortController(), props = { name: 'cube.step', signal: signal.signal, current: () => true, viewState: {}, onViewStateChange: vi.fn(), onGeometryRetire: vi.fn() };
  const ui = render(<GeometryPreview {...props} bytes={new Uint8Array([1])} copyId="first"/>);
  act(() => imports.cad[0].stage('initializing'));
  ui.rerender(<GeometryPreview {...props} bytes={new Uint8Array([2])} copyId="second"/>);
  act(() => imports.cad[0].stage('preparing')); expect(ui.getByRole('status').textContent).toBe('Loading model…');
  act(() => imports.cad[1].stage('initializing')); expect(ui.getByRole('status').textContent).toBe('Initializing CAD runtime…');
  act(() => signal.abort()); act(() => imports.cad[1].stage('importing'));
  expect(ui.getByRole('status').textContent).toBe('Initializing CAD runtime…'); expect(imports.mounts).toHaveLength(0);
});

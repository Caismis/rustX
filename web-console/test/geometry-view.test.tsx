import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { BoxGeometry, Group, Mesh, MeshStandardMaterial, PerspectiveCamera, Vector3 } from 'three';
import { Profiler } from 'react';
import GeometryPreview from '../src/app/components/documents/geometry/GeometryPreview';
import { GeometryViewGlyph } from '../src/app/components/documents/geometry/GeometryViewGlyph';
import { disposeScene, validateScene } from '../src/app/components/documents/geometry/scene-resources';
import { localeController } from '../src/locale/controller';
import type { GeometryImportStage } from '../src/app/components/documents/geometry/stages';
import { GeometryError } from '../src/app/components/documents/geometry/limits';
import * as progressive from '../src/app/components/documents/geometry/progressive';
const graphics = vi.hoisted(() => vi.fn(() => ({ dispose: vi.fn() })));
const imports = vi.hoisted(() => ({ pending: [] as { resolve: (value: unknown) => void; reject: (reason: unknown) => void }[], cad: [] as { stage: (stage: GeometryImportStage) => void; resolve: (value: unknown) => void }[], mounts: [] as { fit: ReturnType<typeof vi.fn>; setOutlines: ReturnType<typeof vi.fn>; setProjection: ReturnType<typeof vi.fn>; setNavigationMode: ReturnType<typeof vi.fn>; focusAxis: ReturnType<typeof vi.fn>; dispose: ReturnType<typeof vi.fn>; publish: (patch: unknown) => void; pending?: unknown; saved?: unknown; flush: ReturnType<typeof vi.fn> }[] }));
vi.mock('../src/app/components/documents/geometry/glb', () => ({ importGlb: () => new Promise((resolve, reject) => { imports.pending.push({ resolve, reject }); }) }));
vi.mock('../src/app/components/documents/geometry/parse', () => ({ parseGeometry: (_bytes: unknown, _format: unknown, _signal: unknown, stage: (stage: GeometryImportStage) => void) => new Promise(resolve => { imports.cad.push({ stage, resolve }); }) }));
vi.mock('../src/app/components/documents/geometry/viewport', () => ({ prepareGeometryGraphics:graphics,mountGeometry: (_element: unknown, _object: unknown, _bounds: unknown, saved: unknown, _labels: unknown, publish: (patch: unknown) => void) => {
  const control = { navigationMode: (saved as { navigationMode?: string } | undefined)?.navigationMode === 'pan' ? 'pan' : 'orbit', setNavigationMode: vi.fn((mode: 'pan' | 'orbit') => { control.navigationMode = mode; publish({ navigationMode: mode === 'pan' ? mode : undefined }); }), setVisible: vi.fn(), focusAxis: vi.fn(), fit: vi.fn(), reset: vi.fn(), focus: vi.fn(), isolate: vi.fn(), restore: vi.fn(), showAll: vi.fn(), refreshPreview: vi.fn(), projection: (saved as { camera?: { projection?: string } } | undefined)?.camera?.projection ?? 'orthographic', setProjection: vi.fn(), setOutlines: vi.fn(async (enabled:boolean) => enabled?'on':'off'), relabel: vi.fn(), inspect: () => ({ name: 'Body', triangles: 12, visibleTriangles: 12, min: [-5, -5, -5], max: [5, 5, 5], dimensions: [10, 10, 10] }), dispose: vi.fn(), publish, saved, pending: undefined as unknown, flush: vi.fn((save = publish) => { if (control.pending) { const patch = control.pending; control.pending = undefined; save(patch); } }), select: vi.fn(), visibility: vi.fn(), parts: [{ id: '0', name: 'Body', depth: 0, kind: 'mesh' }, { id: '0.1', name: 'Wing', depth: 1, kind: 'mesh' }] }; imports.mounts.push(control); return control;
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
it('keeps an explicit Off choice after optional surface preparation settles', async () => {
  const source=graph();let settle!:()=>void;const ready=new Promise<void>(resolve=>{settle=resolve;});
  const ui=render(<GeometryPreview bytes={new Uint8Array([1])} name="cube.step" copyId="copy" signal={new AbortController().signal} current={()=>true} viewState={{geometry:{copyId:'copy',outlines:true}}} onViewStateChange={vi.fn()} onGeometryRetire={vi.fn()}/>);
  await act(async()=>imports.cad[0].resolve({scene:source.value.scene,bounds:source.value.bounds,dispose:source.value.dispose,prepareStart:performance.now(),ready,pendingPreview:true,startPreview:vi.fn()}));
  const button=ui.getByRole('button',{name:'Model outlines'});expect(button.getAttribute('aria-pressed')).toBe('true');
  await act(async()=>fireEvent.click(button));expect(button.getAttribute('aria-pressed')).toBe('false');
  await act(async()=>settle());expect(button.getAttribute('aria-pressed')).toBe('false');expect(imports.mounts[0].setOutlines).toHaveBeenLastCalledWith(false);
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
  for (let i = 0; i < 20; i++) act(() => imports.mounts[0].publish({ camera: { projection: 'orthographic', verticalSpan: 12, position: [i, 2, 3], target: [0, 0, 0], up: [0, 1, 0] }, hidden: [] }));
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
  fireEvent.click(ui.getByRole('button', { name: 'View angle' })); expect(ui.getByRole('menuitem', { name: 'Orthographic' }).getAttribute('aria-current')).toBe('true');
  fireEvent.click(ui.getByRole('menuitem', { name: 'Perspective' })); expect(imports.mounts[0].setProjection).toHaveBeenCalledExactlyOnceWith('perspective');
  act(() => imports.mounts[0].publish({ camera: { projection: 'orthographic', verticalSpan: 12, position: [9, 8, 7], target: [0, 0, 0], up: [0, 1, 0] }, hidden: [] }));
  fireEvent.click(ui.getByRole('button', { name: 'View angle' })); expect(ui.getByRole('menuitem', { name: 'Orthographic' }).getAttribute('aria-current')).toBe('true'); fireEvent.keyDown(ui.getByRole('menu'), { key: 'Escape' });
  rendered.mockClear();
  for (let i = 0; i < 20; i++) act(() => imports.mounts[0].publish({ camera: { projection: 'orthographic', verticalSpan: 12 / (i + 1), position: [9 + i, 8, 7], target: [0, 0, 0], up: [0, 1, 0] }, hidden: [] }));
  expect(rendered).not.toHaveBeenCalled();
});
it('selects a saved perspective camera in the View menu for the same content copy', async () => {
  const geometry = { copyId: 'copy', camera: { projection: 'perspective' as const, position: [9, 8, 7] as [number, number, number], target: [0, 0, 0] as [number, number, number], up: [0, 1, 0] as [number, number, number] } };
  const ui = render(<GeometryPreview bytes={new Uint8Array([1])} name="cube.glb" copyId="copy" signal={new AbortController().signal} current={() => true} viewState={{ geometry }} onViewStateChange={vi.fn()} onGeometryRetire={vi.fn()}/>);
  await act(async () => imports.pending[0].resolve(graph().value)); expect(imports.mounts[0].saved).toEqual(geometry);
  fireEvent.click(ui.getByRole('button', { name: 'View angle' })); expect(ui.getByRole('menuitem', { name: 'Perspective' }).getAttribute('aria-current')).toBe('true');
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

it.each(['en', 'zh'] as const)('keeps outline preparation silent while exposing busy state (%s)', async locale => {
  localeController.setLocale(locale); const source = graph();
  const ui = render(<GeometryPreview bytes={new Uint8Array([1])} name="cube.glb" copyId="copy" signal={new AbortController().signal} current={() => true} viewState={{ geometry: { copyId: 'copy', outlines: false } }} onViewStateChange={vi.fn()} onGeometryRetire={vi.fn()}/>);
  await act(async () => imports.pending[0].resolve(source.value));
  let finish!: (status: 'on' | 'failed') => void;
  imports.mounts[0].setOutlines.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const button = ui.getByRole('button', { name: locale === 'en' ? 'Model outlines' : '轮廓线' });
  fireEvent.click(button); expect(button.getAttribute('aria-busy')).toBe('true'); expect(ui.queryByRole('status')).toBeNull();
  await act(async () => finish('failed')); expect(button.getAttribute('aria-busy')).toBe('false'); expect(ui.getByRole('status').textContent).toBeTruthy();
});

it('maps each standard direction to one distinct convex face and draws the hidden cube edges', () => {
  const views = ['top', 'bottom', 'left', 'right', 'front', 'back'] as const;
  const regions = views.map(view => {
    const ui = render(<GeometryViewGlyph view={view}/>), svg = ui.container.querySelector('svg')!;
    const polygons = [...svg.querySelectorAll('polygon')]; expect(polygons).toHaveLength(1);
    const polygon = polygons[0], points = polygon.getAttribute('points')!.split(' ').map(pair => pair.split(',').map(Number) as [number, number]);
    expect(points).toHaveLength(4); expect(polygon.getAttribute('fill')).toBe('currentColor'); expect(polygon.getAttribute('fill-opacity')).toBe('.32');
    const turns = points.map((point, index) => {
      const next = points[(index + 1) % points.length], after = points[(index + 2) % points.length];
      return (next[0] - point[0]) * (after[1] - next[1]) - (next[1] - point[1]) * (after[0] - next[0]);
    });
    expect(turns.every(turn => Math.abs(turn) > 1e-6)).toBe(true);
    expect(turns.every(turn => Math.sign(turn) === Math.sign(turns[0]))).toBe(true);
    const key = points.map(point => point.join(',')).sort().join('|');
    const hidden = svg.querySelector('path[stroke-dasharray]'); expect(hidden).not.toBeNull();
    expect((hidden?.getAttribute('d')?.match(/M/g) ?? [])).toHaveLength(3);
    expect(svg.querySelectorAll('path')).toHaveLength(2); expect(svg.querySelector('[marker-end]')).toBeNull();
    ui.unmount(); return key;
  });
  expect(new Set(regions).size).toBe(6);
});

it('places each highlighted direction on the same side as its projected standard face normal', () => {
  const camera = new PerspectiveCamera(50, 1, 0.1, 10);
  camera.position.set(1, 1, 1); camera.up.set(0, 1, 0); camera.lookAt(0, 0, 0); camera.updateMatrixWorld(true);
  const normals = {
    front: new Vector3(0, 0, 1), back: new Vector3(0, 0, -1),
    left: new Vector3(-1, 0, 0), right: new Vector3(1, 0, 0),
    top: new Vector3(0, 1, 0), bottom: new Vector3(0, -1, 0),
  } as const;
  for (const [view, normal] of Object.entries(normals)) {
    const projected = normal.clone().project(camera), ui = render(<GeometryViewGlyph view={view as keyof typeof normals}/>);
    const points = [...ui.container.querySelectorAll('polygon')[0].getAttribute('points')!.matchAll(/(-?[\d.]+),(-?[\d.]+)/g)]
      .map(([, x, y]) => [Number(x), Number(y)] as const);
    const centroid = points.reduce(([x, y], point) => [x + point[0] / points.length, y + point[1] / points.length], [0, 0]);
    const side = (value: number) => Math.abs(value) < 1e-8 ? 0 : Math.sign(value);
    expect(side(centroid[0] - 8)).toBe(side(projected.x));
    expect(side(centroid[1] - 8)).toBe(-side(projected.y));
    ui.unmount();
  }
});

it('toggles isolation, switches its target, and can restore after the selection is cleared', async () => {
  localeController.setLocale('en'); const source = graph();
  const ui = render(<GeometryPreview bytes={new Uint8Array([1])} name="cube.glb" copyId="copy" signal={new AbortController().signal} current={() => true} viewState={{}} onViewStateChange={vi.fn()} onGeometryRetire={vi.fn()}/>);
  await act(async () => imports.pending[0].resolve(source.value));
  const pan = ui.getByRole('button', { name: 'Pan' }), outlines = ui.getByRole('button', { name: 'Model outlines' });
  fireEvent.click(pan);
  const sharedActiveClass = [...outlines.classList].find(name => name.includes('controlActive'))!;
  expect(sharedActiveClass).toBeTruthy(); expect(outlines.getAttribute('aria-pressed')).toBe('true');
  expect(pan.classList.contains(sharedActiveClass)).toBe(true);
  const control = imports.mounts[0] as any;
  let selected: string | undefined, isolated: { target: string; hidden: string[] } | undefined;
  control.select.mockImplementation((id: string | undefined) => { selected = id; control.publish({ selected, hidden: [], isolation: isolated }); });
  control.isolate.mockImplementation(() => { isolated = { target: selected!, hidden: [] }; control.publish({ selected, hidden: ['0.1'], isolation: isolated }); });
  control.restore.mockImplementation(() => { isolated = undefined; control.publish({ selected, hidden: [], isolation: undefined }); });
  fireEvent.click(ui.getByRole('button', { name: 'Parts' }));
  fireEvent.click(ui.getByRole('button', { name: 'Expand: Body' }));
  const isolate = ui.getByRole('button', { name: 'Isolate' });
  expect(isolate.getAttribute('aria-pressed')).toBe('false'); expect(isolate).toHaveProperty('disabled', true);
  fireEvent.click(ui.getByRole('button', { name: 'Body' })); expect(isolate).toHaveProperty('disabled', false);
  fireEvent.click(isolate); expect(isolate.getAttribute('aria-pressed')).toBe('true'); expect(isolate.classList.contains(sharedActiveClass)).toBe(true);
  fireEvent.click(ui.getByRole('button', { name: 'Wing' })); fireEvent.click(isolate);
  expect(control.isolate).toHaveBeenCalledTimes(2); expect(control.restore).not.toHaveBeenCalled(); expect(isolate.getAttribute('aria-pressed')).toBe('true');
  fireEvent.click(ui.getByRole('button', { name: 'Wing' }));
  expect(isolate).toHaveProperty('disabled', false); expect(isolate.getAttribute('aria-pressed')).toBe('true');
  fireEvent.click(isolate); expect(control.restore).toHaveBeenCalledOnce(); expect(isolate.getAttribute('aria-pressed')).toBe('false');
  expect(isolate).toHaveProperty('disabled', true); expect(ui.queryByRole('button', { name: 'Exit isolation' })).toBeNull();
});

it('keeps the imported scene and controls through presentation hiding and defers new outlines', async () => {
  const source = graph(), bytes = new Uint8Array([1]), props = { bytes, name: 'cube.glb', copyId: 'copy', signal: new AbortController().signal, current: () => true, viewState: {}, onViewStateChange: vi.fn(), onGeometryRetire: vi.fn() };
  const ui = render(<GeometryPreview {...props}/>);
  ui.rerender(<GeometryPreview {...props} visible={false}/>);
  await act(async () => imports.pending[0].resolve(source.value));
  const control = imports.mounts[0];
  expect(control.setOutlines).not.toHaveBeenCalled();
  expect(source.disposed).not.toHaveBeenCalled();
  ui.rerender(<GeometryPreview {...props} visible/>);
  expect(imports.pending).toHaveLength(1); expect(imports.mounts).toHaveLength(1);
  expect(control.setOutlines).toHaveBeenCalledExactlyOnceWith(true);
  ui.rerender(<GeometryPreview {...props} visible={false}/>);
  expect(control.dispose).not.toHaveBeenCalled(); expect(source.disposed).not.toHaveBeenCalled();
  ui.rerender(<GeometryPreview {...props} visible/>);
  expect(imports.mounts[0]).toBe(control); expect(imports.pending).toHaveLength(1);
  ui.unmount(); expect(control.dispose).toHaveBeenCalledOnce(); expect(source.disposed).toHaveBeenCalledOnce();
});

it('late retirement of an old import preserves the hidden replacement outline demand', async () => {
  const props = { name: 'cube.glb', signal: new AbortController().signal, current: () => true, viewState: {}, onViewStateChange: vi.fn(), onGeometryRetire: vi.fn() };
  const oldBytes = new Uint8Array([1]), newBytes = new Uint8Array([2]);
  const ui = render(<GeometryPreview {...props} visible={false} bytes={oldBytes} copyId="old"/>);
  ui.rerender(<GeometryPreview {...props} visible={false} bytes={newBytes} copyId="new"/>);
  await act(async () => imports.pending[1].resolve(graph().value));
  await act(async () => imports.pending[0].resolve(graph().value));
  expect(imports.mounts[0].setOutlines).not.toHaveBeenCalled();
  ui.rerender(<GeometryPreview {...props} visible bytes={newBytes} copyId="new"/>);
  expect(imports.mounts[0].setOutlines).toHaveBeenCalledExactlyOnceWith(true);
});

it('does not reimport the same copy after background time or unrelated view-state notifications', async () => {
  const bytes = new Uint8Array([1]), source = graph(), props = { bytes, name: 'cube.glb', copyId: 'copy', signal: new AbortController().signal, current: () => true, viewState: {}, onViewStateChange: vi.fn(), onGeometryRetire: vi.fn() };
  const ui = render(<GeometryPreview {...props}/>); await act(async () => imports.pending[0].resolve(source.value));
  vi.useFakeTimers();
  try {
    ui.rerender(<GeometryPreview {...props} visible={false}/>);
    await act(async () => vi.advanceTimersByTime(120_000));
    ui.rerender(<GeometryPreview {...props} visible viewState={{ bodyScrollTop: 17 }}/>);
    expect(imports.pending).toHaveLength(1); expect(imports.mounts).toHaveLength(1); expect(source.disposed).not.toHaveBeenCalled();
  } finally { vi.useRealTimers(); }
});

it.each(['body', 'external'] as const)('hidden provisional completion leaves %s focus untouched', async focus => {
  vi.spyOn(progressive, 'progressivePresentation').mockReturnValue({ setVisible: vi.fn(), add: vi.fn(), dispose: vi.fn(), complete: async () => ({ focusedAxis: 'X', viewportSize: { clientWidth: 800, clientHeight: 600 }, navigationMode: 'orbit' }) });
  const props = { bytes: new Uint8Array([1]), name: 'cube.step', copyId: 'copy', signal: new AbortController().signal, current: () => true, viewState: {}, onViewStateChange: vi.fn(), onGeometryRetire: vi.fn() };
  const ui = render(<><button data-testid="external">External</button><GeometryPreview {...props}/></>);
  ui.rerender(<><button data-testid="external">External</button><GeometryPreview {...props} visible={false}/></>);
  const external = ui.getByTestId('external');
  if (focus === 'external') external.focus(); else (document.activeElement as HTMLElement | null)?.blur();
  const expected = focus === 'external' ? external : document.body;
  const source = graph();
  await act(async () => imports.cad[0].resolve({ scene: source.value.scene, bounds: source.value.bounds, dispose: source.value.dispose, prepareStart: performance.now(), ready: Promise.resolve(), pendingPreview: false, startPreview: vi.fn() }));
  expect(document.activeElement).toBe(expected); expect(imports.mounts[0].focusAxis).not.toHaveBeenCalled();
});

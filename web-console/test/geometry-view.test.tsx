import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { BoxGeometry, Group, Mesh, MeshStandardMaterial } from 'three';
import GeometryPreview from '../src/app/components/documents/geometry/GeometryPreview';
import { disposeScene, validateScene } from '../src/app/components/documents/geometry/scene-resources';
const imports = vi.hoisted(() => ({ pending: [] as { resolve: (value: unknown) => void; reject: (reason: unknown) => void }[], mounts: [] as { fit: ReturnType<typeof vi.fn>; dispose: ReturnType<typeof vi.fn>; publish: (patch: unknown) => void }[] }));
vi.mock('../src/app/components/documents/geometry/glb', () => ({ importGlb: () => new Promise((resolve, reject) => { imports.pending.push({ resolve, reject }); }) }));
vi.mock('../src/app/components/documents/geometry/viewport', () => ({ mountGeometry: (_element: unknown, _object: unknown, _bounds: unknown, _saved: unknown, _labels: unknown, publish: (patch: unknown) => void) => {
  const control = { fit: vi.fn(), dispose: vi.fn(), publish, select: vi.fn(), visibility: vi.fn(), parts: [{ id: '0', name: 'Body', depth: 0, kind: 'mesh' }] }; imports.mounts.push(control); return control;
} }));
afterEach(() => { cleanup(); imports.pending = []; imports.mounts = []; });
function graph() {
  const scene = new Group(), geometry = new BoxGeometry(10, 10, 10); scene.add(new Mesh(geometry, new MeshStandardMaterial()));
  const disposed = vi.spyOn(geometry, 'dispose'); return { value: { scene, scenes: [scene], bounds: validateScene([scene]), dispose: () => disposeScene([scene]) }, disposed };
}
it('copy retirement fences an import before React unmount and disposes its late graph', async () => {
  let owned = true; const changed = vi.fn(), source = graph();
  render(<GeometryPreview bytes={new Uint8Array([1])} name="cube.glb" copyId="first" signal={new AbortController().signal} current={() => owned} viewState={{}} onViewStateChange={changed}/>);
  owned = false; await act(async () => imports.pending[0].resolve(source.value));
  expect(imports.mounts).toHaveLength(0); expect(changed).not.toHaveBeenCalled(); expect(source.disposed).toHaveBeenCalledOnce();
});
it.each(['success', 'failure'])('late %s from a superseded copy cannot clear the replacement controls', async outcome => {
  const signal = new AbortController().signal, changed = vi.fn(), props = { name: 'cube.glb', signal, current: () => true, viewState: {}, onViewStateChange: changed };
  const ui = render(<GeometryPreview {...props} bytes={new Uint8Array([1])} copyId="first"/>);
  ui.rerender(<GeometryPreview {...props} bytes={new Uint8Array([2])} copyId="second"/>);
  const current = graph(); await act(async () => imports.pending[1].resolve(current.value));
  const late = graph(); await act(async () => { if (outcome === 'success') imports.pending[0].resolve(late.value); else imports.pending[0].reject(new Error('late')); });
  fireEvent.click(ui.getByRole('button', { name: 'Fit view' })); expect(imports.mounts[0].fit).toHaveBeenCalledOnce();
  act(() => imports.mounts[0].publish({ selected: '0', hidden: [] })); expect(changed).toHaveBeenLastCalledWith({ geometry: { copyId: 'second', selected: '0', hidden: [] } });
  expect(ui.queryByRole('alert')).toBeNull(); if (outcome === 'success') expect(late.disposed).toHaveBeenCalledOnce();
  ui.unmount(); expect(current.disposed).toHaveBeenCalledOnce(); expect(imports.mounts[0].dispose).toHaveBeenCalledOnce();
});

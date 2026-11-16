import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ArtifactPreview } from '../src/app/components/ArtifactPreview';
import { DocumentBody } from '../src/app/components/workbench-documents/DocumentBody';
import { fileTabView } from '../src/app/components/WorkbenchFiles';
import { PreviewContentOwner } from '../src/client/preview-content';
import type { FilePreviewLease } from '../src/client/session-files';
const previews = vi.hoisted(() => [] as import('../src/app/components/documents/geometry/GeometryPreview').GeometryPreviewProps[]);
vi.mock('../src/app/components/documents/GeometryDocument', () => ({ preloadGeometry: vi.fn(), GeometryDocument: (props: import('../src/app/components/documents/geometry/GeometryPreview').GeometryPreviewProps) => { previews.push(props); return <div/>; } }));
const owners: PreviewContentOwner[] = [];
afterEach(() => { cleanup(); owners.splice(0).forEach(owner => owner.dispose()); previews.length = 0; });
const camera = { projection: 'perspective' as const, position: [9, 8, 7] as [number, number, number], target: [0, 0, 0] as [number, number, number], up: [0, 1, 0] as [number, number, number] };
async function loaded() {
  const data = new Uint8Array([1, 2, 3]), signal = new AbortController(); let authorized = true, retained = true;
  const unused = async (): Promise<never> => { throw new Error('unexpected read'); };
  const owner = new PreviewContentOwner({ current: () => authorized, immutable: true, bytes: async () => ({ data, version: 'v1' }), page: unused, document: unused, probe: async () => 'v1' }); owners.push(owner);
  await act(async () => { owner.activate(); owner.ensure('geometry'); });
  return { owner, data, signal, resources: { content: owner, signal: signal.signal, current: () => !signal.signal.aborted && authorized, sourceCurrent: () => retained && authorized } as FilePreviewLease,
    revoke: () => { authorized = false; }, forget: () => { retained = false; } };
}
it.each(['hide', 'copy', 'authority', 'occurrence'] as const)('artifact final metadata honors the %s fence after body abort', async reason => {
  const f = await loaded(), changed = vi.fn();
  render(<ArtifactPreview artifact={{ name: 'body.glb', image: false, source: { kind: 'artifact', id: 'body' } }} resources={f.resources} viewState={{}} onViewStateChange={changed} onDownload={() => {}}/>);
  const original = previews.at(-1)!; expect(original).toBeDefined();
  act(() => { f.signal.abort(); if (reason === 'copy') f.owner.refresh(); if (reason === 'authority') f.revoke(); if (reason === 'occurrence') f.forget(); });
  const geometry = { copyId: original.copyId, camera };
  act(() => original.onGeometryRetire!(geometry));
  if (reason === 'hide') expect(changed).toHaveBeenCalledExactlyOnceWith({ geometry }); else expect(changed).not.toHaveBeenCalled();
});
it.each(['hide', 'copy', 'authority', 'owner'] as const)('workbench final metadata honors the %s fence after body abort', async reason => {
  const f = await loaded(); let authorized = true;
  const view = fileTabView(undefined, () => authorized), call = vi.fn(); view.preview = { owner: f.owner, path: 'body.glb', call, dispose() {} };
  render(<DocumentBody mode="geometry" text="" bytes={f.data} path="body.glb" view={view} signal={f.signal.signal} retry={() => {}} call={call} onOpen={() => {}}/>);
  const original = previews.at(-1)!, geometry = { copyId: original.copyId, camera };
  act(() => { f.signal.abort(); if (reason === 'copy') f.owner.refresh(); if (reason === 'authority') authorized = false; if (reason === 'owner') view.preview = undefined; });
  act(() => original.onGeometryRetire!(geometry));
  if (reason === 'hide') expect(view.document?.geometry).toEqual(geometry); else expect(view.document?.geometry).toBeUndefined();
});

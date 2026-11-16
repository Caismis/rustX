import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { GeometryPreviewProps } from '../src/app/components/documents/geometry/GeometryPreview';
import { PreviewContentOwner } from '../src/client/preview-content';
import type { FilePreviewLease } from '../src/client/session-files';

type Viewer = (props: GeometryPreviewProps) => React.ReactNode;
const runtime = vi.hoisted(() => ({ load: vi.fn<() => Viewer | Promise<Viewer>>() }));
// A deferred default models import completion independently for overlapping
// attempts, without resetting the mounted ArtifactPreview or its content owner.
vi.mock('../src/app/components/documents/geometry/GeometryPreview', () => ({ get default() { return runtime.load(); } }));
afterEach(() => { cleanup(); vi.resetModules(); runtime.load.mockReset(); });

it('recovers failed runtime code through Refresh in the same artifact preview body', async () => {
  runtime.load.mockImplementation(() => Promise.reject(new Error('Controlled module failure')));
  const { ArtifactPreview } = await import('../src/app/components/ArtifactPreview');
  const controller = new AbortController(), received = vi.fn();
  let version = 'v1';
  let finishRead!: (copy: { data: Uint8Array<ArrayBuffer>; version: string }) => void;
  const bytes = vi.fn(() => version === 'v1' ? Promise.resolve({ data: new Uint8Array([1]), version }) : new Promise<{ data: Uint8Array<ArrayBuffer>; version: string }>(resolve => { finishRead = resolve; }));
  const unused = async (): Promise<never> => { throw new Error('unexpected read'); };
  const owner = new PreviewContentOwner({ current: () => !controller.signal.aborted, immutable: true, bytes, page: unused, document: unused, probe: async () => version });
  await act(async () => { owner.activate(); owner.ensure('geometry'); });
  const resources = { content: owner, signal: controller.signal, current: () => !controller.signal.aborted, sourceCurrent: () => true } as FilePreviewLease;
  const preview = () => <ArtifactPreview artifact={{ name: 'part.stl', image: false, source: { kind: 'artifact', id: 'part' } }} resources={resources} viewState={{}} onViewStateChange={() => {}} onDownload={() => {}}/>;
  try {
    const ui = render(preview());
    await waitFor(() => expect(ui.getByRole('alert').textContent).toContain('Could not preview this model'));
    const body = ui.getByRole('alert').parentElement, refresh = ui.getByRole('button', { name: 'Refresh' });
    const attempts = runtime.load.mock.calls.length;
    ui.rerender(preview());
    await act(async () => {});
    expect(runtime.load).toHaveBeenCalledTimes(attempts);
    let finish!: (viewer: Viewer) => void;
    const ready = new Promise<Viewer>(resolve => { finish = resolve; });
    const Viewer = (value: GeometryPreviewProps) => { received(value); return <div data-testid="model"/>; };
    runtime.load.mockReturnValue(ready);
    version = 'v2';
    await act(async () => fireEvent.click(refresh));
    expect(ui.queryByRole('alert')).toBeNull();
    expect(body?.querySelector('[role="status"]')).toBeTruthy();
    await act(async () => finish(Viewer));
    await waitFor(() => expect(runtime.load.mock.calls.length).toBeGreaterThan(attempts));
    await act(async () => finishRead({ data: new Uint8Array([2]), version: 'v2' }));
    expect((await ui.findByTestId('model')).parentElement).toBe(body);
    expect(ui.getByRole('button', { name: 'Refresh' })).toBe(refresh);
    expect(received).toHaveBeenLastCalledWith(expect.objectContaining({ copyId: `${owner.getSnapshot().revision}:v2`, bytes: new Uint8Array([2]), signal: controller.signal }));
    expect(bytes).toHaveBeenCalledTimes(2);
    const loadedAttempts = runtime.load.mock.calls.length;
    version = 'v3';
    await act(async () => fireEvent.click(refresh));
    await act(async () => finishRead({ data: new Uint8Array([3]), version: 'v3' }));
    expect(runtime.load).toHaveBeenCalledTimes(loadedAttempts);
    expect(received).toHaveBeenLastCalledWith(expect.objectContaining({ copyId: `${owner.getSnapshot().revision}:v3` }));
  } finally { owner.dispose(); }
});

it.each(['resolve', 'reject'] as const)('fences an old copy runtime %s after a newer copy starts', async completion => {
  let oldResolve!: (viewer: Viewer) => void, oldReject!: (error: Error) => void, finish!: (viewer: Viewer) => void;
  const old = new Promise<Viewer>((resolve, reject) => { oldResolve = resolve; oldReject = reject; });
  const next = new Promise<Viewer>(resolve => { finish = resolve; });
  runtime.load.mockReturnValueOnce(old).mockReturnValue(next);
  const { GeometryDocument } = await import('../src/app/components/documents/GeometryDocument');
  const OldViewer = vi.fn(() => <div data-testid="old-model"/>), NewViewer = vi.fn(() => <div data-testid="new-model"/>);
  const controller = new AbortController();
  const value = { bytes: new Uint8Array([1]), name: 'part.stl', copyId: 'old', signal: controller.signal, current: () => true, viewState: {}, onViewStateChange: vi.fn(), onGeometryRetire: vi.fn() };
  const ui = render(<GeometryDocument {...value}/>);
  await act(async () => {});
  ui.rerender(<GeometryDocument {...value} copyId="new" bytes={new Uint8Array([2])}/>);
  await act(async () => {});
  await act(async () => { if (completion === 'resolve') oldResolve(OldViewer); else oldReject(new Error('Old failure')); });
  expect(ui.queryByRole('alert')).toBeNull();
  expect(OldViewer).not.toHaveBeenCalled();
  await act(async () => finish(NewViewer));
  expect(ui.getByTestId('new-model')).toBeTruthy();
});

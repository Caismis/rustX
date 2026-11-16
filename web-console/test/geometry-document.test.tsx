import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { GeometryPreviewProps } from '../src/app/components/documents/geometry/GeometryPreview';

afterEach(() => { cleanup(); vi.doUnmock('../src/app/components/documents/geometry/GeometryPreview'); vi.resetModules(); });
const props = (signal: AbortSignal): GeometryPreviewProps => ({ bytes: new Uint8Array([1]), name: 'part.stl', copyId: 'copy', signal,
  current: () => !signal.aborted, viewState: {}, onViewStateChange: vi.fn(), onGeometryRetire: vi.fn() });

it('mounts resolved runtime code without a Suspense fallback timer', async () => {
  let resolve!: (module: { default: typeof Viewer }) => void;
  const received = vi.fn(), Viewer = (value: GeometryPreviewProps) => { received(value); return <div data-testid="model"/>; };
  vi.doMock('../src/app/components/documents/geometry/GeometryPreview', () => new Promise(done => { resolve = done; }));
  const { GeometryDocument } = await import('../src/app/components/documents/GeometryDocument');
  const value = props(new AbortController().signal), ui = render(<GeometryDocument {...value}/>);
  expect(ui.getByRole('status').textContent).toContain('Loading model');
  await act(async () => {});
  await act(async () => resolve({ default: Viewer }));
  expect(ui.getByTestId('model')).toBeTruthy(); expect(received).toHaveBeenCalledWith(expect.objectContaining(value));
  expect(ui.queryByRole('status')).toBeNull();
});
it('starts the STEP viewer as soon as code and full source are ready', async () => {
  const received = vi.fn(), Viewer = (value: GeometryPreviewProps) => { received(value); return <div data-testid="model"/>; };
  vi.doMock('../src/app/components/documents/geometry/GeometryPreview', () => ({ default: Viewer }));
  const { GeometryDocument } = await import('../src/app/components/documents/GeometryDocument');
  const value = { ...props(new AbortController().signal), name: 'part.step' }, ui = render(<GeometryDocument {...value} bytes={undefined}/>);
  await act(async () => {});
  expect(ui.queryByTestId('model')).toBeNull();
  ui.rerender(<GeometryDocument {...value}/>);
  expect(ui.getByTestId('model')).toBeTruthy();
  expect(received).toHaveBeenCalledWith(expect.objectContaining(value));
});
it('cannot initialize a viewer after its body retires while code is arriving', async () => {
  let resolve!: (module: { default: () => null }) => void;
  const Viewer = vi.fn(() => null);
  vi.doMock('../src/app/components/documents/geometry/GeometryPreview', () => new Promise(done => { resolve = done; }));
  const { GeometryDocument } = await import('../src/app/components/documents/GeometryDocument');
  const controller = new AbortController(), ui = render(<GeometryDocument {...props(controller.signal)}/>);
  await act(async () => {}); controller.abort();
  await act(async () => resolve({ default: Viewer }));
  expect(Viewer).not.toHaveBeenCalled(); ui.unmount();
});
it('reports a failed module load inside the existing preview body', async () => {
  vi.doMock('../src/app/components/documents/geometry/GeometryPreview', () => Promise.reject(new Error('Controlled module failure')));
  const { GeometryDocument } = await import('../src/app/components/documents/GeometryDocument');
  const ui = render(<GeometryDocument {...props(new AbortController().signal)}/>);
  await act(async () => {});
  expect(ui.getByRole('alert').textContent).toContain('Could not preview this model');
});

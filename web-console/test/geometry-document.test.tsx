import { act, cleanup, fireEvent, render } from '@testing-library/react';
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
it('waits for quiet through rough sampling and gestures, then retains the rough picture until fine GPU readiness',async()=>{
  vi.useFakeTimers();
  try{
  const received=vi.fn(),Viewer=(value:GeometryPreviewProps)=>{received(value);return <div data-testid="full-model"/>;};
  vi.doMock('../src/app/components/documents/geometry/GeometryPreview',()=>({default:Viewer}));
  const {GeometryDocument}=await import('../src/app/components/documents/GeometryDocument');
  const value={...props(new AbortController().signal),name:'part.step',rough:{version:'one',positions:new Float32Array([0,0,0,1,0,0,0,1,0,0,0,1])}};
  const ui=render(<GeometryDocument {...value}/>);await act(async()=>{});expect(ui.queryByTestId('full-model')).toBeNull();
  const svg=ui.getByRole('img');Object.assign(svg,{setPointerCapture:vi.fn()});
  fireEvent.pointerDown(svg);
  ui.rerender(<GeometryDocument {...value} rough={{...value.rough,positions:new Float32Array([...value.rough.positions,2,0,0])}}/>);
  await act(async()=>vi.advanceTimersByTime(1000));expect(ui.queryByTestId('full-model')).toBeNull();
  fireEvent.pointerUp(svg);await act(async()=>vi.advanceTimersByTime(119));expect(ui.queryByTestId('full-model')).toBeNull();
  fireEvent.keyDown(svg,{code:'ArrowLeft'});await act(async()=>vi.advanceTimersByTime(119));expect(ui.queryByTestId('full-model')).toBeNull();
  await act(async()=>vi.advanceTimersByTime(1));expect(ui.getByTestId('full-model')).toBeTruthy();expect(ui.container.querySelector('svg[data-geometry-rough]')).not.toBeNull();
  act(()=>received.mock.lastCall![0].onDisplay());expect(ui.container.querySelector('svg[data-geometry-rough]')).toBeNull();
  }finally{vi.useRealTimers();}
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

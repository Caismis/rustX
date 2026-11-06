import { useState, type ReactNode } from 'react';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { PreviewViewState, PreviewViewStateProps } from '../src/presentation/right-panel/preview-view-state';
import { ArtifactPreview } from '../src/presentation/right-panel/ArtifactPreview';
import { HtmlPreview } from '../src/app/components/documents/HtmlPreview';
import { SpreadsheetPreview } from '../src/app/components/documents/SpreadsheetPreview';

type RenderRecord = { number: number; zoom: number; canvas: HTMLCanvasElement; layer: HTMLDivElement; signal: AbortSignal; resolve: () => void };
type OwnerRecord = { resolveDocument: (document: { numPages: number }) => void; renders: RenderRecord[]; dispose: ReturnType<typeof vi.fn> };
const boundary = vi.hoisted(() => ({ owners: [] as OwnerRecord[] }));
vi.mock('../src/client/pdf-document', () => ({ PdfDocumentOwner: class {
  resolveDocument!: (document: { numPages: number }) => void;
  document = new Promise<{ numPages: number }>(resolve => { this.resolveDocument = resolve; });
  pageSizes = this.document.then(document => Array.from({ length: document.numPages }, () => ({ width: 800, height: 1200 })));
  renders: RenderRecord[] = [];
  release(canvas: HTMLCanvasElement) { const record = this.renders.find(record => record.canvas === canvas); if (record) { canvas.width = canvas.height = 0; record.layer.replaceChildren(); } }
  dispose = vi.fn(() => { for (const record of this.renders) this.release(record.canvas); });
  constructor() { boundary.owners.push(this); }
  render(number: number, zoom: number, canvas: HTMLCanvasElement, layer: HTMLDivElement, signal: AbortSignal) {
    canvas.width = 800; canvas.height = 1200; layer.textContent = `page ${number}`;
    return new Promise<void>(resolve => { this.renders.push({ number, zoom, canvas, layer, signal, resolve }); });
  }
} }));
import PdfPreview from '../src/app/components/documents/PdfPreview';
import { useViewScroll } from '../src/presentation/right-panel/use-view-scroll';

afterEach(() => { cleanup(); boundary.owners.splice(0); vi.useRealTimers(); vi.unstubAllGlobals(); });
function Retained({ visible = true, initial = {}, children }: { visible?: boolean; initial?: PreviewViewState; children: (props: PreviewViewStateProps) => ReactNode }) {
  const [viewState, update] = useState(initial);
  return visible ? children({ viewState, onViewStateChange: patch => update(state => ({ ...state, ...patch })) }) : null;
}
function scroll(node: HTMLElement, top: number, left: number) {
  node.scrollTop = top; node.scrollLeft = left; fireEvent.scroll(node);
}
it('refresh layout clamping does not erase the target; user input replaces that target', () => {
  const save = vi.fn(); let restore!: () => void;
  function Body() { const reading = useViewScroll(317, 0, save, 'ready'); restore = reading.restore; return <div ref={reading.ref} onScroll={reading.onScroll}/>; }
  const ui = render(<Body/>), node = ui.container.firstElementChild as HTMLElement;
  let position = 0;
  Object.defineProperty(node, 'scrollTop', { configurable: true, get: () => position, set: value => { position = Math.min(value, 100); } });
  act(() => restore()); fireEvent.scroll(node); expect(position).toBe(100); expect(save).not.toHaveBeenCalled();
  fireEvent.wheel(node); expect(save).toHaveBeenLastCalledWith(100, 0);
  position = 50; fireEvent.scroll(node); expect(save).toHaveBeenLastCalledWith(50, 0);
});

it.each(['text', 'markdown', 'image'] as const)('%s retains bounded scroll metadata across safe body unmount', kind => {
  const content = (props: PreviewViewStateProps) => <ArtifactPreview name="same-name" text={kind === 'image' ? undefined : 'long text'} url="blob:original" image={kind === 'image'} markdown={kind === 'markdown'} loading={false} refresh={() => {}} retry={() => {}} onDownload={() => {}} {...props}/>;
  const view = render(<Retained>{content}</Retained>);
  scroll(view.container.querySelector('[data-preview-scroll="body"]')!, 310, 25);
  if (kind === 'text') fireEvent.click(view.getByRole('button', { name: 'Wrap lines' }));
  view.rerender(<Retained visible={false}>{content}</Retained>);
  expect(view.container.childElementCount).toBe(0);
  view.rerender(<Retained>{content}</Retained>);
  const restored = view.container.querySelector('[data-preview-scroll="body"]')!;
  expect(restored.scrollTop).toBe(310); expect(restored.scrollLeft).toBe(25);
  if (kind === 'text') expect(view.getByRole('button', { name: 'Wrap lines' }).getAttribute('aria-pressed')).toBe('false');
});

it('HTML preserves source mode and source scroll without relaxing opaque sandbox isolation', () => {
  const bytes = new TextEncoder().encode('<h1>Visible text</h1><script>window.bad=true</script>');
  const content = (props: PreviewViewStateProps) => <HtmlPreview bytes={bytes} {...props}/>;
  const view = render(<Retained>{content}</Retained>);
  expect(view.getByTitle('Isolated HTML preview').getAttribute('sandbox')).toBe('');
  fireEvent.click(view.getByRole('button', { name: 'Source' }));
  scroll(view.container.querySelector('[data-preview-scroll="html"]')!, 240, 12);
  view.rerender(<Retained visible={false}>{content}</Retained>);
  view.rerender(<Retained>{content}</Retained>);
  expect(view.getByRole('button', { name: 'Source' }).getAttribute('aria-pressed')).toBe('true');
  expect(view.container.querySelector('iframe')).toBeNull();
  expect(view.container.querySelector('[data-preview-scroll="html"]')!.scrollTop).toBe(240);
  expect(view.container.querySelector('script')).toBeNull();
});

const spreadsheet = vi.hoisted(() => ({ sheets: [{ id: 'a', name: 'First', status: 1, celldata: [] }, { id: 'b', name: 'Second', status: 0, celldata: [] }] }));
vi.mock('../src/app/components/workbench-documents/excel/parse', () => ({ parseExcel: async () => ({ sheets: structuredClone(spreadsheet.sheets), missingResults: 0, unsupportedFeatures: [] }) }));
vi.mock('@fortune-sheet/react', () => ({ Workbook: ({ data, hooks }: { data: typeof spreadsheet.sheets; hooks: { afterActivateSheet: (id: string) => void } }) => <div>
  <output>{data.find(sheet => sheet.status === 1)?.name}</output>
  {data.map(sheet => <button key={sheet.id} onClick={() => hooks.afterActivateSheet(sheet.id)}>Select {sheet.name}</button>)}
</div> }));
it('Spreadsheet restores the selected sheet through the shared browser viewer and clamps a changed source', async () => {
  const bytes = new Uint8Array([1]), signal = new AbortController().signal;
  const content = (props: PreviewViewStateProps) => <SpreadsheetPreview bytes={bytes} signal={signal} {...props}/>;
  const view = render(<Retained>{content}</Retained>);
  await view.findByText('First');
  fireEvent.click(view.getByRole('button', { name: 'Select Second' }));
  view.rerender(<Retained visible={false}>{content}</Retained>);
  view.rerender(<Retained>{content}</Retained>);
  await view.findByText('Second');
  view.rerender(<Retained visible={false}>{content}</Retained>);
  spreadsheet.sheets = [{ id: 'changed', name: 'Changed', status: 1, celldata: [] }];
  view.rerender(<Retained>{content}</Retained>);
  await view.findByText('Changed');
});

it('PDF remount reclaims heavy resources and restores continuous reading zoom and position with a fresh owner', async () => {
  vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} });
  const bytes = new Uint8Array([1]), lease = new AbortController();
  const content = (props: PreviewViewStateProps) => <PdfPreview bytes={bytes} signal={lease.signal} retry={() => {}} {...props}/>;
  const initial: PreviewViewState = { pdfZoom: { kind: 'fixed', scale: 1.5 }, pdfScrollTop: 62, pdfScrollLeft: 13 };
  const view = render(<Retained initial={initial}>{content}</Retained>);
  const first = boundary.owners[0];
  await act(async () => first.resolveDocument({ numPages: 10 }));
  expect(first.renders).toHaveLength(1); expect(first.renders[0]).toMatchObject({ number: 1, zoom: 1.5 });
  expect(view.container.querySelectorAll('[data-pdf-page]')).toHaveLength(10);
  await act(async () => first.renders[0].resolve());
  const port = view.container.querySelector('[data-preview-scroll="pdf"]')!;
  expect(port.scrollTop).toBe(62); expect(port.scrollLeft).toBe(13);
  scroll(port as HTMLElement, 320, 45);
  expect(boundary.owners).toHaveLength(1); expect(first.renders).toHaveLength(1);
  view.rerender(<Retained visible={false} initial={initial}>{content}</Retained>);
  expect(first.dispose).toHaveBeenCalledOnce(); expect(first.renders[0].signal.aborted).toBe(true);
  expect(first.renders[0].canvas.width).toBe(0); expect(first.renders[0].layer.childElementCount).toBe(0);
  view.rerender(<Retained initial={initial}>{content}</Retained>);
  const second = boundary.owners[1];
  await act(async () => second.resolveDocument({ numPages: 10 }));
  expect(second.renders[0]).toMatchObject({ number: 1, zoom: 1.5 });
  await act(async () => second.renders[0].resolve());
  expect(view.container.querySelector('[data-preview-scroll="pdf"]')!.scrollTop).toBe(320);
  expect(view.getByRole('button', { name: 'Zoom', hidden: true }).textContent).toContain('150%');
  vi.unstubAllGlobals();
});

it('retiring a PDF lease immediately cancels a gated render and clears both canvases and text before unmount', async () => {
  vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} });
  vi.useFakeTimers();
  const lease = new AbortController();
  const view = render(<PdfPreview bytes={new Uint8Array([1])} signal={lease.signal} viewState={{ pdfZoom: { kind: 'fixed', scale: 1 } }} onViewStateChange={() => {}} retry={() => {}}/>);
  const owner = boundary.owners[0];
  await act(async () => owner.resolveDocument({ numPages: 2 }));
  expect(owner.renders[0].canvas.width).toBe(800); expect(vi.getTimerCount()).toBeGreaterThanOrEqual(1);
  act(() => lease.abort());
  expect(owner.dispose).toHaveBeenCalledOnce(); expect(owner.renders[0].signal.aborted).toBe(true);
  expect(owner.renders[0].canvas.width).toBe(0); expect(owner.renders[0].canvas.height).toBe(0);
  expect(owner.renders[0].layer.childNodes).toHaveLength(0); expect(vi.getTimerCount()).toBe(0);
  await act(async () => owner.renders[0].resolve());
  expect(view.queryByRole('alert')).toBeNull(); expect(owner.renders[0].canvas.width).toBe(0);
  vi.unstubAllGlobals();
});

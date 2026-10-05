import { useState, type ReactNode } from 'react';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { PreviewViewState, PreviewViewStateProps } from '../src/presentation/right-panel/preview-view-state';
import { ArtifactPreview } from '../src/presentation/right-panel/ArtifactPreview';
import { HtmlPreview } from '../src/app/components/documents/HtmlPreview';
import { WorkbookPreview } from '../src/app/components/documents/WorkbookPreview';

type RenderRecord = { number: number; zoom: number; canvas: HTMLCanvasElement; layer: HTMLDivElement; signal: AbortSignal; resolve: () => void };
type OwnerRecord = { resolveDocument: (document: { numPages: number }) => void; renders: RenderRecord[]; dispose: ReturnType<typeof vi.fn> };
const boundary = vi.hoisted(() => ({ owners: [] as OwnerRecord[] }));
vi.mock('../src/client/pdf-document', () => ({ PdfDocumentOwner: class {
  resolveDocument!: (document: { numPages: number }) => void;
  document = new Promise<{ numPages: number }>(resolve => { this.resolveDocument = resolve; });
  renders: RenderRecord[] = [];
  dispose = vi.fn();
  constructor() { boundary.owners.push(this); }
  render(number: number, zoom: number, canvas: HTMLCanvasElement, layer: HTMLDivElement, signal: AbortSignal) {
    canvas.width = 800; canvas.height = 1200; layer.textContent = `page ${number}`;
    return new Promise<void>(resolve => { this.renders.push({ number, zoom, canvas, layer, signal, resolve }); });
  }
} }));
import PdfPreview from '../src/app/components/documents/PdfPreview';

afterEach(() => { cleanup(); boundary.owners.splice(0); vi.useRealTimers(); });
function Retained({ visible = true, initial = {}, children }: { visible?: boolean; initial?: PreviewViewState; children: (props: PreviewViewStateProps) => ReactNode }) {
  const [viewState, update] = useState(initial);
  return visible ? children({ viewState, onViewStateChange: patch => update(state => ({ ...state, ...patch })) }) : null;
}
function scroll(node: HTMLElement, top: number, left: number) {
  node.scrollTop = top; node.scrollLeft = left; fireEvent.scroll(node);
}

it.each(['text', 'markdown', 'image'] as const)('%s retains bounded scroll metadata across safe body unmount', kind => {
  const content = (props: PreviewViewStateProps) => <ArtifactPreview name="same-name" text={kind === 'image' ? undefined : 'long text'} url="blob:original" image={kind === 'image'} markdown={kind === 'markdown'} loading={false} retry={() => {}} onDownload={() => {}} {...props}/>;
  const view = render(<Retained>{content}</Retained>);
  scroll(view.container.querySelector('[data-preview-scroll="body"]')!, 310, 25);
  if (kind !== 'image') fireEvent.click(view.getByRole('button', { name: 'Wrap lines' }));
  view.rerender(<Retained visible={false}>{content}</Retained>);
  expect(view.container.childElementCount).toBe(0);
  view.rerender(<Retained>{content}</Retained>);
  const restored = view.container.querySelector('[data-preview-scroll="body"]')!;
  expect(restored.scrollTop).toBe(310); expect(restored.scrollLeft).toBe(25);
  if (kind !== 'image') expect(view.getByRole('button', { name: 'Wrap lines' }).getAttribute('aria-pressed')).toBe('false');
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

it('Workbook restores selected sheet, finite row window and scroll; changed source shape clamps safely', () => {
  const workbook = { kind: 'xlsx' as const, sheets: [
    { name: 'First', cells: [], truncated: false },
    { name: 'Second', cells: Array.from({ length: 205 }, (_, i) => ({ address: `A${i + 1}`, type: 'str', value: `cell ${i + 1}` })), truncated: false },
  ] };
  const content = (props: PreviewViewStateProps) => <WorkbookPreview workbook={workbook} {...props}/>;
  const view = render(<Retained>{content}</Retained>);
  fireEvent.change(view.getByRole('combobox', { name: 'Sheet' }), { target: { value: '1' } });
  fireEvent.click(view.getByRole('button', { name: 'Next' }));
  expect(view.getByText('cell 101')).toBeTruthy(); expect(view.queryByText('cell 100')).toBeNull();
  expect(view.getAllByRole('row')).toHaveLength(101);
  scroll(view.container.querySelector('[data-preview-scroll="workbook"]')!, 87, 5);
  view.rerender(<Retained visible={false}>{content}</Retained>);
  view.rerender(<Retained>{content}</Retained>);
  expect((view.getByRole('combobox', { name: 'Sheet' }) as HTMLSelectElement).value).toBe('1');
  expect(view.getByText('cell 101')).toBeTruthy();
  expect(view.container.querySelector('[data-preview-scroll="workbook"]')!.scrollTop).toBe(87);
  view.rerender(<Retained>{props => <WorkbookPreview workbook={{ kind: 'xlsx', sheets: [{ name: 'Changed', cells: [], truncated: false }] }} {...props}/>}</Retained>);
  expect((view.getByRole('combobox', { name: 'Sheet' }) as HTMLSelectElement).value).toBe('0');
  expect(view.getAllByRole('row')).toHaveLength(1);
});

it('PDF remount reclaims heavy resources and restores page, zoom and position with a fresh owner', async () => {
  const bytes = new Uint8Array([1]), lease = new AbortController();
  const content = (props: PreviewViewStateProps) => <PdfPreview bytes={bytes} signal={lease.signal} retry={() => {}} {...props}/>;
  const initial = { pdfPage: 7, pdfZoom: 1.5, pdfScrollTop: 62, pdfScrollLeft: 13 };
  const view = render(<Retained initial={initial}>{content}</Retained>);
  const first = boundary.owners[0];
  await act(async () => first.resolveDocument({ numPages: 10 }));
  expect(first.renders).toHaveLength(1); expect(first.renders[0]).toMatchObject({ number: 7, zoom: 1.5 });
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
  expect(second.renders[0]).toMatchObject({ number: 7, zoom: 1.5 });
  await act(async () => second.renders[0].resolve());
  expect(view.container.querySelector('[data-preview-scroll="pdf"]')!.scrollTop).toBe(320);
  expect((view.getByRole('combobox', { name: 'Zoom' }) as HTMLSelectElement).value).toBe('1.5');
});

it('retiring a PDF lease immediately cancels a gated render and clears both canvases and text before unmount', async () => {
  vi.useFakeTimers();
  const lease = new AbortController();
  const view = render(<PdfPreview bytes={new Uint8Array([1])} signal={lease.signal} viewState={{}} onViewStateChange={() => {}} retry={() => {}}/>);
  const owner = boundary.owners[0];
  await act(async () => owner.resolveDocument({ numPages: 2 }));
  expect(owner.renders[0].canvas.width).toBe(800); expect(vi.getTimerCount()).toBe(1);
  act(() => lease.abort());
  expect(owner.dispose).toHaveBeenCalledOnce(); expect(owner.renders[0].signal.aborted).toBe(true);
  expect(owner.renders[0].canvas.width).toBe(0); expect(owner.renders[0].canvas.height).toBe(0);
  expect(owner.renders[0].layer.childNodes).toHaveLength(0); expect(vi.getTimerCount()).toBe(0);
  await act(async () => owner.renders[0].resolve());
  expect(view.queryByRole('alert')).toBeNull(); expect(owner.renders[0].canvas.width).toBe(0);
});

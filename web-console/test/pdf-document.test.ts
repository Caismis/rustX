import { createElement, useState } from 'react';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import PdfPreview from '../src/app/components/documents/PdfPreview';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
const api = vi.hoisted(() => ({ load: vi.fn(), bridge: vi.fn(), text: vi.fn(), cleanup: vi.fn() }));
vi.mock('pdfjs-dist', () => ({ getDocument: api.load, PDFWorker: { create: api.bridge }, TextLayer: class {
  static cleanup = api.cleanup; cancel = vi.fn(); render = api.text;
} }));
import { PdfDocumentOwner } from '../src/client/pdf-document';
import type { PreviewViewState } from '../src/presentation/right-panel/preview-view-state';
function deferred<T>() { let resolve!: (v: T) => void, reject!: (cause: unknown) => void; const promise = new Promise<T>((r, fail) => { resolve = r; reject = fail; }); return { promise, resolve, reject }; }
const workers: FakeWorker[] = [], owners: PdfDocumentOwner[] = [];
class FakeWorker { onerror?: (event: Event) => void; terminate = vi.fn(); constructor() { workers.push(this); } }
let page: any, document: any, destroy: ReturnType<typeof vi.fn>;
beforeEach(() => {
  api.load.mockReset(); api.bridge.mockReset(); api.text.mockReset(); api.cleanup.mockReset();
  vi.stubGlobal('Worker', FakeWorker);
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({ drawImage: vi.fn() } as unknown as CanvasRenderingContext2D);
  destroy = vi.fn(async () => {}); api.bridge.mockReturnValue({ destroy: vi.fn() }); api.text.mockResolvedValue(undefined);
  page = { getViewport: vi.fn(() => ({ width: 600, height: 800 })), cleanup: vi.fn(),
    render: vi.fn(() => ({ promise: Promise.resolve(), cancel: vi.fn() })),
    streamTextContent: () => new ReadableStream({ start(controller) { controller.close(); } }),
  };
  document = { filterFactory: { destroy: vi.fn() }, numPages: 3, getPage: vi.fn(async () => page) };
  api.load.mockReturnValue({ promise: Promise.resolve(document), destroy });
});
afterEach(() => { cleanup(); vi.useRealTimers(); for (const owner of owners.splice(0)) owner.dispose(); workers.splice(0); vi.unstubAllGlobals(); });
function owner() { const value = new PdfDocumentOwner(new Uint8Array([1]), vi.fn()); owners.push(value); return value; }
function view() { return { canvas: window.document.createElement('canvas'), layer: window.document.createElement('div'), signal: new AbortController() }; }
it('admits two visible PDF workers, rejects a third and releases each once on disposal or worker failure', () => {
  const first = owner(), second = owner(); expect(() => owner()).toThrow('capacity'); first.dispose(); first.dispose();
  expect(workers[0].terminate).toHaveBeenCalledOnce(); expect(destroy).toHaveBeenCalledOnce();
  const replacement = owner(); workers[1].onerror!(new Event('error')); second.dispose(); expect(workers[1].terminate).toHaveBeenCalledOnce();
  replacement.dispose(); expect(workers[2].terminate).toHaveBeenCalledOnce();
});
it('rejects a huge page before canvas allocation or rendering', async () => {
  page.getViewport.mockReturnValue({ width: 1e9, height: 1e9 });
  const pdf = owner(), v = view();
  await expect(pdf.render(1, 1, v.canvas, v.layer, v.signal.signal)).rejects.toThrow('parser_limit');
  expect(page.render).not.toHaveBeenCalled(); expect(v.canvas.width).toBe(300); expect(page.cleanup).toHaveBeenCalledOnce();
});
it('a gated obsolete page cannot write dimensions, start rendering or publish text', async () => {
  const entered = deferred<void>(), held = deferred<any>();
  document.getPage.mockImplementation(() => { entered.resolve(); return held.promise; });
  const pdf = owner(), v = view(), work = pdf.render(1, 1, v.canvas, v.layer, v.signal.signal);
  const rejected = expect(work).rejects.toThrow(); await entered.promise; v.signal.abort(); held.resolve(page); await rejected;
  expect(page.render).not.toHaveBeenCalled(); expect(v.canvas.width).toBe(300); expect(v.layer.childElementCount).toBe(0);
});
it('serializes page render settlement before starting another page', async () => {
  const held = deferred<void>(), entered = deferred<void>(), cancel = vi.fn();
  page.render.mockImplementationOnce(() => { entered.resolve(); return { promise: held.promise, cancel }; });
  const pdf = owner(), a = view(), b = view();
  const first = pdf.render(1, 1, a.canvas, a.layer, a.signal.signal), rejected = expect(first).rejects.toThrow();
  await entered.promise; a.signal.abort(); const next = pdf.render(2, 1, b.canvas, b.layer, b.signal.signal);
  expect(document.getPage).toHaveBeenCalledTimes(1); expect(cancel).toHaveBeenCalled();
  held.resolve(); await rejected; await next; expect(document.getPage).toHaveBeenCalledTimes(2);
});
it('rejects excessive page count and text-layer items', async () => {
  document.numPages = 101; const tooMany = owner(); await expect(tooMany.document).rejects.toThrow('parser_limit'); tooMany.dispose();
  document.numPages = 1;
  page.streamTextContent = () => new ReadableStream({ start(controller) {
    controller.enqueue({ items: Array.from({ length: 10001 }, () => ({ str: 'x' })), styles: {} }); controller.close();
  } });
  const pdf = owner(), v = view(); await expect(pdf.render(1, 1, v.canvas, v.layer, v.signal.signal)).rejects.toThrow('parser_limit');
  expect(api.text).not.toHaveBeenCalled();
});

it('bounds scratch canvas count, individual and aggregate pixels before allocation and clears all backing stores', async () => {
  const { PdfCanvasBudget } = await import('../src/client/pdf-document');
  const context = vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({} as CanvasRenderingContext2D);
  try {
    const budget = new PdfCanvasBudget();
    expect(() => budget.create(4097, 1)).toThrow('parser_limit'); expect(context).not.toHaveBeenCalled();
    const entries = Array.from({ length: 4 }, () => budget.create(2048, 2048));
    expect(() => budget.create(1, 1)).toThrow('parser_limit'); expect(context).toHaveBeenCalledTimes(4);
    budget.reset(entries[0], 1, 1);
    // Ordinary Office knockout groups need more than eight tiny canvases.
    const extra = Array.from({ length: 124 }, () => budget.create(1, 1));
    expect(() => budget.create(1, 1)).toThrow('parser_limit');
    expect(() => budget.reset(extra[0], 2048, 2048)).toThrow('parser_limit');
    const backing = [...entries, ...extra].map(entry => entry.canvas!);
    budget.dispose(); budget.dispose();
    expect(backing.every(canvas => canvas.width === 0 && canvas.height === 0)).toBe(true);
    expect(entries.every(entry => entry.canvas === null && entry.context === null)).toBe(true);
    expect(() => budget.create(1, 1)).toThrow('parser_limit');
  } finally { context.mockRestore(); }
});

it('disables worker fetching, XFA and unbudgeted offscreen rendering', () => {
  owner();
  expect(api.load.mock.calls[0][0]).toMatchObject({ useWorkerFetch: false, useSystemFonts: false, disableFontFace: true,
    useWasm: false, enableXfa: false, enableHWA: false,
    isOffscreenCanvasSupported: false, isImageDecoderSupported: false, stopAtErrors: true });
  expect(typeof api.load.mock.calls[0][0].CanvasFactory).toBe('function');
});
it('terminal retirement clears the selected page backing store and text nodes', async () => {
  const pdf = owner(), v = view();
  await pdf.render(1, 1, v.canvas, v.layer, v.signal.signal);
  v.layer.textContent = 'selected page';
  expect(v.canvas.width).toBe(600);
  pdf.dispose();
  expect([v.canvas.width, v.canvas.height, v.layer.childNodes.length]).toEqual([0, 0, 0]);
  expect(document.filterFactory.destroy).toHaveBeenCalled();
  expect(() => pdf.document).toThrow('obsolete');
});
it('keeps finite page geometry while releasing independent offscreen bitmaps', async () => {
  const pdf = owner(), a = view(), b = view();
  await pdf.document; expect(document.getPage).not.toHaveBeenCalled();
  expect(await pdf.render(1, 1, a.canvas, a.layer, a.signal.signal)).toEqual({ width: 600, height: 800 });
  await pdf.render(2, 1, b.canvas, b.layer, b.signal.signal);
  expect([a.canvas.width, b.canvas.width]).toEqual([600, 600]);
  pdf.release(a.canvas);
  expect([a.canvas.width, b.canvas.width]).toEqual([0, 600]);
  pdf.dispose(); expect(b.canvas.width).toBe(0);
});
it('reads and caches only the reference geometry without starting bitmap or text work', async () => {
  document.numPages = 100;
  const pdf = owner(), geometry = pdf.firstPageSize;
  expect(pdf.firstPageSize).toBe(geometry);
  expect(await geometry).toEqual({ width: 600, height: 800 });
  expect(document.getPage).toHaveBeenCalledExactlyOnceWith(1);
  expect(page.render).not.toHaveBeenCalled(); expect(api.text).not.toHaveBeenCalled();
  expect(HTMLCanvasElement.prototype.getContext).not.toHaveBeenCalled();
  expect(page.cleanup).not.toHaveBeenCalled();
});
it.each([0, -1, NaN, Infinity, 1e9])('rejects invalid reference geometry %s before canvas allocation', async width => {
  page.getViewport.mockReturnValue({ width, height: 800 });
  const pdf = owner();
  await expect(pdf.firstPageSize).rejects.toThrow('parser_limit');
  expect(page.render).not.toHaveBeenCalled(); expect(api.text).not.toHaveBeenCalled();
  expect(HTMLCanvasElement.prototype.getContext).not.toHaveBeenCalled();
});
it('retiring pending reference geometry prevents it from publishing a later page', async () => {
  const held = deferred<any>(); document.getPage.mockReturnValue(held.promise);
  const pdf = owner(), geometry = pdf.firstPageSize, rejected = expect(geometry).rejects.toThrow('obsolete');
  await vi.waitFor(() => expect(document.getPage).toHaveBeenCalledOnce());
  pdf.dispose(); held.resolve(page); await rejected;
  expect(page.getViewport).not.toHaveBeenCalled(); expect(page.render).not.toHaveBeenCalled();
  expect(workers[0].terminate).toHaveBeenCalledOnce(); expect(destroy).toHaveBeenCalledOnce();
});
it('preserves PDF-point admission while bounding the fitted CSS raster separately', async () => {
  page.getViewport.mockImplementation(({ scale }: { scale: number }) => ({ width: 3500 * scale, height: 1000 * scale, scale }));
  const pdf = owner(), v = view();
  expect((await pdf.render(1, 0.25, v.canvas, v.layer, v.signal.signal)).width).toBeGreaterThan(4096);
  expect(v.canvas.width).toBeLessThan(4096);
  expect(v.canvas.width * v.canvas.height).toBeLessThanOrEqual(4 * 1024 * 1024);
});
it('publishes zoom replacements only after their bitmap and text finish', async () => {
  const pdf = owner(), v = view();
  await pdf.render(1, 1, v.canvas, v.layer, v.signal.signal);
  const held = deferred<void>(), started = deferred<void>();
  page.render.mockImplementationOnce(() => { started.resolve(); return { promise: held.promise, cancel: vi.fn() }; });
  const work = pdf.render(1, 0.5, v.canvas, v.layer, v.signal.signal);
  await started.promise;
  expect(v.canvas.width).toBe(600); expect(v.layer.childElementCount).toBe(1);
  held.resolve(); await work;
  expect(v.canvas.width).toBe(300); expect(v.layer.childElementCount).toBe(1);
});

it('retirement cancels a held text stream and cannot start a text layer afterwards', async () => {
  const entered = deferred<void>(), canceled = vi.fn();
  page.streamTextContent = () => new ReadableStream({ pull() { entered.resolve(); }, cancel: canceled });
  const pdf = owner(), v = view(), work = pdf.render(1, 1, v.canvas, v.layer, v.signal.signal);
  const rejected = expect(work).rejects.toThrow('obsolete');
  await entered.promise; pdf.dispose(); await rejected;
  expect(canceled).toHaveBeenCalledOnce(); expect(api.text).not.toHaveBeenCalled();
  expect(canceled).toHaveBeenCalledWith(expect.any(Error));
  expect(v.canvas.width).toBe(0); expect(v.layer.childNodes.length).toBe(0);
});
it('cleans the global text measurement cache again after a canceled text task settles', async () => {
  const entered = deferred<void>(), held = deferred<void>();
  api.text.mockImplementation(() => { entered.resolve(); return held.promise; });
  const pdf = owner(), v = view(), work = pdf.render(1, 1, v.canvas, v.layer, v.signal.signal);
  const rejected = expect(work).rejects.toThrow('obsolete');
  await entered.promise; pdf.dispose();
  const immediate = api.cleanup.mock.calls.length;
  held.resolve(); await rejected;
  expect(api.cleanup.mock.calls.length).toBe(immediate + 1);
});

it.each(['bridge', 'load'] as const)('failed %s construction detaches old handlers and releases admission before another owner', boundary => {
  api[boundary].mockImplementationOnce(() => { throw new Error('construction failed'); });
  expect(() => owner()).toThrow('worker_failure');
  expect(workers[0].onerror).toBeNull(); expect(workers[0].terminate).toHaveBeenCalledOnce();
  expect(() => owner()).not.toThrow();
});

it('does not count healthy pages waiting for the serialized render slot against their active deadline', async () => {
  vi.useFakeTimers(); document.numPages = 2;
  page.render.mockImplementation(() => ({ promise: new Promise<void>(resolve => setTimeout(resolve, 8000)), cancel: vi.fn() }));
  const ui = render(createElement(PdfPreview, { bytes: new Uint8Array([1]), signal: new AbortController().signal, viewState: { pdfZoom: { kind: 'fixed', scale: 1 } }, onViewStateChange() {}, retry() {} }));
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  expect(page.render).toHaveBeenCalledTimes(1);
  await act(async () => { await vi.advanceTimersByTimeAsync(8000); });
  expect(page.render).toHaveBeenCalledTimes(2);
  await act(async () => { await vi.advanceTimersByTimeAsync(7001); });
  expect(ui.queryByRole('alert')).toBeNull(); expect(workers[0].terminate).not.toHaveBeenCalled();
  await act(async () => { await vi.advanceTimersByTimeAsync(999); });
  expect(ui.queryAllByRole('status')).toHaveLength(0); expect(ui.getAllByRole('img')).toHaveLength(2);
});
it('times out an actually stuck render once and retires its worker and deadline', async () => {
  vi.useFakeTimers(); const held = deferred<void>(), failed = vi.fn();
  page.render.mockReturnValue({ promise: held.promise, cancel: vi.fn() });
  const pdf = new PdfDocumentOwner(new Uint8Array([1]), failed); owners.push(pdf); const v = view();
  const work = pdf.render(1, 1, v.canvas, v.layer, v.signal.signal), rejected = expect(work).rejects.toThrow('obsolete');
  await vi.advanceTimersByTimeAsync(15000);
  expect(failed).toHaveBeenCalledExactlyOnceWith('parser_timeout'); expect(workers[0].terminate).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
  held.resolve(); await rejected;
  expect(v.layer.childElementCount).toBe(0);
});
it('cancels a queued page immediately without starting it or disturbing the active page deadline', async () => {
  vi.useFakeTimers(); const held = deferred<void>();
  page.render.mockReturnValueOnce({ promise: held.promise, cancel: vi.fn() });
  const pdf = owner(), a = view(), b = view();
  const first = pdf.render(1, 1, a.canvas, a.layer, a.signal.signal);
  await vi.advanceTimersByTimeAsync(0);
  const queued = pdf.render(2, 1, b.canvas, b.layer, b.signal.signal), rejected = expect(queued).rejects.toThrow();
  b.signal.abort(); await rejected;
  expect(page.render).toHaveBeenCalledTimes(1); expect(vi.getTimerCount()).toBe(1);
  held.resolve(); await first; await vi.advanceTimersByTimeAsync(0);
  expect(page.render).toHaveBeenCalledTimes(1); expect(vi.getTimerCount()).toBe(0);
});

it('opens the first page of a long document without requesting offscreen page geometry', async () => {
  vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} });
  document.numPages = 100;
  document.getPage.mockImplementation(async (number: number) => {
    if (number !== 1) throw Error('Offscreen page must remain unread');
    return page;
  });
  const ui = render(createElement(PdfPreview, { bytes: new Uint8Array([1]), signal: new AbortController().signal, viewState: { pdfZoom: { kind: 'fixed', scale: 1 } }, onViewStateChange: () => {}, retry: () => {} }));
  await vi.waitFor(() => expect(ui.container.querySelector('canvas')?.width).toBe(600));
  expect(ui.container.querySelectorAll('[data-pdf-page]')).toHaveLength(100);
  expect(new Set(document.getPage.mock.calls.map(([number]: [number]) => number))).toEqual(new Set([1]));
  expect(document.getPage.mock.calls.length).toBeLessThanOrEqual(2);
  expect(ui.queryByRole('alert')).toBeNull();
});

it('keeps a finite load deadline while reference geometry is pending after the page count arrives', async () => {
  vi.useFakeTimers();
  vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} });
  document.numPages = 100;
  const held = deferred<any>(); document.getPage.mockReturnValue(held.promise);
  const ui = render(createElement(PdfPreview, { bytes: new Uint8Array([1]), signal: new AbortController().signal, viewState: { pdfZoom: { kind: 'fixed', scale: 1 } }, onViewStateChange() {}, retry() {} }));
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  expect(ui.container.querySelectorAll('[data-pdf-page]')).toHaveLength(100);
  expect(ui.queryByRole('toolbar', { name: 'Zoom' })).toBeNull();
  await act(async () => { await vi.advanceTimersByTimeAsync(15000); });
  expect(ui.getByRole('alert')).toBeTruthy(); expect(workers[0].terminate).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
  await act(async () => { held.resolve(page); });
  expect(page.render).not.toHaveBeenCalled(); expect(ui.queryByRole('toolbar', { name: 'Zoom' })).toBeNull();
  ui.unmount(); expect(workers[0].terminate).toHaveBeenCalledOnce();
});

it('keeps document zoom and Fit width available after the first bitmap is canceled and page 20 is visible', async () => {
  const observed = new Map<Element, (visible: boolean) => void>();
  vi.stubGlobal('IntersectionObserver', class {
    constructor(private readonly callback: IntersectionObserverCallback) {}
    observe(node: Element) { observed.set(node, visible => this.callback([{ target: node, isIntersecting: visible } as IntersectionObserverEntry], this as unknown as IntersectionObserver)); }
    disconnect() {}
  });
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(424);
  document.numPages = 100;
  const firstBitmap = deferred<void>(), cancel = vi.fn(() => firstBitmap.reject(new Error('canceled')));
  page.render.mockReturnValue({ promise: firstBitmap.promise, cancel });
  const later = { ...page, getViewport: vi.fn(() => ({ width: 300, height: 600 })), render: vi.fn(() => ({ promise: Promise.resolve(), cancel: vi.fn() })) };
  document.getPage.mockImplementation(async (number: number) => {
    if (number === 1) return page;
    if (number === 20) return later;
    throw Error('Offscreen pages must remain unread');
  });
  const preference = vi.fn(), controller = new AbortController();
  function Surface() {
    const [viewState, setView] = useState<PreviewViewState>({ pdfZoom: { kind: 'fixed', scale: 1 } });
    return createElement(PdfPreview, { bytes, signal: controller.signal, viewState, onViewStateChange: patch => { preference(patch); setView(state => ({ ...state, ...patch })); }, retry() {} });
  }
  const bytes = new Uint8Array([1]), ui = render(createElement(Surface));
  await vi.waitFor(() => expect(page.render).toHaveBeenCalledOnce());
  const first = ui.container.querySelector('[data-pdf-page="1"]')!, twentieth = ui.container.querySelector('[data-pdf-page="20"]')!;
  act(() => { observed.get(first)!(false); observed.get(twentieth)!(true); });
  await vi.waitFor(() => expect(later.render).toHaveBeenCalledOnce());
  expect(cancel).toHaveBeenCalled();
  const canvas = twentieth.querySelector('canvas')!;
  expect(canvas.width).toBe(300); expect(canvas.parentElement?.hidden).toBe(false);
  const controls = ui.getByRole('toolbar', { name: 'Zoom' });
  fireEvent.pointerEnter(controls);
  await vi.waitFor(() => expect(controls.getAttribute('data-document-zoom-visible')).toBe('true'));
  fireEvent.click(ui.getByRole('button', { name: 'Zoom' }));
  fireEvent.click(ui.getByRole('menuitem', { name: '150%' }));
  await vi.waitFor(() => expect(canvas.width).toBe(450));
  expect(preference).toHaveBeenCalledWith({ pdfZoom: { kind: 'fixed', scale: 1.5 } });
  fireEvent.click(ui.getByRole('button', { name: 'Zoom' }));
  fireEvent.click(ui.getByRole('menuitem', { name: 'Fit width' }));
  await vi.waitFor(() => expect(canvas.width).toBe(200));
  expect(preference).toHaveBeenCalledWith({ pdfZoom: { kind: 'fit-width' } });
  expect(ui.getByRole('button', { name: 'Zoom' }).textContent).toContain('67%');
  expect(new Set(document.getPage.mock.calls.map(([number]: [number]) => number))).toEqual(new Set([1, 20]));
  expect(ui.queryByRole('alert')).toBeNull();
  ui.unmount();
  expect(workers[0].terminate).toHaveBeenCalledOnce();
  expect([canvas.width, canvas.height, twentieth.querySelector('[data-pdf-text]')?.childElementCount]).toEqual([0, 0, 0]);
});

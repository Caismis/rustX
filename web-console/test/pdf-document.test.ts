import { afterEach, beforeEach, expect, it, vi } from 'vitest';
const api = vi.hoisted(() => ({ load: vi.fn(), bridge: vi.fn(), text: vi.fn(), cleanup: vi.fn() }));
vi.mock('pdfjs-dist', () => ({ getDocument: api.load, PDFWorker: { create: api.bridge }, TextLayer: class {
  static cleanup = api.cleanup; cancel = vi.fn(); render = api.text;
} }));
import { PdfDocumentOwner } from '../src/client/pdf-document';
function deferred<T>() { let resolve!: (v: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
const workers: FakeWorker[] = [], owners: PdfDocumentOwner[] = [];
class FakeWorker { onerror?: (event: Event) => void; terminate = vi.fn(); constructor() { workers.push(this); } }
let page: any, document: any, destroy: ReturnType<typeof vi.fn>;
beforeEach(() => {
  api.load.mockReset(); api.bridge.mockReset(); api.text.mockReset(); api.cleanup.mockReset();
  vi.stubGlobal('Worker', FakeWorker);
  destroy = vi.fn(async () => {}); api.bridge.mockReturnValue({ destroy: vi.fn() }); api.text.mockResolvedValue(undefined);
  page = { getViewport: vi.fn(() => ({ width: 600, height: 800 })), cleanup: vi.fn(),
    render: vi.fn(() => ({ promise: Promise.resolve(), cancel: vi.fn() })),
    streamTextContent: () => new ReadableStream({ start(controller) { controller.close(); } }),
  };
  document = { filterFactory: { destroy: vi.fn() }, numPages: 3, getPage: vi.fn(async () => page) };
  api.load.mockReturnValue({ promise: Promise.resolve(document), destroy });
});
afterEach(() => { for (const owner of owners.splice(0)) owner.dispose(); workers.splice(0); vi.unstubAllGlobals(); });
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
    const extra = Array.from({ length: 4 }, () => budget.create(1, 1));
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

it('retirement cancels a held text stream and cannot start a text layer afterwards', async () => {
  const entered = deferred<void>(), canceled = vi.fn();
  page.streamTextContent = () => new ReadableStream({ pull() { entered.resolve(); }, cancel: canceled });
  const pdf = owner(), v = view(), work = pdf.render(1, 1, v.canvas, v.layer, v.signal.signal);
  const rejected = expect(work).rejects.toThrow('obsolete');
  await entered.promise; pdf.dispose(); await rejected;
  expect(canceled).toHaveBeenCalledOnce(); expect(api.text).not.toHaveBeenCalled();
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

import { DERIVED_PDF_MAX_BYTES } from '../../shared/documents.ts';
import { getDocument, PDFWorker, TextLayer, type PDFDocumentProxy, type RenderTask } from 'pdfjs-dist';
import type { BaseFilterFactory } from 'pdfjs-dist/types/src/display/filter_factory';
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
import { PDF_LIMITS as limits } from './pdf-limits';

type CanvasEntry = { canvas: HTMLCanvasElement | null; context: CanvasRenderingContext2D | null };
/** Bounds PDF.js scratch canvases separately from the single presentation canvas. */
export class PdfCanvasBudget {
  private entries = new Map<CanvasEntry, number>();
  private pixels = 0;
  private retired = false;
  private admit(width: number, height: number, previous = 0) {
    if (this.retired || !Number.isInteger(width) || !Number.isInteger(height) || width < 1 || height < 1
      || width > limits.canvasSide || height > limits.canvasSide || width * height > limits.canvasPixels
      || this.pixels - previous + width * height > limits.scratchPixels) throw new Error('parser_limit');
  }
  create(width: number, height: number): CanvasEntry {
    this.admit(width, height);
    if (this.entries.size >= limits.scratchCanvases) throw new Error('parser_limit');
    const canvas = document.createElement('canvas');
    canvas.width = width; canvas.height = height;
    const context = canvas.getContext('2d');
    if (!context) { canvas.width = canvas.height = 0; throw new Error('worker_failure'); }
    const entry = { canvas, context };
    this.entries.set(entry, width * height); this.pixels += width * height;
    return entry;
  }
  reset(entry: CanvasEntry, width: number, height: number) {
    const previous = this.entries.get(entry);
    if (previous === undefined || !entry.canvas) throw new Error('obsolete');
    this.admit(width, height, previous);
    entry.canvas.width = width; entry.canvas.height = height;
    this.entries.set(entry, width * height); this.pixels += width * height - previous;
  }
  destroy(entry: CanvasEntry) {
    const previous = this.entries.get(entry);
    if (previous === undefined) return;
    if (entry.canvas) entry.canvas.width = entry.canvas.height = 0;
    entry.canvas = null; entry.context = null;
    this.entries.delete(entry); this.pixels -= previous;
  }
  dispose() { this.retired = true; for (const entry of this.entries.keys()) this.destroy(entry); }
}
let workers = 0;
/** One explicit native worker, one serialized page render, no global workerPort. */
export class PdfDocumentOwner {
  private canvases = new PdfCanvasBudget();
  private worker?: Worker;
  private pdfWorker?: PDFWorker;
  private loading?: ReturnType<typeof getDocument>;
  private ready?: Promise<PDFDocumentProxy>;
  private loaded?: PDFDocumentProxy;
  get document(): Promise<PDFDocumentProxy> {
    if (!this.ready) throw new Error('obsolete');
    return this.ready;
  }
  private retired = false;
  private tail: Promise<unknown> = Promise.resolve();
  private cancelPage?: () => void;
  private presentation?: { canvas: HTMLCanvasElement; container: HTMLDivElement };
  constructor(bytes: Uint8Array<ArrayBuffer>, failed: (code: string) => void) {
    if (bytes.length > DERIVED_PDF_MAX_BYTES) throw new Error('too_large');
    if (workers >= limits.workers) throw new Error('capacity');
    try { this.worker = new Worker(workerUrl, { type: 'module' }); }
    catch { throw new Error('worker_failure'); }
    workers++;
    const workerFailed = (event: Event) => { event.preventDefault(); if (!this.retired) { failed('worker_failure'); this.dispose(); } };
    this.worker.onerror = workerFailed;
    this.worker.onmessageerror = workerFailed;
    try {
      this.pdfWorker = PDFWorker.create({ port: this.worker });
      const budget = this.canvases;
      class CanvasFactory {
        create = budget.create.bind(budget);
        reset = budget.reset.bind(budget);
        destroy = budget.destroy.bind(budget);
      }
      this.loading = getDocument({ CanvasFactory, enableHWA: false, isOffscreenCanvasSupported: false, isImageDecoderSupported: false, data: bytes.slice(), worker: this.pdfWorker, useWasm: false, enableXfa: false,
        useWorkerFetch: false, useSystemFonts: false, disableFontFace: true, maxImageSize: limits.canvasPixels, canvasMaxAreaInBytes: limits.canvasPixels * 4,
        disableAutoFetch: true, disableStream: true, disableRange: true, stopAtErrors: true });
    } catch {
      this.worker.onerror = null; this.worker.onmessageerror = null;
      this.pdfWorker?.destroy(); this.canvases.dispose(); this.worker.terminate(); workers--; throw new Error('worker_failure');
    }
    this.ready = this.loading.promise.then(document => {
      if (this.retired) throw new Error('obsolete');
      this.loaded = document;
      if (document.numPages > limits.pdfPages) throw new Error('parser_limit');
      return document;
    });
    // The owner observes terminal rejection even when closed before a consumer subscribes.
    void this.document.catch(() => {});
  }
  render(number: number, zoom: number, canvas: HTMLCanvasElement, container: HTMLDivElement, signal: AbortSignal): Promise<void> {
    const current = () => { signal.throwIfAborted(); if (this.retired) throw new Error('obsolete'); };
    this.cancelPage?.();
    const work = this.tail.catch(() => {}).then(async () => {
      current();
      const document = await this.document; current();
      const page = await document.getPage(number);
      let render: RenderTask | undefined, text: TextLayer | undefined, cancelTextRead: (() => void) | undefined;
      const abort = () => { render?.cancel(); text?.cancel(); cancelTextRead?.(); };
      this.cancelPage = abort;
      signal.addEventListener('abort', abort, { once: true });
      try {
        current();
        const viewport = page.getViewport({ scale: zoom });
        const width = Math.ceil(viewport.width), height = Math.ceil(viewport.height);
        if (!Number.isFinite(width * height) || width < 1 || height < 1 || width > limits.canvasSide || height > limits.canvasSide
          || width * height > limits.canvasPixels) throw new Error('parser_limit');
        this.presentation = { canvas, container };
        canvas.width = width; canvas.height = height;
        container.style.setProperty('--scale-factor', String(zoom));
        container.style.setProperty('--total-scale-factor', String(zoom));
        container.style.width = `${width}px`; container.style.height = `${height}px`;
        render = page.render({ canvas, viewport });
        await render.promise;
        current();
        const reader = page.streamTextContent().getReader();
        const cancelReader = () => { void reader.cancel().catch(() => {}); };
        cancelTextRead = cancelReader;
        const content: Awaited<ReturnType<typeof page.getTextContent>> = { items: [], styles: {}, lang: null };
        let characters = 0;
        try {
          for (;;) {
            current();
            const chunk: ReadableStreamReadResult<Awaited<ReturnType<typeof page.getTextContent>>> = await reader.read(); if (chunk.done) break;
            characters += chunk.value.items.reduce((n, item) => n + ('str' in item ? item.str.length : 0), 0);
            if (content.items.length + chunk.value.items.length > limits.textItems || characters > limits.textCharacters) throw new Error('parser_limit');
            content.items.push(...chunk.value.items); Object.assign(content.styles, chunk.value.styles);
          }
        } finally { cancelTextRead = undefined; cancelReader(); }
        current();
        text = new TextLayer({ textContentSource: content, container, viewport });
        await text.render(); current();
      } finally {
        signal.removeEventListener('abort', abort);
        if (this.cancelPage === abort) this.cancelPage = undefined;
        abort(); page.cleanup(); (document.filterFactory as BaseFilterFactory).destroy(); TextLayer.cleanup();
      }
    });
    this.tail = work;
    return work;
  }
  dispose() {
    if (this.retired) return;
    this.retired = true; this.cancelPage?.(); this.cancelPage = undefined;
    this.tail = Promise.resolve();
    const loading = this.loading, worker = this.worker;
    this.loading = undefined; this.ready = undefined; this.worker = undefined;
    // The supplied native worker is terminated immediately. PDF.js may never
    // receive its Terminate ACK, so release public DOM filter resources here.
    // disableFontFace avoids installing document fonts into application state.
    (this.loaded?.filterFactory as BaseFilterFactory | undefined)?.destroy(); this.loaded = undefined;
    void loading?.destroy().catch(() => {});
    if (worker) { worker.onerror = null; worker.onmessageerror = null; }
    this.pdfWorker?.destroy(); this.pdfWorker = undefined; worker?.terminate(); workers--;
    this.canvases.dispose(); TextLayer.cleanup();
    if (this.presentation) {
      this.presentation.canvas.width = this.presentation.canvas.height = 0;
      this.presentation.container.replaceChildren(); this.presentation = undefined;
    }
  }
}

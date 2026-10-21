/* Copyright (c) 2026 DeepSeek. MIT. Page geometry/rendering adapted; native resource owner retained. See PROVENANCE.md. */
import { createPdfBinaryDataFactory } from './pdf-assets';
import { DERIVED_PDF_MAX_BYTES } from '../../shared/documents.ts';
import { getDocument, PDFWorker, TextLayer, type PDFDocumentProxy, type RenderTask } from 'pdfjs-dist';
import type { BaseFilterFactory } from 'pdfjs-dist/types/src/display/filter_factory';
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
import { PDF_LIMITS as limits } from './pdf-limits';

export interface PdfPageSize { readonly width: number; readonly height: number }

type CanvasEntry = { canvas: HTMLCanvasElement | null; context: CanvasRenderingContext2D | null };
/** Bounds PDF.js scratch canvases separately from the resident page bitmaps. */
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
  clear() { for (const entry of this.entries.keys()) this.destroy(entry); }
  dispose() { this.retired = true; this.clear(); }
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
  private renderTimer?: ReturnType<typeof setTimeout>;
  private readonly presentations = new Map<HTMLCanvasElement, { container: HTMLDivElement; pixels: number }>();
  private rendering?: HTMLCanvasElement;
  private sizes?: Promise<readonly PdfPageSize[]>;
  /** Finite geometry lets every page hold its place while bitmaps remain viewport-local. */
  get pageSizes(): Promise<readonly PdfPageSize[]> {
    return this.sizes ??= this.document.then(async document => {
      const sizes: PdfPageSize[] = [];
      for (let number = 1; number <= document.numPages; number++) {
        if (this.retired) throw new Error('obsolete');
        const page = await document.getPage(number);
        try { if (this.retired) throw new Error('obsolete'); sizes.push(this.pageSize(page)); }
        finally { page.cleanup(); }
      }
      return sizes;
    });
  }
  private pageSize(page: Awaited<ReturnType<PDFDocumentProxy['getPage']>>): PdfPageSize {
    const { width, height } = page.getViewport({ scale: 96 / 72 });
    // Admission stays in PDF points; CSS uses the same 96/72 conversion as DSH.
    // Raster allocations below remain independently capped at the native limits.
    const pointsWidth = width * 72 / 96, pointsHeight = height * 72 / 96;
    if (!Number.isFinite(width * height) || pointsWidth < 1 || pointsHeight < 1 || pointsWidth > limits.canvasSide || pointsHeight > limits.canvasSide || pointsWidth * pointsHeight > limits.canvasPixels) throw new Error('parser_limit');
    return { width, height };
  }
  /** Release only the page leaving the reading window; its layout metadata stays inert. */
  release(canvas: HTMLCanvasElement) {
    const held = this.presentations.get(canvas);
    if (!held) return;
    canvas.width = canvas.height = 0;
    held.container.replaceChildren();
    this.presentations.delete(canvas);
  }
  constructor(bytes: Uint8Array<ArrayBuffer>, private readonly failed: (code: string) => void) {
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
      this.loading = getDocument({ BinaryDataFactory: createPdfBinaryDataFactory(), cMapPacked: true, CanvasFactory, enableHWA: false, isOffscreenCanvasSupported: false, isImageDecoderSupported: false, data: bytes.slice(), worker: this.pdfWorker, useWasm: false, enableXfa: false,
        useWorkerFetch: false, useSystemFonts: false, disableFontFace: true, maxImageSize: limits.canvasPixels, canvasMaxAreaInBytes: limits.canvasPixels * 4,
        disableAutoFetch: true, disableStream: true, disableRange: true, stopAtErrors: true });
    } catch {
      this.worker.onerror = null; this.worker.onmessageerror = null;
      this.pdfWorker?.destroy(); this.canvases.dispose(); this.worker.terminate(); workers--; throw new Error('worker_failure');
    }
    this.ready = this.loading.promise.then(document => {
      if (this.retired) throw new Error('obsolete');
      this.loaded = document;
      if (document.numPages < 1 || document.numPages > limits.pdfPages) throw new Error('parser_limit');
      return document;
    });
    // The owner observes terminal rejection even when closed before a consumer subscribes.
    void this.document.catch(() => {});
  }
  render(number: number, zoom: number, canvas: HTMLCanvasElement, container: HTMLDivElement, signal: AbortSignal): Promise<void> {
    if (this.retired || signal.aborted) return Promise.reject(new Error('obsolete'));
    if (!this.presentations.has(canvas)) this.presentations.set(canvas, { container, pixels: canvas.width * canvas.height });
    const current = () => { signal.throwIfAborted(); if (this.retired) throw new Error('obsolete'); };
    const work = this.tail.catch(() => {}).then(async () => {
      current();
      // A page owns its deadline only after the serialized render slot starts.
      this.renderTimer = setTimeout(() => { this.dispose(); this.failed('parser_timeout'); }, limits.renderWatchdogMs);
      const clearDeadline = () => { clearTimeout(this.renderTimer); this.renderTimer = undefined; };
      signal.addEventListener('abort', clearDeadline, { once: true });
      try {
        current();
        const document = await this.document; current();
        const page = await document.getPage(number);
        let render: RenderTask | undefined, text: TextLayer | undefined, cancelTextRead: (() => void) | undefined;
        const abort = () => { render?.cancel(); text?.cancel(); cancelTextRead?.(); };
        this.cancelPage = abort;
        signal.addEventListener('abort', abort, { once: true });
        const buffer = window.document.createElement('canvas'); this.rendering = buffer;
        try {
          current();
          const size = this.pageSize(page);
          if (!Number.isFinite(zoom) || zoom <= 0 || zoom > 4) throw new Error('parser_limit');
          const viewport = page.getViewport({ scale: 96 / 72 });
          const ratio = Math.min((globalThis.devicePixelRatio || 1) * zoom, Math.sqrt(limits.canvasPixels / (size.width * size.height)), limits.canvasSide / size.width, limits.canvasSide / size.height);
          const width = Math.max(1, Math.floor(size.width * ratio)), height = Math.max(1, Math.floor(size.height * ratio));
          const pixels = [...this.presentations].reduce((total, [node, entry]) => total + (node === canvas ? 0 : entry.pixels), 0);
          if (pixels + width * height > limits.scratchPixels) throw new Error('parser_limit');
          buffer.width = width; buffer.height = height;
          const layer = window.document.createElement('div'); layer.className = 'textLayer';
          layer.style.setProperty('--scale-factor', String(viewport.scale));
          layer.style.setProperty('--total-scale-factor', String(viewport.scale));
          layer.style.width = `${size.width}px`; layer.style.height = `${size.height}px`;
          render = page.render({ canvas: buffer, viewport, transform: ratio === 1 ? undefined : [ratio, 0, 0, ratio, 0, 0] });
          await render.promise;
          current();
          const reader = page.streamTextContent().getReader();
          // PDF.js requires an Error reason before marking its stream controller
          // closed. Omitting it leaves queued worker chunks targeting a closed RS.
          const cancelReader = () => { void reader.cancel(new Error('obsolete')).catch(() => {}); };
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
          text = new TextLayer({ textContentSource: content, container: layer, viewport });
          await text.render(); current();
          const context = canvas.getContext('2d');
          if (!context) throw new Error('worker_failure');
          // Publish a complete bitmap and matching text together. Zooming never
          // clears the old readable page while a replacement is being rendered.
          canvas.width = width; canvas.height = height;
          context.drawImage(buffer, 0, 0);
          container.replaceChildren(layer);
          this.presentations.set(canvas, { container, pixels: width * height });
        } finally {
          buffer.width = buffer.height = 0;
          if (this.rendering === buffer) this.rendering = undefined;
          signal.removeEventListener('abort', abort);
          if (this.cancelPage === abort) this.cancelPage = undefined;
          abort(); page.cleanup(); this.canvases.clear(); (document.filterFactory as BaseFilterFactory).destroy(); TextLayer.cleanup();
        }
      } finally { signal.removeEventListener('abort', clearDeadline); clearDeadline(); }
    });
    this.tail = work;
    // Cancellation of a queued page settles immediately; its retired queue seat
    // remains serialized until prior rendering has actually finished.
    return new Promise<void>((resolve, reject) => {
      const abort = () => reject(signal.reason);
      signal.addEventListener('abort', abort, { once: true });
      void work.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort));
      if (signal.aborted) abort();
    });
  }
  dispose() {
    if (this.retired) return;
    this.retired = true; clearTimeout(this.renderTimer); this.renderTimer = undefined; this.cancelPage?.(); this.cancelPage = undefined;
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
    if (this.rendering) this.rendering.width = this.rendering.height = 0;
    for (const canvas of this.presentations.keys()) this.release(canvas);
  }
}

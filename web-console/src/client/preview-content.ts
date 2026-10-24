/** A logical tab owns content and the physical read barrier; mounted bodies own
 * only renderer resources. Abort retires publication, never an operation slot. */
import { WorkspaceHostError } from '../workspaces/host';
export interface PreviewTextPage { text: string; version: string; offset: number; lines: number; eof: boolean }
export interface PreviewBytes { data: Uint8Array<ArrayBuffer>; version: string }
export type PreviewLoadMode = 'pages' | 'bytes' | 'docx' | 'pptx';
export interface PreviewObservation { version: string; close(): Promise<void>; done: Promise<void> }
export interface PreviewReader {
  current(): boolean;
  immutable: boolean;
  page(offset: number, signal: AbortSignal): Promise<PreviewTextPage>;
  bytes(signal: AbortSignal): Promise<PreviewBytes>;
  document(extension: 'docx' | 'pptx', signal: AbortSignal): Promise<PreviewBytes>;
  observe?(signal: AbortSignal, changed: () => void): Promise<PreviewObservation>;
  probe(signal: AbortSignal): Promise<string>;
}
export interface PreviewContentSnapshot {
  revision: number; mode?: PreviewLoadMode; loading: boolean;
  text?: string; data?: Uint8Array<ArrayBuffer>; version?: string;
  page?: PreviewTextPage; error?: unknown; failedOffset?: number; loadedLines: number;
}
type Intent = { kind: 'read'; offset: number } | { kind: 'validate' };
const TEXT_PREFIX_BYTES = 64 * 1024 * 1024;
export class PreviewContentOwner {
  private state: PreviewContentSnapshot = { revision: 0, loading: false, loadedLines: 0 };
  private saved?: PreviewContentSnapshot;
  private listeners = new Set<() => void>();
  private epoch = 0;
  private active?: Promise<void>;
  private activeIntent?: Intent;
  private resume?: Intent;
  private controller?: AbortController;
  private pending?: Intent;
  private wantedLines = 0;
  private closed = false;
  private visible = false;
  private observation?: PreviewObservation;
  private lifetime = new AbortController();
  private retirement: Promise<void> = Promise.resolve();
  private observedVersion?: string;
  constructor(private readonly reader: PreviewReader) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.state;
  get automaticRefresh() { return this.reader.immutable || this.reader.observe !== undefined; }
  get retired() { return this.closed; }
  get retainedBytes() { const value = this.saved ?? this.state; return (value.data?.byteLength ?? 0) + (value.text?.length ?? 0) * 2; }
  private publish(patch: Partial<PreviewContentSnapshot>) { this.state = { ...this.state, ...patch }; this.notify(); }
  private notify() { this.listeners.forEach(listener => listener()); }
  activate() {
    if (this.closed || this.visible) return;
    if (!this.reader.current()) { this.dispose(); return; }
    this.visible = true;
    // A previously hidden payload is never exposed until current authority and
    // version are rechecked. A live watch alone is not permission to reuse it.
    if (!this.reader.immutable && this.reader.observe && this.state.mode) {
      this.saved = this.state;
      this.state = { revision: this.state.revision, mode: this.state.mode, loading: true, loadedLines: 0 };
      this.pending = { kind: 'validate' }; this.notify(); this.drain();
    }
  }
  deactivate() {
    if (!this.visible) return;
    this.visible = false; this.epoch++; this.pending = undefined; this.resume = undefined; this.controller?.abort();
    if (!this.observation) this.retireObservation();
    this.publish({ loading: false });
    if (!this.reader.immutable && !this.reader.observe) this.evict();
  }
  private current(epoch: number) { return !this.closed && this.visible && this.reader.current() && this.epoch === epoch; }
  ensure(mode: PreviewLoadMode, wantedLines = 0) {
    if (!this.visible || !this.reader.current() || this.closed) return;
    if (this.state.mode !== mode) {
      this.wantedLines = this.state.mode === undefined && Number.isSafeInteger(wantedLines) ? Math.max(0, wantedLines) : 0;
      this.reset(mode, !this.observation && !!this.reader.observe); return;
    }
    if (this.state.error !== undefined || this.state.text !== undefined || this.state.data !== undefined || this.pending || this.active && !this.controller?.signal.aborted) return;
    this.pending = { kind: 'read', offset: 1 }; this.publish({ loading: true }); this.drain();
  }
  refresh() {
    if (!this.visible || !this.reader.current() || this.closed || !this.state.mode) return;
    this.wantedLines = Math.max(this.wantedLines, this.state.loadedLines, this.saved?.loadedLines ?? 0); this.reset(this.state.mode, !this.observation && !!this.reader.observe);
  }
  private reset(mode: PreviewLoadMode, validate = false) {
    this.epoch++; this.controller?.abort(); this.saved = undefined; this.resume = undefined;
    this.pending = validate ? { kind: 'validate' } : { kind: 'read', offset: 1 };
    this.state = { revision: this.state.revision + 1, mode, loading: true, loadedLines: 0 };
    this.notify(); this.drain();
  }
  /** Hints revoke publication immediately. Equal-version hints can reuse the
   * already loaded prefix after an authorized probe, without a content reread. */
  invalidate = () => {
    if (this.closed) return;
    if (this.activeIntent?.kind === 'read' && this.activeIntent.offset > 1 && this.state.text !== undefined) this.resume ??= this.activeIntent;
    this.epoch++; this.controller?.abort();
    if (!this.visible) { this.evict(); return; }
    this.saved ??= this.state;
    this.wantedLines = Math.max(this.wantedLines, this.saved.loadedLines);
    this.state = { ...this.state, loading: true };
    this.pending = { kind: 'validate' }; this.notify(); this.drain();
  };
  more() {
    if (!this.visible || !this.reader.current() || this.state.mode !== 'pages' || !this.state.page || this.state.page.eof || this.state.loading) return;
    this.pending = { kind: 'read', offset: this.state.page.offset + this.state.page.lines };
    this.publish({ loading: true, error: undefined, failedOffset: undefined }); this.drain();
  }
  retry() {
    if (!this.reader.current() || this.state.loading) return;
    if (this.state.failedOffset && this.state.failedOffset > 1 && this.state.text !== undefined) {
      this.pending = { kind: 'read', offset: this.state.failedOffset };
      this.publish({ loading: true, error: undefined, failedOffset: undefined }); this.drain();
    } else this.refresh();
  }
  private drain() {
    const mode = this.state.mode, intent = this.pending;
    if (this.active || !this.visible || !this.reader.current() || !mode || !intent || this.closed) return;
    this.pending = undefined;
    const epoch = this.epoch, controller = new AbortController(); this.controller = controller;
    this.activeIntent = intent;
    const operation = (async () => {
      try {
        if (intent.kind === 'validate') {
          let version: string;
          if (!this.observation && this.reader.observe) {
            await this.retirement;
            if (!this.current(epoch)) return;
            const lifetime = this.lifetime;
            const observation = await this.reader.observe(lifetime.signal, this.invalidate);
            if (this.closed || this.lifetime !== lifetime || lifetime.signal.aborted) { await observation.close(); return; }
            this.observation = observation; version = observation.version;
            void observation.done.then(() => {
              if (this.observation !== observation || this.closed) return;
              this.observation = undefined; this.epoch++; this.controller?.abort(); this.pending = undefined;
              this.saved = undefined; this.evict(); this.publish({ error: new Error('File observation unavailable') });
            });
          } else version = await this.reader.probe(controller.signal);
          if (!this.current(epoch)) return;
          const saved = this.saved; this.saved = undefined;
          const previousVersion = saved?.version && saved.mode !== 'docx' && saved.mode !== 'pptx' ? saved.version : this.observedVersion;
          this.observedVersion = version;
          if (saved && previousVersion === version && (saved.error !== undefined || saved.text !== undefined || saved.data !== undefined)) {
            this.pending = this.resume; this.resume = undefined;
            this.state = { ...saved, loading: this.pending !== undefined }; this.notify();
          } else {
            this.resume = undefined;
            this.state = { revision: this.state.revision + 1, mode, loading: true, loadedLines: 0 };
            this.pending = { kind: 'read', offset: 1 }; this.notify();
          }
          return;
        }
        if (mode !== 'pages') {
          const result = await (mode === 'bytes' ? this.reader.bytes(controller.signal) : this.reader.document(mode, controller.signal));
          if (this.current(epoch)) this.publish({ data: result.data, version: result.version, loading: false });
        } else {
          const page = await this.reader.page(intent.offset, controller.signal);
          if (!this.current(epoch)) return;
          if (intent.offset > 1 && this.state.version !== undefined && page.version !== this.state.version) { this.refresh(); return; }
          const text = intent.offset === 1 ? page.text : `${this.state.text ?? ''}${page.lines ? '\n' : ''}${page.text}`;
          if (text.length * 2 > TEXT_PREFIX_BYTES) throw new Error('parser_limit');
          const loadedLines = page.offset + page.lines - 1;
          if (page.eof) this.wantedLines = loadedLines;
          this.publish({ text, version: page.version, page, loadedLines, loading: false });
          if (!page.eof && loadedLines < this.wantedLines) { this.pending = { kind: 'read', offset: loadedLines + 1 }; this.publish({ loading: true }); }
        }
      } catch (error) {
        if (this.current(epoch)) {
          // Metadata failure means retained bytes are no longer authorized.
          if (intent.kind === 'validate' || error instanceof WorkspaceHostError && (error.kind === 'authority_replaced'
            || error.native?.kind === 'session_file_read' && ['unauthorized', 'unavailable', 'missing', 'not_regular', 'replaced'].includes(error.native.reason))) {
            this.saved = undefined; this.resume = undefined; this.state = { revision: this.state.revision + 1, mode, loading: false, loadedLines: 0 };
          }
          this.publish({ error, failedOffset: intent.kind === 'read' ? intent.offset : undefined, loading: false });
        }
      }
    })();
    this.active = operation;
    void operation.finally(() => { if (this.active === operation) { this.active = undefined; this.activeIntent = undefined; this.controller = undefined; this.drain(); } });
  }
  evict() {
    this.saved = undefined; this.resume = undefined;
    if (!this.visible) this.retireObservation();
    this.state = { revision: this.state.revision + 1, mode: this.state.mode, loading: false, loadedLines: 0 }; this.notify();
  }
  private retireObservation() {
    const observation = this.observation; this.observation = undefined;
    this.lifetime.abort(); this.lifetime = new AbortController();
    this.retirement = Promise.all([this.retirement, observation?.close()]).then(() => {});
    void this.retirement.catch(() => {});
  }
  dispose() {
    if (this.closed) return;
    this.closed = true; this.visible = false; this.epoch++; this.pending = undefined;
    this.controller?.abort(); this.retireObservation(); this.evict();
  }
}
/** Shared by all retained Sessions. Active renderers have separate budgets. */
export class PreviewContentCache {
  private inactive = new Map<PreviewContentOwner, () => void>();
  retain(owner: PreviewContentOwner) {
    this.release(owner);
    if (owner.retired) return;
    this.inactive.set(owner, owner.subscribe(() => this.trim())); this.trim();
  }
  release(owner: PreviewContentOwner) { this.inactive.get(owner)?.(); this.inactive.delete(owner); }
  private trim() {
    let bytes = [...this.inactive.keys()].reduce((sum, entry) => sum + entry.retainedBytes, 0);
    for (const entry of this.inactive.keys()) {
      if (this.inactive.size <= 8 && bytes <= 64 * 1024 * 1024) break;
      bytes -= entry.retainedBytes; this.release(entry); entry.evict();
    }
  }
}

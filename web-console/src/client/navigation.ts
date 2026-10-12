export interface NavigationIntent {
  current(): boolean;
  finish(): void;
}

/** Presentation authority only, never a cancellation token for native mutations.
 * Preparation supersedes continuations; only route publication/authority
 * replacement revokes controls belonging to the committed conversation. */
export class NavigationEpoch {
  private epoch = 0;
  private committed = 0;
  private pending?: { kind: 'sidebar' | 'conversation'; intent: NavigationIntent };
  private listeners = new Set<() => void>();
  getRevision = () => this.epoch;
  private notify() { this.listeners.forEach(listener => listener()); }
  private advance() { this.epoch++; this.notify(); }
  /** Publish a replacement route or retire its connection authority. */
  invalidate() { this.committed++; this.supersede(); }
  /** Supersede preparation/read continuations without replacing the route. */
  supersede() { this.pending = undefined; this.advance(); }
  capture() { const epoch = this.epoch; return () => this.epoch === epoch; }
  captureCommitted() { const committed = this.committed; return () => this.committed === committed; }
  begin(kind: 'sidebar' | 'conversation'): NavigationIntent {
    this.epoch++;
    const current = this.capture();
    const intent: NavigationIntent = {
      current: () => this.pending?.intent === intent && current(),
      finish: () => { if (this.pending?.intent === intent) this.retire(); },
    };
    this.pending = { kind, intent };
    this.notify();
    return intent;
  }
  retire(kind?: 'sidebar' | 'conversation') {
    if (!this.pending || (kind && this.pending.kind !== kind)) return;
    this.supersede();
  }
  subscribe(listener: () => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
}

/** A UI continuation fence, never a cancellation token for server mutations. */
export class NavigationEpoch {
  private epoch = 0;
  private listeners = new Set<() => void>();
  invalidate() { this.epoch++; this.listeners.forEach(listener => listener()); }
  capture() { const epoch = this.epoch; return () => this.epoch === epoch; }
  subscribe(listener: () => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
}

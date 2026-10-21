/** Web list preference only. Native Session identity and execution stay untouched. */
export class SessionPins {
  private ids: readonly string[] = [];
  private listeners = new Set<() => void>();
  private key: string;
  constructor(scope: string, private storage?: Pick<Storage, 'getItem' | 'setItem'>) {
    this.key = `rustx-session-pins-v1:${scope}`;
    try { this.read(storage?.getItem(this.key)); } catch { /* Browser preferences may be unavailable. */ }
  }
  private read(raw?: string | null) {
    const value: unknown = JSON.parse(raw ?? '[]');
    this.ids = Array.isArray(value) ? [...new Set(value.filter((id): id is string => typeof id === 'string'))] : [];
  }
  getSnapshot = () => this.ids;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    const changed = (event: StorageEvent) => {
      if (event.key !== this.key) return;
      try { this.read(event.newValue); this.listeners.forEach(notify => notify()); } catch { /* Ignore malformed presentation preferences. */ }
    };
    globalThis.addEventListener?.('storage', changed);
    return () => { this.listeners.delete(listener); globalThis.removeEventListener?.('storage', changed); };
  };
  toggle(id: string) { this.save(this.ids.includes(id) ? this.ids.filter(value => value !== id) : [id, ...this.ids]); }
  remove(id: string) { if (this.ids.includes(id)) this.save(this.ids.filter(value => value !== id)); }
  private save(ids: readonly string[]) {
    this.ids = ids;
    try { this.storage?.setItem(this.key, JSON.stringify(ids)); } catch { /* Still applies for this page. */ }
    this.listeners.forEach(listener => listener());
  }
}

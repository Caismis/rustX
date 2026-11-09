import { endpointIdentity } from './endpoint';

const KEY = 'rustx-workspace-selection-v1';
/** Explicit browser preference, scoped to the endpoint rather than a process
 * incarnation. A current Host catalog must still admit the stored registration. */
export class WorkspaceSelectionPreference {
  private values: Record<string, string> = {};
  private readonly client = crypto.randomUUID();
  private sequence = 0;
  constructor(private readonly storage?: Pick<Storage, 'getItem' | 'setItem'>) {
    try {
      const value: unknown = JSON.parse(storage?.getItem(KEY) ?? '{}');
      if (value && typeof value === 'object' && !Array.isArray(value)) for (const [endpoint, id] of Object.entries(value)) {
        if (URL.canParse(endpoint) && typeof id === 'string' && id) this.values[endpointIdentity(endpoint)] = id;
      }
    } catch { /* Browser storage may be disabled. */ }
  }
  read(endpoint: string | undefined) { return endpoint && URL.canParse(endpoint) ? this.values[endpointIdentity(endpoint)] : undefined; }
  select(endpoint: string, id: string) {
    if (!URL.canParse(endpoint) || !id) return;
    this.values[endpointIdentity(endpoint)] = id;
    try { this.storage?.setItem(KEY, JSON.stringify(this.values)); } catch { /* Keep the current browser selection usable. */ }
    return { client: this.client, sequence: ++this.sequence };
  }
  current(intent: { client: string; sequence: number }) { return intent.client === this.client && intent.sequence === this.sequence; }
}

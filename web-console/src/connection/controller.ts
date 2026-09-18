import type { AppServerClient } from '../client/app-server';
import { carrierFetch } from '../carrier/http';

export type ConnectionMode = 'local' | 'remote';
export interface ConnectionSelection { mode: ConnectionMode; selectedMode: ConnectionMode; busy: boolean; error?: string }
/** Selects the source of admission material; the native client owns socket/protocol state. */
export class ConnectionController {
  private state: ConnectionSelection = { mode: 'local', selectedMode: 'local', busy: false };
  private listeners = new Set<() => void>();
  private epoch = 0;
  // Remember explicit Remote material for the page lifetime, including while Local is active.
  private remote?: { endpoint: string; token: string };
  constructor(private client: AppServerClient, private fetchBootstrap: (path: string, init?: RequestInit) => Promise<Response> = carrierFetch) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.state;
  private publish(patch: Partial<ConnectionSelection>) { this.state = { ...this.state, ...patch }; this.listeners.forEach(listener => listener()); }
  async select(mode: ConnectionMode) {
    const epoch = ++this.epoch;
    this.publish({ selectedMode: mode, busy: true, error: undefined });
    if (mode === 'local') await this.local(epoch);
    else if (this.remote) await this.connect(this.remote.endpoint, this.remote.token, epoch, 'remote');
    else this.publish({ busy: false });
  }

  start = () => this.reconnect();
  async reconnect() {
    const epoch = ++this.epoch;
    this.publish({ busy: true, error: undefined });
    if (this.state.mode === 'local') await this.local(epoch);
    else if (this.remote) await this.connect(this.remote.endpoint, this.remote.token, epoch, 'remote');
    else this.publish({ busy: false, error: 'Enter a Remote App Server in Connection Settings.' });
  }
  async connectRemote(endpoint: string, token: string) {
    const epoch = ++this.epoch;
    this.publish({ busy: true, error: undefined });
    await this.connect(endpoint, token, epoch, 'remote');
  }
  async disconnect() {
    const epoch = ++this.epoch; this.publish({ busy: false, error: undefined });
    try { await this.client.disconnect(); }
    catch { if (epoch === this.epoch) this.publish({ error: 'Previous connection did not close. Reload before reconnecting.' }); }
  }
  private async local(epoch: number) {
    try {
      const response = await this.fetchBootstrap('/__rustx/bootstrap', { credentials: 'same-origin', cache: 'no-store', redirect: 'error' });
      if (!response.ok || !response.headers.get('content-type')?.includes('application/json')) throw new Error('No local managed connection is available. Reopen the launcher URL or configure a Remote App Server in Settings.');
      const material: unknown = await response.json();
      if (!material || typeof material !== 'object' || !('connectionMode' in material) || material.connectionMode !== 'local'
        || !('appServerEndpoint' in material) || typeof material.appServerEndpoint !== 'string'
        || !('appServerTransportToken' in material) || typeof material.appServerTransportToken !== 'string') throw new Error('Invalid local bootstrap response.');
      if (epoch === this.epoch) await this.connect(material.appServerEndpoint, material.appServerTransportToken, epoch, 'local');
    } catch { if (epoch === this.epoch) this.publish({ busy: false, error: 'No local managed connection is available. Reopen the launcher URL or configure a Remote App Server in Settings.' }); }
  }
  private async connect(endpoint: string, token: string, epoch: number, mode: ConnectionMode) {
    try { await this.client.connect(endpoint, token, this.client.isSameAuthority(endpoint) ? 'reconnect' : 'replace-authority', () => {
      // Commit ownership even if the new socket subsequently fails. No fallback.
      if (mode === 'remote') this.remote = { endpoint, token };
      if (epoch === this.epoch) this.publish({ mode, selectedMode: mode });
    }); }
    catch (error) { if (epoch === this.epoch) this.publish({ error: error instanceof Error ? error.message : 'Connection failed.' }); }
    finally { if (epoch === this.epoch) this.publish({ busy: false }); }
  }
}

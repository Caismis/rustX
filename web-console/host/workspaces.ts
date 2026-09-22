/** Local trusted Product Host. This module runs in Node, never in the browser. */
import { readFileSync, writeFileSync, renameSync, realpathSync, statSync, existsSync } from 'node:fs';
import { isAbsolute } from 'node:path';
import { randomUUID } from 'node:crypto';
import { sameEndpoint } from '../src/workspaces/endpoint.ts';
import type { ProductHostWorkspaces, WorkspaceCatalog, SessionLocation, WorkspaceConfigurationOperation, WorkspaceConfigurationResult, WorkspaceConfigurationReread } from '../src/workspaces/host.ts';
import { AppServerClient } from '../../tui/src/app-server/client.ts';
import { WebSocketTransport } from '../../tui/src/app-server/websocket-transport.ts';

export interface LocalHostConfig {
  transportToken?: string;
  endpoint: string;
  roots: { id: string; cwd: string; displayName: string }[];
  picker: boolean;
  /** Operator-owned file outside the browser and rustX runtime root. */
  metadataFile: string;
}
type Registration = { id: string; location: string; displayName: string };
export class LocalWorkspaceHost implements ProductHostWorkspaces {
  private registrations: Registration[];
  private readonly roots: LocalHostConfig['roots'];
  private readonly config: LocalHostConfig;
  constructor(config: LocalHostConfig) {
    this.config = config;
    if (!isAbsolute(config.metadataFile)) throw new Error('Host metadataFile must be absolute');
    this.roots = config.roots.map(root => {
      if (!isAbsolute(root.cwd) || !statSync(root.cwd).isDirectory()) throw new Error('Host roots must be absolute existing directories');
      return { ...root, cwd: realpathSync(root.cwd) };
    });
    if (new Set(this.roots.map(root => root.id)).size !== this.roots.length || new Set(this.roots.map(root => root.cwd)).size !== this.roots.length) throw new Error('Duplicate Host location');
    this.registrations = existsSync(config.metadataFile) ? JSON.parse(readFileSync(config.metadataFile, 'utf8')) as Registration[]
      : this.roots.map(root => ({ id: randomUUID(), location: root.id, displayName: root.displayName }));
    if (!Array.isArray(this.registrations) || this.registrations.some(row => typeof row.id !== 'string' || typeof row.displayName !== 'string' || !this.roots.some(root => root.id === row.location))) throw new Error('Invalid Host registrations');
    this.commit(this.registrations);
  }
  private commit(rows: Registration[]) {
    const temporary = `${this.config.metadataFile}.${randomUUID()}.tmp`;
    writeFileSync(temporary, JSON.stringify(rows), { mode: 0o600 });
    renameSync(temporary, this.config.metadataFile);
    this.registrations = rows;
  }
  private registered(id: string) {
    const row = this.registrations.find(row => row.id === id);
    if (!row) throw new Error('Unknown Workspace registration');
    return row;
  }
  private cwd(location: string) {
    const root = this.roots.find(root => root.id === location);
    if (!root || realpathSync(root.cwd) !== root.cwd || !statSync(root.cwd).isDirectory()) throw new Error('Authorized location is unavailable');
    return root.cwd;
  }
  private route(endpoint: string) {
    if (!sameEndpoint(endpoint, this.config.endpoint)) throw new Error('Workspace Host belongs to a different rustX process');
  }
  async listWorkspaces(): Promise<WorkspaceCatalog> {
    return { endpoint: this.config.endpoint,
      workspaces: this.registrations.map(row => ({ ...row, displayPath: this.roots.find(root => root.id === row.location)!.cwd })),
      picker: this.config.picker ? { kind: 'configured', locations: this.roots.map(root => ({ id: root.id, displayName: root.displayName })) }
        : { kind: 'unavailable', reason: 'This Host has no directory picker. Ask its operator to configure authorized roots.' } };
  }
  async adoptWorkspace(location: string) {
    if (!this.config.picker) throw new Error('Directory picker unavailable');
    this.cwd(location);
    if (this.registrations.some(row => row.location === location)) return;
    this.commit([...this.registrations, { id: randomUUID(), location, displayName: this.roots.find(root => root.id === location)!.displayName }]);
  }
  async renameWorkspace(id: string, displayName: string) {
    this.registered(id);
    if (!displayName.trim() || displayName.length > 120) throw new Error('Workspace name must contain 1–120 characters');
    this.commit(this.registrations.map(row => row.id === id ? { ...row, displayName: displayName.trim() } : row));
  }
  async reorderWorkspace(id: string, before?: string) {
    const row = this.registered(id);
    if (before === id) return;
    if (before) this.registered(before);
    const rows = this.registrations.filter(row => row.id !== id);
    rows.splice(before ? rows.findIndex(row => row.id === before) : rows.length, 0, row);
    this.commit(rows);
  }
  /** Per-registration authority lane. A native configuration mutation holds its
   * registration's lane from first resolution to final reread, so removal can
   * never commit while that registration still has a native write in flight.
   * rename/reorder/adopt commit synchronously between awaits and the reads are
   * pure, so they need no lane. */
  private lanes = new Map<string, Promise<unknown>>();
  private lane<T>(id: string, operation: () => Promise<T>): Promise<T> {
    const previous = this.lanes.get(id) ?? Promise.resolve();
    const work = previous.catch(() => {}).then(operation);
    this.lanes.set(id, work);
    return work.finally(() => { if (this.lanes.get(id) === work) this.lanes.delete(id); });
  }
  async removeWorkspace(id: string) {
    return this.lane(id, async () => { this.registered(id); this.commit(this.registrations.filter(row => row.id !== id)); });
  }
  async resolveWorkspace(id: string, endpoint: string) { this.route(endpoint); return { cwd: this.cwd(this.registered(id).location) }; }
  async configureWorkspace(id: string, endpoint: string, operation: WorkspaceConfigurationOperation): Promise<WorkspaceConfigurationResult> {
    return this.lane(id, async () => {
      const { cwd } = await this.resolveWorkspace(id, endpoint);
      if (!this.config.transportToken) throw new Error('Workspace Host has no native configuration connection');
      const transport = await WebSocketTransport.connect({ endpoint: this.config.endpoint, token: this.config.transportToken });
      const client = await AppServerClient.initialize({ transport, identity: { name: 'rustx-product-host', version: '0.1.0' } }).catch(error => { transport.close(); throw error; });
      try {
        // Resolve again after asynchronous admission, immediately before submission.
        if ((await this.resolveWorkspace(id, endpoint)).cwd !== cwd) throw new Error('Workspace authority changed');
        const target = { kind: 'workspace' as const, directory: cwd };
        if (operation.kind === 'write') {
          // The native write is the linearization point. Once it acknowledges,
          // the mutation is committed and nothing below may turn it into a
          // rejection; the authoritative reread is a separate, independent fact.
          const acknowledgement = await client.call('configuration/sourceWrite', { target, expected_revision: operation.expected_revision, mutation: operation.mutation }, 'source_settings');
          let reread: WorkspaceConfigurationReread;
          try {
            const result = await client.call('configuration/sourcesRead', { target }, 'source_settings');
            reread = (await this.resolveWorkspace(id, endpoint)).cwd === cwd
              ? { status: 'observed', projection: result.projection }
              : { status: 'failed', error: 'Workspace authority changed during the authoritative reread' };
          } catch (error) { reread = { status: 'failed', error: error instanceof Error ? error.message : String(error) }; }
          return { kind: 'write', commit: { acknowledgement: acknowledgement.projection, reread } };
        }
        if (operation.kind === 'reconcile') await client.call('configuration/reconcile', { target }, 'configuration_application');
        else if (operation.kind !== 'read') throw new Error('Unknown configuration operation');
        const result = await client.call('configuration/sourcesRead', { target }, 'source_settings');
        if ((await this.resolveWorkspace(id, endpoint)).cwd !== cwd) throw new Error('Workspace authority changed');
        return { kind: operation.kind, projection: result.projection };
      } finally { await client.close(); }
    });
  }
  async classifyLocations(cwds: readonly string[], endpoint: string): Promise<SessionLocation[]> {
    this.route(endpoint);
    if (cwds.length > 32) throw new Error('Host classification is bounded to 32 summaries');
    // Exact canonical root membership is deliberate. No recursive allocation or
    // filesystem sandbox is implied; missing directories remain ungrouped.
    const roots = new Map<string, string>();
    for (const root of this.roots) { try { roots.set(this.cwd(root.id), root.id); } catch { /* unavailable */ } }
    return cwds.map(cwd => {
      try {
        const location = isAbsolute(cwd) ? roots.get(realpathSync(cwd)) : undefined;
        if (location === undefined) return { authorized: false };
        const workspaceId = this.registrations.find(row => row.location === location)?.id;
        return { authorized: true, ...(workspaceId ? { workspaceId } : {}) };
      } catch { return { authorized: false }; }
    });
  }
}

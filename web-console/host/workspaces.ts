/** Local trusted Product Host. This module runs in Node, never in the browser. */
import { readFileSync, writeFileSync, renameSync, realpathSync, statSync, existsSync } from 'node:fs';
import { isAbsolute } from 'node:path';
import { randomUUID } from 'node:crypto';
import type { ProductHostWorkspaces, WorkspaceCatalog } from '../src/workspaces/host.ts';

export interface LocalHostConfig {
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
  constructor(private readonly config: LocalHostConfig) {
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
    if (new URL(endpoint).href !== new URL(this.config.endpoint).href) throw new Error('Workspace Host belongs to a different rustX process');
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
  async removeWorkspace(id: string) { this.registered(id); this.commit(this.registrations.filter(row => row.id !== id)); }
  async resolveWorkspace(id: string, endpoint: string) { this.route(endpoint); return { cwd: this.cwd(this.registered(id).location) }; }
  async groupSessions(cwds: readonly string[], endpoint: string) {
    this.route(endpoint);
    if (cwds.length > 32) throw new Error('Host grouping is bounded to 32 summaries');
    // Exact canonical root membership is deliberate. No recursive allocation or
    // filesystem sandbox is implied; missing directories remain ungrouped.
    const roots = new Map<string, string>();
    for (const row of this.registrations) { try { roots.set(this.cwd(row.location), row.id); } catch { /* unavailable */ } }
    return cwds.map(cwd => { try { return isAbsolute(cwd) ? roots.get(realpathSync(cwd)) ?? null : null; } catch { return null; } });
  }
}

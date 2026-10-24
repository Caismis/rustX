import type { WorkbenchCall } from '../WorkbenchFiles';
import { PreviewDemand } from '../../../client/session-files';
import type { PreviewObservation } from '../../../client/preview-content';
import { settlementFailureKind } from '../../../workspaces/host';
import { FILE_PREVIEW_LIMITS, isFileData, type FileBytes } from '../../../../shared/file-preview';
type Resource = { version?: string; revision: number; pending: boolean; checking?: Promise<void>; observation?: PreviewObservation; ready: Promise<void>; bytes?: Promise<FileBytes> };
/** Renderer dependencies retain existing Workspace authority and the shared
 * 32-observation cap. A serial lane per body fits the native two-read budget. */
export class DocumentDependencies {
  private readonly resources = new Map<string, Resource>();
  private readonly lifetime = new AbortController();
  private readonly demand = new PreviewDemand(1, 64, 'File settlement unavailable', cause => settlementFailureKind(cause) !== undefined);
  private bytes = 0;
  private invalidated = false;
  constructor(private readonly call: WorkbenchCall, private readonly changed: () => void, private readonly sourceBytes: number) {}
  private current() { return !this.lifetime.signal.aborted && !this.invalidated; }
  private retire() { if (this.current()) { this.invalidated = true; this.lifetime.abort(); this.changed(); } }
  private resource(path: string): Resource {
    let resource = this.resources.get(path); if (resource) return resource;
    if (this.resources.size >= 64) throw new Error('capacity');
    resource = { revision: 0, pending: false, ready: Promise.resolve() }; this.resources.set(path, resource);
    const record = resource;
    if (this.call.observe) record.ready = this.demand.run(this.lifetime.signal, async () => {
      const observation = await this.call.observe!(path, () => this.invalidate(path, record), this.lifetime.signal);
      record.observation = observation; record.version = observation.version;
      void observation.done.then(() => this.retire());
      if (!this.current()) await observation.close();
    });
    return record;
  }
  private invalidate(path: string, record: Resource) {
    if (!this.current()) return;
    record.revision++; record.pending = true;
    if (record.checking) return;
    record.checking = (async () => {
      await record.ready;
      while (record.pending && this.current()) {
        record.pending = false; const revision = record.revision;
        const result = await this.demand.run(this.lifetime.signal, () => this.call({ kind: 'preview', path, read: { kind: 'bytes', range: { offset: 0, length: 1 } } }, this.lifetime.signal));
        if (!this.current() || revision !== record.revision) continue;
        if (!result.preview?.version) throw new Error('malformed');
        if (record.version !== result.preview.version) { record.version = result.preview.version; this.retire(); return; }
      }
    })().catch(() => this.retire()).finally(() => { record.checking = undefined; });
  }
  async read(path: string, signal: AbortSignal): Promise<FileBytes> {
    signal.throwIfAborted(); const record = this.resource(path);
    await record.ready;
    signal.throwIfAborted();
    if (!this.current() || this.invalidated) throw new Error('obsolete');
    record.bytes ??= this.demand.run(this.lifetime.signal, async () => {
      const result = await this.call({ kind: 'preview', path, read: { kind: 'bytes' } }, this.lifetime.signal);
      if (!this.current() || this.invalidated) throw new Error('obsolete');
      const bytes = result.preview;
      if (!bytes || !isFileData(bytes.data) || bytes.offset !== 0 || !bytes.eof || bytes.bytes !== bytes.data.byteLength) throw new Error('malformed');
      if (bytes.data.byteLength + this.bytes + this.sourceBytes > FILE_PREVIEW_LIMITS.fileBytes) throw new Error('too_large');
      this.bytes += bytes.data.byteLength; record.version = bytes.version; return bytes;
    });
    const bytes = await record.bytes;
    await record.checking; signal.throwIfAborted();
    if (!this.current() || this.invalidated || record.version !== bytes.version) throw new Error('obsolete');
    return bytes;
  }
  dispose() {
    this.lifetime.abort();
    for (const record of this.resources.values()) { void record.ready.then(() => record.observation?.close()).catch(() => {}); }
    this.resources.clear();
  }
}

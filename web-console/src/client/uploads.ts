import type { UploadPolicy, UploadReceipt, UploadedFile, UploadOutcome } from '../../../protocol/app-server/v34';
import { uploadOperationId } from '../../../protocol/app-server/upload';

export type IntakeInput = { file: File; directory?: boolean };
export type IntakeReason = 'count' | 'size' | 'batch' | 'name' | 'directory' | 'policy';
export type IntakeFile = { id: string; file: File; operation: string; status: 'draft' | 'queued' | 'uploading' | 'ready' | 'rejected' | 'failed' | 'uncertain' | 'reconciling'; reason?: IntakeReason; receipt?: UploadReceipt; error?: string };
export type UploadPort = { upload(files: readonly File[], operation: string): Promise<UploadedFile[]>; status(operation: string): Promise<UploadOutcome> };
const safeName = (name: string) => !!name && new TextEncoder().encode(name).length <= 255 && !/[\p{Cc}/\\:*?"<>|]/u.test(name) && !/[. ]$/.test(name) && !/^(CON|PRN|AUX|NUL|COM\d|LPT\d)(\.|$)/i.test(name);

/** One normalization path, with bounded visible rejection and no silent truncation. */
export function intake(inputs: readonly IntakeInput[], existing: readonly IntakeFile[], policy?: UploadPolicy): IntakeFile[] {
  if (!inputs.length) return [];
  const row = (file: File, reason?: IntakeReason): IntakeFile => ({ id: uploadOperationId(), operation: uploadOperationId(), file, status: reason ? 'rejected' : 'draft', reason });
  if (!policy || inputs.length + existing.length > policy.max_files) return [row(new File([], `${inputs.length} files`), policy ? 'count' : 'policy')];
  let total = existing.reduce((sum, item) => sum + (item.status === 'rejected' ? 0 : item.file.size), 0);
  return inputs.map(({ file, directory }) => {
    const reason = directory ? 'directory' : !safeName(file.name) ? 'name' : file.size > policy.max_file_bytes ? 'size' : total + file.size > policy.max_batch_bytes ? 'batch' : undefined;
    if (!reason) total += file.size;
    return row(file, reason);
  });
}
export function transferInputs(data: DataTransfer): IntakeInput[] {
  const items = Array.from(data.items ?? []).filter(item => item.kind === 'file');
  if (items.length) return items.map(item => {
    const entry = item.webkitGetAsEntry?.();
    return { file: item.getAsFile() ?? new File([], entry?.name ?? ''), directory: entry?.isDirectory ?? false };
  });
  return Array.from(data.files).map(file => ({ file }));
}
export function pasteText(value: string, start: number, end: number, text: string) {
  return { value: value.slice(0, start) + text + value.slice(end), caret: start + text.length };
}

/** Client-lifetime presentation owner. No storage, replay, or native rollback. */
export class AttachmentIntake {
  private files: readonly IntakeFile[] = [];
  private binding?: string;
  private revision = 0;
  bind(binding: string) {
    if (this.binding === binding) return;
    this.binding = binding; this.revision++;
    this.files = this.files.map(file => ['queued', 'uploading', 'reconciling'].includes(file.status) ? { ...file, status: 'uncertain' } : file);
  }
  private listeners = new Set<() => void>();
  private actions = new Map<string, { revision: number }>();
  snapshot = () => this.files;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private publish(files: readonly IntakeFile[]) { this.files = files; for (const listener of this.listeners) listener(); }
  private update(id: string, patch: Partial<IntakeFile>) { this.publish(this.files.map(file => file.id === id ? { ...file, ...patch } : file)); }
  remove(id: string) { this.publish(this.files.filter(file => file.id !== id)); }
  clear() { this.publish([]); }
  add(inputs: readonly IntakeInput[], policy: UploadPolicy | undefined, port?: UploadPort) {
    const added = intake(inputs, this.files, policy).map(file => port && file.status === 'draft' ? { ...file, status: 'queued' as const } : file);
    this.publish([...this.files.filter(file => !added.some(row => row.reason && row.reason === file.reason && ['count', 'policy'].includes(row.reason))), ...added]);
    if (port) {
      const revision = this.revision;
      // Sequence this finite selection once. Rebinding stops undispatched work;
      // observation/reconnect never restarts it or repeats an operation.
      void (async () => { for (const file of added) {
        if (revision !== this.revision) break;
        if (file.status === 'queued') await this.upload(file.id, port);
      } })();
    }
  }
  async reconcile(id: string, port: UploadPort) {
    const file = this.files.find(file => file.id === id);
    if (!file || file.status !== 'uncertain' || this.actions.get(id)?.revision === this.revision) return;
    const action = { revision: this.revision };
    const { revision } = action;
    this.actions.set(id, action); this.update(id, { status: 'reconciling' });
    try { const outcome = await port.status(file.operation); if (revision === this.revision) this.apply(id, outcome); }
    catch (error) { if (revision === this.revision) this.update(id, { status: 'uncertain', error: String(error) }); }
    finally { if (this.actions.get(id) === action) this.actions.delete(id); }
  }
  private apply(id: string, outcome: UploadOutcome) {
    if (outcome.state === 'ready' && outcome.files.length === 1) this.update(id, { status: 'ready', receipt: outcome.files[0].receipt, error: undefined });
    else this.update(id, { status: outcome.state === 'absent' || outcome.state === 'failed' ? 'failed' : 'uncertain' });
  }
  async upload(id: string, port: UploadPort) {
    const file = this.files.find(file => file.id === id);
    if (!file || !['draft', 'queued', 'failed'].includes(file.status) || this.actions.get(id)?.revision === this.revision) return;
    const action = { revision: this.revision };
    const { revision } = action;
    this.actions.set(id, action);
    const operation = file.status === 'failed' ? uploadOperationId() : file.operation;
    this.update(id, { status: 'uploading', operation, error: undefined });
    try {
      const result = await port.upload([file.file], operation);
      if (result.length !== 1) throw new Error('Invalid upload result');
      if (revision === this.revision) this.update(id, { status: 'ready', receipt: result[0].receipt });
    } catch (error) {
      // All failures require native evidence before Retry can become legal.
      try { const outcome = await port.status(operation); if (revision === this.revision) this.apply(id, outcome); }
      catch { if (revision === this.revision) this.update(id, { status: 'uncertain', error: String(error) }); }
    } finally { if (this.actions.get(id) === action) this.actions.delete(id); }
  }
}
export class AttachmentIntakes {
  private owners = new Map<string, AttachmentIntake>();
  owner(key: string, binding = key) { let owner = this.owners.get(key); if (!owner) { owner = new AttachmentIntake(); this.owners.set(key, owner); } owner.bind(binding); return owner; }
}

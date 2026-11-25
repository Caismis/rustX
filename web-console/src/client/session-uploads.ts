import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { uploadOperationId, type transferUpload } from '../../../protocol/app-server/upload';
import { RequestNotDispatched, RpcFailure, isOutcomeUncertain } from './errors';
import { validateRaster } from './raster';
import { UploadFailure } from './uploads';
import type { AttachmentTarget, UploadReceipt, UploadedFile, UploadedFileRef } from '../../../protocol/app-server/v81';
import type { ClientView, ConnectionRequest, SessionView } from './contracts';

/** What native uploads read from and write to their client. */
export interface SessionUploadPort {
  request: ConnectionRequest;
  state(): Readonly<ClientView>;
  current(generation: number): boolean;
  publish(patch: Pick<ClientView, 'uncertain'>): void;
  target(id: string): AttachmentTarget;
  /** The exact attachment proof still controls the Session. */
  controls(id: string, admission: SessionView['attachmentObservation']): boolean;
  carrier: typeof transferUpload;
}

const MAX_PENDING = 32;
const MAX_RECORDS_PER_SESSION = 64;
const MAX_THUMBNAIL_BYTES = 16 * 1024 * 1024;
const RASTER_MIME = /^image\/(png|jpe?g|gif|webp|avif|bmp)$/i;

export interface UploadedAttachmentMetadata {
  readonly name: string;
  readonly bytes: number;
  readonly imageFile?: File;
}

export type UploadAttachmentKey =
  | { readonly receipt: UploadReceipt }
  | { readonly file: UploadedFileRef };

interface UploadAuthority {
  readonly generation: number;
  readonly target: AttachmentTarget;
  readonly intentRevision: number;
}

interface PendingUploadEntry {
  readonly sessionId: string;
  readonly authority: UploadAuthority;
  readonly files: readonly File[];
}

interface StoredUploadRecord {
  readonly receiptKey: string;
  readonly fileKey: string;
  readonly authority: UploadAuthority;
  readonly metadata: UploadedAttachmentMetadata;
  seenQueued: boolean;
}

interface SessionAttachmentStore {
  readonly records: StoredUploadRecord[];
  readonly byReceipt: Map<string, StoredUploadRecord>;
  readonly byFile: Map<string, StoredUploadRecord>;
}

export const uploadReceiptKey = (receipt: UploadReceipt) => `${receipt.session_id}\0${receipt.batch_id}\0${receipt.token}`;
export const uploadFileRefKey = (file: UploadedFileRef) => `${file.batch_id}\0${file.name}`;

async function validatedImageFile(file: File): Promise<File | undefined> {
  if (file.size <= 0 || file.size > MAX_THUMBNAIL_BYTES) return undefined;
  if (file.type && !RASTER_MIME.test(file.type)) return undefined;
  try {
    validateRaster(new Uint8Array(await file.arrayBuffer()));
    return file;
  } catch {
    return undefined;
  }
}

/** One native upload operation of an attached Session: exact prepare, a
 * single carrier transfer that is never retransmitted, and the authoritative
 * status read that settles it or leaves it uncertain. Attachment intake owns
 * the files before and after; this owns only the wire operation and bounded
 * live-authority metadata for acknowledged uploads. */
export class SessionUploads {
  private readonly pending = new Map<string, PendingUploadEntry>();
  private readonly sessions = new Map<string, SessionAttachmentStore>();
  constructor(private readonly port: SessionUploadPort) {}

  lookupAttachment(id: string, key: UploadAttachmentKey): UploadedAttachmentMetadata | undefined {
    const store = this.sessions.get(id);
    if (!store) return undefined;
    const record = 'receipt' in key
      ? store.byReceipt.get(uploadReceiptKey(key.receipt))
      : store.byFile.get(uploadFileRefKey(key.file));
    if (!record || !this.isAuthorityAlive(id, record.authority)) return undefined;
    return record.metadata;
  }

  pruneAttachments(id: string, activeReceipts: ReadonlySet<string>, activeFiles: ReadonlySet<string>) {
    const store = this.sessions.get(id);
    if (!store) return;
    for (let index = store.records.length - 1; index >= 0; index--) {
      const record = store.records[index];
      const inUse = activeReceipts.has(record.receiptKey) || activeFiles.has(record.fileKey);
      if (inUse) {
        record.seenQueued = true;
      } else if (record.seenQueued || !this.isAuthorityAlive(id, record.authority)) {
        this.deleteRecord(store, index);
      }
    }
    if (store.records.length === 0) this.sessions.delete(id);
  }

  retire(id: string) {
    this.sessions.delete(id);
    for (const [key, entry] of this.pending) {
      if (entry.sessionId === id) this.pending.delete(key);
    }
  }

  retireAll() {
    this.pending.clear();
    this.sessions.clear();
  }

  private isAuthorityAlive(id: string, authority: UploadAuthority): boolean {
    const view = this.port.state().views[id];
    const admission = view?.attachmentObservation;
    return this.port.current(authority.generation)
      && !!admission
      && admission.generation === authority.generation
      && admission.intentRevision === authority.intentRevision
      && sameTarget(admission.target, authority.target)
      && sameTarget(view?.target, authority.target);
  }

  private pendingKey(id: string, operationId: string) {
    return `${id}\0${operationId}`;
  }

  private rememberPending(id: string, operationId: string, entry: PendingUploadEntry) {
    const key = this.pendingKey(id, operationId);
    if (!this.pending.has(key) && this.pending.size >= MAX_PENDING) {
      const oldest = this.pending.keys().next().value;
      if (oldest !== undefined) this.pending.delete(oldest);
    }
    this.pending.set(key, entry);
  }

  private deleteRecord(store: SessionAttachmentStore, index: number) {
    const [removed] = store.records.splice(index, 1);
    if (!removed) return;
    if (store.byReceipt.get(removed.receiptKey) === removed) store.byReceipt.delete(removed.receiptKey);
    if (store.byFile.get(removed.fileKey) === removed) store.byFile.delete(removed.fileKey);
  }

  private recordUploaded(id: string, authority: UploadAuthority, uploaded: UploadedFile, file: File, imageFile?: File) {
    if (!this.isAuthorityAlive(id, authority)) return;
    let store = this.sessions.get(id);
    if (!store) {
      store = { records: [], byReceipt: new Map(), byFile: new Map() };
      this.sessions.set(id, store);
    }
    const receiptKey = uploadReceiptKey(uploaded.receipt);
    const fileKey = uploadFileRefKey(uploaded.file);
    const existingIndex = store.records.findIndex(item => item.receiptKey === receiptKey || item.fileKey === fileKey);
    if (existingIndex >= 0) this.deleteRecord(store, existingIndex);
    while (store.records.length >= MAX_RECORDS_PER_SESSION) this.deleteRecord(store, 0);
    const record: StoredUploadRecord = {
      receiptKey,
      fileKey,
      authority,
      metadata: {
        name: uploaded.file.name || file.name,
        bytes: file.size,
        ...(imageFile ? { imageFile } : {}),
      },
      seenQueued: false,
    };
    store.records.push(record);
    store.byReceipt.set(receiptKey, record);
    store.byFile.set(fileKey, record);
  }

  async uploadStatus(id: string, operationId: string) {
    const target = this.port.target(id), generation = this.port.state().generation;
    const { outcome } = await this.port.request({ method: 'session/uploadStatus', params: { target, operation_id: operationId } }, 'upload_status');
    const key = this.pendingKey(id, operationId);
    const pending = this.pending.get(key);
    if (outcome.state !== 'unresolved') this.pending.delete(key);
    if (outcome.state === 'ready' && pending && this.isAuthorityAlive(id, pending.authority)) {
      for (let index = 0; index < outcome.files.length; index++) {
        const uploaded = outcome.files[index];
        const file = pending.files[index];
        if (!file) continue;
        const imageFile = await validatedImageFile(file);
        this.recordUploaded(id, pending.authority, uploaded, file, imageFile);
      }
    }
    if (outcome.state !== 'unresolved' && this.port.current(generation) && sameTarget(this.port.state().views[id]?.target, target)
      && this.port.state().uncertain.some(item => item.sessionId === id && item.uploadOperationId === operationId)) {
      this.port.publish({ uncertain: this.port.state().uncertain.filter(item => item.sessionId !== id || item.uploadOperationId !== operationId) });
    }
    return outcome;
  }
  async upload(id: string, files: readonly File[], evidence?: { current: () => boolean; acknowledged: (files: UploadedFile[]) => void }, operationId = uploadOperationId()): Promise<UploadedFile[]> {
    let target: AttachmentTarget;
    try { target = this.port.target(id); } catch (error) { throw new UploadFailure('failed', error); }
    const policy = this.port.state().capabilities?.upload_policy;
    if (!policy) throw new UploadFailure('failed', 'Upload policy unavailable');
    if (files.length !== 1 || files.some(file => !Number.isSafeInteger(file.size) || file.size < 0)) throw new UploadFailure('failed', 'Invalid upload selection');
    const admission = this.port.state().views[id]?.attachmentObservation;
    const current = () => (!evidence || evidence.current()) && this.port.controls(id, admission) && sameTarget(admission?.target, target);
    if (!current() || !admission) throw new UploadFailure('failed', 'Upload authority changed');
    const authority: UploadAuthority = {
      generation: admission.generation,
      target: admission.target,
      intentRevision: admission.intentRevision,
    };
    this.rememberPending(id, operationId, { sessionId: id, authority, files: [...files] });
    const read = async () => {
      const outcome = await this.uploadStatus(id, operationId).catch(error => { throw new UploadFailure('uncertain', error); });
      if (outcome.state !== 'ready') throw new UploadFailure(outcome.state === 'unresolved' ? 'uncertain' : 'failed');
      evidence?.acknowledged(outcome.files);
      if (!current()) throw new UploadFailure('uncertain', 'Upload belongs to a retired view');
      return outcome.files;
    };
    let transfer;
    try {
      ({ transfer } = await this.port.request({ method: 'session/uploadPrepare', params: { target, operation_id: operationId, files: [{ name: files[0].name, size: files[0].size }] } }, 'upload_prepared', undefined, current));
    } catch (error) {
      if (error instanceof RequestNotDispatched) {
        this.pending.delete(this.pendingKey(id, operationId));
        throw new UploadFailure('failed', error);
      }
      // A server refusal published no new preparation, but the exact operation
      // may already exist. Read its authority before permitting a fresh Retry.
      if (error instanceof RpcFailure && !isOutcomeUncertain(error) && current()) return read();
      throw new UploadFailure('uncertain', error);
    }
    try {
      if (!current()) throw new Error('Upload authority changed');
      await this.port.carrier(transfer, files, policy, this.port.state().endpoint);
    } catch {
      // No retransmission: the authoritative read below repairs carrier loss.
    }
    if (!current()) throw new UploadFailure('uncertain', 'Upload authority changed');
    return read();
  }

}

import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { uploadOperationId, type transferUpload } from '../../../protocol/app-server/upload';
import { RequestNotDispatched, RpcFailure, isOutcomeUncertain } from './errors';
import { UploadFailure } from './uploads';
import type { AttachmentTarget, UploadedFile } from '../../../protocol/app-server/v57';
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

/** One native upload operation of an attached Session: exact prepare, a
 * single carrier transfer that is never retransmitted, and the authoritative
 * status read that settles it or leaves it uncertain. Attachment intake owns
 * the files before and after; this owns only the wire operation. */
export class SessionUploads {
  constructor(private readonly port: SessionUploadPort) {}
  async uploadStatus(id: string, operationId: string) {
    const target = this.port.target(id), generation = this.port.state().generation;
    const { outcome } = await this.port.request({ method: 'session/uploadStatus', params: { target, operation_id: operationId } }, 'upload_status');
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
    if (!current()) throw new UploadFailure('failed', 'Upload authority changed');
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
      if (error instanceof RequestNotDispatched) throw new UploadFailure('failed', error);
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

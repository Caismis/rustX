import { UploadFailure, uploadFailureState } from '../../client/uploads';
import { uploadOperationId } from '../../../../protocol/app-server/upload';
import { isOutcomeUncertain } from '../../client/app-server';
import { WorkspaceHostError } from '../../workspaces/host';
import type { SessionModelConfig, UploadReceipt, UploadOutcome } from '../../../../protocol/app-server/v36';

export interface FirstDraft { workspaceId: string; text: string; files: readonly File[]; attachmentIds?: readonly string[]; model?: SessionModelConfig }
export interface CreatedSession { id: string; node: string; conversation: string; diagnostic?: string }
export interface FirstSubmitPort {
  current(): boolean;
  create(draft: FirstDraft, acknowledged: (session: CreatedSession) => void): Promise<CreatedSession>;
  /** Called once, after Session ownership is published, before continuation. */
  handoff(session: CreatedSession): void;
  attach(session: CreatedSession): Promise<void>;
  upload(session: CreatedSession, file: File, acknowledged: (receipt: UploadReceipt) => void, operation?: string): Promise<UploadReceipt>;
  status?(session: CreatedSession, operation: string): Promise<UploadOutcome>;
  send(session: CreatedSession, draft: FirstDraft, receipts: readonly UploadReceipt[], acknowledged: () => void): Promise<void>;
}
export type FirstSubmitPhase = 'creating' | 'attaching' | 'uploading' | 'admitting' | 'admitted' | 'rejected' | 'failed' | 'uncertain' | 'discarded' | 'paused';
export interface FirstSubmission {
  readonly binding: string;
  readonly authority: number;
  readonly draft: FirstDraft;
  readonly session?: CreatedSession;
  readonly receipts: readonly UploadReceipt[];
  readonly operations: readonly string[];
  readonly attachmentIds: readonly string[];
  readonly phase: FirstSubmitPhase;
  readonly failedPhase?: FirstSubmitPhase;
  readonly uploadIndex?: number;
  readonly error?: unknown;
}
const terminal = (phase: FirstSubmitPhase) => ['admitted', 'rejected', 'failed', 'uncertain', 'discarded', 'paused'].includes(phase);
const uncertain = (error: unknown) => isOutcomeUncertain(error) || error instanceof WorkspaceHostError && error.uncertain;
function live(port: FirstSubmitPort) { if (!port.current()) throw new Error('Authority changed. No operation was replayed.'); }

/** One client-lifetime owner of first submissions, indexed by the original draft
 * binding until acknowledgement, then by exact native Session. This is local
 * intent, never canonical history or a durable queue. Render only subscribes.
 * No cancellation of promises: their acknowledged facts must still be captured.
 * Fences prevent the *next* dispatch, never erase the previous acknowledgement. */
export class FirstSubmissions {
  private recovery = new Set<FirstSubmission>();
  private drafts = new Map<string, FirstSubmission>();
  private sessions = new Map<string, FirstSubmission>();
  private listeners = new Set<() => void>();
  private disposed = false;
  private authority = 0;
  private detached: readonly FirstSubmission[] = [];
  detachedSnapshot = () => this.detached;
  private refreshDetached() {
    const next = [...this.drafts.values()].filter(value => (value.authority !== this.authority || !value.session) && !['admitted', 'discarded'].includes(value.phase));
    if (next.length || this.detached.length) this.detached = next;
  }
  /** Replaced endpoints cannot expose an old Session operation under a reused ID. */
  retireAuthority() { this.authority++; this.refreshDetached(); for (const listener of this.listeners) listener(); }
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  draft = (binding: string) => this.drafts.get(`${this.authority}:${binding}`);
  session = (id: string) => this.sessions.get(`${this.authority}:${id}`);
  private publish(value: FirstSubmission) {
    if (this.disposed) return;
    this.drafts.set(`${value.authority}:${value.binding}`, value);
    if (value.session) this.sessions.set(`${value.authority}:${value.session.id}`, value);
    this.refreshDetached();
    for (const listener of this.listeners) listener();
  }
  /** Admission and explicit discard release File references. Failed/uncertain
   * intent remains inspectable. Discard is unavailable during an in-flight RPC. */
  discard(value: FirstSubmission) {
    if (!terminal(value.phase) || this.drafts.get(`${value.authority}:${value.binding}`) !== value) return;
    this.publish({ ...value, phase: 'discarded', draft: { ...value.draft, text: '', files: [] }, receipts: [], error: undefined });
  }
  dispose() {
    this.disposed = true; this.recovery.clear(); this.drafts.clear(); this.sessions.clear(); this.detached = []; this.listeners.clear();
  }
  removeUpload(value: FirstSubmission, id: string) {
    if (!value.session || !['failed', 'uncertain', 'paused'].includes(value.phase)
      || value.phase !== 'paused' && value.failedPhase !== 'uploading' || this.session(value.session.id) !== value) return;
    const index = value.attachmentIds.indexOf(id); if (index < 0) return;
    const resolved = value.phase === 'paused' || index === value.uploadIndex;
    this.publish({ ...value, phase: resolved ? 'paused' : value.phase, failedPhase: resolved ? undefined : value.failedPhase, uploadIndex: resolved ? undefined : value.uploadIndex! - (index < value.uploadIndex! ? 1 : 0),
      draft: { ...value.draft, files: value.draft.files.filter((_, at) => at !== index) },
      attachmentIds: value.attachmentIds.filter((_, at) => at !== index),
      operations: value.operations.filter((_, at) => at !== index),
      receipts: value.receipts.filter((_, at) => at !== index) });
  }
  /** Recovery never creates a Session or admits a turn. Each action addresses
   * the retained exact operation and requires a separate Continue gesture. */
  async recoverUpload(value: FirstSubmission, port: FirstSubmitPort, retry: boolean) {
    if (!value.session || value.uploadIndex === undefined || value.failedPhase !== 'uploading'
      || value.phase !== (retry ? 'failed' : 'uncertain') || this.recovery.has(value)
      || this.session(value.session.id) !== value || !port.current()) return;
    this.recovery.add(value);
    const index = value.uploadIndex;
    let next = { ...value, phase: 'uploading' as FirstSubmitPhase };
    this.publish(next);
    try {
      if (retry) {
        const operations = [...value.operations]; operations[index] = uploadOperationId();
        next = { ...next, operations }; this.publish(next);
        let acknowledged = false;
        const capture = (receipt: UploadReceipt) => { if (!acknowledged) { acknowledged = true; next = { ...next, receipts: [...next.receipts, receipt] }; this.publish(next); } };
        const receipt = await port.upload(value.session, value.draft.files[index], capture, operations[index]);
        capture(receipt);
        this.publish({ ...next, phase: 'paused', failedPhase: undefined });
      } else {
        const outcome = await port.status?.(value.session, value.operations[index]);
        if (outcome?.state === 'ready' && outcome.files.length === 1) this.publish({ ...next, receipts: [...value.receipts, outcome.files[0].receipt], phase: 'paused', failedPhase: undefined });
        else this.publish({ ...next, phase: outcome?.state === 'absent' || outcome?.state === 'failed' ? 'failed' : 'uncertain' });
      }
    } catch (error) { this.publish({ ...next, phase: next.receipts.length > index ? 'paused' : uploadFailureState(error), failedPhase: next.receipts.length > index ? undefined : 'uploading', error }); }
    finally { this.recovery.delete(value); }
  }
  async continueUploads(value: FirstSubmission, port: FirstSubmitPort) {
    if (!value.session || value.phase !== 'paused' || this.session(value.session.id) !== value || !port.current()) return;
    let next = value;
    const update = (patch: Partial<FirstSubmission>) => { next = { ...next, ...patch }; this.publish(next); };
    try {
      for (let index = next.receipts.length; index < next.draft.files.length; index++) {
        update({ phase: 'uploading', uploadIndex: index });
        if (!port.current()) throw new UploadFailure('failed', 'Upload not dispatched');
        let acknowledged = false;
        const capture = (receipt: UploadReceipt) => { if (!acknowledged) { acknowledged = true; update({ receipts: [...next.receipts, receipt] }); } };
        const receipt = await port.upload(next.session!, next.draft.files[index], capture, next.operations[index]);
        capture(receipt);
      }
      live(port); update({ phase: 'admitting' });
      await port.send(next.session!, next.draft, next.receipts, () => update({ phase: 'admitted', receipts: [], draft: { ...next.draft, text: '', files: [] } }));
      update({ phase: 'admitted', receipts: [], draft: { ...next.draft, text: '', files: [] } });
    } catch (error) { if (next.phase !== 'admitted') { const captured = next.phase === 'uploading' && next.uploadIndex !== undefined && next.receipts.length > next.uploadIndex; update({ phase: captured ? 'paused' : next.phase === 'uploading' ? uploadFailureState(error) : uncertain(error) ? 'uncertain' : 'failed', failedPhase: captured ? undefined : next.phase, error }); } }
  }
  async submit(binding: string, draft: FirstDraft, port: FirstSubmitPort, sealed?: () => void, rejected?: (files: readonly File[], ids: readonly string[]) => boolean): Promise<boolean> {
    const previous = this.drafts.get(`${this.authority}:${binding}`);
    if (this.disposed || previous && !['rejected', 'discarded'].includes(previous.phase) || !port.current()
      || !draft.workspaceId || !draft.text.trim() && !draft.files.length) return false;
    // The accepted gesture transfers File ownership from intake to this owner.
    const authority = this.authority;
    let value: FirstSubmission = { authority, binding, draft: { ...draft, files: [...draft.files] }, receipts: [], operations: draft.files.map(() => uploadOperationId()), attachmentIds: draft.attachmentIds ?? draft.files.map(() => uploadOperationId()), phase: 'creating' };
    const update = (patch: Partial<FirstSubmission>) => { value = { ...value, ...patch }; this.publish(value); };
    const current = () => { if (this.disposed) throw new Error('Client disposed'); live(port); };
    update({});
    sealed?.();
    try {
      let handedOff = false;
      let handoffError: unknown;
      const committed = (session: CreatedSession) => {
        if (handedOff) return;
        // Capture and transfer synchronously at the decoded ACK, before another
        // socket/navigation event can retire the draft component.
        update({ session, phase: 'attaching' });
        handedOff = true;
        try { if (this.disposed) throw new Error('Client disposed'); port.handoff(session); }
        catch (error) { handoffError = error; }
      };
      const session = await port.create(value.draft, committed);
      committed(session);
      if (handoffError) throw handoffError;
      if (session.diagnostic) throw new Error(session.diagnostic);
      current();
      await port.attach(session);
      current();
      for (const [uploadIndex, file] of value.draft.files.entries()) {
        update({ phase: 'uploading', uploadIndex });
        try { current(); } catch (error) { throw new UploadFailure('failed', error); }
        let acknowledged = false;
        const capture = (receipt: UploadReceipt) => { if (!acknowledged) { acknowledged = true; update({ receipts: [...value.receipts, receipt] }); } };
        const receipt = await port.upload(session, file, capture, value.operations[uploadIndex]);
        capture(receipt);
        current();
      }
      update({ phase: 'admitting' }); current();
      const admitted = () => update({ phase: 'admitted', receipts: [], draft: { ...value.draft, text: '', files: [] } });
      await port.send(session, value.draft, value.receipts, admitted);
      // Successful admission remains success even if navigation retired meanwhile.
      update({ phase: 'admitted', receipts: [], draft: { ...value.draft, text: '', files: [] } });
      return true;
    } catch (error) {
      if (value.phase === 'admitted') return true;
      const captured = value.phase === 'uploading' && value.uploadIndex !== undefined && value.receipts.length > value.uploadIndex;
      const phase = captured ? 'paused' : value.phase === 'uploading' ? uploadFailureState(error) : uncertain(error) ? 'uncertain' : value.session ? 'failed' : 'rejected';
      const returned = phase === 'rejected' && rejected?.(value.draft.files, value.attachmentIds);
      update({ phase, failedPhase: captured ? undefined : value.phase, error, ...(returned ? { draft: { ...value.draft, files: [] } } : {}) });
      return false;
    }
  }
}

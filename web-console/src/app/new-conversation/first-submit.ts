import { isOutcomeUncertain } from '../../client/app-server';
import { WorkspaceHostError } from '../../workspaces/host';
import type { SessionModelConfig, UploadReceipt } from '../../../../protocol/app-server/v31';

export interface FirstDraft { workspaceId: string; text: string; files: readonly File[]; model?: SessionModelConfig }
export interface CreatedSession { id: string; node: string; conversation: string; diagnostic?: string }
export interface FirstSubmitPort {
  current(): boolean;
  create(draft: FirstDraft, acknowledged: (session: CreatedSession) => void): Promise<CreatedSession>;
  /** Called once, after Session ownership is published, before continuation. */
  handoff(session: CreatedSession): void;
  attach(session: CreatedSession): Promise<void>;
  upload(session: CreatedSession, file: File, acknowledged: (receipt: UploadReceipt) => void): Promise<UploadReceipt>;
  send(session: CreatedSession, draft: FirstDraft, receipts: readonly UploadReceipt[], acknowledged: () => void): Promise<void>;
}
export type FirstSubmitPhase = 'creating' | 'attaching' | 'uploading' | 'admitting' | 'admitted' | 'rejected' | 'failed' | 'uncertain' | 'discarded';
export interface FirstSubmission {
  readonly binding: string;
  readonly authority: number;
  readonly draft: FirstDraft;
  readonly session?: CreatedSession;
  readonly receipts: readonly UploadReceipt[];
  readonly phase: FirstSubmitPhase;
  readonly failedPhase?: FirstSubmitPhase;
  readonly uploadIndex?: number;
  readonly error?: unknown;
}
const terminal = (phase: FirstSubmitPhase) => ['admitted', 'rejected', 'failed', 'uncertain', 'discarded'].includes(phase);
const uncertain = (error: unknown) => isOutcomeUncertain(error) || error instanceof WorkspaceHostError && error.uncertain;
function live(port: FirstSubmitPort) { if (!port.current()) throw new Error('Authority changed. No operation was replayed.'); }

/** One client-lifetime owner of first submissions, indexed by the original draft
 * binding until acknowledgement, then by exact native Session. This is local
 * intent, never canonical history or a durable queue. Render only subscribes.
 * No cancellation of promises: their acknowledged facts must still be captured.
 * Fences prevent the *next* dispatch, never erase the previous acknowledgement. */
export class FirstSubmissions {
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
    this.disposed = true; this.drafts.clear(); this.sessions.clear(); this.detached = []; this.listeners.clear();
  }
  async submit(binding: string, draft: FirstDraft, port: FirstSubmitPort): Promise<boolean> {
    const previous = this.drafts.get(`${this.authority}:${binding}`);
    if (this.disposed || previous && !['rejected', 'discarded'].includes(previous.phase) || !port.current()
      || !draft.workspaceId || !draft.text.trim() && !draft.files.length) return false;
    // Copies seal order/intent at the gesture. File contents stay browser-owned.
    const authority = this.authority;
    let value: FirstSubmission = { authority, binding, draft: { ...draft, files: [...draft.files] }, receipts: [], phase: 'creating' };
    const update = (patch: Partial<FirstSubmission>) => { value = { ...value, ...patch }; this.publish(value); };
    const current = () => { if (this.disposed) throw new Error('Client disposed'); live(port); };
    update({});
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
        update({ phase: 'uploading', uploadIndex }); current();
        let acknowledged = false;
        const capture = (receipt: UploadReceipt) => { if (!acknowledged) { acknowledged = true; update({ receipts: [...value.receipts, receipt] }); } };
        const receipt = await port.upload(session, file, capture);
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
      update({ phase: uncertain(error) ? 'uncertain' : value.session ? 'failed' : 'rejected', failedPhase: value.phase, error });
      return false;
    }
  }
}

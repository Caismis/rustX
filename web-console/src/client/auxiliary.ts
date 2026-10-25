import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import type { AttachmentTarget, AuxiliarySegment, AuxiliarySubmissionReceipt, AuxiliaryResult } from '../../../protocol/app-server/v57';
import type { AppServerClient } from './app-server';
import { isOutcomeUncertain, RpcFailure } from './errors';
function diagnostic(error: unknown): string {
  if (error instanceof RpcFailure && error.error.data?.kind === 'auxiliary' && error.error.data.reason === 'input_too_large') return 'input_too_large';
  return String(error).slice(0, 512);
}

export type BtwPhase = 'submitting' | 'uncertain' | 'preparing' | 'running' | 'retrying' | 'settling' | 'complete' | 'cancelled' | 'failed' | 'incomplete' | 'unavailable' | 'notAdmitted';
export interface BtwQuestion {
  readonly correlation: string; readonly question: string; readonly phase: BtwPhase;
  readonly receipt?: AuxiliarySubmissionReceipt; readonly pendingCall?: string; readonly scope?: string; readonly text: string; readonly request?: string;
  readonly attempt?: number; readonly frontier: number; readonly complete: boolean; readonly settled?: boolean;
  readonly diagnostic?: string; readonly cancelling?: boolean;
}
interface Entry { question: BtwQuestion; target: AttachmentTarget; authority?: string; readFailed?: boolean; }
/** Sixteen bounded question slots per client, including hidden views. One slot
 * contains <=16 KiB input, <=8 KiB output and <=2 KiB identity/diagnostics.
 * At most one visible owner may poll; it holds one bounded response at a time.
 * Closing releases observation only. No path retries an uncertain submission. */
export class AuxiliaryQuestions {
  private entries = new Map<string, Entry>();
  private listeners = new Set<() => void>();
  private viewing?: { key: string; target: AttachmentTarget; current(): boolean; abort: AbortController };
  private reading?: Promise<void>;
  constructor(private client: AppServerClient) {
    client.onDispose(() => { this.viewing?.abort.abort(); this.viewing = undefined; this.entries.clear(); this.emit(); });
  }
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  snapshot = (id: string): BtwQuestion | undefined => this.entry(id)?.question;
  private key(id: string) { const state = this.client.getSnapshot(); return JSON.stringify([state.authorityId, id, state.views[id]?.target?.conversation_id]); }
  private entry(id: string) {
    const state = this.client.getSnapshot();
    if (state.views[id]?.target) return this.entries.get(this.key(id));
    // Transport loss removes the attachment, not the original correlation.
    // Never choose between different Conversation identities or authorities.
    const candidates = [...this.entries.values()].filter(entry => entry.authority === state.authorityId && entry.target.session_id === id);
    return candidates.length === 1 ? candidates[0] : undefined;
  }
  private emit() { for (const listener of this.listeners) listener(); }
  private set(entry: Entry, patch: Partial<BtwQuestion>) { entry.question = { ...entry.question, ...patch }; this.emit(); }
  private context(id: string) {
    const state = this.client.getSnapshot(), view = state.views[id], target = view?.target, generation = state.generation;
    if (!target || !this.client.hasAuxiliaryAuthority(id)) throw new Error('Auxiliary attachment is unavailable.');
    return { target, current: () => generation === this.client.getSnapshot().generation && sameTarget(target, this.client.getSnapshot().views[id]?.target) && this.client.hasAuxiliaryAuthority(id) };
  }
  observe(id: string): () => void {
    const context = this.context(id), abort = new AbortController();
    this.viewing?.abort.abort();
    const owner = { key: this.key(id), ...context, abort }; this.viewing = owner;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const current = () => this.viewing === owner && context.current() && !abort.signal.aborted;
    const poll = async () => {
      if (!current()) return;
      const entry = this.entry(id);
      if (entry && !entry.readFailed && this.client.getSnapshot().views[id]?.observationState !== 'unavailable' && !entry.question.complete && !['failed', 'cancelled', 'unavailable', 'notAdmitted'].includes(entry.question.phase)) await this.reconcile(id, current, abort.signal);
      if (current() && this.client.getSnapshot().views[id]?.observationState !== 'unavailable') timer = setTimeout(() => { void poll(); }, 1000);
    };
    void poll();
    return () => { clearTimeout(timer); abort.abort(); if (this.viewing === owner) this.viewing = undefined; };
  }
  async submit(id: string, text: string): Promise<void> {
    if (!text || new TextEncoder().encode(text).length > 16_384) throw new Error('Question must contain 1–16384 UTF-8 bytes.');
    const old = this.entry(id)?.question;
    if (old && !old.settled) return;
    const context = this.context(id), key = this.key(id), authority = this.client.getSnapshot().authorityId;
    if (!this.entries.has(key) && this.entries.size >= 16) throw new Error('Temporary question capacity is full.');
    const entry: Entry = { target: context.target, authority, question: { correlation: crypto.randomUUID(), question: text, phase: 'submitting', text: '', frontier: 0, complete: false } };
    this.entries.set(key, entry); this.emit();
    try {
      await this.client.request({ method: 'auxiliary/btw', params: { target: context.target, submission: { correlation: entry.question.correlation, question: text } } }, 'auxiliary_submitted', ({ receipt }) => {
        // Decode-time identity survives a disconnect in response pumping, before
        // the Promise continuation can lose its connection authority.
        if (this.entries.get(key) === entry && receipt.correlation === entry.question.correlation && !entry.question.settled) this.set(entry, { receipt, scope: receipt.scope, ...(['submitting', 'uncertain'].includes(entry.question.phase) ? { phase: 'preparing' as const } : {}) });
      }, context.current);
    } catch (error) {
      if (this.entries.get(key) === entry && !entry.question.receipt && !entry.question.settled) {
        const uncertain = isOutcomeUncertain(error);
        this.set(entry, { phase: uncertain ? 'uncertain' : 'failed', settled: !uncertain, diagnostic: diagnostic(error) });
      }
    }
  }
  reconcile(id: string, current?: () => boolean, signal?: AbortSignal): Promise<void> {
    if (this.reading) return this.reading;
    const owner = this.viewing;
    const active = current ?? (() => !owner || (this.viewing === owner && owner.current() && !owner.abort.signal.aborted));
    const pending = this.read(id, active, signal ?? owner?.abort.signal);
    this.reading = pending;
    void pending.finally(() => { if (this.reading === pending) this.reading = undefined; });
    return pending;
  }
  private async read(id: string, current: () => boolean, signal?: AbortSignal) {
    const entry = this.entry(id); if (!entry || entry.question.settled) return;
    if (!current() || signal?.aborted || !this.client.hasAuxiliaryAuthority(id)) return;
    const context = this.context(id), same = () => current() && context.current() && this.entry(id) === entry && !entry.question.settled;
    entry.readFailed = false;
    try {
      if (!entry.question.receipt) {
        const { lookup } = await this.client.request({ method: 'auxiliary/submissionLookup', params: { target: context.target, correlation: entry.question.correlation, scope: entry.question.scope ?? null } }, 'auxiliary_submission_lookup', undefined, same, signal);
        if (!same()) return;
        // Original submission evidence may have arrived while this read was
        // pending. A weaker lookup cannot replace that receipt or a rejection.
        if (!entry.question.receipt) {
          if (lookup.state === 'known') this.set(entry, { receipt: lookup.receipt, scope: lookup.receipt.scope, phase: 'preparing' });
          else { if (lookup.state === 'pending') this.set(entry, { scope: lookup.scope, ...(lookup.call_id ? { pendingCall: lookup.call_id } : {}) }); this.set(entry, { phase: lookup.state === 'confirmed_not_admitted' ? 'notAdmitted' : lookup.state === 'pending' ? 'preparing' : 'uncertain', settled: lookup.state === 'confirmed_not_admitted' }); return; }
        }
      }
      const receipt = entry.question.receipt!;
      const { status } = await this.client.request({ method: 'auxiliary/status', params: { target: context.target, call_id: receipt.call_id } }, 'auxiliary_status', undefined, same, signal);
      if (!same() || status.call_id !== receipt.call_id) return;
      if (status.result === 'retained') {
        const { result } = await this.client.request({ method: 'auxiliary/result', params: { target: context.target, call_id: receipt.call_id } }, 'auxiliary_result', undefined, same, signal);
        if (!same()) return;
        if (result) { this.acceptResult(entry, result); return; }
      }
      if (!status.executing) this.set(entry, { phase: 'unavailable', settled: true });
      else if (status.inference_stopped || status.outcome) this.set(entry, { phase: 'settling' });
      else if (!entry.question.request) this.set(entry, { phase: 'preparing' });
    } catch (error) {
      if (same()) { entry.readFailed = true; this.set(entry, { phase: entry.question.receipt ? 'incomplete' : 'uncertain', diagnostic: diagnostic(error) }); }
    }
  }
  private acceptResult(entry: Entry, result: AuxiliaryResult) {
    if (result.call_id !== entry.question.receipt?.call_id || entry.question.complete) return;
    if (result.value.kind === 'btw' && result.execution === 'completed' && result.final_request_id && new TextEncoder().encode(result.value.text).length <= 8192) {
      this.set(entry, { text: result.value.text, request: result.final_request_id, phase: 'complete', complete: true, settled: true });
    } else if (result.value.kind === 'rejected') this.set(entry, { phase: result.value.reason === 'cancelled' ? 'cancelled' : 'failed', settled: true, diagnostic: result.value.reason });
    else this.set(entry, { phase: 'incomplete' });
  }
  segment(target: AttachmentTarget, segment: AuxiliarySegment) {
    const entry = this.entry(target.session_id), owner = this.viewing;
    if (!entry || !owner || owner.key !== this.key(target.session_id) || !owner.current() || !sameTarget(owner.target, target) || entry.question.settled || entry.question.receipt?.call_id !== segment.call_id) return;
    const previous = entry.question;
    if (segment.kind === 'reset') {
      if (previous.attempt !== undefined && segment.attempt <= previous.attempt) return;
      this.set(entry, { request: segment.request_id, attempt: segment.attempt, frontier: 0, text: '', phase: previous.request ? 'retrying' : 'running', diagnostic: undefined });
    } else {
      const bytes = new TextEncoder().encode(segment.text).length;
      if (previous.request !== segment.request_id || previous.attempt !== segment.attempt || Number(segment.start) !== previous.frontier || Number(segment.end) !== previous.frontier + bytes || bytes > 1024 || Number(segment.end) > 8192) { this.set(entry, { phase: 'incomplete' }); return; }
      if (previous.phase === 'incomplete') return;
      this.set(entry, { text: previous.text + segment.text, frontier: Number(segment.end), phase: 'running' });
    }
  }
  async cancel(id: string) {
    const entry = this.entry(id), call = entry?.question.receipt?.call_id ?? entry?.question.pendingCall; if (!entry || !call || entry.question.cancelling) return;
    const context = this.context(id); this.set(entry, { cancelling: true });
    try { await this.client.request({ method: 'auxiliary/cancel', params: { target: context.target, call_id: call } }, 'auxiliary_cancelled', undefined, context.current); }
    finally { if (this.entry(id) === entry) this.set(entry, { cancelling: false }); }
  }
}

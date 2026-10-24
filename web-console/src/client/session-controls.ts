import { sameTarget } from '../../../protocol/app-server/attachment-identity';
import { replaceTranscript } from './transcript';
import { OutcomeUncertain, RequestNotDispatched, RpcFailure, isOutcomeUncertain } from './errors';
import type { AttachmentTarget, GoalMutation, GoalRef, PendingInboundRef, Request1 } from '../../../protocol/app-server/v57';
import type { ObservationPhase } from '../../../protocol/app-server/observation';
import type { SessionLifecycles } from './session-lifecycle/system';
import type { CancellationTarget, ClientView, CompactionRequestEvidence, ConnectionRequest, GoalControlOutcome, InboundControlOutcome, SessionView } from './contracts';

/** GoalDomain serializes its bounded rejection into the error message. Only the
 * reason is displayed; its embedded `current` is never adopted as authority. */
function goalRefusal(error: unknown) {
  if (!(error instanceof RpcFailure)) return error instanceof Error ? error.message : String(error);
  try {
    const rejection: unknown = JSON.parse(error.error.message);
    if (rejection && typeof rejection === 'object' && 'reason' in rejection && typeof rejection.reason === 'string') return rejection.reason;
  } catch { /* Non-Goal refusal: show the transport message. */ }
  return error.message;
}

/** What attached-Session controls read from and write to their client. */
export interface SessionControlPort {
  request: ConnectionRequest;
  state(): Readonly<ClientView>;
  current(generation: number): boolean;
  /** The connection is initialized and connected. */
  controlReady(): boolean;
  setSession(id: string, patch: Partial<Pick<SessionView, 'compactionRequest' | 'cancellation' | 'history'>>): void;
  publish(patch: Pick<ClientView, 'uncertain'>): void;
  target(id: string): AttachmentTarget;
  observe: SessionLifecycles['observe'];
  observes: SessionLifecycles['observes'];
  observesAttachment(id: string, admission: SessionView['attachmentObservation']): boolean;
  observationPhase(id: string): ObservationPhase | undefined;
  /** Authoritative reread of one attachment; see `AppServerClient.refresh`. */
  refresh(id: string): Promise<void>;
}

/** CAS-bound and gesture-owned controls of an attached Session: Goal
 * mutation, pending inbound edit/removal, manual compaction and Turn
 * cancellation. Each holds only bounded local evidence of its own operation;
 * native state, read again after a known outcome, stays authoritative. */
export class SessionControls {
  constructor(private readonly port: SessionControlPort) {}
  /** One CAS-bound Goal control. `expected` is the authoritative GoalRef the caller
   * rendered. A known outcome (applied or refused) is not projection convergence:
   * `observed` is true only when a snapshot read requested after the outcome
   * succeeded for this attachment. A lost response stays uncertain. Nothing is
   * retried and no newer revision is ever substituted. */
  /** Load the original typed Goal objective at one exact native read cut. */
  async goalObjective(id: string, expected: GoalRef): Promise<string> {
    const view = this.port.state().views[id], target = view?.target, cut = view?.checkpoint?.read_cut, generation = this.port.state().generation;
    const current = () => this.port.current(generation) && sameTarget(this.port.state().views[id]?.target, target)
      && this.port.state().views[id]?.snapshot?.goal?.current?.reference.id === expected.id
      && this.port.state().views[id]?.snapshot?.goal?.current?.reference.revision === expected.revision;
    if (!target || !cut || !current()) throw new Error('Goal observation was replaced.');
    const path = ['current', 'objective'];
    const { chunk } = await this.port.request({ method: 'conversation/checkpointValueRead', params: {
      target, agent_id: null, cut, domain: 'goal', item_offset: 0, path, offset: 0, max_bytes: 65_536,
    } }, 'checkpoint_value_chunk', undefined, current);
    if (!current()) throw new Error('Goal observation was replaced.');
    if(chunk.bytes_base64.length>87_384 || chunk.bytes_base64.length%4!==0 || chunk.bytes_base64.length/4*3-(chunk.bytes_base64.endsWith('==')?2:chunk.bytes_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
    const bytes = Uint8Array.from(atob(chunk.bytes_base64), value => value.charCodeAt(0));
    if (JSON.stringify(chunk.cut) !== JSON.stringify(cut) || chunk.domain !== 'goal' || chunk.item_offset !== 0
      || JSON.stringify(chunk.path) !== JSON.stringify(path) || chunk.offset !== 0 || chunk.next_offset != null
      || !Number.isSafeInteger(chunk.readable_end) || chunk.readable_end !== bytes.length || bytes.length > 65_536) throw new Error('Goal objective range changed.');
    const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    const original: unknown = chunk.representation === 'utf8_text' ? text : chunk.representation === 'canonical_json_v1' ? JSON.parse(text) : undefined;
    if (typeof original !== 'string' || new TextEncoder().encode(original).length > 8192) throw new Error('Goal objective exceeds its native bound.');
    return original;
  }
  async controlGoal(id: string, expected: GoalRef, mutation: GoalMutation): Promise<GoalControlOutcome> {
    const generation = this.port.state().generation;
    let target: AttachmentTarget;
    try { target = this.port.target(id); } catch (error) { return { status: 'rejected', reason: error instanceof Error ? error.message : String(error), observed: false }; }
    const current = () => this.port.current(generation) && sameTarget(this.port.state().views[id]?.target, target);
    try {
      await this.port.request({ method: 'goal/control', params: { target, control: { action: 'mutate', expected, mutation } } }, 'goal_mutation');
    } catch (error) {
      if (error instanceof OutcomeUncertain) return { status: 'uncertain' };
      if (!current()) return { status: 'obsolete' };
      return { status: 'rejected', reason: goalRefusal(error), observed: await this.reread(id, current) };
    }
    if (!current()) return { status: 'obsolete' };
    return { status: 'applied', observed: await this.reread(id, current) };
  }
  /** `refresh` re-marks the view dirty, so even a coalesced in-flight refresh
   * completes a snapshot request issued after this call before resolving. */
  private async reread(id: string, current: () => boolean) {
    const observation = this.port.observe(id);
    if (!observation || !current()) return false;
    try { await this.port.refresh(id); } catch { return false; }
    return current() && observation.current() && this.port.state().views[id]?.attachment === 'attached';
  }
  async editInbound(id: string, expected: PendingInboundRef, text: string): Promise<InboundControlOutcome> {
    return this.controlInbound(id, target => ({ method: 'inbound/edit', params: { target, expected, text } }));
  }
  async removeInbound(id: string, expected: PendingInboundRef): Promise<InboundControlOutcome> {
    return this.controlInbound(id, target => ({ method: 'inbound/remove', params: { target, expected } }));
  }
  private async controlInbound(id: string, operation: (target: AttachmentTarget) => Request1): Promise<InboundControlOutcome> {
    const generation = this.port.state().generation;
    let target: AttachmentTarget;
    try { target = this.port.target(id); } catch (error) { return { status: 'rejected', reason: String(error), observed: false }; }
    const current = () => this.port.current(generation) && sameTarget(this.port.state().views[id]?.target, target);
    try {
      const result = await this.port.request(operation(target), 'inbound_mutation');
      if (!current()) return { status: 'obsolete' };
      if (result.outcome.status === 'durability_uncertain') return { status: 'uncertain' };
      return { status: 'known', outcome: result.outcome, observed: await this.rereadPending(id, current) };
    } catch (error) {
      if (!current()) return isOutcomeUncertain(error) ? { status: 'uncertain' } : { status: 'obsolete' };
      if (isOutcomeUncertain(error)) return { status: 'uncertain' };
      return { status: 'rejected', reason: String(error), observed: await this.rereadPending(id, current) };
    }
  }
  /** Pending edits/removals invalidate historical windows too. Replace after
   * the final coalesced read, so an older in-flight response cannot restore a
   * removed row through the normal append-only transcript merge. */
  private async rereadPending(id: string, current: () => boolean) {
    const observation = this.port.observe(id);
    const observed = await this.reread(id, current) && !!observation?.current();
    const view = this.port.state().views[id];
    if (observed && view?.snapshot) this.port.setSession(id, { history: replaceTranscript(view.snapshot.transcript, view.history) });
    return observed;
  }
  private compactionRequests = new Map<string, CompactionRequestEvidence>();
  /** Claims the local gesture synchronously. Native maintenance owns admission. */
  compact(id: string, consume: () => void = () => {}): boolean {
    const attachment = this.port.observe(id);
    if (!attachment) throw new RequestNotDispatched('Attachment control authority was revoked.');
    const target = attachment.target, generation = this.port.state().generation;
    const pending = this.compactionRequests.get(id);
    if (pending?.generation === generation && sameTarget(pending.target, target)) return false;
    const operation: CompactionRequestEvidence = { baselineCount: this.port.state().views[id]?.snapshot?.context?.compaction_count ?? 0, generation, requestId: crypto.randomUUID(), authorityId: this.port.state().authorityId, target, status: 'submitting' };
    this.compactionRequests.set(id, operation);
    this.port.setSession(id, { compactionRequest: operation });
    consume();
    const current = () => this.port.current(generation) && attachment.current() && sameTarget(this.port.state().views[id]?.target, target)
      && this.port.state().views[id]?.compactionRequest?.requestId === operation.requestId;
    const release = () => { if (this.compactionRequests.get(id) === operation) this.compactionRequests.delete(id); };
    void this.port.request({ method: 'context/compact', params: { target, request_id: operation.requestId } }, 'context_compacted', undefined, current)
      .then(result => {
        release();
        if (!current()) return;
        // The response follows native release. Read failure cannot erase this fact.
        this.port.setSession(id, { compactionRequest: { ...operation, status: 'succeeded', receipt: result.compaction } });
      }, error => {
        release();
        if (this.port.state().views[id]?.compactionRequest !== operation) return;
        if (!current() && (this.port.state().views[id]?.target || this.port.state().authorityId !== operation.authorityId)) return;
        this.port.setSession(id, { compactionRequest: { ...operation, status: isOutcomeUncertain(error) ? 'uncertain' : 'failed', diagnostic: String(error) } });
      });
    return true;
  }

  cancellationTarget(id: string): CancellationTarget | undefined {
    const view = this.port.state().views[id], attempt = view?.snapshot?.attempt;
    // This proof owns the live attachment even when its observation source is
    // unavailable. The coordinator selects the current Attempt at dispatch.
    if (!this.port.observesAttachment(id, view?.attachmentObservation) || !this.port.controlReady() || view?.attachmentIntent !== 'wanted'
      || !view.target || view.deleting || this.cancellationPending(id)) return;
    if (this.port.observationPhase(id) === 'following' && (!attempt || attempt.attempt_id == null || !['admitted','running'].includes(attempt.phase.type)
      || view.snapshot?.shutting_down || view.snapshot?.durability_failure || view.snapshot?.pending_interactions?.length)) return;
    return { generation: this.port.state().generation, target: view.target, attemptId: attempt?.attempt_id ?? undefined, attachment: view.attachmentObservation };
  }
  private cancellationPending(id: string): boolean {
    const view = this.port.state().views[id], cancellation = view?.cancellation;
    // An acknowledgement cannot establish that its Attempt is still current
    // while observation is unavailable. A later explicit gesture selects anew.
    return !!cancellation && (cancellation.status !== 'acknowledged'
      || this.port.observationPhase(id) === 'following' && cancellation.attemptId === view.snapshot?.attempt?.attempt_id);
  }
  async cancelTurn(expected: CancellationTarget) {
    const { target, generation, attemptId, attachment } = expected, id = target.session_id;
    const current = () => this.port.current(generation) && this.port.controlReady()
      && this.port.observes(id, attachment) && sameTarget(this.port.state().views[id]?.target, target);
    if (!current() || this.cancellationPending(id)) return;
    const operation = { attemptId, status: 'in-flight' as const };
    this.port.setSession(id, { cancellation: operation });
    try {
      const accepted = await this.port.request({ method: 'turn/cancel', params: { target } }, 'cancellation_accepted', undefined,
        () => current() && this.port.state().views[id]?.cancellation === operation);
      if (current() && this.port.state().views[id].cancellation === operation) {
        this.port.setSession(id, { cancellation: { attemptId: accepted.attempt_id, status: 'acknowledged' } });
      }
    } catch (error) {
      // The exact operation owns cleanup even if its unsent transport was retired.
      // Acknowledgement or a successor operation replaces this marker.
      if (this.port.state().views[id]?.cancellation === operation) {
        this.port.setSession(id, { cancellation: isOutcomeUncertain(error) ? { attemptId, status: 'uncertain' } : undefined });
      }
      throw error;
    }
  }
  reconcileCompaction(id: string) {
    const snapshot = this.port.state().views[id].snapshot!;
    const request = this.port.state().views[id].compactionRequest;
    const target = this.port.state().views[id].target;
    if (request) {
      if (request.authorityId !== this.port.state().authorityId || (request.generation === this.port.state().generation && !sameTarget(request.target, target)) || request.target.conversation_id !== target?.conversation_id
        || request.target.runtime_incarnation !== target?.runtime_incarnation) {
        this.port.setSession(id, { compactionRequest: undefined });
      } else if (snapshot.context?.compaction_in_progress && ['succeeded', 'failed'].includes(request.status)) {
        this.port.setSession(id, { compactionRequest: undefined });
      } else {
        const evidence = snapshot.context?.manual_compaction;
        if (evidence?.request_id === request.requestId && evidence.released) {
          // Native release ends gesture ownership independently of the RPC reply.
          if (this.compactionRequests.get(id)?.requestId === request.requestId) this.compactionRequests.delete(id);
          if (request.status !== 'succeeded') this.port.setSession(id, { compactionRequest: { ...request, status: evidence.error ? 'failed' : 'succeeded', diagnostic: evidence.error ?? undefined } });
          if (this.port.state().uncertain.some(item => item.compactionRequestId === request.requestId && item.sessionId === id)) {
            this.port.publish({ uncertain: this.port.state().uncertain.filter(item => item.compactionRequestId !== request.requestId || item.sessionId !== id) });
          }
        }
      }
    }
  }
  reconcileCancellation(id: string) {
    const snapshot = this.port.state().views[id].snapshot!;
    const cancellation = this.port.state().views[id].cancellation;
    // Until acknowledgement the retained display Attempt is not the operation's
    // native target, so even its observed settlement cannot resolve this request.
    if (cancellation?.status === 'acknowledged' && cancellation.attemptId !== undefined
      && snapshot.attempt?.attempt_id === cancellation.attemptId && snapshot.attempt.phase.type === 'settled') {
      this.port.setSession(id, { cancellation: undefined });
    }
  }
}

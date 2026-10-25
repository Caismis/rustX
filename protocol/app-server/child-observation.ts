import type { AgentObservation, AgentObservationCursor, RuntimeClientCheckpoint, RuntimeClientSnapshot, RuntimeClientEvent, CheckpointReadCut } from './v57.ts';
import { validateObservationCut } from './observation.ts';
import { checkpointSnapshot, foldRuntimeEvent, contentBindings, observationCheckpoint } from './projection.ts';

export class ChildObservation {
  phase: 'checkpointLoading' | 'liveFollowing' | 'durableOnly' | 'resyncRequired' | 'unavailable' = 'checkpointLoading';
  checkpoint?: RuntimeClientCheckpoint;
  snapshot?: RuntimeClientSnapshot;
  cursor?: AgentObservationCursor;
  /** Last completely parsed native baseline/fold, retained during repair. */
  appliedCursor?: AgentObservationCursor;
  get content() { return this.snapshot ? contentBindings(this.snapshot) : []; }
  readonly agentId: string; readonly conversationId: string; readonly activationId: string;
  constructor(agentId: string, conversationId: string, activationId: string) { this.agentId = agentId; this.conversationId = conversationId; this.activationId = activationId; }
  private identity(cursor: AgentObservationCursor) {
    return cursor.agent_id === this.agentId && cursor.conversation_id === this.conversationId && cursor.activation_id === this.activationId;
  }
  private cutIdentity(cut: CheckpointReadCut) {
    return cut.conversation_id === this.conversationId && cut.activation_id === this.activationId;
  }
  private eventIdentity(event: RuntimeClientEvent, cursor: string): boolean {
    try {
      if (event.type === 'checkpoint_advanced') {
        if (event.checkpoint.snapshot.conversation_id !== this.conversationId || event.checkpoint.cursor !== cursor) return false;
        validateObservationCut(event.checkpoint.read_cut, this.conversationId, this.activationId, cursor);
        return true;
      }
      if (event.type !== 'projection_transition') return false;
      validateObservationCut(event.metadata.read_cut, this.conversationId, this.activationId, cursor);
      return event.events.every(fact => fact.type !== 'projection_transition' && fact.type !== 'checkpoint_advanced' || this.eventIdentity(fact, cursor));
    } catch { return false; }
  }
  apply(observation: AgentObservation) {
    if (observation.type === 'events') {
      if (!this.cursor || !this.snapshot || this.phase !== 'liveFollowing' || !this.identity(observation.after) || !this.identity(observation.cursor) || observation.after.cursor !== this.cursor.cursor) { this.phase = 'resyncRequired'; throw new Error('Child observation continuity lost.'); }
      if (observation.events.length > 32) throw new Error('Child event batch exceeds its bound.');
      for (const record of observation.events) {
        if (BigInt(record.cursor) !== BigInt(this.cursor.cursor) + 1n) { this.phase = 'resyncRequired'; throw new Error('Child observation gap.'); }
        if (!this.eventIdentity(record.event, record.cursor)) { this.phase = 'resyncRequired'; throw new Error('Child event cut evidence mismatch.'); }
        let snapshot: RuntimeClientSnapshot;
        try { snapshot = foldRuntimeEvent(this.snapshot, record.event); }
        catch (error) { this.phase = 'resyncRequired'; throw error; }
        if (snapshot.conversation_id !== this.conversationId) { this.phase = 'resyncRequired'; throw new Error('Child event conversation mismatch.'); }
        this.snapshot = snapshot;
        this.checkpoint = observationCheckpoint(snapshot, record.cursor) ?? this.checkpoint;
        this.cursor = { ...this.cursor, cursor: record.cursor };
        this.appliedCursor=this.cursor;
      }
      if (this.cursor.cursor !== observation.cursor.cursor) { this.phase = 'resyncRequired'; throw new Error('Child batch frontier mismatch.'); }
      return;
    }
    if (observation.agent_id !== this.agentId || observation.conversation_id !== this.conversationId || observation.activation_id !== this.activationId) throw new Error('Child activation was replaced.');
    if (observation.type === 'resync_required') { this.phase = 'resyncRequired'; this.cursor = undefined; return; }
    if (observation.checkpoint.snapshot.conversation_id !== this.conversationId || !this.cutIdentity(observation.checkpoint.read_cut)) throw new Error('Child checkpoint identity mismatch.');
    validateObservationCut(observation.checkpoint.read_cut, this.conversationId, this.activationId, observation.checkpoint.cursor);
    if (observation.source === 'live' && (!observation.cursor || !this.identity(observation.cursor))) throw new Error('Missing live Child cursor.');
    if (observation.source === 'durable_only' && observation.cursor) throw new Error('DurableOnly cannot have a live cursor.');
    if (observation.cursor && observation.cursor.cursor !== observation.checkpoint.cursor) throw new Error('Child checkpoint cursor mismatch.');
    if (observation.cursor && this.appliedCursor && BigInt(observation.cursor.cursor) < BigInt(this.appliedCursor.cursor)) throw new Error('Child checkpoint predates the locally Applied Cursor.');
    this.checkpoint = observation.checkpoint;
    this.snapshot = checkpointSnapshot(observation.checkpoint);
    this.cursor = observation.cursor ?? undefined;
    if(this.cursor)this.appliedCursor=this.cursor;
    this.phase = observation.source === 'live' ? 'liveFollowing' : 'durableOnly';
  }
}

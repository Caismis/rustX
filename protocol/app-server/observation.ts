import type { AttachmentTarget, CheckpointReadCut, RuntimeClientCheckpoint, RuntimeClientEvent } from './v57.ts';

/** Validate the returned native Root cut before installing any client state. */
export function validateRootCheckpoint(target: AttachmentTarget, checkpoint: RuntimeClientCheckpoint, applied?: string): void {
  if (checkpoint.snapshot.conversation_id !== target.conversation_id) throw new Error('Mismatched Root checkpoint identity.');
  validateObservationCut(checkpoint.read_cut, target.conversation_id, null, checkpoint.cursor);
  if (applied !== undefined && checkpointCursor(checkpoint.cursor) < checkpointCursor(applied)) throw new Error('Checkpoint predates the locally Applied Cursor.');
}

/** Validate only the native scope-bearing carriers, using the actual record cursor. */
export function validateRootEvent(target: AttachmentTarget, event: RuntimeClientEvent, cursor: string): void {
  if (event.type === 'checkpoint_advanced') {
    validateRootCheckpoint(target, event.checkpoint);
    if (event.checkpoint.cursor !== cursor) throw new Error('Root checkpoint event cursor mismatch.');
  } else if (event.type === 'projection_transition') {
    validateObservationCut(event.metadata.read_cut, target.conversation_id, null, cursor);
    for (const fact of event.events) validateRootEvent(target, fact, cursor);
  }
}

export function validateObservationCut(cut: CheckpointReadCut, conversationId: string, activationId: string | null, cursor: string): void {
  if (cut.conversation_id !== conversationId || (cut.activation_id ?? null) !== activationId) throw new Error('Mismatched checkpoint identity.');
  checkpointCursor(cursor);
  if (cursor !== cut.revision) throw new Error('Checkpoint cursor does not match its native revision.');
  checkpointCursor(cut.through);
  const source = cut.source_cut;
  if (source && (source.read.conversation_id !== conversationId || source.read.journal !== cut.through
    || source.read.ledger_through !== source.ledger_through || source.read.surface_revision !== source.surface_revision
    || source.read.transcript_total !== source.transcript_total)) throw new Error('Mismatched checkpoint source cut.');
}

function checkpointCursor(value: string): bigint {
  if (typeof value !== 'string' || value.length > 20 || !/^(0|[1-9][0-9]*)$/.test(value)) throw new Error('Invalid native checkpoint cursor.');
  const exact = BigInt(value);
  if (exact > 18_446_744_073_709_551_615n) throw new Error('Invalid native checkpoint cursor.');
  return exact;
}

/** Client application authority. Transport acceptance never advances this cursor. */
export type ObservationPhase = 'attached' | 'checkpointLoading' | 'subscribePending' | 'following' | 'resyncRequired' | 'unavailable' | 'retired';
export type ObservationRecord<E> = { registrationId: string; cursor: string; event: E } | { registrationId: string; resync: true } | { registrationId: string; unavailable: true };

export class ObservationChain<E> {
  phase: ObservationPhase = 'checkpointLoading';
  applied?: string;
  registrationId?: string;
  private requested?: string;
  private pending: { record: ObservationRecord<E>; fold: (event: E, cursor: string) => void }[] = [];

  installCheckpoint(cursor: string) {
    const next = BigInt(cursor);
    if (this.applied !== undefined && next < BigInt(this.applied)) throw new Error('Checkpoint predates the locally Applied Cursor.');
    this.applied = cursor;
    this.registrationId = undefined;
    this.requested = undefined;
    this.pending = [];
    this.phase = 'attached';
  }
  beginCheckpointLoading() {
    this.phase = 'checkpointLoading';
    this.registrationId = undefined;
    this.requested = undefined;
    this.pending = [];
  }
  beginSubscription(after: string) {
    if (after !== this.applied) throw new Error('Subscription must start at the locally Applied Cursor.');
    this.requested = after;
    this.registrationId = undefined;
    this.pending = [];
    this.phase = 'subscribePending';
  }
  acceptReceipt(registrationId: string, after: string): boolean {
    if (this.phase !== 'subscribePending' || after !== this.requested || after !== this.applied) return false;
    this.registrationId = registrationId;
    this.phase = 'following';
    const pending = this.pending;
    this.pending = [];
    for (const { record, fold } of pending) this.deliver(record, fold);
    return this.phase === 'following';
  }
  deliver(record: ObservationRecord<E>, fold: (event: E, cursor: string) => void): 'buffered' | 'ignored' | 'applied' | 'recovery' | 'unavailable' {
    if (this.phase === 'subscribePending') {
      if (this.pending.length === 32) { this.requireRecovery(); return 'recovery'; }
      this.pending.push({ record, fold });
      return 'buffered';
    }
    if (this.phase !== 'following' || record.registrationId !== this.registrationId) return 'ignored';
    if ('unavailable' in record) { this.markUnavailable(); return 'unavailable'; }
    if ('resync' in record) { this.requireRecovery(); return 'recovery'; }
    const previous = BigInt(this.applied!);
    const next = BigInt(record.cursor);
    if (next <= previous) return 'ignored';
    if (next !== previous + 1n) { this.requireRecovery(); return 'recovery'; }
    try { fold(record.event, record.cursor); }
    catch { this.requireRecovery(); return 'recovery'; }
    this.applied = record.cursor;
    return 'applied';
  }
  markUnavailable() { this.phase = 'unavailable'; this.pending = []; }
  requireRecovery() { this.phase = 'resyncRequired'; this.pending = []; }
  retire() { this.phase = 'retired'; this.pending = []; this.registrationId = undefined; }
}

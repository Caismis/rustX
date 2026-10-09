import type { AttachmentTarget, MethodResult, RuntimeClientSessionDeletionResult } from '../../../../protocol/app-server/v38';

export interface Admission {
  current(): boolean;
  validate(signal: AbortSignal): Promise<boolean>;
  sent?(): void;
}
export type Attached = Extract<MethodResult, { type: 'attached' }>;
export type FailureKind = 'unsent' | 'uncertain' | 'refused' | 'stale-route' | 'local';
export interface Observation {
  readonly generation: number;
  readonly target: AttachmentTarget;
  readonly nodeId: string;
  readonly intentRevision: number;
}
/** Actor-owned facts; SessionView only exposes this readonly projection. */
export interface LifecycleFacts {
  readonly attachmentIntent: 'wanted' | 'released';
  readonly attachmentIntentRevision: number;
  readonly attachmentObservation?: Observation;
  readonly attachment: 'detached' | 'attaching' | 'attached' | 'resynchronizing' | 'stale' | 'error';
  readonly target?: AttachmentTarget;
  readonly nodeId?: string;
  readonly nodeConversationId?: string;
  readonly attachmentNodeId?: string;
  readonly deleting?: boolean;
  readonly deletionRecovery?: 'committed_cleanup_pending' | 'committed_durability_uncertain';
  readonly deletionCommitted?: LifecycleFacts['deletionRecovery'];
  readonly recoveringDeletion?: boolean;
  readonly error?: string;
}
/** Finite I/O only. No lifecycle decisions or writable SessionView dependency. */
export interface SessionLifecyclePort {
  resolveNode(current: () => boolean): Promise<{ node: string; conversation: string }>;
  conversation(node: string, current: () => boolean): Promise<string>;
  admit(current: () => boolean): Promise<false | Admission>;
  attach(node: string, admission: Admission): Promise<Attached>;
  detach(target: AttachmentTarget, admission: Admission): Promise<void>;
  switchNode(target: AttachmentTarget, node: string, admission: Admission): Promise<Extract<MethodResult, { type: 'session' }>['session']>;
  delete(revision: string, admission: Admission): Promise<RuntimeClientSessionDeletionResult>;
  recover(admission: Admission): Promise<RuntimeClientSessionDeletionResult>;
  inspectDeletion(current: () => boolean): Promise<RuntimeClientSessionDeletionResult>;
  observeAttached(result: Attached, current: () => boolean): Promise<void>;
  refresh(target: AttachmentTarget): Promise<void>;
  cold(current: () => boolean): void;
  classify(error: unknown): FailureKind;
}
export const emptyFacts: LifecycleFacts = { attachmentIntent: 'released', attachmentIntentRevision: 0, attachment: 'detached' };
export const sameClaim = (a?: AttachmentTarget, b?: AttachmentTarget) => !!a && !!b && a.session_id === b.session_id
  && a.conversation_id === b.conversation_id && a.runtime_incarnation === b.runtime_incarnation && a.attachment_id === b.attachment_id;

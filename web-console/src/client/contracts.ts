import type { LifecycleFacts } from './session-lifecycle/port';
import type { lifecycleAvailability } from './session-lifecycle/machine';
import type { TraceCache } from './trace';
import type { PublicationHead } from './publication-heads';
import type { TranscriptCache } from './transcript';
import type {
  AttachmentTarget, ConfigRepairReceipt, ConfigurationApplication, InteractionRef, MethodResult, ObservedTurnPage, PendingMutationOutcome,
  Request1, RuntimeClientCursor, RuntimeClientSnapshot, ServerCapabilities, SessionPersistentState, SessionSummary, SourceTarget, UserInputBlock,
} from '../../../protocol/app-server/v57';

/** The published read model of one App Server connection and the operation
 * contracts its consumers hold. The client is their only writer; everything
 * here is a replaceable cache or bounded local evidence, never native authority. */

/** Exact attachment for a current-Attempt command; attemptId is presentation only. */
export interface CancellationTarget {
  readonly generation: number;
  readonly target: AttachmentTarget;
  readonly attemptId?: string;
  readonly attachment: SessionView['attachmentObservation'];
}
export type InboundControlOutcome =
  | { status: 'known'; outcome: PendingMutationOutcome; observed: boolean }
  | { status: 'uncertain' }
  | { status: 'obsolete' }
  | { status: 'rejected'; reason: string; observed: boolean };

export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'reconnecting' | 'resynchronizing' | 'stale' | 'incompatible' | 'error';
export type OutlinePagingIntent = { type: 'latest' } | { type: 'page'; offset: number };
export interface CompactionRequestEvidence {
  /** Presentation ordering only; never resolves an uncertain request. */
  baselineCount: number;
  generation: number;
  requestId: string;
  authorityId?: string;
  target: AttachmentTarget;
  status: 'submitting' | 'uncertain' | 'succeeded' | 'failed';
  diagnostic?: string;
  receipt?: Extract<MethodResult, {type:'context_compacted'}>['compaction'];
}
export interface SessionView extends Omit<LifecycleFacts, 'error' | 'attachmentIntentRevision'> {
  /** Readonly actor projection; absent on catalog-only presentation rows. */
  readonly attachmentIntentRevision?: number;
  readonly lifecycle?: ReturnType<typeof lifecycleAvailability>;
  compactionRequest?: CompactionRequestEvidence;
  id: string;
  /** Last native catalog row, retained when the Sidebar reads a different page. */
  summary?: SessionSummary;
  snapshot?: RuntimeClientSnapshot;
  publicationHeads?: readonly PublicationHead[];
  checkpoint?: import('../../../protocol/app-server/v57').RuntimeClientCheckpoint;
  observationState?: import('../../../protocol/app-server/observation').ObservationPhase;
  interactionDetail?:{target:AttachmentTarget;cut:import('../../../protocol/app-server/v57').CheckpointReadCut;body:import('../../../protocol/app-server/v57').RoutedInteraction};
  interactionPage?:{target:AttachmentTarget;page:import('../../../protocol/app-server/v57').CheckpointPage;rows:import('../../../protocol/app-server/v57').RuntimeClientInteractionHeader[];offset:number};
  trace?: TraceCache;
  cursor?: RuntimeClientCursor;
  history?: TranscriptCache;
  /** Read-only durable history, never an execution snapshot or control claim. */
  preview?: { conversationId: string; history: TranscriptCache };
  statisticsPreview?: { conversationId: string; nodeId?: string; cut: import('../../../protocol/app-server/v57').CheckpointReadCut; statistics: import('../../../protocol/app-server/v57').ConversationStatistics; occupancy?: import('../../../protocol/app-server/v57').RuntimeClientContextOccupancy | null };
  tracePreview?: { conversationId: string; cache: TraceCache };
  turnOutline?: { paging: OutlinePagingIntent; page?: ObservedTurnPage; loading?: boolean; error?: string };
  turnNavigation?: { intent: number; pending?: string; error?: string };
  settings?: SessionPersistentState;
  /** Exact acknowledged MessageIds awaiting projection reconciliation, not queue authority. */
  submissions?: readonly Submission[];
  /** Current-generation turn/start or turn/steer requests awaiting an outcome.
   * Transport ownership only, including unsent requests in the bounded pipeline. */
  inboundRequests?: number;
  modelIntent?: { config: import('../../../protocol/app-server/v57').SessionModelConfig; phase: 'waiting' | 'applying' | 'failed'; error?: string };
  modelMutation?: { generation: number; status: 'in-flight' | 'acknowledged' | 'uncertain' };
  cancellation?: { attemptId?: string; status: 'in-flight' | 'acknowledged' | 'uncertain' };
  error?: string;
}
/** Exists only after `inbound_accepted` names the server MessageId, which is its
 * sole identity; settles when an authoritative snapshot contains that MessageId. */
export interface Submission {
  messageId: string;
  content: readonly UserInputBlock[];
}
/** Settled local outcome of one CAS-bound native Goal control. A known outcome is
 * not projection convergence: `observed` reports whether an authoritative snapshot
 * read after the outcome succeeded. `uncertain` means the response was lost;
 * `obsolete` means the connection or attachment changed and nothing may apply. */
export type GoalControlOutcome =
  | { status: 'applied'; observed: boolean }
  | { status: 'rejected'; reason: string; observed: boolean }
  | { status: 'uncertain' }
  | { status: 'obsolete' };
export interface UncertainOperation {
  /** Known lineage commit with uncertain durability, distinct from a lost ACK. */
  committedNode?: { sessionId: string; nodeId: string };
  diagnostic?: string;
  uploadOperationId?: string;
  compactionRequestId?: string;
  id: string;
  method: Request1['method'];
  sessionId?: string;
  interactionKey?: string;
  generation: number;
}
/** The latest native automatic repair receipt of one configuration source
 * (Issue #464). `receipt` is absent when native retains none for the
 * source — withdrawn, never recorded, or forgotten by its finite ledger —
 * which never says the source is valid. `authorityId` is the negotiated App Server
 * authority that announced it: a receipt id names one repair occurrence
 * within its authority only, so equal ids from distinct authorities are
 * distinct repairs. `sequence` is the native repair-ledger sequence the
 * observation is current at — a notice's `sequence` or a read's
 * `repair_sequence` — which alone orders two observations of a source; a
 * receipt id never does. */
export interface SourceRepairObservation {
  authorityId: string;
  target: SourceTarget;
  receipt?: ConfigRepairReceipt;
  sequence: string;
}
export interface ClientView {
  configuration?: Readonly<Record<string, ConfigurationApplication>>;
  configurationObservation?: Readonly<Record<string,{version:string;phase:'deferred'|'loading'|'unavailable';error?:string}>>;
  /** Source repair observations of this connection, by `sourceRepairKey`,
   * least recently observed first. Source-scoped: never keyed by a Session or
   * an application scope, replaced on every connection, whose server
   * announces each latest receipt again, and finite: the User and
   * `SOURCE_REPAIR_WORKSPACES` Workspaces (`retainSourceRepair`). Repair is
   * not application: `configuration` reports that. */
  sourceRepairs?: Readonly<Record<string, SourceRepairObservation>>;
  authorityRevision?: number;
  /** Native AppServerHost identity; unlike generation, stable across reconnect. */
  authorityId?: string;
  detached?: readonly DetachedEvidence[];
  endpoint?: string;
  connection: ConnectionState;
  generation: number;
  capabilities?: ServerCapabilities;
  error?: string;
  sessions: readonly SessionSummary[];
  nextOffset?: number | null;
  views: Readonly<Record<string, SessionView>>;
  uncertain: readonly UncertainOperation[];
  interactionOperations: Readonly<Record<string, { sessionId: string; status: 'in-flight' | 'uncertain' | 'acknowledged' }>>;
}
/** Read-only historical evidence. Never consulted by attachment/control admission. */
export interface DetachedEvidence {
  authority: string;
  authorityId?: string;
  operations: readonly UncertainOperation[];
  sessions: readonly (Pick<SessionView, 'id' | 'error' | 'modelMutation' | 'cancellation'> & { deletion?: 'uncertain' | SessionView['deletionRecovery'] })[];
}
/** An operation-owned proof, re-observed after transport backpressure and checked
 * synchronously again before send. The transport never interprets Host policy.
 * `validate` must stop its reads when `signal` aborts; the transport never waits for it. */
export interface OperationAdmission {
  /** Transport evidence only; called exactly when native dispatch starts. */
  sent?: () => void;
  current: () => boolean;
  validate: (signal: AbortSignal) => Promise<boolean>;
}
export const interactionKey = (ref: InteractionRef) => JSON.stringify([ref.conversation_id, ref.interaction_id]);
/** One correlated App Server request of the connection's bounded pipeline.
 * No call is ever retried; `dispatchCurrent` is re-checked adjacent to send. */
export type ConnectionRequest = <T extends MethodResult['type']>(
  operation: Request1,
  expected: T,
  acknowledged?: (result: Extract<MethodResult, { type: T }>) => void,
  dispatchCurrent?: (() => boolean) | OperationAdmission,
  observationSignal?: AbortSignal,
) => Promise<Extract<MethodResult, { type: T }>>;
/** The browser WebSocket surface one connection uses. */
export interface Socket {
  readonly readyState: number;
  onopen: ((event: Event) => unknown) | null;
  onmessage: ((event: MessageEvent) => unknown) | null;
  onclose: ((event: CloseEvent) => unknown) | null;
  onerror: ((event: Event) => unknown) | null;
  send(data: string): void;
  close(): void;
}
export type SocketFactory = (url: string, protocols: string[]) => Socket;

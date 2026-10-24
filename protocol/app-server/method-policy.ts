/**
 * The one exhaustive App Server method policy shared by every client
 * (Issue #487).
 *
 * Each generated method states, deliberately, what a lost response may mean
 * and which bounded client lane it occupies:
 *
 * - `read`: pure observation; a lost response is safely retryable.
 * - `connection_local`: state that dies with this connection (negotiation,
 *   subscriptions, cancellation of this connection's own requests); never an
 *   authoritative product/runtime change.
 * - `side_effecting`: may have committed when its response is lost, so the
 *   outcome is uncertain and nothing is replayed.
 *
 * Lanes keep observation and admission from occupying the slots needed to
 * stop or inspect work (`control`), and `unbounded` marks domain settlement
 * that has no RPC response deadline. Each transport keeps its own queue,
 * backpressure and lifetime implementation; only these semantics are shared.
 *
 * Pure and browser-safe: generated types only.
 */
import type { Request1 } from './v57.ts';

export type MethodName = Request1['method'];
export type MethodEffect = 'read' | 'side_effecting' | 'connection_local';
export type RequestLane = 'observation' | 'control' | 'rpc';

interface MethodPolicy {
  readonly effect: MethodEffect;
  /** Absent: the ordinary `rpc` lane. */
  readonly lane?: Exclude<RequestLane, 'rpc'>;
  /** Domain settlement with no RPC response deadline. */
  readonly unbounded?: true;
  /** Parameter-sensitive reads of an otherwise effectful method. */
  readonly readWhen?: (request: Request1) => boolean;
}

export const METHOD_POLICY: Readonly<Record<MethodName, MethodPolicy>> = Object.freeze({
  'auxiliary/classify': { effect: 'side_effecting' },
  'auxiliary/btw': { effect: 'side_effecting' },
  'auxiliary/status': { effect: 'read' },
  'auxiliary/result': { effect: 'read' },
  'auxiliary/submissionLookup': { effect: 'read' },
  'auxiliary/cancel': { effect: 'side_effecting', lane: 'control' },
  'auxiliary/discard': { effect: 'side_effecting' },
  'session/switchNode': { effect: 'side_effecting' },
  'session/transcript': { effect: 'read' },
  'session/history': { effect: 'read' },
  'session/historyContentRead': { effect: 'read' },
  'session/historyPage': { effect: 'read' },
  'session/historyValueRead': { effect: 'read' },
  'session/statistics': { effect: 'read' },
  'session/traceHistory': { effect: 'read' },
  'session/traceHistoryDetail': { effect: 'read' },
  'session/tracePage': { effect: 'read' },
  'session/traceValueRead': { effect: 'read' },
  'session/traceContentRead': { effect: 'read' },
  'session/turns': { effect: 'read' },
  'agent/transcript': { effect: 'read' },
  'agent/turns': { effect: 'read' },
  'agent/trace': { effect: 'read' },
  'agent/traceLocateTool': { effect: 'read' },
  'agent/traceDetail': { effect: 'read' },
  'agent/statistics': { effect: 'read' },
  'session/trace': { effect: 'read' },
  'session/traceLocateTool': { effect: 'read' },
  // Inspection detail is a pure historical read: it advances no cursor,
  // consumes no pending work, and settles nothing, so a lost response is
  // safely retryable.
  'session/traceDetail': { effect: 'read' },
  'session/projections': { effect: 'read' },
  'session/projectionDetail': { effect: 'read' },
  'session/forkProjection': { effect: 'side_effecting' },
  'artifact/read': { effect: 'read' },
  // Bounded pure reads under transport-granted delivery access.
  'delivery/read': { effect: 'read' },
  'delivery/locate': { effect: 'read' },
  // Names an in-flight request of this connection only.
  'delivery/cancel': { effect: 'connection_local', lane: 'control' },
  'session/uploadPrepare': { effect: 'side_effecting' },
  'session/uploadStatus': { effect: 'read' },
  'configuration/sourcesRead': { effect: 'read' },
  'mcp/probe': { effect: 'read', unbounded: true },
  'session/effectiveConfiguration': { effect: 'read' },
  'session/effectiveConfigurationValueRead': { effect: 'read' },
  'session/effectiveConfigurationPage': { effect: 'read' },
  'configuration/sourceWrite': { effect: 'side_effecting' },
  // Exact scoped Workflow file reads and offline candidate checks: they
  // execute, prepare and write nothing (Issue #491).
  'workflow/list': { effect: 'read' },
  'workflow/read': { effect: 'read' },
  'workflow/modelChoices': { effect: 'read' },
  'workflow/check': { effect: 'read' },
  'session/model': { effect: 'read' },
  'session/models': { effect: 'read' },
  'session/setModel': { effect: 'side_effecting' },
  'session/selectModel': { effect: 'side_effecting' },
  'resources/read': { effect: 'read' },
  'context/compact': { effect: 'side_effecting', unbounded: true },
  'workflow/scheduled/getRecovery': { effect: 'read', lane:'control' },
  'workflow/scheduled/cancel': { effect: 'side_effecting', lane:'control' },
  'workflow/scheduled/reconcile': { effect: 'side_effecting', lane:'control' },
  'schedule/list': { effect: 'read' },
  'schedule/get': { effect: 'read' },
  'schedule/command': { effect: 'side_effecting' },
  'schedule/recoverStorage': { effect: 'side_effecting' },
  'goal/control': { effect: 'side_effecting', readWhen: request => request.method === 'goal/control' && request.params.control.action === 'show' },
  'job/status': { effect: 'read' },
  'job/list': { effect: 'read' },
  'job/wait': { effect: 'read', unbounded: true },
  'agent/list': { effect: 'read' },
  'agent/observe': { effect: 'read', lane: 'observation' },
  'agent/observeCancel': { effect: 'connection_local', lane: 'control' },
  'agent/artifactRead': { effect: 'read' },
  'agent/deliveryRead': { effect: 'read' },
  'agent/deliveryLocate': { effect: 'read' },
  'agent/sendMessage': { effect: 'side_effecting' },
  // Waiting observes its captured activation; a lost response cannot mutate it.
  // The client never automatically retries reads or mutations.
  'agent/wait': { effect: 'read', unbounded: true },
  'job/cancel': { effect: 'side_effecting', lane: 'control' },
  'agent/status': { effect: 'read' },
  'agent/interrupt': { effect: 'side_effecting', lane: 'control' },
  // Negotiation and subscriptions die with this connection; neither changes
  // authoritative product/runtime state. Re-establish them after reconnect.
  initialize: { effect: 'connection_local' },
  'server/info': { effect: 'read' },
  'server/diagnostics': { effect: 'read' },
  'session/list': { effect: 'read' },
  'session/exportPrepare': { effect: 'connection_local' },
  'session/summary': { effect: 'read' },
  'session/create': { effect: 'side_effecting' },
  'session/read': { effect: 'read' },
  'session/name': { effect: 'side_effecting' },
  'session/tree': { effect: 'read' },
  'session/node': { effect: 'read' },
  'session/nodeValueRead': { effect: 'read' },
  'session/nodeOriginHistory': { effect: 'read' },
  'session/boundaries': { effect: 'read' },
  'session/fork': { effect: 'side_effecting' },
  'session/branch': { effect: 'side_effecting' },
  'session/deletePreview': { effect: 'read' },
  'session/delete': { effect: 'side_effecting' },
  'session/recoverDeletion': { effect: 'side_effecting' },
  'session/attach': { effect: 'side_effecting' },
  'session/detach': { effect: 'side_effecting', lane: 'control' },
  'session/checkpoint': { effect: 'read' },
  'configuration/sourceValueRead': { effect: 'read' },
  'configuration/sourcePage': { effect: 'read' },
  'session/attachmentStatus': { effect: 'read' },
  'session/editorRead': { effect: 'read' },
  'conversation/contentRead': { effect: 'read' },
  'conversation/checkpointPage': { effect: 'read' },
  'conversation/checkpointValueRead': { effect: 'read' },
  'session/subscribe': { effect: 'connection_local' },
  'turn/start': { effect: 'side_effecting' },
  'turn/steer': { effect: 'side_effecting' },
  'inbound/edit': { effect: 'side_effecting' },
  'inbound/remove': { effect: 'side_effecting' },
  'turn/cancel': { effect: 'side_effecting', lane: 'control' },
  'interaction/respond': { effect: 'side_effecting' },
  'interaction/cancel': { effect: 'side_effecting', lane: 'control' },
  'session/settings': { effect: 'read' },
  'session/settingsValueRead': { effect: 'read' },
  'session/configuration': { effect: 'read' },
  'session/configurationValueRead': { effect: 'read' },
  'configuration/reconcile': { effect: 'side_effecting' },
  'session/adoptConfiguration': { effect: 'side_effecting' },
} satisfies Record<MethodName, MethodPolicy>);

/** What a lost response to this exact request may mean. */
export function requestEffect(request: Request1): MethodEffect {
  const policy = METHOD_POLICY[request.method];
  return policy.readWhen?.(request) ? 'read' : policy.effect;
}

/** The bounded client lane one method occupies. */
export function requestLane(method: MethodName): RequestLane {
  return METHOD_POLICY[method].lane ?? 'rpc';
}

/** Whether one method's response is bounded by the ordinary RPC deadline. */
export function hasResponseDeadline(method: MethodName): boolean {
  return METHOD_POLICY[method].unbounded !== true;
}

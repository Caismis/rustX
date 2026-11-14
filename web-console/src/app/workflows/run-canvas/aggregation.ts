import type {
  RuntimeClientInteractionHeader,
  WorkflowExecutionOutcome,
  WorkflowInstanceView,
  WorkflowNodeInstance,
  WorkflowNodeKind,
  WorkflowNodeTraceState,
  WorkflowRunId,
  WorkflowRunView,
  WorkflowState,
  WorkflowTraceNode,
  WorkflowWait,
} from '../../../../../protocol/app-server/v78';
import type { BlockPath } from '../document/model';
import type { SceneNode } from '../ui/editor/scene';
import {
  blockPathFromDefinition,
  qualifiedNodeViewId,
  resolveSceneNodeId,
  sameWorkflowRunId,
  visitIdentityKey,
} from './identity';

export type AggregateNodeStatus =
  | 'running'
  | 'waiting'
  | 'draining'
  | 'pending'
  | WorkflowExecutionOutcome
  | 'incomplete';

export interface NormalizedVisitRecord {
  readonly key: string;
  readonly viewId: string;
  readonly order: number;
  readonly instance: WorkflowNodeInstance;
  readonly kind?: WorkflowNodeKind | null;
  readonly state: WorkflowNodeTraceState;
  readonly reviewPending: boolean;
  readonly traceNode?: WorkflowTraceNode;
  readonly instanceView?: WorkflowInstanceView;
}

export interface NodeRunOverlay {
  readonly viewId: string;
  readonly nodeId: string;
  readonly blockPath: BlockPath;
  readonly status: AggregateNodeStatus;
  readonly activeCount: number;
  readonly waitingCount: number;
  readonly pendingCount: number;
  readonly failedCount: number;
  readonly incompleteCount: number;
  readonly completedCount: number;
  readonly visitCount: number;
  readonly omittedVisits: number;
  readonly waitingReasons: readonly WorkflowWait[];
  readonly visits: readonly NormalizedVisitRecord[];
}

export interface RunOverlaySummary {
  readonly byViewId: ReadonlyMap<string, NodeRunOverlay>;
  readonly totalVisitsSeen: number;
  readonly omittedInstances: number;
  readonly traceIncomplete: boolean;
}

export const DEFAULT_MAX_VISITS_PER_NODE = 200;

/** Severity rank for aggregating concurrent / repeated visits on one qualified
 * definition node: active states always outrank completed/settled ones. */
const STATUS_PRECEDENCE: Record<AggregateNodeStatus, number> = {
  running: 100,
  waiting: 90,
  draining: 80,
  pending: 70,
  failed: 60,
  timed_out: 55,
  denied: 50,
  cancelled: 45,
  outcome_unknown: 40,
  incomplete: 30,
  completed: 10,
};

export function statusOfTraceState(state: WorkflowNodeTraceState | WorkflowState, reviewPending = false): AggregateNodeStatus {
  if (reviewPending && state.type !== 'settled' && state.type !== 'incomplete') return 'waiting';
  switch (state.type) {
    case 'running': return 'running';
    case 'waiting': return 'waiting';
    case 'draining': return 'draining';
    case 'pending': return 'pending';
    case 'incomplete': return 'incomplete';
    case 'settled': return state.outcome;
  }
}

export function isActiveAggregateStatus(status: AggregateNodeStatus): boolean {
  return status === 'running' || status === 'waiting' || status === 'draining' || status === 'pending';
}

export function isAbnormalAggregateStatus(status: AggregateNodeStatus): boolean {
  return status === 'failed'
    || status === 'timed_out'
    || status === 'denied'
    || status === 'cancelled'
    || status === 'outcome_unknown'
    || status === 'incomplete';
}

function mergeVisitState(
  traceState: WorkflowNodeTraceState | undefined,
  liveState: WorkflowNodeTraceState | undefined,
): WorkflowNodeTraceState {
  if (!traceState) return liveState!;
  if (!liveState) return traceState;
  // Once settled, a concrete visit never un-settles.
  if (traceState.type === 'settled') return traceState;
  if (liveState.type === 'settled') return liveState;
  // Live waiting/running outranks a stale trace incomplete/pending marker.
  return liveState;
}

function pendingReviewVisitKeys(
  pendingInteractions: readonly RuntimeClientInteractionHeader[] | undefined,
  runId?: WorkflowRunId,
): Set<string> {
  const keys = new Set<string>();
  if (!pendingInteractions?.length) return keys;
  for (const item of pendingInteractions) {
    if (item.kind !== 'review') continue;
    const target = item.workflow_node ?? (item.body?.kind.type === 'review' ? item.body.kind.review.instance : undefined);
    if (target && (!runId || sameWorkflowRunId(target.block.run, runId))) keys.add(visitIdentityKey(target));
  }
  return keys;
}

export function effectiveTraceVisitState(
  state: WorkflowNodeTraceState,
  run: WorkflowRunView | undefined,
  liveInstance: WorkflowInstanceView | undefined,
): WorkflowNodeTraceState {
  if (state.type === 'settled' || state.type === 'incomplete') return state;
  if (!run || run.state.type === 'settled') return { type: 'incomplete' };
  if (!liveInstance || liveInstance.state.type === 'settled') return { type: 'incomplete' };
  return liveInstance.state;
}

export function aggregateRunOverlay(options: {
  readonly runId?: WorkflowRunId;
  readonly run?: WorkflowRunView | null;
  readonly traceNodes?: readonly WorkflowTraceNode[];
  readonly selectedTraceNode?: WorkflowTraceNode;
  readonly pendingInteractions?: readonly RuntimeClientInteractionHeader[];
  readonly sceneNodes?: readonly SceneNode[];
  readonly selectedVisit?: WorkflowNodeInstance | null;
  readonly selectedVisitKey?: string | null;
  readonly maxVisitsPerNode?: number;
  readonly traceIncomplete?: boolean;
}): RunOverlaySummary {
  const maxVisitsPerNode = Math.max(1, options.maxVisitsPerNode ?? DEFAULT_MAX_VISITS_PER_NODE);
  const run = options.run && (!options.runId || sameWorkflowRunId(options.run.id, options.runId))
    ? options.run
    : undefined;
  const targetRunId = options.runId ?? run?.id;
  const runSettled = run?.state.type === 'settled';
  // Pending interaction badges require a live, non-settled authoritative WorkflowRunView.
  const reviewKeys = run && !runSettled
    ? pendingReviewVisitKeys(options.pendingInteractions, targetRunId)
    : new Set<string>();
  const selectedKey = options.selectedVisitKey !== undefined
    ? (options.selectedVisitKey ?? undefined)
    : (options.selectedVisit ? visitIdentityKey(options.selectedVisit) : undefined);
  let incompleteIdentity = false;

  const liveByKey = new Map<string, WorkflowInstanceView>();
  for (const instanceView of run?.instances ?? []) {
    if (instanceView.node == null) continue;
    if (instanceView.visit == null) { incompleteIdentity = true; continue; }
    if (targetRunId && !sameWorkflowRunId(instanceView.block.run, targetRunId)) continue;
    const key = visitIdentityKey({
      block: instanceView.block,
      node: instanceView.node,
      visit: instanceView.visit,
    });
    liveByKey.set(key, instanceView);
  }

  const traceInput = options.selectedTraceNode
    && !(options.traceNodes ?? []).some(node => visitIdentityKey(node.instance) === visitIdentityKey(options.selectedTraceNode!.instance))
    ? [...(options.traceNodes ?? []), options.selectedTraceNode]
    : (options.traceNodes ?? []);

  const byVisitKey = new Map<string, NormalizedVisitRecord>();
  let order = 0;

  for (const traceNode of traceInput) {
    if (targetRunId && !sameWorkflowRunId(traceNode.instance.block.run, targetRunId)) continue;
    const viewId = options.sceneNodes
      ? resolveSceneNodeId(options.sceneNodes, traceNode.instance)
      : qualifiedNodeViewId(traceNode.instance.block.definition.blocks, traceNode.instance.node);
    if (!viewId) continue;
    const key = visitIdentityKey(traceNode.instance);
    const liveInst = liveByKey.get(key);
    const baseState = effectiveTraceVisitState(traceNode.state, run, liveInst);
    const reviewPending = !!run && !runSettled && baseState.type !== 'settled' && reviewKeys.has(key);
    byVisitKey.set(key, {
      key,
      viewId,
      order: order++,
      instance: traceNode.instance,
      kind: traceNode.kind,
      state: baseState,
      reviewPending,
      traceNode,
    });
  }

  for (const instanceView of run?.instances ?? []) {
    if (instanceView.node == null || instanceView.visit == null) continue;
    if (targetRunId && !sameWorkflowRunId(instanceView.block.run, targetRunId)) continue;
    const instance: WorkflowNodeInstance = {
      block: instanceView.block,
      node: instanceView.node,
      visit: instanceView.visit,
    };
    const viewId = options.sceneNodes
      ? resolveSceneNodeId(options.sceneNodes, instance)
      : qualifiedNodeViewId(instance.block.definition.blocks, instance.node);
    if (!viewId) continue;
    const key = visitIdentityKey(instance);
    const existing = byVisitKey.get(key);
    const mergedState = mergeVisitState(existing?.state, effectiveTraceVisitState(instanceView.state, run, instanceView));
    const reviewPending = !runSettled && mergedState.type !== 'settled' && reviewKeys.has(key);
    byVisitKey.set(key, {
      key,
      viewId,
      order: existing?.order ?? order++,
      instance: existing?.instance ?? instance,
      kind: existing?.kind ?? instanceView.kind,
      state: mergedState,
      reviewPending,
      traceNode: existing?.traceNode,
      instanceView,
    });
  }

  interface Bucket {
    nodeId: string;
    blockPath: BlockPath;
    status: AggregateNodeStatus;
    bestRank: number;
    activeCount: number;
    waitingCount: number;
    pendingCount: number;
    failedCount: number;
    incompleteCount: number;
    completedCount: number;
    visitCount: number;
    waitingReasons: Set<WorkflowWait>;
    records: NormalizedVisitRecord[];
  }

  const buckets = new Map<string, Bucket>();

  for (const record of byVisitKey.values()) {
    const status = statusOfTraceState(record.state, record.reviewPending);
    const rank = STATUS_PRECEDENCE[status];
    let bucket = buckets.get(record.viewId);
    if (!bucket) {
      bucket = {
        nodeId: record.instance.node,
        blockPath: blockPathFromDefinition(record.instance.block.definition),
        status,
        bestRank: rank,
        activeCount: 0,
        waitingCount: 0,
        pendingCount: 0,
        failedCount: 0,
        incompleteCount: 0,
        completedCount: 0,
        visitCount: 0,
        waitingReasons: new Set(),
        records: [],
      };
      buckets.set(record.viewId, bucket);
    }

    bucket.visitCount += 1;
    if (rank > bucket.bestRank) {
      bucket.bestRank = rank;
      bucket.status = status;
    }

    if (record.reviewPending) {
      bucket.waitingReasons.add('review');
    } else if (record.state.type === 'waiting') {
      bucket.waitingReasons.add(record.state.reason);
    }

    if (isActiveAggregateStatus(status)) {
      bucket.activeCount += 1;
      if (status === 'waiting') bucket.waitingCount += 1;
      if (status === 'pending') bucket.pendingCount += 1;
    } else if (status === 'completed') {
      bucket.completedCount += 1;
    } else if (status === 'incomplete') {
      bucket.incompleteCount += 1;
    } else {
      bucket.failedCount += 1;
    }

    bucket.records.push(record);
  }

  const byViewId = new Map<string, NodeRunOverlay>();
  for (const [viewId, bucket] of buckets) {
    let retained = bucket.records;
    if (retained.length > maxVisitsPerNode) {
      // Preserve active visits, explicitly selected visit, and most recent visits under a strict bound.
      const pinnedKeys = new Set<string>();
      const prioritized = retained.filter(item => item.key === selectedKey);
      for (const item of prioritized) pinnedKeys.add(item.key);
      for (const item of retained) {
        const status = statusOfTraceState(item.state, item.reviewPending);
        if (isActiveAggregateStatus(status) && !pinnedKeys.has(item.key)) {
          pinnedKeys.add(item.key);
          prioritized.push(item);
        }
      }
      const remainingBudget = Math.max(0, maxVisitsPerNode - prioritized.length);
      const tail = remainingBudget > 0
        ? retained.filter(item => !pinnedKeys.has(item.key)).slice(-remainingBudget)
        : [];
      retained = [...prioritized.slice(0, maxVisitsPerNode), ...tail].sort((a, b) => a.order - b.order);
    }

    byViewId.set(viewId, {
      viewId,
      nodeId: bucket.nodeId,
      blockPath: bucket.blockPath,
      status: bucket.status,
      activeCount: bucket.activeCount,
      waitingCount: bucket.waitingCount,
      pendingCount: bucket.pendingCount,
      failedCount: bucket.failedCount,
      incompleteCount: bucket.incompleteCount,
      completedCount: bucket.completedCount,
      visitCount: bucket.visitCount,
      omittedVisits: Math.max(0, bucket.visitCount - retained.length),
      waitingReasons: [...bucket.waitingReasons],
      visits: retained,
    });
  }

  return {
    byViewId,
    totalVisitsSeen: byVisitKey.size,
    omittedInstances: run?.omitted_instances ?? 0,
    traceIncomplete: Boolean(options.traceIncomplete || incompleteIdentity),
  };
}

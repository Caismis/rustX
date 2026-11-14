import '@xyflow/react/dist/base.css';
import { ReactFlowProvider } from '@xyflow/react';
import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { createPortal } from 'react-dom';
import type {
  CheckpointReadCut,
  RuntimeClientInteractionHeader,
  TraceFieldSource,
  WorkflowNodeInstance,
  WorkflowRunView,
  WorkflowRunId,
  WorkflowTraceNode,
} from '../../../../../protocol/app-server/v78';
import { workflowKey } from '../../../bindings/workflow-children';
import { checkpointReadCutKey } from '../../../../../protocol/app-server/content-identity';
import { traceInspectionHead, type TraceFieldReader } from '../../../client/trace';
import { readWorkflowNodeBody } from '../../../client/trace-content';
import { useTranslation } from '../../../locale/react';
import { JsonTree, type JsonTreeLabels } from '../../../presentation/primitives/JsonTree';
import { StateDot, type StateDotState } from '../../../presentation/primitives/StateDot';
import { useModalFocus } from '../../../presentation/primitives/useModalFocus';
import { useSubagents } from '../../agent/subagent-context';
import { workflowStatus } from '../../agent/workflow-state';
import { formatDurationMillis, wireCount } from '../../trajectory/timeline';
import { blockAt, nodeAt, type BlockPath, type WorkflowDocument } from '../document/model';
import { nodePresentation } from '../document/geometry';
import { Canvas, type Problems, type Selection } from '../ui/editor/Canvas';
import { projectScene } from '../ui/editor/scene';
import {
  aggregateRunOverlay,
  effectiveTraceVisitState,
  statusOfTraceState,
  type AggregateNodeStatus,
  type NodeRunOverlay,
  type NormalizedVisitRecord,
} from './aggregation';
import type { WorkflowRunCanvasTarget } from './context';
import type { WorkflowRunSnapshotTransport } from './contract';
import type { ResolvedWorkflowTraceContext, TraceContextStatus } from './trace-lookup';
import {
  blockPathFromDefinition,
  qualifiedNodeViewId,
  resolveSceneNodeId,
  sameWorkflowRunId,
  visitIdentityKey,
} from './identity';
import {
  DEFAULT_MAX_TRACE_NODES,
  DEFAULT_TRACE_BATCH_PAGES,
  readBoundedTraceWorkflowNodes,
  WorkflowRunSnapshotSession,
  type RunSnapshotPhase,
} from './loader';
import { viewId } from '../ui/editor/selection';
import css from './RunCanvas.module.css';

const EMPTY_PROBLEMS: Problems = { nodes: new Map(), edges: new Set() };
const EMPTY_DRAFTS = {};
const NOOP_EDIT = () => {};
const NOOP_DUPLICATE = () => undefined;
const NOOP_STATE = () => ({ generation: 0 });

function jsonLabels(tx: ReturnType<typeof useTranslation>): JsonTreeLabels {
  return {
    copyValue: tx('trajectory:copy.copy-value'),
    copyJson: tx('trajectory:copy.copy-json'),
    copyPath: tx('trajectory:copy.copy-path'),
    copyPrettyJson: tx('trajectory:copy.copy-pretty-json'),
    copyCompactJson: tx('trajectory:copy.copy-compact-json'),
    copied: tx('trajectory:copied'),
    copyFailed: tx('trajectory:copy.copy-failed'),
    collapseNode: tx('trajectory:collapse'),
    expandNode: tx('trajectory:expand'),
    copyButtonTitle: action => action,
  };
}

function dotForAggregateStatus(status: AggregateNodeStatus): StateDotState {
  switch (status) {
    case 'completed': return 'done';
    case 'running':
    case 'draining': return 'ongoing';
    case 'waiting': return 'warning';
    case 'failed':
    case 'timed_out':
    case 'denied': return 'error';
    default: return 'idle';
  }
}

function findStartEntry(block: ReturnType<typeof blockAt> | undefined) {
  if (!block) return undefined;
  return Object.entries(block.nodes).find(([, def]) => def.type === 'start');
}

function selectionFromVisit(visit?: WorkflowNodeInstance, readyDoc?: WorkflowDocument): Selection {
  if (!visit) return undefined;
  const path = blockPathFromDefinition(visit.block.definition);
  const block = readyDoc ? blockAt(readyDoc, path) : undefined;
  if (block) {
    const startEntry = findStartEntry(block);
    if (startEntry && startEntry[0] === visit.node) return { kind: 'start', path };
    if (nodeAt(block, visit.node)) return { kind: 'node', id: visit.node, path };
  }
  if (visit.node === 'start') return { kind: 'start', path };
  return { kind: 'node', id: visit.node, path };
}

function defaultVisitForNode(overlay: NodeRunOverlay | undefined, preferredKey?: string): NormalizedVisitRecord | undefined {
  if (!overlay?.visits.length) return undefined;
  if (preferredKey) {
    const exact = overlay.visits.find(item => item.key === preferredKey);
    if (exact) return exact;
  }
  return overlay.visits.find(item => ['running', 'waiting', 'draining', 'pending'].includes(statusOfTraceState(item.state, item.reviewPending)))
    ?? overlay.visits[overlay.visits.length - 1];
}

let nextReaderToken = 1;
const readerTokens = new WeakMap<TraceFieldReader, number>();
function readerIdentityToken(reader?: TraceFieldReader): string {
  if (!reader) return 'none';
  let token = readerTokens.get(reader);
  if (!token) {
    token = nextReaderToken++;
    readerTokens.set(reader, token);
  }
  return String(token);
}

function durableChildRef(visit: NormalizedVisitRecord | undefined): { childAgentId: string; activationId: string } | undefined {
  if (!visit) return undefined;
  const iv = visit.instanceView;
  if (iv?.child && iv.child_agent_id) {
    return { childAgentId: iv.child_agent_id, activationId: iv.child };
  }
  const tn = visit.traceNode;
  if (tn?.child && tn.child_agent_id) {
    return { childAgentId: tn.child_agent_id, activationId: tn.child };
  }
  return undefined;
}

function VisitFieldPanel({
  visitKey,
  fieldKind,
  hasTraceNode,
  traceStatus,
  traceError,
  field,
  omitted,
  reader,
  cut,
  owned,
}: {
  visitKey: string;
  fieldKind: 'input' | 'output';
  hasTraceNode: boolean;
  traceStatus?: TraceContextStatus;
  traceError?: string;
  field?: TraceFieldSource | null;
  omitted: boolean;
  reader?: TraceFieldReader;
  cut?: CheckpointReadCut;
  owned: () => boolean;
}) {
  const tx = useTranslation();
  const [state, setState] = useState<{ key: string; loading: boolean; value?: unknown; error?: string }>({ key: '', loading: false });
  const cutKey = cut ? checkpointReadCutKey(cut) : 'none';
  const requestKey = `${visitKey}:${fieldKind}:${readerIdentityToken(reader)}:${cutKey}:${field ? JSON.stringify(field) : 'none'}`;
  const latest = useRef(requestKey);
  latest.current = requestKey;

  useEffect(() => {
    if (!field || !reader || !cut || omitted) return;
    let mounted = true;
    const isCurrent = () => mounted && latest.current === requestKey && owned();
    setState({ key: requestKey, loading: true });
    void readWorkflowNodeBody(reader, cut, field, isCurrent)
      .then(value => {
        if (isCurrent()) setState({ key: requestKey, loading: false, value });
      })
      .catch(error => {
        if (mounted && latest.current === requestKey && owned()) {
          setState({ key: requestKey, loading: false, error: error instanceof Error ? error.message : String(error) });
        }
      });
    return () => { mounted = false; };
  }, [requestKey, field, reader, cut, omitted, owned]);

  if (omitted) {
    return <p className={css.muted}>{tx('trajectory:workflow.body-omitted')}</p>;
  }
  if (!hasTraceNode) {
    if (traceStatus === 'loading') {
      return <p role="status" className={css.muted}>{tx('trajectory:details.loading')}</p>;
    }
    if (traceStatus === 'error') {
      return <p role="alert" className={css.muted}>{traceError ?? tx('workflows:run-canvas.unavailable')}</p>;
    }
    return <p className={css.muted}>{tx('workflows:run-canvas.no-trace-field')}</p>;
  }
  if (!field) {
    return <p className={css.muted}>{tx(fieldKind === 'input' ? 'trajectory:workflow.input-not-recorded' : 'trajectory:workflow.output-not-recorded')}</p>;
  }
  if (state.key === requestKey && state.error) {
    return <p role="alert" className={css.muted}>{state.error}</p>;
  }
  if (state.key === requestKey && state.loading) {
    return <p role="status" className={css.muted}>{tx('trajectory:details.loading')}</p>;
  }

  let resolved: unknown;
  if (state.key === requestKey && !state.loading && state.value !== undefined) {
    resolved = state.value;
  } else if (!reader || !cut) {
    return <p className={css.muted}>{tx('workflows:run-canvas.no-trace-field')}</p>;
  } else {
    return <p role="status" className={css.muted}>{tx('trajectory:details.loading')}</p>;
  }

  return typeof resolved === 'object' && resolved !== null
    ? <JsonTree data={resolved} label={fieldKind === 'input' ? tx('workflows:run-canvas.input') : tx('workflows:run-canvas.output')} labels={jsonLabels(tx)} />
    : <pre className={css.muted}>{String(resolved)}</pre>;
}

export function WorkflowRunCanvas({
  target,
  liveRun,
  liveRuns,
  pendingInteractions,
  resolvedTrace,
  traceStatus,
  traceError,
  onClose,
  createTransport,
  onResolveRunId,
  organizeFn,
}: {
  target: WorkflowRunCanvasTarget;
  liveRun?: WorkflowRunView;
  liveRuns?: readonly WorkflowRunView[];
  pendingInteractions?: readonly RuntimeClientInteractionHeader[];
  resolvedTrace?: ResolvedWorkflowTraceContext | null;
  traceStatus?: TraceContextStatus;
  traceError?: string;
  onClose: () => void;
  createTransport?: (target: WorkflowRunCanvasTarget) => WorkflowRunSnapshotTransport | null;
  organizeFn?: (doc: WorkflowDocument) => Promise<WorkflowDocument>;
  onResolveRunId?: (runId: WorkflowRunId) => void;
}) {
  const tx = useTranslation();
  const scope = useSubagents();
  const titleId = useId();
  const dialogRef = useRef<HTMLElement | null>(null);
  // The conversation owner restores focus only for an explicit, still-owned close.
  useModalFocus(true, dialogRef, onClose, false);

  const [pagedTraceNodes, setPagedTraceNodes] = useState<{
    ownerKey: string;
    runIdentityKey: string;
    cut: CheckpointReadCut;
    reader: TraceFieldReader;
    nodes: readonly WorkflowTraceNode[];
    incomplete: boolean;
    nextOffset: number | null;
    total: string;
    loadingMore?: boolean;
    failed?: boolean;
  } | null>(null);
  const traceDedupeRef = useRef<{ ownerKey: string; seenIds: Set<string>; seenVisits: Set<string> }>({
    ownerKey: '',
    seenIds: new Set(),
    seenVisits: new Set(),
  });
  const [retryCount, setRetryCount] = useState(0);

  const [selection, setSelection] = useState<Selection>(() => selectionFromVisit(target.selectedVisit));
  const [selectedVisitKey, setSelectedVisitKey] = useState<string | undefined>(() =>
    target.selectedVisit ? visitIdentityKey(target.selectedVisit) : undefined);
  const [focusNonce, setFocusNonce] = useState(0);
  const [visitTab, setVisitTab] = useState<'overview' | 'input' | 'output'>('overview');

  const sessionRef = useRef<WorkflowRunSnapshotSession | null>(null);
  if (!sessionRef.current) sessionRef.current = new WorkflowRunSnapshotSession(organizeFn);

  // Bounded trace node paging when opened from a Trace record whose nodes are not all supplied yet,
  // or when resolving the exact WorkflowRunId / advancing live cut while the modal remains open.
  const openTraceCutKey = target.traceCut ? checkpointReadCutKey(target.traceCut) : null;
  const hasNewerResolvedTrace = Boolean(
    resolvedTrace && (!openTraceCutKey || checkpointReadCutKey(resolvedTrace.traceCut) !== openTraceCutKey),
  );
  const effectiveTraceRoot = hasNewerResolvedTrace ? resolvedTrace!.traceRoot : (target.traceRoot ?? resolvedTrace?.traceRoot);
  const effectiveTraceCut = hasNewerResolvedTrace ? resolvedTrace!.traceCut : (target.traceCut ?? resolvedTrace?.traceCut);
  const effectiveTraceReader = hasNewerResolvedTrace ? resolvedTrace!.traceReader : (target.traceReader ?? resolvedTrace?.traceReader);

  const runIdentityKey = JSON.stringify([
    target.conversationId,
    target.runId ? workflowKey(target.runId) : (target.traceRoot?.id ?? resolvedTrace?.traceRoot.id ?? target.workflowId ?? ''),
    openTraceCutKey,
  ]);
  const pageIdentity = JSON.stringify([
    target.conversationId,
    target.runId ? workflowKey(target.runId) : (effectiveTraceRoot?.id ?? target.workflowId ?? ''),
    effectiveTraceCut ? checkpointReadCutKey(effectiveTraceCut) : null,
  ]);
  const pagedForExactCut = pagedTraceNodes?.ownerKey === pageIdentity ? pagedTraceNodes : null;
  // During an in-flight cut advancement on the same open run, keep the last coherent (nodes, cut, reader) triple together.
  const pagedForCurrent = pagedForExactCut
    ?? (pagedTraceNodes?.runIdentityKey === runIdentityKey ? pagedTraceNodes : null);
  const [exactVisitLookup, setExactVisitLookup] = useState<
    | { readonly ownerKey: string; readonly status: 'ready'; readonly node: WorkflowTraceNode | null }
    | { readonly ownerKey: string; readonly status: 'error'; readonly message: string }
    | null
  >(null);
  const [resolvedRootIdentity, setResolvedRootIdentity] = useState<{
    readonly ownerKey: string;
    readonly runId?: WorkflowRunId;
    readonly workflowId?: string;
    readonly error?: string;
  } | null>(null);
  const useStaticTargetTrace = Boolean(target.traceNodes && !pagedForCurrent && !pagedForExactCut);
  const effectiveTraceNodes = useStaticTargetTrace ? target.traceNodes : pagedForCurrent?.nodes;
  const coherentTraceCut = useStaticTargetTrace ? target.traceCut : (pagedForCurrent?.cut ?? effectiveTraceCut);
  const coherentTraceReader = useStaticTargetTrace ? target.traceReader : (pagedForCurrent?.reader ?? effectiveTraceReader);
  const traceIncomplete = useStaticTargetTrace ? (target.traceIncomplete ?? false) : (pagedForCurrent?.incomplete ?? false);
  const coherentCutKey = coherentTraceCut ? checkpointReadCutKey(coherentTraceCut) : null;
  // Never carry a pre-opened selectedTraceNode across a cut advancement.
  const targetSelectedTraceNodeAtCut = target.selectedTraceNode
    && (!openTraceCutKey || !coherentCutKey || openTraceCutKey === coherentCutKey)
    ? target.selectedTraceNode
    : undefined;
  const rootReaderToken = readerIdentityToken(effectiveTraceReader);
  const rootIdentityKey = (!target.runId && !target.selectedTraceNode
    && effectiveTraceRoot && effectiveTraceCut && effectiveTraceReader)
    ? JSON.stringify([
      target.conversationId,
      effectiveTraceRoot.id,
      checkpointReadCutKey(effectiveTraceCut),
      rootReaderToken,
      retryCount,
    ])
    : null;
  const ownedRootIdentity = useRef(rootIdentityKey);
  ownedRootIdentity.current = rootIdentityKey;
  const exactRootIdentity = rootIdentityKey && resolvedRootIdentity?.ownerKey === rootIdentityKey
    ? resolvedRootIdentity
    : null;
  const resolvedRunId = target.runId
    ?? target.selectedTraceNode?.instance.block.run
    ?? exactRootIdentity?.runId;

  const resolvedRunKey = resolvedRunId ? workflowKey(resolvedRunId) : '';
  useEffect(() => {
    if (resolvedRunId && !target.runId) onResolveRunId?.(resolvedRunId);
  }, [resolvedRunKey, onResolveRunId, target.runId]);

  const runKey = resolvedRunId ? workflowKey(resolvedRunId) : `root:${effectiveTraceRoot?.id ?? target.workflowId ?? 'workflow'}`;
  const targetIdentity = JSON.stringify([target.conversationId, runKey, openTraceCutKey]);

  // Never fall back to a stale open-time target.run snapshot; live evidence requires a current authoritative owner projection.
  const activeRun = useMemo(() => {
    if (liveRun && (!resolvedRunId || sameWorkflowRunId(liveRun.id, resolvedRunId))) return liveRun;
    if (!resolvedRunId || !liveRuns?.length) return undefined;
    return liveRuns.find(candidate => sameWorkflowRunId(candidate.id, resolvedRunId));
  }, [liveRun, liveRuns, resolvedRunId]);

  const [snapshotPhase, setSnapshotPhase] = useState<RunSnapshotPhase>({ phase: 'loading' });
  const ownedIdentity = useRef(targetIdentity);
  ownedIdentity.current = targetIdentity;
  const ownedPageIdentity = useRef(pageIdentity);
  ownedPageIdentity.current = pageIdentity;
  const lifetime = useRef(0);
  const pagingButton = useRef<HTMLButtonElement | null>(null);
  const pagingFocus = useRef<string | null>(null);
  useLayoutEffect(() => {
    if (pagingFocus.current !== pageIdentity || pagedForExactCut?.loadingMore
      || pagedForExactCut?.nextOffset !== null) return;
    pagingFocus.current = null;
    if (document.activeElement === document.body) {
      dialogRef.current?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus({ preventScroll: true });
    }
  }, [pageIdentity, pagedForExactCut?.loadingMore, pagedForExactCut?.nextOffset]);

  // Closing or retiring the owning view releases pending reads and layouts.
  useEffect(() => {
    const session = sessionRef.current ?? new WorkflowRunSnapshotSession(organizeFn);
    sessionRef.current = session;
    return () => {
      lifetime.current++;
      session.dispose();
      if (sessionRef.current === session) sessionRef.current = null;
    };
  }, []);

  // Re-sync initial selection when caller opens a different concrete visit/run target.
  const targetVisitKey = target.selectedVisit ? visitIdentityKey(target.selectedVisit) : undefined;
  useEffect(() => {
    setSelection(selectionFromVisit(target.selectedVisit));
    setSelectedVisitKey(targetVisitKey);
    setVisitTab('overview');
    setFocusNonce(value => value + 1);
  }, [targetIdentity, targetVisitKey]);

  const hasResolvedRunId = Boolean(resolvedRunId);
  const awaitingRunIdResolution = !hasResolvedRunId && Boolean(
    (rootIdentityKey && !exactRootIdentity)
    || traceStatus === 'loading',
  );

  // Resolve typed root run identity from finite trace.records.<seq>.detail when opened on a WorkflowStarted-only root.
  useEffect(() => {
    if (!rootIdentityKey || !effectiveTraceRoot || !effectiveTraceCut || !effectiveTraceReader) return;
    let active = true;
    const batchKey = rootIdentityKey;
    const fetchRoot = effectiveTraceRoot;
    const fetchCut = effectiveTraceCut;
    const fetchReader = effectiveTraceReader;
    const fetchConversationId = target.conversationId;
    const admittedLifetime = lifetime.current;
    const owned = () => active
      && lifetime.current === admittedLifetime
      && ownedRootIdentity.current === batchKey;
    const sequence = /^trace:([1-9][0-9]*)$/.exec(fetchRoot.id)?.[1];
    if (!sequence) {
      setResolvedRootIdentity({ ownerKey: batchKey, error: 'Original Workflow inspection head is unavailable.' });
      return () => { active = false; };
    }
    void fetchReader.page(fetchCut, `trace.records.${sequence}.detail`, 0, owned)
      .then(page => {
        if (!owned()) return;
        if (checkpointReadCutKey(page.read_cut) !== checkpointReadCutKey(fetchCut)) {
          throw new Error('Original Workflow inspection head cut changed.');
        }
        const head = traceInspectionHead(page, fetchRoot.id);
        if (!head || head.content.type !== 'workflow') {
          throw new Error('Original Workflow inspection head is unavailable.');
        }
        if (head.content.run_id.conversation_id !== fetchCut.conversation_id
          || (fetchConversationId && head.content.run_id.conversation_id !== fetchConversationId)) {
          throw new Error('Original Workflow inspection head conversation changed.');
        }
        setResolvedRootIdentity({
          ownerKey: batchKey,
          runId: head.content.run_id,
          workflowId: head.content.workflow_id,
        });
      })
      .catch(error => {
        if (owned()) {
          setResolvedRootIdentity({
            ownerKey: batchKey,
            error: error instanceof Error ? error.message : String(error),
          });
        }
      });
    return () => { active = false; };
  }, [rootIdentityKey, effectiveTraceRoot, effectiveTraceCut, effectiveTraceReader, target.conversationId]);

  // Fenced immutable snapshot fetch (live or history cut); never runs on live status updates or trace paging.
  useEffect(() => {
    let active = true;
    const owned = () => active && ownedIdentity.current === targetIdentity;
    if (!resolvedRunId) {
      if (awaitingRunIdResolution) {
        setSnapshotPhase({ phase: 'loading' });
      } else if (exactRootIdentity?.error) {
        setSnapshotPhase({ phase: 'error', message: exactRootIdentity.error });
      } else {
        setSnapshotPhase({ phase: 'unavailable', reason: 'missing_snapshot' });
      }
      return () => { active = false; sessionRef.current?.invalidate(); };
    }
    const resolvedTarget = target.runId ? target : { ...target, runId: resolvedRunId };
    const transport = target.transport ?? createTransport?.(resolvedTarget) ?? null;
    if (!transport) {
      setSnapshotPhase({ phase: 'unavailable', reason: 'unsupported' });
      return () => { active = false; sessionRef.current?.invalidate(); };
    }
    void sessionRef.current!.load({
      scopeKey: `${targetIdentity}:${retryCount}`,
      runId: resolvedRunId,
      transport,
      owned,
      onPhase: phase => {
        if (owned()) setSnapshotPhase(phase);
      },
    });
    return () => {
      active = false;
      sessionRef.current?.invalidate();
    };
  }, [targetIdentity, target.transport, retryCount, hasResolvedRunId, awaitingRunIdResolution, exactRootIdentity?.error]);

  // Bounded initial/updated trace batch (2 pages = <= 64 nodes) when opened from unexpanded Trace, Chat, or advancing cut.
  useEffect(() => {
    if ((target.traceNodes && !hasNewerResolvedTrace) || !effectiveTraceRoot || !effectiveTraceReader || !effectiveTraceCut) {
      if (!hasNewerResolvedTrace) setPagedTraceNodes(null);
      return;
    }
    let active = true;
    const batchOwnerKey = pageIdentity;
    const fetchCut = effectiveTraceCut;
    const fetchReader = effectiveTraceReader;
    const owned = () => active && ownedPageIdentity.current === batchOwnerKey;
    const seenIds = new Set<string>();
    const seenVisits = new Set<string>();
    traceDedupeRef.current = { ownerKey: batchOwnerKey, seenIds, seenVisits };
    void readBoundedTraceWorkflowNodes(fetchReader, fetchCut, effectiveTraceRoot, owned, DEFAULT_MAX_TRACE_NODES, {
      startOffset: 0,
      maxPages: DEFAULT_TRACE_BATCH_PAGES,
      expectedRunId: target.runId,
      seenIds,
      seenVisits,
    })
      .then(result => {
        if (!owned()) return;
        setPagedTraceNodes(result
          ? {
            ownerKey: batchOwnerKey,
            runIdentityKey,
            cut: fetchCut,
            reader: fetchReader,
            nodes: result.nodes,
            incomplete: result.incomplete,
            nextOffset: result.nextOffset,
            total: result.total,
          }
          : {
            ownerKey: batchOwnerKey,
            runIdentityKey,
            cut: fetchCut,
            reader: fetchReader,
            nodes: [],
            incomplete: false,
            nextOffset: null,
            total: '0',
            failed: true,
          });
      })
      .catch(() => {
        if (owned()) {
          setPagedTraceNodes({
            ownerKey: batchOwnerKey,
            runIdentityKey,
            cut: fetchCut,
            reader: fetchReader,
            nodes: [],
            incomplete: false,
            nextOffset: null,
            total: '0',
            failed: true,
          });
        }
      });
    return () => { active = false; };
  }, [pageIdentity, runIdentityKey, hasNewerResolvedTrace, target.traceNodes, effectiveTraceRoot, effectiveTraceReader, effectiveTraceCut, target.runId]);

  const loadMoreTraceVisits = () => {
    if (!pagedForExactCut || pagedForExactCut.nextOffset === null || pagedForExactCut.loadingMore) return;
    if (!effectiveTraceRoot) return;
    const currentOffset = pagedForExactCut.nextOffset;
    const batchIdentity = pageIdentity;
    const batchCut = pagedForExactCut.cut;
    const batchReader = pagedForExactCut.reader;
    const dedupe = traceDedupeRef.current.ownerKey === batchIdentity
      ? traceDedupeRef.current
      : { ownerKey: batchIdentity, seenIds: new Set<string>(), seenVisits: new Set<string>() };
    traceDedupeRef.current = dedupe;
    setPagedTraceNodes(current => current && current.ownerKey === batchIdentity ? { ...current, loadingMore: true } : current);
    const admittedLifetime = lifetime.current;
    const owned = () => lifetime.current === admittedLifetime && ownedPageIdentity.current === batchIdentity;
    void readBoundedTraceWorkflowNodes(batchReader, batchCut, effectiveTraceRoot, owned, DEFAULT_MAX_TRACE_NODES, {
      startOffset: currentOffset,
      maxPages: DEFAULT_TRACE_BATCH_PAGES,
      expectedRunId: target.runId ?? resolvedRunId,
      seenIds: dedupe.seenIds,
      seenVisits: dedupe.seenVisits,
    })
      .then(result => {
        if (!owned() || !result) return;
        if (result.nextOffset === null && document.activeElement === pagingButton.current) {
          pagingFocus.current = batchIdentity;
        }
        setPagedTraceNodes(current => {
          if (!current || current.ownerKey !== batchIdentity) return current;
          return {
            ownerKey: batchIdentity,
            runIdentityKey: current.runIdentityKey,
            cut: current.cut,
            reader: current.reader,
            nodes: [...current.nodes, ...result.nodes],
            incomplete: result.incomplete,
            nextOffset: result.nextOffset,
            total: result.total,
            loadingMore: false,
          };
        });
      })
      .catch(() => {
        if (owned()) {
          setPagedTraceNodes(current => current && current.ownerKey === batchIdentity
            ? { ...current, loadingMore: false, failed: true }
            : current);
        }
      });
  };

  const readyDoc = snapshotPhase.phase === 'ready' ? snapshotPhase.document : undefined;
  const readyHeader = snapshotPhase.phase === 'ready' ? snapshotPhase.header : undefined;

  // Upgrade initial selection to Start once immutable snapshot reveals whether an arbitrary Start ID (e.g. `workflow_start`) owns the selected visit.
  useEffect(() => {
    if (!readyDoc || !target.selectedVisit) return;
    const exact = selectionFromVisit(target.selectedVisit, readyDoc);
    if (exact?.kind === 'start') {
      setSelection(current => (current?.kind === 'node' && current.id === target.selectedVisit?.node ? exact : current));
    }
  }, [readyDoc, target.selectedVisit]);

  // Scene nodes are computed purely from the immutable document so status updates never relayout or rebuild nodes unnecessarily.
  const sceneNodes = useMemo(() => readyDoc ? projectScene(readyDoc).nodes : undefined, [readyDoc]);

  const baseOverlay = useMemo(() => aggregateRunOverlay({
    runId: resolvedRunId,
    run: activeRun,
    traceNodes: effectiveTraceNodes,
    selectedTraceNode: targetSelectedTraceNodeAtCut,
    pendingInteractions: activeRun ? pendingInteractions : undefined,
    sceneNodes,
    selectedVisit: target.selectedVisit,
    traceIncomplete,
  }), [
    resolvedRunId,
    activeRun,
    effectiveTraceNodes,
    targetSelectedTraceNodeAtCut,
    pendingInteractions,
    sceneNodes,
    target.selectedVisit,
    traceIncomplete,
  ]);

  const selectedPath: BlockPath = useMemo(() => {
    if (selection && 'path' in selection && selection.path) return selection.path;
    return target.selectedVisit ? blockPathFromDefinition(target.selectedVisit.block.definition) : [];
  }, [selection, target.selectedVisit]);

  const selectedViewId = useMemo(() => {
    if (!selection) return undefined;
    if (selection.kind === 'start') return viewId(selectedPath, 'start');
    if (selection.kind === 'node') return qualifiedNodeViewId(selectedPath.map(s => [s.node, 'body']).flat(), selection.id);
    return undefined;
  }, [selection, selectedPath]);

  const baseSelectedOverlay = selectedViewId ? baseOverlay.byViewId.get(selectedViewId) : undefined;
  const targetMatchesSelectedView = Boolean(
    target.selectedVisit
    && selectedViewId
    && ((sceneNodes
      ? resolveSceneNodeId(sceneNodes, target.selectedVisit)
      : (selection?.kind === 'start'
        ? viewId(blockPathFromDefinition(target.selectedVisit.block.definition), 'start')
        : qualifiedNodeViewId(target.selectedVisit.block.definition.blocks, target.selectedVisit.node))) === selectedViewId),
  );
  const selectedMissingVisitInstance = useMemo<WorkflowNodeInstance | undefined>(() => {
    if (baseSelectedOverlay) {
      if (selectedVisitKey && selectedVisitKey === targetVisitKey && targetMatchesSelectedView && target.selectedVisit) {
        const targetVisitRecord = baseSelectedOverlay.visits.find(item => item.key === targetVisitKey);
        if (targetVisitRecord) return targetVisitRecord.traceNode ? undefined : targetVisitRecord.instance;
        const inNodes = effectiveTraceNodes?.some(item => visitIdentityKey(item.instance) === targetVisitKey);
        const inSelected = targetSelectedTraceNodeAtCut
          && visitIdentityKey(targetSelectedTraceNodeAtCut.instance) === targetVisitKey;
        return !inNodes && !inSelected ? target.selectedVisit : undefined;
      }
      const baseVisit = defaultVisitForNode(baseSelectedOverlay, selectedVisitKey);
      return baseVisit && !baseVisit.traceNode ? baseVisit.instance : undefined;
    }
    if (targetMatchesSelectedView && target.selectedVisit) {
      const key = visitIdentityKey(target.selectedVisit);
      const inNodes = effectiveTraceNodes?.some(item => visitIdentityKey(item.instance) === key);
      const inSelected = targetSelectedTraceNodeAtCut
        && visitIdentityKey(targetSelectedTraceNodeAtCut.instance) === key;
      if (!inNodes && !inSelected) return target.selectedVisit;
    }
    return undefined;
  }, [
    baseSelectedOverlay,
    selectedVisitKey,
    targetVisitKey,
    targetMatchesSelectedView,
    target.selectedVisit,
    effectiveTraceNodes,
    targetSelectedTraceNodeAtCut,
  ]);

  const awaitingInitialTraceBatch = Boolean(
    !useStaticTargetTrace && effectiveTraceRoot && effectiveTraceReader && coherentTraceCut && !pagedForCurrent,
  );
  const exactLookupCut = !awaitingInitialTraceBatch ? coherentTraceCut : undefined;
  const exactLookupReader = !awaitingInitialTraceBatch ? coherentTraceReader : undefined;
  const exactLookupVisit = selectedMissingVisitInstance && exactLookupCut && exactLookupReader?.workflowNode
    ? selectedMissingVisitInstance
    : undefined;
  const exactLookupVisitKey = exactLookupVisit ? visitIdentityKey(exactLookupVisit) : null;
  const exactVisitKey = exactLookupVisit && exactLookupCut && exactLookupReader
    ? JSON.stringify([
      target.conversationId,
      exactLookupCut.activation_id ?? null,
      checkpointReadCutKey(exactLookupCut),
      exactLookupVisitKey,
      readerIdentityToken(exactLookupReader),
      retryCount,
    ])
    : null;
  const ownedExactVisitKey = useRef(exactVisitKey);
  ownedExactVisitKey.current = exactVisitKey;
  const exactVisitState = exactVisitKey && exactVisitLookup?.ownerKey === exactVisitKey
    ? exactVisitLookup
    : null;
  const exactMatchedTraceNode = exactVisitState?.status === 'ready'
    ? exactVisitState.node ?? undefined
    : undefined;

  useEffect(() => {
    const workflowNodeFn = exactLookupReader?.workflowNode;
    if (!exactVisitKey || !exactLookupVisit || !exactLookupCut || !workflowNodeFn) return;
    let active = true;
    const batchKey = exactVisitKey;
    const fetchCut = exactLookupCut;
    const fetchVisit = exactLookupVisit;
    const fetchVisitKey = visitIdentityKey(fetchVisit);
    const fetchConversationId = target.conversationId;
    const admittedLifetime = lifetime.current;
    const owned = () => active
      && lifetime.current === admittedLifetime
      && ownedExactVisitKey.current === batchKey;
    if (fetchVisit.block.run.conversation_id !== fetchCut.conversation_id
      || (fetchConversationId && fetchCut.conversation_id !== fetchConversationId)) {
      setExactVisitLookup({
        ownerKey: batchKey,
        status: 'error',
        message: 'Original Workflow node conversation changed.',
      });
      return () => { active = false; };
    }
    void workflowNodeFn(fetchCut, fetchVisit, owned)
      .then(node => {
        if (!owned()) return;
        if (node && (visitIdentityKey(node.instance) !== fetchVisitKey
          || node.instance.block.run.conversation_id !== fetchCut.conversation_id)) {
          throw new Error('Original Workflow node header is unavailable.');
        }
        setExactVisitLookup({ ownerKey: batchKey, status: 'ready', node });
      })
      .catch(error => {
        if (owned()) {
          setExactVisitLookup({
            ownerKey: batchKey,
            status: 'error',
            message: error instanceof Error ? error.message : String(error),
          });
        }
      });
    return () => { active = false; };
  }, [exactVisitKey, exactLookupVisit, exactLookupCut, exactLookupReader, target.conversationId]);

  const resolvedSelectedTraceNode = targetSelectedTraceNodeAtCut ?? exactMatchedTraceNode;
  const overlay = useMemo(() => resolvedSelectedTraceNode === targetSelectedTraceNodeAtCut
    ? baseOverlay
    : aggregateRunOverlay({
      runId: resolvedRunId,
      run: activeRun,
      traceNodes: effectiveTraceNodes,
      selectedTraceNode: resolvedSelectedTraceNode,
      pendingInteractions: activeRun ? pendingInteractions : undefined,
      sceneNodes,
      selectedVisit: target.selectedVisit,
      traceIncomplete,
    }), [
    resolvedSelectedTraceNode,
    targetSelectedTraceNodeAtCut,
    baseOverlay,
    resolvedRunId,
    activeRun,
    effectiveTraceNodes,
    pendingInteractions,
    sceneNodes,
    target.selectedVisit,
    traceIncomplete,
  ]);

  const selectedOverlay = selectedViewId ? overlay.byViewId.get(selectedViewId) : undefined;
  const currentVisit = useMemo<NormalizedVisitRecord | undefined>(() => {
    if (selectedOverlay) return defaultVisitForNode(selectedOverlay, selectedVisitKey);
    if (target.selectedVisit && selectedViewId && targetMatchesSelectedView) {
      const key = visitIdentityKey(target.selectedVisit);
      const traceMatch = effectiveTraceNodes?.find(item => visitIdentityKey(item.instance) === key)
        ?? (resolvedSelectedTraceNode && visitIdentityKey(resolvedSelectedTraceNode.instance) === key
          ? resolvedSelectedTraceNode
          : undefined);
      if (traceMatch) {
        const liveInst = activeRun?.instances.find(inst => inst.node != null && inst.visit != null && visitIdentityKey({
          block: inst.block,
          node: inst.node,
          visit: inst.visit!,
        }) === key);
        const state = effectiveTraceVisitState(traceMatch.state, activeRun, liveInst);
        return {
          key,
          viewId: selectedViewId,
          order: 0,
          instance: traceMatch.instance,
          kind: traceMatch.kind,
          state,
          reviewPending: false,
          traceNode: traceMatch,
          ...(liveInst ? { instanceView: liveInst } : {}),
        } satisfies NormalizedVisitRecord;
      }
    }
    return undefined;
  }, [
    selectedOverlay,
    selectedVisitKey,
    target.selectedVisit,
    targetMatchesSelectedView,
    resolvedSelectedTraceNode,
    activeRun,
    selectedViewId,
    effectiveTraceNodes,
  ]);

  const selectCanvasElement = (next: Selection) => {
    setSelection(next);
    setVisitTab('overview');
    if (next?.kind === 'node') {
      const nextViewId = qualifiedNodeViewId((next.path ?? []).flatMap(s => [s.node, 'body']), next.id);
      const nextOverlay = overlay.byViewId.get(nextViewId);
      const picked = defaultVisitForNode(nextOverlay, selectedVisitKey);
      setSelectedVisitKey(picked?.key);
    } else if (next?.kind === 'start') {
      const nextViewId = viewId(next.path ?? [], 'start');
      const nextOverlay = overlay.byViewId.get(nextViewId);
      const picked = defaultVisitForNode(nextOverlay, selectedVisitKey);
      setSelectedVisitKey(picked?.key);
    } else {
      setSelectedVisitKey(undefined);
    }
  };

  const focusNodeAtPath = (path: BlockPath, nodeId?: string) => {
    if (nodeId) {
      selectCanvasElement({ kind: 'node', id: nodeId, path });
    } else {
      selectCanvasElement({ kind: 'start', path: [] });
    }
    setFocusNonce(value => value + 1);
  };

  const selectedStartEntry = useMemo(() => {
    if (!readyDoc || selection?.kind !== 'start') return undefined;
    const block = blockAt(readyDoc, selectedPath);
    return findStartEntry(block);
  }, [readyDoc, selection?.kind, selectedPath]);

  const selectedDefinition = useMemo(() => {
    if (!readyDoc) return undefined;
    if (selection?.kind === 'start') return selectedStartEntry?.[1];
    if (selection?.kind === 'node') {
      const block = blockAt(readyDoc, selectedPath);
      return block ? nodeAt(block, selection.id) : undefined;
    }
    return undefined;
  }, [readyDoc, selection, selectedPath, selectedStartEntry]);

  const selectedLabel = selection?.kind === 'node'
    ? (readyDoc ? nodePresentation(readyDoc, selectedPath, selection.id)?.title : undefined) || selection.id
    : selection?.kind === 'start'
      ? (readyDoc && selectedStartEntry
        ? nodePresentation(readyDoc, selectedPath, selectedStartEntry[0])?.title || selectedStartEntry[0]
        : target.selectedVisit?.node ?? tx('workflows:canvas.start-title'))
      : selection?.kind === 'note'
        ? tx('workflows:run-canvas.synthetic-note')
        : target.selectedVisit?.node ?? tx('workflows:run-canvas.root');

  const displayWorkflowId = target.workflowId
    ?? readyHeader?.workflow_id
    ?? targetSelectedTraceNodeAtCut?.instance.block.definition.workflow_id
    ?? exactRootIdentity?.workflowId
    ?? tx('common:workflow.kind.block');

  const liveDisconnectedNotice = target.sourceMode === 'live' && !activeRun;

  const bannerMessage = (() => {
    switch (snapshotPhase.phase) {
      case 'loading': return { text: tx('workflows:run-canvas.loading'), error: false, retryable: false };
      case 'unavailable': return { text: tx('workflows:run-canvas.unavailable'), error: false, retryable: false };
      case 'corrupt': return { text: tx('workflows:run-canvas.corrupt'), error: true, retryable: false };
      case 'expired': return { text: tx('workflows:run-canvas.expired'), error: false, retryable: false };
      case 'denied': return { text: tx('workflows:run-canvas.denied'), error: true, retryable: false };
      case 'error': return { text: tx('workflows:run-canvas.error', { p0: snapshotPhase.message }), error: true, retryable: true };
      default: return null;
    }
  })();

  const childRef = durableChildRef(currentVisit);
  const canOpenChild = !!scope?.openWorkflow && !!childRef;
  const lifetimeAtRender = lifetime.current;
  const currentOwned = useMemo(
    () => () => lifetime.current === lifetimeAtRender && ownedIdentity.current === targetIdentity,
    [targetIdentity, lifetimeAtRender],
  );
  const fieldPanelTraceStatus: TraceContextStatus | undefined = currentVisit?.traceNode
    ? 'ready'
    : exactVisitKey
      ? (!exactVisitState ? 'loading' : exactVisitState.status === 'error' ? 'error' : 'ready')
      : awaitingInitialTraceBatch
        ? 'loading'
        : pagedForExactCut?.failed
          ? 'error'
          : (effectiveTraceNodes || effectiveTraceRoot ? 'ready' : traceStatus);
  const fieldPanelTraceError = !currentVisit?.traceNode && exactVisitState?.status === 'error'
    ? exactVisitState.message
    : (!currentVisit?.traceNode && pagedForExactCut?.failed ? tx('workflows:run-canvas.unavailable') : traceError);
  const fieldPanelReader = currentVisit?.traceNode && currentVisit.traceNode === targetSelectedTraceNodeAtCut && !pagedForCurrent
    ? (target.traceReader ?? coherentTraceReader)
    : coherentTraceReader;
  const fieldPanelCut = currentVisit?.traceNode && currentVisit.traceNode === targetSelectedTraceNodeAtCut && !pagedForCurrent
    ? (target.traceCut ?? coherentTraceCut)
    : coherentTraceCut;

  return createPortal(<div
    className={css.backdrop}
    role="dialog"
    aria-modal="true"
    aria-labelledby={titleId}
    data-workflow-run-canvas=""
    data-workflow-run-id={runKey}
    onMouseDown={event => {
      if (event.target === event.currentTarget) onClose();
    }}
  >
    <section ref={dialogRef} tabIndex={-1} className={css.dialog}>
      <header className={css.header}>
        <div className={css.titleGroup}>
          <h2 id={titleId} className={css.title}>{tx('workflows:run-canvas.title', { p0: displayWorkflowId })}</h2>
          <span className={css.badge}>{tx('workflows:run-canvas.read-only')}</span>
          <span className={css.badge}>{tx(activeRun && activeRun.state.type !== 'settled' ? 'workflows:run-canvas.source-live' : 'workflows:run-canvas.source-history')}</span>
          {readyHeader && <span className={css.provenance}>
            <span>{tx('workflows:run-canvas.revision', { p0: readyHeader.source_revision.slice(0, 12) })}</span>
            <code>{tx('workflows:run-canvas.digest', { p0: readyHeader.program_digest.slice(0, 12) })}</code>
            {!readyHeader.has_source_presentation && <span className={css.badge}>{tx('workflows:run-canvas.auto-layout')}</span>}
          </span>}
        </div>
        <button type="button" data-modal-autofocus className={css.closeButton} aria-label={tx('workflows:run-canvas.close')} onClick={onClose}>
          {tx('trajectory:details.close')}
        </button>
      </header>

      <nav className={css.breadcrumbs} aria-label={tx('workflows:run-canvas.breadcrumbs')}>
        <button type="button" className={css.crumbButton} onClick={() => focusNodeAtPath([])}>
          {tx('workflows:run-canvas.root')}
        </button>
        {selectedPath.map((step, index) => {
          const ancestorPath = selectedPath.slice(0, index);
          return <span key={`${index}:${step.node}`} style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
            <span className={css.crumbSep} aria-hidden="true">/</span>
            <button
              type="button"
              className={css.crumbButton}
              aria-label={tx('workflows:run-canvas.focus-parent', { p0: step.node })}
              onClick={() => focusNodeAtPath(ancestorPath, step.node)}
            >
              {step.node}
            </button>
          </span>;
        })}
        {selection?.kind === 'node' && <>
          <span className={css.crumbSep} aria-hidden="true">/</span>
          <span className={`${css.crumbButton} ${css.crumbCurrent}`}>{selection.id}</span>
        </>}
      </nav>

      {bannerMessage && <p role={bannerMessage.error ? 'alert' : 'status'} className={`${css.banner} ${bannerMessage.error ? css.bannerError : ''}`}>
        <span>{bannerMessage.text}</span>
        {bannerMessage.retryable && <button type="button" className={css.actionButton} onClick={() => setRetryCount(c => c + 1)}>
          {tx('workflows:run-canvas.retry')}
        </button>}
      </p>}

      {(overlay.omittedInstances > 0 || overlay.traceIncomplete || liveDisconnectedNotice) && <p role="status" className={css.banner}>
        <span>
          {liveDisconnectedNotice
            ? tx('workflows:run-canvas.live-disconnected')
            : pagedForExactCut && pagedForExactCut.nextOffset !== null
              ? `${tx('workflows:run-canvas.partial-coverage', { p0: String(pagedForExactCut.nodes.length), p1: pagedForExactCut.total })} · ${tx('workflows:run-canvas.omitted-notice')}`
              : tx('workflows:run-canvas.omitted-notice')}
        </span>
        {pagedForExactCut && pagedForExactCut.nextOffset !== null && (
          <button
            type="button"
            className={css.actionButton}
            ref={pagingButton}
            disabled={pagedForExactCut.loadingMore}
            onClick={loadMoreTraceVisits}
          >
            {tx('workflows:run-canvas.load-more')}
          </button>
        )}
      </p>}

      <div className={css.body}>
        <div className={css.stage}>
          {readyDoc ? <ReactFlowProvider>
            <Canvas
              document={readyDoc}
              path={selectedPath}
              viewKey={`run-canvas:${target.conversationId}:${runKey}`}
              viewEpoch={0}
              selection={selection}
              onSelect={selectCanvasElement}
              problems={EMPTY_PROBLEMS}
              edit={NOOP_EDIT}
              editable={false}
              readOnly
              persistViewport={false}
              focusNonce={focusNonce}
              runOverlay={overlay.byViewId}
              currentState={NOOP_STATE}
              drafts={EMPTY_DRAFTS}
              onDuplicate={NOOP_DUPLICATE}
              tools={[]}
            />
          </ReactFlowProvider> : <div className={css.emptyCanvas}>
            <span>{bannerMessage?.text ?? tx('workflows:run-canvas.unavailable')}</span>
          </div>}
        </div>

        <aside className={css.inspector} aria-label={tx('workflows:run-canvas.node-details')}>
          <div className={css.inspectorHead}>
            <h3 className={css.inspectorTitle}>{selectedLabel}</h3>
            <div className={css.inspectorMeta}>
              {selectedDefinition && <span>{tx(`workflows:kind.${selectedDefinition.type}` as Parameters<typeof tx>[0])}</span>}
              {selectedOverlay && <>
                <span>
                  <StateDot state={dotForAggregateStatus(selectedOverlay.status)} />{' '}
                  {tx(`workflows:run-canvas.status.${selectedOverlay.status}`)}
                </span>
                <span>
                  {tx(selectedOverlay.visitCount === 1 ? 'workflows:run-canvas.visit-count.one' : 'workflows:run-canvas.visit-count.other', { p0: String(selectedOverlay.visitCount) })}
                </span>
              </>}
            </div>
          </div>

          <div className={css.inspectorScroll}>
            {selection?.kind === 'note' ? <p className={css.muted}>{tx('workflows:run-canvas.synthetic-note')}</p>
              : !selectedOverlay?.visits.length && !currentVisit ? <p className={css.muted}>
                {selection?.kind === 'node' ? tx('workflows:run-canvas.visits-empty') : tx('workflows:run-canvas.select-node-hint')}
              </p> : <>
                {selectedOverlay && selectedOverlay.visits.length > 0 && <div>
                  <h4 className={css.sectionLabel}>{tx('workflows:run-canvas.visits-tab')} ({selectedOverlay.visits.length}{selectedOverlay.omittedVisits > 0 ? ` / ${selectedOverlay.visitCount}` : ''})</h4>
                  <ul className={css.visitsList} role="listbox" aria-label={tx('workflows:run-canvas.visits-tab')}>
                    {selectedOverlay.visits.map(visit => {
                      const status = statusOfTraceState(visit.state, visit.reviewPending);
                      const iterations = visit.instance.block.invocations.slice(1);
                      const isSelected = currentVisit?.key === visit.key;
                      const waitReason = visit.reviewPending ? 'review' : visit.state.type === 'waiting' ? visit.state.reason : undefined;
                      const statusText = waitReason
                        ? tx('common:activity.waiting-for', { state: tx(`common:state.${waitReason}`) })
                        : tx(`workflows:run-canvas.status.${status}`);
                      return <li key={visit.key}>
                        <button
                          type="button"
                          role="option"
                          aria-selected={isSelected}
                          className={css.visitButton}
                          data-run-visit-key={visit.key}
                          onClick={() => setSelectedVisitKey(visit.key)}
                        >
                          <span>
                            {tx('workflows:run-canvas.visit-item', { p0: String(visit.instance.visit + 1) })}
                            {iterations.length > 0 ? ` · ${tx('workflows:run-canvas.iterations', { p0: iterations.join('.') })}` : ''}
                          </span>
                          <span>
                            <StateDot state={dotForAggregateStatus(status)} /> {statusText}
                          </span>
                        </button>
                      </li>;
                    })}
                  </ul>
                </div>}

                {currentVisit && <>
                  <div className={css.tabs} role="tablist" aria-label={tx('workflows:run-canvas.selected-visit')}>
                    {(['overview', 'input', 'output'] as const).map(tab => <button
                      key={tab}
                      type="button"
                      role="tab"
                      aria-selected={visitTab === tab}
                      className={css.tabButton}
                      onClick={() => setVisitTab(tab)}
                    >
                      {tab === 'overview' ? tx('trajectory:tab.summary') : tab === 'input' ? tx('workflows:run-canvas.input') : tx('workflows:run-canvas.output')}
                    </button>)}
                  </div>

                  <div role="tabpanel" aria-label={visitTab === 'overview' ? tx('trajectory:tab.summary') : visitTab === 'input' ? tx('workflows:run-canvas.input') : tx('workflows:run-canvas.output')}>
                    {visitTab === 'overview' && <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                      <div className={css.inspectorMeta}>
                        <span>{tx('trajectory:workflow.node-state')}: {currentVisit.state.type === 'incomplete' ? tx('workflows:run-canvas.status.incomplete') : workflowStatus(currentVisit.state, tx)}</span>
                        {currentVisit.instance.block.invocations.length > 1 && <span>
                          {tx('workflows:run-canvas.iterations', { p0: currentVisit.instance.block.invocations.slice(1).join('.') })}
                        </span>}
                        {currentVisit.instanceView?.iteration != null && <span>
                          {tx('workflows:run-canvas.loop-iteration', { p0: `${currentVisit.instanceView.iteration}/${currentVisit.instanceView.iterations_max ?? '—'}` })}
                        </span>}
                        {currentVisit.instanceView?.loop_exit && <span>
                          {tx(currentVisit.instanceView.loop_exit === 'loop_break'
                            ? 'common:workflow.loop.loop_break'
                            : 'common:workflow.loop.loop_completed')}
                        </span>}
                        {currentVisit.instanceView?.checks_passed != null && <span>
                          {tx(currentVisit.instanceView.checks_passed ? 'common:workflow.checks-passed' : 'common:workflow.checks-failed')}
                        </span>}
                        {currentVisit.traceNode?.timing.duration_ms != null && <span>
                          {tx('trajectory:timing.duration')}: {formatDurationMillis(tx, wireCount(currentVisit.traceNode.timing.duration_ms))}
                        </span>}
                      </div>
                      {canOpenChild && childRef && <button
                        type="button"
                        className={css.actionButton}
                        onClick={() => {
                          onClose();
                          void scope!.openWorkflow!(childRef.childAgentId, childRef.activationId, currentVisit.instance.node);
                        }}
                      >
                        {tx('workflows:run-canvas.open-child')}
                      </button>}
                    </div>}

                    {visitTab === 'input' && <VisitFieldPanel
                      visitKey={currentVisit.key}
                      fieldKind="input"
                      hasTraceNode={!!currentVisit.traceNode}
                      traceStatus={fieldPanelTraceStatus}
                      traceError={fieldPanelTraceError}
                      field={currentVisit.traceNode?.input}
                      omitted={currentVisit.traceNode?.input_omitted ?? false}
                      reader={fieldPanelReader}
                      cut={fieldPanelCut}
                      owned={currentOwned}
                    />}

                    {visitTab === 'output' && <VisitFieldPanel
                      visitKey={currentVisit.key}
                      fieldKind="output"
                      hasTraceNode={!!currentVisit.traceNode}
                      traceStatus={fieldPanelTraceStatus}
                      traceError={fieldPanelTraceError}
                      field={currentVisit.traceNode?.output}
                      omitted={currentVisit.traceNode?.output_omitted ?? false}
                      reader={fieldPanelReader}
                      cut={fieldPanelCut}
                      owned={currentOwned}
                    />}
                  </div>
                </>}
              </>}
          </div>
        </aside>
      </div>
    </section>
  </div>, document.body);
}

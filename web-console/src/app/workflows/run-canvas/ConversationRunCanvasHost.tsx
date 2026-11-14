import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type {
  AttachmentTarget,
  CheckpointReadCut,
  MethodResult,
  Request1,
  RuntimeClientInteractionHeader,
  WorkflowRunView,
  WorkflowRunId,
} from '../../../../../protocol/app-server/v78';
import { checkpointReadCutKey } from '../../../../../protocol/app-server/content-identity';
import type { AppServerClient } from '../../../client/app-server';
import type { Observation } from '../../../client/session-lifecycle/port';
import type { TraceCache } from '../../../client/trace';
import { WorkflowRunCanvasContext, type WorkflowRunCanvasTarget } from './context';
import { workflowKey } from '../../../bindings/workflow-children';
import {
  createHistorySnapshotTransport,
  createLiveSnapshotTransport,
  type SnapshotRequestInvoker,
} from './snapshot-reader';
import { useWorkflowContent, type WorkflowView } from '../../agent/useWorkflowContent';
import {
  resolveWorkflowTraceForRun,
  type ResolvedWorkflowTraceContext,
  type TraceContextStatus,
} from './trace-lookup';
import { WorkflowRunCanvas } from './WorkflowRunCanvas';

export interface ConversationRunCanvasHostProps {
  client?: AppServerClient;
  sessionId?: string;
  conversationId?: string;
  attachmentTarget?: AttachmentTarget;
  attachmentObservation?: Observation;
  /** False when the conversation's attachment/observation/access is revoked,
   * disconnected, or retired; synchronously closes and blocks retained callbacks. */
  ownerCurrent?: boolean;
  agentId?: string | null;
  expectedActivationId?: string | null;
  liveCheckpointCut?: CheckpointReadCut;
  historyNodeId?: string | null;
  historyCut?: CheckpointReadCut;
  traceCache?: TraceCache;
  /** Current-cut inline exact runs (`view.snapshot.workflows.runs`). Never a prior retained cut. */
  inlineRuns?: readonly WorkflowRunView[];
  /** Optional externally hydrated runs, used ONLY when `liveRunsCurrentCut !== false`. */
  liveRuns?: readonly WorkflowRunView[];
  /** False while an external workflow header loader is still fetching a replacement cut. */
  liveRunsCurrentCut?: boolean;
  /** Live workflow view used to hydrate omitted instances boundedly ONLY while the canvas is open. */
  liveWorkflowView?: WorkflowView;
  pendingInteractions?: readonly RuntimeClientInteractionHeader[];
  children: ReactNode;
}

interface AdmittedCanvasState {
  readonly scope: symbol;
  readonly openSeq: number;
  readonly target: WorkflowRunCanvasTarget;
}

interface TraceResolutionState {
  readonly scope: symbol;
  readonly openSeq: number;
  readonly status: TraceContextStatus;
  readonly resolved?: ResolvedWorkflowTraceContext;
  readonly error?: string;
}

/**
 * Provides conversation-scoped Run Canvas activation for Chat and Trajectory views
 * and synchronously fences/closes active modals when owning conversation/attachment
 * identity changes or observation ownership is revoked, disconnected, or retired.
 */
export function ConversationRunCanvasHost({
  client,
  sessionId,
  conversationId,
  attachmentTarget,
  attachmentObservation,
  ownerCurrent = true,
  agentId,
  expectedActivationId,
  liveCheckpointCut,
  historyNodeId,
  historyCut,
  traceCache,
  inlineRuns,
  liveRuns,
  liveRunsCurrentCut = true,
  liveWorkflowView,
  pendingInteractions,
  children,
}: ConversationRunCanvasHostProps) {
  const [openState, setOpenState] = useState<AdmittedCanvasState | null>(null);
  const [traceResolution, setTraceResolution] = useState<TraceResolutionState | null>(null);
  const openSeqRef = useRef(0);
  const triggerRef = useRef<HTMLElement | null>(null);

  const currentScope = useMemo(
    () => Symbol('run-canvas-scope'),
    [
      client,
      sessionId,
      conversationId,
      attachmentTarget?.attachment_id,
      attachmentObservation,
      agentId,
      expectedActivationId,
      historyNodeId,
    ],
  );
  const checkLiveObservation = useCallback(() => {
    if (!ownerCurrent) return false;
    if (attachmentObservation && client && sessionId) {
      return client.isAttachmentObservationCurrent(sessionId, attachmentObservation);
    }
    return true;
  }, [ownerCurrent, attachmentObservation, client, sessionId]);

  const scopeValidNow = checkLiveObservation();

  const latestScope = useRef<{
    scope: symbol;
    conversationId?: string;
    valid: boolean;
    checkLive: () => boolean;
  }>({
    scope: currentScope,
    conversationId,
    valid: scopeValidNow,
    checkLive: checkLiveObservation,
  });
  latestScope.current = {
    scope: currentScope,
    conversationId,
    valid: scopeValidNow,
    checkLive: checkLiveObservation,
  };

  // Synchronous render-time ownership gate: a replacement/revoked/retired scope never
  // renders even a single stale frame from a previous conversation or attachment.
  const activeTarget = openState && scopeValidNow && openState.scope === currentScope
    ? openState.target
    : null;
  const activeOpenSeq = openState && scopeValidNow && openState.scope === currentScope
    ? openState.openSeq
    : null;
  const latestOpenSeq = useRef(activeOpenSeq);
  latestOpenSeq.current = activeOpenSeq;

  // Release any superseded state cleanly after commit.
  useEffect(() => {
    if (openState && (!scopeValidNow || openState.scope !== currentScope)) {
      setOpenState(null);
      setTraceResolution(null);
      triggerRef.current = null;
    }
  }, [openState, scopeValidNow, currentScope]);

  const targetHasCompleteTrace = Boolean(
    activeTarget?.traceRoot && activeTarget.traceCut && activeTarget.traceReader,
  );
  const activeRunIdKey = activeTarget?.runId ? workflowKey(activeTarget.runId) : '';
  const activeTraceLookupConv = activeTarget?.conversationId ?? conversationId ?? '';
  const traceCacheCutKey = traceCache?.page.read_cut ? checkpointReadCutKey(traceCache.page.read_cut) : '';
  const targetTraceCutKey = activeTarget?.traceCut ? checkpointReadCutKey(activeTarget.traceCut) : '';
  const traceFrontierKey = JSON.stringify([
    traceCacheCutKey,
    traceCache?.page.records.length ?? 0,
    traceCache?.epoch ?? 0,
    liveCheckpointCut ? checkpointReadCutKey(liveCheckpointCut) : '',
  ]);
  const liveCutKey = liveCheckpointCut ? checkpointReadCutKey(liveCheckpointCut) : '';
  const hasNewerTraceFrontier = Boolean(targetHasCompleteTrace && (
    (traceCacheCutKey && traceCacheCutKey !== targetTraceCutKey)
    || (liveCutKey && liveCutKey !== targetTraceCutKey)
  ));

  // View-scoped bounded instance hydration ONLY for the open run while the modal is open.
  const openRunContent = useWorkflowContent(
    client,
    activeTarget ? liveWorkflowView : undefined,
    Boolean(client && activeTarget && scopeValidNow && liveWorkflowView),
    agentId ?? undefined,
    activeTarget?.runId ? { hydrateRunId: activeTarget.runId, maxInstancePages: 4 } : undefined,
  );

  // Lazily resolve authoritative TraceRecord + immutable read cut on open and when live trace frontier advances.
  useEffect(() => {
    if (!activeTarget || activeOpenSeq === null) return;
    if (targetHasCompleteTrace && !hasNewerTraceFrontier) return;
    if (!client || !attachmentTarget || !scopeValidNow || !activeTarget.runId || !activeTraceLookupConv) {
      if (!targetHasCompleteTrace) {
        setTraceResolution({ scope: currentScope, openSeq: activeOpenSeq, status: 'unavailable' });
      }
      return;
    }
    let mounted = true;
    const readOwned = () => latestOpenSeq.current === activeOpenSeq
      && latestScope.current.scope === currentScope
      && latestScope.current.valid
      && latestScope.current.checkLive();
    const owned = () => mounted && readOwned();
    setTraceResolution(prev => (
      prev && prev.scope === currentScope && prev.openSeq === activeOpenSeq && prev.status === 'ready'
        ? prev
        : { scope: currentScope, openSeq: activeOpenSeq, status: 'loading' }
    ));
    void resolveWorkflowTraceForRun({
      client,
      attachmentTarget,
      agentId: agentId ?? undefined,
      expectedConversationId: activeTraceLookupConv,
      expectedActivationId: expectedActivationId ?? null,
      runId: activeTarget.runId,
      owned,
      readOwned,
    })
      .then(resolved => {
        if (!owned()) return;
        if (resolved) {
          setTraceResolution({ scope: currentScope, openSeq: activeOpenSeq, status: 'ready', resolved });
        } else if (!targetHasCompleteTrace) {
          setTraceResolution(prev => (
            prev && prev.scope === currentScope && prev.openSeq === activeOpenSeq && prev.status === 'ready'
              ? prev
              : { scope: currentScope, openSeq: activeOpenSeq, status: 'unavailable' }
          ));
        }
      })
      .catch(error => {
        if (!owned()) return;
        if (!targetHasCompleteTrace) {
          setTraceResolution(prev => (
            prev && prev.scope === currentScope && prev.openSeq === activeOpenSeq && prev.status === 'ready'
              ? prev
              : {
                scope: currentScope,
                openSeq: activeOpenSeq,
                status: 'error',
                error: error instanceof Error ? error.message : String(error),
              }
          ));
        }
      });
    return () => { mounted = false; };
  }, [
    client,
    attachmentTarget,
    agentId,
    expectedActivationId,
    currentScope,
    scopeValidNow,
    activeOpenSeq,
    activeRunIdKey,
    activeTraceLookupConv,
    targetHasCompleteTrace,
    hasNewerTraceFrontier,
    traceFrontierKey,
  ]);

  const close = useCallback(() => {
    const current = latestScope.current;
    if (current.scope !== currentScope || !current.valid || !current.checkLive()) return;
    setOpenState(state => state?.scope === currentScope ? null : state);
    const opener = triggerRef.current;
    triggerRef.current = null;
    if (opener && opener.isConnected) {
      queueMicrotask(() => {
        const latest = latestScope.current;
        if (latest.scope === currentScope && latest.valid && latest.checkLive()
          && latestOpenSeq.current === null && opener.isConnected) {
          opener.focus({ preventScroll: true });
        }
      });
    }
  }, [currentScope]);

  const resolveRunId = useCallback((runId: WorkflowRunId) => {
    const latest = latestScope.current;
    if (latest.scope !== currentScope || !latest.valid || !latest.checkLive()
      || (latest.conversationId && runId.conversation_id !== latest.conversationId)) return;
    setOpenState(state => state?.scope === currentScope && state.openSeq === activeOpenSeq && !state.target.runId
      ? { ...state, target: { ...state.target, runId } }
      : state);
  }, [currentScope, activeOpenSeq]);

  const contextValue = useMemo(() => {
    const admittedScope = currentScope;
    return {
      openRunCanvas(nextTarget: WorkflowRunCanvasTarget, trigger?: HTMLElement | null) {
        const latest = latestScope.current;
        if (latest.scope !== admittedScope || !latest.valid || !latest.checkLive()) {
          return;
        }
        if (latest.conversationId && nextTarget.conversationId && nextTarget.conversationId !== latest.conversationId) {
          return;
        }
        triggerRef.current = trigger ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
        const targetWithConversation = nextTarget.conversationId || !latest.conversationId
          ? nextTarget
          : { ...nextTarget, conversationId: latest.conversationId };
        const openSeq = ++openSeqRef.current;
        setOpenState({ scope: admittedScope, openSeq, target: targetWithConversation });
      },
    };
  }, [currentScope]);

  const createTransport = useCallback((resolvedTarget: WorkflowRunCanvasTarget) => {
    if (!client || !resolvedTarget.runId) return null;
    const admittedScope = currentScope;
    const invoker: SnapshotRequestInvoker = <TResponse,>(
      method:
        | 'conversation/workflowRunSnapshotHeader'
        | 'conversation/workflowRunSnapshotChunk'
        | 'session/workflowRunSnapshotHeader'
        | 'session/workflowRunSnapshotChunk',
      params: unknown,
      expectedType: string,
      current: () => boolean,
    ) => client.request(
      { method, params } as Request1,
      expectedType as MethodResult['type'],
      undefined,
      () => latestScope.current.scope === admittedScope
        && latestScope.current.valid
        && latestScope.current.checkLive()
        && current(),
    ) as Promise<TResponse>;

    if (attachmentTarget) {
      return createLiveSnapshotTransport(invoker, {
        target: attachmentTarget,
        agentId: agentId ?? null,
        cut: resolvedTarget.traceCut ?? liveCheckpointCut ?? null,
        runId: resolvedTarget.runId,
      });
    }

    const cut = resolvedTarget.traceCut ?? historyCut;
    if (sessionId && cut) {
      return createHistorySnapshotTransport(invoker, {
        sessionId,
        nodeId: historyNodeId ?? null,
        cut,
        runId: resolvedTarget.runId,
      });
    }

    return null;
  }, [client, currentScope, attachmentTarget, agentId, liveCheckpointCut, sessionId, historyNodeId, historyCut]);

  const matchingTraceResolution = traceResolution
    && traceResolution.scope === currentScope
    && traceResolution.openSeq === activeOpenSeq
    ? traceResolution
    : null;

  const isSourceLiveAttached = Boolean(attachmentTarget) && scopeValidNow;
  const authoritativeLiveRuns = useMemo(() => {
    if (!isSourceLiveAttached) return undefined;
    if (openRunContent.isCurrentCut && openRunContent.currentSnapshot) {
      return openRunContent.currentSnapshot.runs;
    }
    if (liveRunsCurrentCut && liveRuns) {
      return liveRuns;
    }
    return inlineRuns;
  }, [isSourceLiveAttached, openRunContent.isCurrentCut, openRunContent.currentSnapshot, liveRunsCurrentCut, liveRuns, inlineRuns]);

  return (
    <WorkflowRunCanvasContext.Provider value={contextValue}>
      {children}
      {activeTarget && (
        <WorkflowRunCanvas
          target={isSourceLiveAttached ? activeTarget : { ...activeTarget, sourceMode: 'history' }}
          liveRuns={authoritativeLiveRuns}
          onResolveRunId={resolveRunId}
          pendingInteractions={pendingInteractions}
          resolvedTrace={matchingTraceResolution?.resolved}
          traceStatus={matchingTraceResolution?.status === 'ready'
            ? 'ready'
            : (targetHasCompleteTrace ? 'ready' : (matchingTraceResolution?.status ?? 'unavailable'))}
          traceError={matchingTraceResolution?.error}
          onClose={close}
          createTransport={createTransport}
        />
      )}
    </WorkflowRunCanvasContext.Provider>
  );
}

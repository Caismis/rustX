import { act, render, screen, waitFor } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { CheckpointReadCut, MethodResult, Request, TraceRecord, WorkflowRunId } from '../../protocol/app-server/v78';
import { RpcFailure } from '../src/client/errors';
import { Server } from './fixture';
import { ConversationRunCanvasHost } from '../src/app/workflows/run-canvas/ConversationRunCanvasHost';
import { useWorkflowRunCanvas, type WorkflowRunCanvasTarget } from '../src/app/workflows/run-canvas/context';
import type { WorkflowRunCanvas as ActualWorkflowRunCanvas } from '../src/app/workflows/run-canvas/WorkflowRunCanvas';

type CanvasProps = ComponentProps<typeof ActualWorkflowRunCanvas>;
const capturedCanvasProps: CanvasProps[] = [];

vi.mock('../src/app/workflows/run-canvas/WorkflowRunCanvas', () => ({
  WorkflowRunCanvas: (props: CanvasProps) => {
    capturedCanvasProps.push(props);
    return (
      <div
        data-testid="mock-run-canvas"
        data-run-invocation={props.target.runId?.invocation ?? ''}
        data-trace-status={props.traceStatus ?? ''}
        data-trace-error={props.traceError ?? ''}
        data-resolved-root={props.resolvedTrace?.traceRoot.id ?? ''}
      />
    );
  },
}));

afterEach(() => {
  capturedCanvasProps.length = 0;
});

function OpenerBridge({ onReady }: { onReady: (open: (target: WorkflowRunCanvasTarget) => void) => void }) {
  const ctx = useWorkflowRunCanvas();
  onReady(target => ctx?.openRunCanvas(target));
  return null;
}

function makeWorkflowRoot(id: string, rev: string, convId: string, activationId: string | null): {
  root: TraceRecord;
  cut: CheckpointReadCut;
} {
  const cut: CheckpointReadCut = {
    conversation_id: convId,
    revision: rev,
    through: rev,
    domain_revisions: {},
    collection_cuts: {},
    activation_id: activationId,
  };
  const root: TraceRecord = {
    id,
    position: rev,
    location: { attempt_id: `a-${rev}`, step_id: 's1' },
    kind: 'workflow',
    state: 'completed',
    timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '25' },
    calls: [],
    attachments: [],
    fields: [],
    collections: [{ domain: `trace.records.${rev}.workflow.nodes`, revision: rev, total: '1', loaded: '0', next_offset: '0' }],
    native_id: null,
    originating_tool_call_id: null,
    has_detail: false,
    truncated: false,
  };
  return { root, cut };
}

describe('ConversationRunCanvasHost onRetryTrace ownership', () => {
  it('suppresses stale onRetryTrace callbacks across close/reopen and scope retirement while allowing active retry and fencing held responses', async () => {
    const server = new Server();
    await server.attached('A');
    const attTarget = server.client.getSnapshot().views.A!.target!;
    const convId = attTarget.conversation_id;

    const runA: WorkflowRunId = { conversation_id: convId, origin: { type: 'tool', attempt_id: 'a-A' }, invocation: '1' };
    const runB: WorkflowRunId = { conversation_id: convId, origin: { type: 'tool', attempt_id: 'a-B' }, invocation: '2' };
    const runC: WorkflowRunId = { conversation_id: convId, origin: { type: 'tool', attempt_id: 'a-C' }, invocation: '3' };
    const invA = runA.invocation;
    const invB = runB.invocation;
    const invC = runC.invocation;

    const locateOutcomes = new Map<string, () => MethodResult>();
    const locateCounts = new Map<string, number>();

    server.held.add('session/traceLocateWorkflow');
    server.handlers.set('session/traceLocateWorkflow', request => {
      if (request.method !== 'session/traceLocateWorkflow') throw new Error('unexpected method');
      const outcome = locateOutcomes.get(String(request.id));
      if (!outcome) throw new Error(`missing outcome for request ${String(request.id)}`);
      return outcome();
    });

    const requestsFor = (invocation: string) =>
      server.requests
        .filter(item => item.request.method === 'session/traceLocateWorkflow' && item.request.params.run_id.invocation === invocation)
        .map(item => item.request as Extract<Request, { method: 'session/traceLocateWorkflow' }>);

    let openCanvas!: (target: WorkflowRunCanvasTarget) => void;
    try {
      const ui = render(
        <ConversationRunCanvasHost
          client={server.client}
          sessionId="A"
          conversationId={convId}
          expectedActivationId={null}
          attachmentTarget={attTarget}
          ownerCurrent
        >
          <OpenerBridge onReady={open => { openCanvas = open; }} />
        </ConversationRunCanvasHost>,
      );

      // 1. Open target A, hold first locate call, fail it, retry once (second locate A held), and capture onRetryTraceA.
      act(() => {
        openCanvas({ conversationId: convId, runId: runA, workflowId: 'flow_a', sourceMode: 'history' });
      });
      const reqA1 = await server.waitFor('session/traceLocateWorkflow', 1);
      locateCounts.set(invA, requestsFor(invA).length);
      expect(locateCounts.get(invA)).toBe(1);

      locateOutcomes.set(String(reqA1.id), () => {
        throw new RpcFailure({ code: -32000, message: 'A initial failure' });
      });
      await act(async () => { server.reply(reqA1); });
      await waitFor(() => expect(screen.getByTestId('mock-run-canvas').getAttribute('data-trace-status')).toBe('error'));
      const retryA = capturedCanvasProps.at(-1)!.onRetryTrace!;

      // Trigger retry on A to have a held A success in flight, then close A and open B.
      act(() => { retryA(); });
      const reqA2 = await server.waitFor('session/traceLocateWorkflow', 2);
      act(() => capturedCanvasProps.at(-1)!.onClose());

      act(() => {
        openCanvas({ conversationId: convId, runId: runB, workflowId: 'flow_b', sourceMode: 'history' });
      });
      const reqB1 = await server.waitFor('session/traceLocateWorkflow', 3);
      expect(requestsFor(invB)).toHaveLength(1);
      expect(screen.getByTestId('mock-run-canvas').getAttribute('data-trace-status')).toBe('loading');

      // Invoking stale retryA while B is open on a new openSeq must be suppressed (no second run-B request).
      act(() => { retryA(); });
      expect(requestsFor(invB)).toHaveLength(1);

      // Late success from superseded run-A (reqA2) remains fenced and does not overwrite B.
      const { root: rootA, cut: cutA } = makeWorkflowRoot('trace:A', '10', convId, null);
      locateOutcomes.set(String(reqA2.id), () => ({
        type: 'trace_workflow_location',
        location: { record_id: 'trace:A', page: { read_cut: cutA, records: [rootA], updates: [], next_cursor: null } },
      }));
      await act(async () => { server.reply(reqA2); });
      expect(screen.getByTestId('mock-run-canvas').getAttribute('data-trace-status')).toBe('loading');
      expect(screen.getByTestId('mock-run-canvas').getAttribute('data-resolved-root')).toBe('');

      // Reject reqB1 -> B enters error state; stale retryA still suppressed; current retryB MUST retry once and succeed.
      locateOutcomes.set(String(reqB1.id), () => {
        throw new RpcFailure({ code: -32000, message: 'B temporary error' });
      });
      await act(async () => { server.reply(reqB1); });
      await waitFor(() => expect(screen.getByTestId('mock-run-canvas').getAttribute('data-trace-status')).toBe('error'));
      const retryB = capturedCanvasProps.at(-1)!.onRetryTrace!;

      act(() => { retryA(); });
      expect(requestsFor(invB)).toHaveLength(1);

      // Positive case: active retryB restarts run-B lookup once and succeeds.
      act(() => { retryB(); });
      const reqB2 = await server.waitFor('session/traceLocateWorkflow', 4);
      expect(requestsFor(invB)).toHaveLength(2);
      const { root: rootB, cut: cutB } = makeWorkflowRoot('trace:B', '11', convId, null);
      locateOutcomes.set(String(reqB2.id), () => ({
        type: 'trace_workflow_location',
        location: { record_id: 'trace:B', page: { read_cut: cutB, records: [rootB], updates: [], next_cursor: null } },
      }));
      await act(async () => { server.reply(reqB2); });
      await waitFor(() => expect(screen.getByTestId('mock-run-canvas').getAttribute('data-trace-status')).toBe('ready'));
      expect(screen.getByTestId('mock-run-canvas').getAttribute('data-resolved-root')).toBe('trace:B');
      const capturedRetryBAfterSuccess = capturedCanvasProps.at(-1)!.onRetryTrace!;

      // 2. Scope retirement & replacement scope: retire ownerCurrent, then mount replacement conversation scope with held C.
      ui.rerender(
        <ConversationRunCanvasHost
          client={server.client}
          sessionId="A"
          conversationId={convId}
          expectedActivationId={null}
          attachmentTarget={attTarget}
          ownerCurrent={false}
        >
          <OpenerBridge onReady={open => { openCanvas = open; }} />
        </ConversationRunCanvasHost>,
      );
      expect(screen.queryByTestId('mock-run-canvas')).toBeNull();
      act(() => { capturedRetryBAfterSuccess(); });
      expect(requestsFor(invB)).toHaveLength(2);

      // Open replacement scope on session B with target C held.
      const runConConv2: WorkflowRunId = { ...runC, conversation_id: 'conv-2' };
      ui.rerender(
        <ConversationRunCanvasHost
          client={server.client}
          sessionId="B"
          conversationId="conv-2"
          expectedActivationId={null}
          attachmentTarget={{ ...attTarget, conversation_id: 'conv-2' }}
          ownerCurrent
        >
          <OpenerBridge onReady={open => { openCanvas = open; }} />
        </ConversationRunCanvasHost>,
      );
      act(() => {
        openCanvas({ conversationId: 'conv-2', runId: runConConv2, workflowId: 'flow_c', sourceMode: 'history' });
      });
      await server.waitFor('session/traceLocateWorkflow', 5);
      expect(requestsFor(invC)).toHaveLength(1);

      // Stale callbacks from retired scopes/runs cannot restart C's held lookup.
      act(() => {
        capturedRetryBAfterSuccess();
        retryB();
        retryA();
      });
      expect(requestsFor(invC)).toHaveLength(1);
      expect(requestsFor(invB)).toHaveLength(2);
    } finally {
      server.client.disconnect();
    }
  });
});
